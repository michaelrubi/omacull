//! Decoding ahead of the cursor: previews, full-size developments of the
//! raw and thumbnails made on a pool of threads, in the order they're
//! wanted, and converted to the monitor's colours.
//!
//! The app says what it wants, most wanted first, every time that changes
//! (the cursor moved, the filmstrip scrolled); that replaces whatever was
//! still waiting, so a frame the cursor has passed is never decoded late.

use std::collections::{HashSet, VecDeque};
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use crate::color::{Display, Space};
use crate::image::{self, Histogram, Image};
use crate::raw::Info;
use crate::{develop, thumbs};

/// Work on one frame, by its index in the folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Job {
    /// The embedded preview, for the loupe.
    Preview(usize),
    /// The raw developed at full size, for 100% zoom.
    Full(usize),
    /// The filmstrip thumbnail, from the cache or made and cached.
    Thumbnail(usize),
    /// Make sure the thumbnail is cached, for later, without returning it.
    Cache(usize),
}

/// A frame ready for the loupe.
pub struct Decoded {
    /// Upright, in the monitor's colours.
    pub image: Image,
    /// Each pixel's clipping and sharpness ([`image::mark`]), measured
    /// before the conversion.
    pub marks: Vec<u8>,
    /// Of the camera's own rendering, the preview.
    pub histogram: Histogram,
    pub info: Info,
}

pub enum Output {
    Preview(Decoded),
    Full(Decoded),
    Thumbnail(Image),
    /// The thumbnail is in the cache.
    Cached,
}

pub struct Loaded {
    pub job: Job,
    pub result: io::Result<Output>,
}

#[derive(Default)]
struct Queue {
    waiting: VecDeque<Job>,
    running: HashSet<Job>,
    stop: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    work: Condvar,
    /// What to convert to, and how many times that has changed.
    display: Mutex<(Arc<Display>, u64)>,
}

pub struct Loader {
    shared: Arc<Shared>,
    /// Each with the display it was converted for.
    results: Receiver<(Loaded, u64)>,
    threads: Vec<JoinHandle<()>>,
}

impl Loader {
    /// A loader for these raws, caching thumbnails in `cache`. `wake` is
    /// called whenever something has loaded.
    pub fn new(
        raws: Vec<PathBuf>,
        cache: Option<PathBuf>,
        display: Arc<Display>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let count = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8);
        Self::with_threads(count, raws, cache, display, wake)
    }

    fn with_threads(
        count: usize,
        raws: Vec<PathBuf>,
        cache: Option<PathBuf>,
        display: Arc<Display>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let display = Mutex::new((display, 0));
        let shared = Arc::new(Shared { queue: Mutex::default(), work: Condvar::new(), display });
        let (tx, results) = channel();
        let (raws, cache, wake) = (Arc::new(raws), Arc::new(cache), Arc::new(wake));
        let threads = (0..count)
            .map(|n| {
                let (shared, tx) = (shared.clone(), tx.clone());
                let (raws, cache, wake) = (raws.clone(), cache.clone(), wake.clone());
                std::thread::Builder::new()
                    .name(format!("loader-{n}"))
                    .spawn(move || {
                        while let Some(job) = shared.next() {
                            let (display, generation) = shared.display.lock().unwrap().clone();
                            let result = run(job, &raws, cache.as_ref().as_deref(), &display);
                            shared.queue.lock().unwrap().running.remove(&job);
                            if tx.send((Loaded { job, result }, generation)).is_err() {
                                return;
                            }
                            wake();
                        }
                    })
                    .expect("spawn loader thread")
            })
            .collect();
        Self { shared, results, threads }
    }

    /// Replace what's waiting with `jobs`, most wanted first. Jobs already
    /// running aren't started again.
    pub fn want(&self, jobs: impl IntoIterator<Item = Job>) {
        let mut queue = self.shared.queue.lock().unwrap();
        let waiting: VecDeque<Job> = jobs.into_iter().filter(|j| !queue.running.contains(j)).collect();
        queue.waiting = waiting;
        drop(queue);
        self.shared.work.notify_all();
    }

    /// What's loaded since last asked, leaving out what was converted for
    /// another monitor.
    pub fn loaded(&self) -> impl Iterator<Item = Loaded> + '_ {
        let current = self.shared.display.lock().unwrap().1;
        self.results.try_iter().filter(move |(_, g)| *g == current).map(|(loaded, _)| loaded)
    }

    /// Convert to another monitor's colours from now on. What was loaded
    /// for the last one is never returned.
    pub fn set_display(&self, display: Arc<Display>) {
        let mut current = self.shared.display.lock().unwrap();
        *current = (display, current.1 + 1);
    }

    /// Whether anything is waiting or running.
    pub fn busy(&self) -> bool {
        let queue = self.shared.queue.lock().unwrap();
        !queue.waiting.is_empty() || !queue.running.is_empty()
    }
}

impl Shared {
    /// The next job to run, waiting for one; None when stopping.
    fn next(&self) -> Option<Job> {
        let mut queue = self.queue.lock().unwrap();
        loop {
            if queue.stop {
                return None;
            }
            // One full-size development at a time: each holds a few hundred
            // megabytes while it runs, and takes a core for a while.
            let developing = queue.running.iter().any(|j| matches!(j, Job::Full(_)));
            let next = queue.waiting.iter().position(|j| !(developing && matches!(j, Job::Full(_))));
            if let Some(job) = next.and_then(|at| queue.waiting.remove(at)) {
                queue.running.insert(job);
                return Some(job);
            }
            queue = self.work.wait(queue).unwrap();
        }
    }
}

/// Measure a frame, then convert it for the monitor.
fn decoded(mut image: Image, preview: &Image, space: Space, info: Info, display: &Display) -> Decoded {
    let marks = image::marks(&image);
    let histogram = Histogram::of(preview);
    display.convert(&mut image, space);
    Decoded { image, marks, histogram, info }
}

fn run(job: Job, raws: &[PathBuf], cache: Option<&std::path::Path>, display: &Display) -> io::Result<Output> {
    let raw = |i: usize| raws.get(i).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such frame"));
    match job {
        Job::Preview(i) => {
            let (preview, space, info) = image::preview_with_info(raw(i)?)?;
            Ok(Output::Preview(decoded(preview.clone(), &preview, space, info, display)))
        }
        Job::Full(i) => {
            let (full, preview, space, info) = develop::full(raw(i)?)?;
            Ok(Output::Full(decoded(full, &preview, space, info, display)))
        }
        Job::Thumbnail(i) => {
            let mut thumbnail = thumbs::load(raw(i)?, cache)?;
            display.convert(&mut thumbnail, Space::Srgb);
            Ok(Output::Thumbnail(thumbnail))
        }
        Job::Cache(i) => {
            if let Some(cache) = cache {
                thumbs::warm(raw(i)?, cache)?;
            }
            Ok(Output::Cached)
        }
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        let mut queue = self.shared.queue.lock().unwrap();
        queue.stop = true;
        queue.waiting.clear();
        drop(queue);
        self.shared.work.notify_all();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Arw, Folder};
    use std::time::{Duration, Instant};

    fn wait(loader: &Loader, count: usize) -> Vec<Loaded> {
        let started = Instant::now();
        let mut loaded = Vec::new();
        while loaded.len() < count {
            assert!(started.elapsed() < Duration::from_secs(10), "timed out");
            loaded.extend(loader.loaded());
            std::thread::sleep(Duration::from_millis(1));
        }
        loaded
    }

    fn size(result: &io::Result<Output>) -> Option<(usize, usize)> {
        match result.as_ref().ok()? {
            Output::Preview(d) | Output::Full(d) => Some((d.image.width, d.image.height)),
            Output::Thumbnail(image) => Some((image.width, image.height)),
            Output::Cached => Some((0, 0)),
        }
    }

    #[test]
    fn loads_previews_and_thumbnails_and_caches() {
        let folder = Folder::with_raws("loader", 3, &Arw { orientation: 6, ..Arw::default() });
        let raws: Vec<_> = (1..=3).map(|i| folder.raw(i)).collect();
        let cache = folder.0.join("cache");
        let loader = Loader::new(raws, Some(cache.clone()), Arc::new(Display::srgb()), || {});
        let jobs = [Job::Preview(0), Job::Thumbnail(1), Job::Cache(2), Job::Preview(7), Job::Full(0)];
        loader.want(jobs);
        let mut loaded = wait(&loader, jobs.len());
        loaded.sort_by_key(|l| format!("{:?}", l.job));
        let sizes: Vec<_> = loaded.iter().map(|l| (l.job, size(&l.result))).collect();
        assert_eq!(
            sizes,
            [
                (Job::Cache(2), Some((0, 0))),
                // The fake raws have no raw data to develop.
                (Job::Full(0), None),
                (Job::Preview(0), Some((32, 48))),
                (Job::Preview(7), None),
                (Job::Thumbnail(1), Some((32, 48))),
            ]
        );
        let Ok(Output::Preview(preview)) = &loaded[2].result else { panic!("no preview") };
        assert_eq!(preview.marks.len(), 32 * 48);
        assert_eq!(preview.histogram.0[0].iter().sum::<u32>(), 32 * 48);
        assert_eq!(preview.info.exif.iso, Some(400));
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 2);
        assert!(!loader.busy());
    }

    #[test]
    fn what_was_converted_for_another_monitor_is_dropped() {
        let folder = Folder::with_raws("loader-display", 1, &Arw::default());
        let loader = Loader::with_threads(1, vec![folder.raw(1)], None, Arc::new(Display::srgb()), || {});
        loader.want([Job::Preview(0)]);
        let started = Instant::now();
        while loader.busy() {
            assert!(started.elapsed() < Duration::from_secs(10), "timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
        loader.set_display(Arc::new(Display::srgb()));
        assert_eq!(loader.loaded().count(), 0);
        loader.want([Job::Preview(0)]);
        assert_eq!(wait(&loader, 1).len(), 1);
    }

    #[test]
    fn one_full_size_development_at_a_time() {
        let loader = Loader::with_threads(0, Vec::new(), None, Arc::new(Display::srgb()), || {});
        loader.want([Job::Full(0), Job::Full(1), Job::Preview(2)]);
        assert_eq!(loader.shared.next(), Some(Job::Full(0)));
        assert_eq!(loader.shared.next(), Some(Job::Preview(2)), "the next development waits");
        loader.shared.queue.lock().unwrap().running.remove(&Job::Full(0));
        assert_eq!(loader.shared.next(), Some(Job::Full(1)));
    }

    #[test]
    fn what_is_no_longer_wanted_is_dropped_and_whats_running_isnt_started_again() {
        // With no threads, nothing is taken off the queue.
        let loader = Loader::with_threads(0, Vec::new(), None, Arc::new(Display::srgb()), || {});
        let waiting = || loader.shared.queue.lock().unwrap().waiting.iter().copied().collect::<Vec<_>>();
        loader.want([Job::Thumbnail(3), Job::Preview(1), Job::Cache(2)]);
        assert_eq!(waiting(), [Job::Thumbnail(3), Job::Preview(1), Job::Cache(2)]);
        assert!(loader.busy());
        loader.shared.queue.lock().unwrap().running.insert(Job::Preview(2));
        loader.want([Job::Preview(2), Job::Preview(3)]);
        assert_eq!(waiting(), [Job::Preview(3)]);
    }
}
