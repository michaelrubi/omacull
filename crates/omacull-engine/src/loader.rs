//! Decoding ahead of the cursor: previews and thumbnails made on a pool of
//! threads, in the order they're wanted.
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

use crate::image::{self, Image};
use crate::thumbs;

/// Work on one frame, by its index in the folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Job {
    /// The embedded preview, for the loupe.
    Preview(usize),
    /// The filmstrip thumbnail, from the cache or made and cached.
    Thumbnail(usize),
    /// Make sure the thumbnail is cached, for later, without returning it.
    Cache(usize),
}

pub struct Loaded {
    pub job: Job,
    /// The pixels, or None for [`Job::Cache`].
    pub result: io::Result<Option<Image>>,
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
}

pub struct Loader {
    shared: Arc<Shared>,
    results: Receiver<Loaded>,
    threads: Vec<JoinHandle<()>>,
}

impl Loader {
    /// A loader for these raws, caching thumbnails in `cache`. `wake` is
    /// called whenever something has loaded.
    pub fn new(raws: Vec<PathBuf>, cache: Option<PathBuf>, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let count = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8);
        Self::with_threads(count, raws, cache, wake)
    }

    fn with_threads(
        count: usize,
        raws: Vec<PathBuf>,
        cache: Option<PathBuf>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let shared = Arc::new(Shared { queue: Mutex::default(), work: Condvar::new() });
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
                            let result = run(job, &raws, cache.as_ref().as_deref());
                            shared.queue.lock().unwrap().running.remove(&job);
                            if tx.send(Loaded { job, result }).is_err() {
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

    /// What's loaded since last asked.
    pub fn loaded(&self) -> impl Iterator<Item = Loaded> + '_ {
        self.results.try_iter()
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
            if let Some(job) = queue.waiting.pop_front() {
                queue.running.insert(job);
                return Some(job);
            }
            queue = self.work.wait(queue).unwrap();
        }
    }
}

fn run(job: Job, raws: &[PathBuf], cache: Option<&std::path::Path>) -> io::Result<Option<Image>> {
    let raw = |i: usize| raws.get(i).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such frame"));
    match job {
        Job::Preview(i) => image::preview(raw(i)?).map(Some),
        Job::Thumbnail(i) => thumbs::load(raw(i)?, cache).map(Some),
        Job::Cache(i) => match cache {
            Some(cache) => thumbs::warm(raw(i)?, cache).map(|()| None),
            None => Ok(None),
        },
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

    #[test]
    fn loads_previews_and_thumbnails_and_caches() {
        let folder = Folder::with_raws("loader", 3, &Arw { orientation: 6, ..Arw::default() });
        let raws: Vec<_> = (1..=3).map(|i| folder.raw(i)).collect();
        let cache = folder.0.join("cache");
        let loader = Loader::new(raws, Some(cache.clone()), || {});
        loader.want([Job::Preview(0), Job::Thumbnail(1), Job::Cache(2), Job::Preview(7)]);
        let mut loaded = wait(&loader, 4);
        loaded.sort_by_key(|l| format!("{:?}", l.job));
        let sizes: Vec<_> = loaded
            .iter()
            .map(|l| (l.job, l.result.as_ref().map(|i| i.as_ref().map(|i| (i.width, i.height))).ok()))
            .collect();
        assert_eq!(
            sizes,
            [
                (Job::Cache(2), Some(None)),
                (Job::Preview(0), Some(Some((32, 48)))),
                (Job::Preview(7), None),
                (Job::Thumbnail(1), Some(Some((32, 48)))),
            ]
        );
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 2);
        assert!(!loader.busy());
    }

    #[test]
    fn what_is_no_longer_wanted_is_dropped_and_whats_running_isnt_started_again() {
        // With no threads, nothing is taken off the queue.
        let loader = Loader::with_threads(0, Vec::new(), None, || {});
        let waiting = || loader.shared.queue.lock().unwrap().waiting.iter().copied().collect::<Vec<_>>();
        loader.want([Job::Thumbnail(3), Job::Preview(1), Job::Cache(2)]);
        assert_eq!(waiting(), [Job::Thumbnail(3), Job::Preview(1), Job::Cache(2)]);
        assert!(loader.busy());
        loader.shared.queue.lock().unwrap().running.insert(Job::Preview(2));
        loader.want([Job::Preview(2), Job::Preview(3)]);
        assert_eq!(waiting(), [Job::Preview(3)]);
    }
}
