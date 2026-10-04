//! Faces found in the background, and the signals measured: the whole
//! folder, nearest the cursor first, kept in caches so a folder is only
//! looked through once.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use omacull_engine::faces::{self, Face};
use omacull_engine::image::{self, Image};
use omacull_engine::signals::{self, Signals};

/// Finds the faces in an upright frame.
pub type Find = Box<dyn FnMut(&Image) -> Result<Vec<Face>, String> + Send>;
/// Makes a [`Find`], or says why there isn't one (no ONNX Runtime, no
/// model).
pub type Finder = Arc<dyn Fn() -> Result<Find, String> + Send + Sync>;

/// YuNet on the system's ONNX Runtime.
pub fn yunet() -> Finder {
    Arc::new(|| {
        let mut detector = omacull_ai::Detector::new()?;
        Ok(Box::new(move |image: &Image| detector.detect(image)) as Find)
    })
}

enum Found {
    /// A frame looked at: its faces, unless they can't be found, and its
    /// signals, unless it couldn't be read.
    Looked(usize, Option<Vec<Face>>, Option<Signals>),
    /// Faces can't be found, and why; cached ones still come.
    Unavailable(String),
}

/// A raw's preview, and where the camera focused in it.
fn look(raw: &std::path::Path) -> Option<(Image, Option<[f32; 2]>)> {
    match image::preview_with_info(raw) {
        Ok((preview, _, info)) => Some((preview, info.focus)),
        Err(e) => {
            log::warn!("looking at {}: {e}", raw.display());
            None
        }
    }
}

#[derive(Default)]
struct Queue {
    waiting: VecDeque<usize>,
    stop: bool,
}

pub struct Faces {
    found: HashMap<usize, Vec<Face>>,
    measured: HashMap<usize, Signals>,
    /// Frames that have been looked at, whatever came of it.
    looked: HashSet<usize>,
    queue: Arc<(Mutex<Queue>, Condvar)>,
    results: Receiver<Found>,
    thread: Option<JoinHandle<()>>,
    /// What was last asked for.
    wanted: Vec<usize>,
    /// Why faces can't be found, if they can't.
    pub unavailable: Option<String>,
}

impl Faces {
    /// Looks through `raws`, keeping faces in `cache` and signals in
    /// `measured`.
    pub fn new(
        raws: Vec<PathBuf>,
        cache: Option<PathBuf>,
        measured: Option<PathBuf>,
        finder: Finder,
        wake: impl Fn() + Send + 'static,
    ) -> Self {
        let queue: Arc<(Mutex<Queue>, Condvar)> = Arc::default();
        let (tx, results) = channel();
        let shared = queue.clone();
        let thread = std::thread::Builder::new()
            .name("faces".into())
            .spawn(move || {
                let mut find = match finder() {
                    Ok(find) => Some(find),
                    Err(e) => {
                        log::info!("faces: {e}");
                        let _ = tx.send(Found::Unavailable(e));
                        wake();
                        None
                    }
                };
                loop {
                    let index = {
                        let (lock, ready) = &*shared;
                        let mut queue = lock.lock().unwrap();
                        loop {
                            if queue.stop {
                                return;
                            }
                            if let Some(i) = queue.waiting.pop_front() {
                                break i;
                            }
                            queue = ready.wait(queue).unwrap();
                        }
                    };
                    let Some(raw) = raws.get(index) else { continue };
                    let cached = cache.as_deref().and_then(|dir| faces::load(raw, dir));
                    let mut preview = None;
                    let found = match (cached, &mut find) {
                        (Some(found), _) => Some(found),
                        (None, Some(find)) => {
                            preview = look(raw);
                            Some(match preview.as_ref().map(|(preview, _)| find(preview)) {
                                Some(Ok(found)) => {
                                    if let Some(dir) = &cache
                                        && let Err(e) = faces::store(raw, dir, &found)
                                    {
                                        log::warn!("couldn't cache faces: {e}");
                                    }
                                    found
                                }
                                Some(Err(e)) => {
                                    log::warn!("faces in {}: {e}", raw.display());
                                    Vec::new()
                                }
                                None => Vec::new(),
                            })
                        }
                        (None, None) => None,
                    };
                    let cached = measured.as_deref().and_then(|dir| signals::load(raw, dir));
                    let signals = cached.or_else(|| {
                        let (preview, focus) = preview.take().or_else(|| look(raw))?;
                        let signals = signals::measure(&preview, focus, found.as_deref().unwrap_or(&[]));
                        // Kept once the faces are known: they'd change it.
                        if let (Some(dir), Some(_)) = (&measured, &found)
                            && let Err(e) = signals::store(raw, dir, &signals)
                        {
                            log::warn!("couldn't cache signals: {e}");
                        }
                        Some(signals)
                    });
                    if tx.send(Found::Looked(index, found, signals)).is_err() {
                        return;
                    }
                    wake();
                }
            })
            .expect("spawn the faces thread");
        Self {
            found: HashMap::new(),
            measured: HashMap::new(),
            looked: HashSet::new(),
            queue,
            results,
            thread: Some(thread),
            wanted: Vec::new(),
            unavailable: None,
        }
    }

    /// Take in what's been found.
    pub fn update(&mut self) {
        for found in self.results.try_iter() {
            match found {
                Found::Looked(index, faces, signals) => {
                    self.looked.insert(index);
                    if let Some(faces) = faces {
                        self.found.insert(index, faces);
                    }
                    if let Some(signals) = signals {
                        self.measured.insert(index, signals);
                    }
                }
                Found::Unavailable(why) => self.unavailable = Some(why),
            }
        }
    }

    /// Look at these frames next, in this order, leaving out what's done.
    pub fn want(&mut self, order: impl IntoIterator<Item = usize>) {
        let wanted: Vec<usize> = order.into_iter().filter(|i| !self.looked.contains(i)).collect();
        if wanted != self.wanted {
            let (lock, ready) = &*self.queue;
            lock.lock().unwrap().waiting = wanted.iter().copied().collect();
            ready.notify_all();
            self.wanted = wanted;
        }
    }

    /// The faces found in a frame, left to right; None if it hasn't been
    /// looked at yet.
    pub fn of(&self, index: usize) -> Option<Vec<Face>> {
        let mut faces = self.found.get(&index)?.clone();
        faces.sort_by(|a, b| a.between_eyes()[0].total_cmp(&b.between_eyes()[0]));
        Some(faces)
    }

    /// What was measured of a frame; None if it hasn't been looked at yet.
    pub fn signals(&self, index: usize) -> Option<&Signals> {
        self.measured.get(&index)
    }

    /// How many frames have been measured.
    pub fn measured(&self) -> usize {
        self.measured.len()
    }
}

impl Drop for Faces {
    fn drop(&mut self) {
        let (lock, ready) = &*self.queue;
        lock.lock().unwrap().stop = true;
        ready.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use omacull_engine::testing::{Arw, Folder};
    use std::time::{Duration, Instant};

    /// Two faces in every frame, without a model: one on the left, a
    /// larger one on the right.
    pub fn two_faces() -> Finder {
        Arc::new(|| {
            Ok(Box::new(|_: &Image| {
                Ok(vec![
                    Face { score: 0.9, bounds: [0.6, 0.2, 0.9, 0.7], eyes: [[0.68, 0.4], [0.82, 0.4]], open: None },
                    Face { score: 0.8, bounds: [0.1, 0.3, 0.3, 0.6], eyes: [[0.15, 0.4], [0.25, 0.4]], open: None },
                ])
            }) as Find)
        })
    }

    pub fn none_set_up() -> Finder {
        Arc::new(|| Err("no detector in tests".into()))
    }

    fn wait(faces: &mut Faces, done: impl Fn(&Faces) -> bool) {
        let started = Instant::now();
        while !done(faces) {
            assert!(started.elapsed() < Duration::from_secs(10), "timed out");
            std::thread::sleep(Duration::from_millis(1));
            faces.update();
        }
    }

    #[test]
    fn faces_are_found_cached_and_read_back() {
        let folder = Folder::with_raws("faces-worker", 3, &Arw::default());
        let raws: Vec<_> = (1..=3).map(|i| folder.raw(i)).collect();
        let (cache, measured) = (folder.0.join("faces"), folder.0.join("signals"));
        let mut faces = Faces::new(raws.clone(), Some(cache.clone()), Some(measured.clone()), two_faces(), || {});
        faces.want([2, 0]);
        wait(&mut faces, |f| f.of(0).is_some() && f.of(2).is_some());
        let found = faces.of(2).unwrap();
        assert!(found[0].between_eyes()[0] < found[1].between_eyes()[0], "left to right");
        assert_eq!(faces.of(1), None, "not asked for");
        // Measured too, with the faces found.
        let signals = *faces.signals(2).unwrap();
        assert_eq!((signals.faces, signals.highlights), (2, 0.0));
        assert!((signals.face - 0.15).abs() < 1e-5, "the larger face's share of the frame");
        assert_eq!((faces.signals(1), faces.measured()), (None, 2));
        assert_eq!(signals::load(&raws[2], &measured), Some(signals));
        drop(faces);

        // Without a detector, what's cached still comes, and the reason
        // for the rest.
        let mut faces = Faces::new(raws.clone(), Some(cache), Some(measured.clone()), none_set_up(), || {});
        faces.want([0, 1]);
        wait(&mut faces, |f| f.of(0).is_some() && f.signals(1).is_some() && f.unavailable.is_some());
        assert_eq!(faces.of(0).unwrap().len(), 2);
        assert_eq!(faces.of(1), None);
        // What can be measured without faces is, but isn't kept: found
        // later, the faces would change it.
        assert_eq!(faces.signals(1).map(|s| (s.faces, s.eyes)), Some((0, None)));
        assert_eq!(signals::load(&raws[1], &measured), None);
    }
}
