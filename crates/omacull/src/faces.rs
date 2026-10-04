//! Faces found in the background: the whole folder, nearest the cursor
//! first, kept in a cache so a folder is only looked through once.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use omacull_engine::faces::{self, Face};
use omacull_engine::image::{self, Image};

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
    Faces(usize, Vec<Face>),
    /// Faces can't be found, and why; cached ones still come.
    Unavailable(String),
}

#[derive(Default)]
struct Queue {
    waiting: VecDeque<usize>,
    stop: bool,
}

pub struct Faces {
    found: HashMap<usize, Vec<Face>>,
    queue: Arc<(Mutex<Queue>, Condvar)>,
    results: Receiver<Found>,
    thread: Option<JoinHandle<()>>,
    /// What was last asked for.
    wanted: Vec<usize>,
    /// Why faces can't be found, if they can't.
    pub unavailable: Option<String>,
}

impl Faces {
    pub fn new(raws: Vec<PathBuf>, cache: Option<PathBuf>, finder: Finder, wake: impl Fn() + Send + 'static) -> Self {
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
                    let found = match (cached, &mut find) {
                        (Some(found), _) => found,
                        (None, Some(find)) => match image::preview(raw).map_err(|e| e.to_string()).and_then(|p| {
                            find(&p)
                        }) {
                            Ok(found) => {
                                if let Some(dir) = &cache
                                    && let Err(e) = faces::store(raw, dir, &found)
                                {
                                    log::warn!("couldn't cache faces: {e}");
                                }
                                found
                            }
                            Err(e) => {
                                log::warn!("faces in {}: {e}", raw.display());
                                Vec::new()
                            }
                        },
                        (None, None) => continue,
                    };
                    if tx.send(Found::Faces(index, found)).is_err() {
                        return;
                    }
                    wake();
                }
            })
            .expect("spawn the faces thread");
        Self { found: HashMap::new(), queue, results, thread: Some(thread), wanted: Vec::new(), unavailable: None }
    }

    /// Take in what's been found.
    pub fn update(&mut self) {
        for found in self.results.try_iter() {
            match found {
                Found::Faces(index, faces) => _ = self.found.insert(index, faces),
                Found::Unavailable(why) => self.unavailable = Some(why),
            }
        }
    }

    /// Look at these frames next, in this order, leaving out what's done.
    pub fn want(&mut self, order: impl IntoIterator<Item = usize>) {
        let wanted: Vec<usize> = order.into_iter().filter(|i| !self.found.contains_key(i)).collect();
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
                    Face { score: 0.9, bounds: [0.6, 0.2, 0.9, 0.7], eyes: [[0.68, 0.4], [0.82, 0.4]] },
                    Face { score: 0.8, bounds: [0.1, 0.3, 0.3, 0.6], eyes: [[0.15, 0.4], [0.25, 0.4]] },
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
        let cache = folder.0.join("faces");
        let mut faces = Faces::new(raws.clone(), Some(cache.clone()), two_faces(), || {});
        faces.want([2, 0]);
        wait(&mut faces, |f| f.of(0).is_some() && f.of(2).is_some());
        let found = faces.of(2).unwrap();
        assert!(found[0].between_eyes()[0] < found[1].between_eyes()[0], "left to right");
        assert_eq!(faces.of(1), None, "not asked for");
        drop(faces);

        // Without a detector, what's cached still comes, and the reason
        // for the rest.
        let mut faces = Faces::new(raws, Some(cache), none_set_up(), || {});
        faces.want([0, 1]);
        wait(&mut faces, |f| f.of(0).is_some() && f.unavailable.is_some());
        assert_eq!(faces.of(0).unwrap().len(), 2);
        assert_eq!(faces.of(1), None);
    }
}
