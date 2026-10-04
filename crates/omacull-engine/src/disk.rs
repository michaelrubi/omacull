//! Marks on their way to disk: the sidecar first, then a row in the
//! decision log, on a thread of their own so a slow disk never holds up the
//! next frame. Marks are written in the order they were made.
//!
//! The decision log (`~/.local/share/omacull/decisions.jsonl`) is the
//! training set for auto-cull: one JSON object a line, appended for every
//! mark that reached its sidecar. See "Decision log" in docs/DESIGN.md for
//! what each field means. Frames looked at and left unmarked are logged
//! too, when a folder is left: what's passed over is a choice as well.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::cull::{Filter, Rating};
use crate::raw::RawFile;
use crate::sidecar;
use crate::signals::{Signals, Standing, Suggestion};

/// The decision log's schema version, the `v` of every row.
pub const LOG_VERSION: u32 = 1;

/// `$XDG_DATA_HOME/omacull/decisions.jsonl`, or
/// `~/.local/share/omacull/decisions.jsonl`.
pub fn default_log() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(dir.join("omacull").join("decisions.jsonl"))
}

/// How a mark came about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum How {
    /// A mark key.
    Mark,
    /// Undo put back the mark before.
    Undo,
    Redo,
    /// No mark: the frame was looked at and left as it was. Logged when
    /// the folder is left; nothing is written to its sidecar.
    Pass,
}

/// A mark made, to be written.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub path: PathBuf,
    pub rating: Rating,
    pub was: Rating,
    pub how: How,
    /// The view it was made in: "loupe" (and later "compare", "survey").
    pub view: &'static str,
    /// The other frames on screen when it was made.
    pub compared: Vec<PathBuf>,
    pub filter: Filter,
    /// How long the frame had been on screen.
    pub dwell: Duration,
    pub at: SystemTime,
    /// What was measured of the frame, and how it stood among the frames
    /// it was shot with, if that was known by then.
    pub signals: Option<Signals>,
    pub standing: Option<Standing>,
    /// The mark Omacull was suggesting for it, if any.
    pub suggested: Option<Suggestion>,
}

/// A raw, identified well enough to find it again after the folder moves:
/// by name, size and capture time as well as by path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FileId {
    pub path: PathBuf,
    pub size: u64,
    /// Seconds since 1970.
    pub modified: u64,
    pub captured: Option<String>,
}

impl FileId {
    pub fn of(path: &Path) -> io::Result<Self> {
        let meta = fs::metadata(path)?;
        let modified = meta.modified()?.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        let captured = RawFile::open(path).ok().and_then(|raw| raw.captured);
        Ok(Self { path: fs::canonicalize(path)?, size: meta.len(), modified, captured })
    }
}

/// One row of the decision log.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Decision {
    pub v: u32,
    /// When the mark was made, in milliseconds since 1970.
    pub at: u64,
    /// When Omacull was started, in milliseconds since 1970: the same for
    /// every mark made in one sitting.
    pub session: u64,
    pub file: FileId,
    pub rating: Rating,
    pub was: Rating,
    pub how: How,
    pub view: &'static str,
    pub compared: Vec<FileId>,
    pub filter: Filter,
    pub dwell_ms: u64,
    pub signals: Option<Signals>,
    pub standing: Option<Standing>,
    pub suggested: Option<Suggestion>,
}

pub fn millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

impl Decision {
    pub fn new(mark: &Mark, session: u64) -> io::Result<Self> {
        Ok(Self {
            v: LOG_VERSION,
            at: millis(mark.at),
            session,
            file: FileId::of(&mark.path)?,
            rating: mark.rating,
            was: mark.was,
            how: mark.how,
            view: mark.view,
            compared: mark.compared.iter().map(|p| FileId::of(p)).collect::<io::Result<_>>()?,
            filter: mark.filter,
            dwell_ms: mark.dwell.as_millis() as u64,
            signals: mark.signals,
            standing: mark.standing,
            suggested: mark.suggested,
        })
    }
}

/// Something that went wrong on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// The mark didn't reach the sidecar, which holds `on_disk`.
    Sidecar { path: PathBuf, error: String, on_disk: Rating },
    /// The mark is in the sidecar but not in the decision log.
    Log(String),
}

/// What the disk thread is asked to do.
enum Work {
    Mark(Box<Mark>),
    /// Say when everything before has been written.
    Flush(Sender<()>),
}

pub struct Disk {
    marks: Option<Sender<Work>>,
    problems: Receiver<Problem>,
    thread: Option<JoinHandle<()>>,
}

fn append(log: &Path, decision: &Decision) -> io::Result<()> {
    if let Some(dir) = log.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut line = serde_json::to_vec(decision)?;
    line.push(b'\n');
    OpenOptions::new().create(true).append(true).open(log)?.write_all(&line)
}

impl Disk {
    /// Writes to sidecars, and logs to `log` if there is one. `wake` is
    /// called when there's a problem to collect.
    pub fn new(log: Option<PathBuf>, session: u64, wake: impl Fn() + Send + 'static) -> Self {
        let (marks, rx) = channel::<Work>();
        let (tx, problems) = channel();
        let thread = std::thread::Builder::new()
            .name("disk".into())
            .spawn(move || {
                for work in rx {
                    let mark = match work {
                        Work::Mark(mark) => mark,
                        Work::Flush(done) => {
                            let _ = done.send(());
                            continue;
                        }
                    };
                    // A frame passed over has nothing to write but its row.
                    let written = if mark.how == How::Pass { Ok(()) } else { sidecar::write(&mark.path, mark.rating) };
                    let problem = match written {
                        Err(e) => Some(Problem::Sidecar {
                            path: mark.path.clone(),
                            error: e.to_string(),
                            on_disk: sidecar::read(&mark.path).ok().flatten().unwrap_or(0),
                        }),
                        Ok(()) => log
                            .as_ref()
                            .and_then(|log| Decision::new(&mark, session).and_then(|d| append(log, &d)).err())
                            .map(|e| Problem::Log(e.to_string())),
                    };
                    if let Some(problem) = problem {
                        log::warn!("{problem:?}");
                        if tx.send(problem).is_err() {
                            return;
                        }
                        wake();
                    }
                }
            })
            .expect("spawn disk thread");
        Self { marks: Some(marks), problems, thread: Some(thread) }
    }

    pub fn write(&self, mark: Mark) {
        if let Some(marks) = &self.marks {
            let _ = marks.send(Work::Mark(Box::new(mark)));
        }
    }

    /// Wait for every mark sent so far to be written, as before handing
    /// the folder to darktable.
    pub fn flush(&self) {
        let (done, wait) = channel();
        if let Some(marks) = &self.marks
            && marks.send(Work::Flush(done)).is_ok()
        {
            let _ = wait.recv();
        }
    }

    pub fn problems(&self) -> impl Iterator<Item = Problem> + '_ {
        self.problems.try_iter()
    }

    /// Wait for every mark sent so far to be written. Nothing more can be
    /// written after.
    pub fn finish(&mut self) {
        self.marks = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Disk {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidecar::REJECT;
    use crate::testing::{Arw, Folder};

    fn mark(path: PathBuf, rating: Rating, was: Rating) -> Mark {
        Mark {
            path,
            rating,
            was,
            how: How::Mark,
            view: "loupe",
            compared: Vec::new(),
            filter: Filter::Undecided,
            dwell: Duration::from_millis(1500),
            at: UNIX_EPOCH + Duration::from_millis(1_791_000_000_123),
            signals: None,
            standing: None,
            suggested: None,
        }
    }

    #[test]
    fn marks_reach_the_sidecars_and_the_log_in_order() {
        let folder = Folder::with_raws("disk", 2, &Arw::default());
        let log = folder.0.join("log").join("decisions.jsonl");
        let mut disk = Disk::new(Some(log.clone()), 1_791_000_000_000, || {});
        disk.write(mark(folder.raw(1), 3, 0));
        disk.write(Mark { how: How::Undo, ..mark(folder.raw(1), 0, 3) });
        disk.write(mark(folder.raw(2), REJECT, 0));
        disk.flush();
        assert_eq!(sidecar::read(&folder.raw(2)).unwrap(), Some(REJECT), "written by the time flush returns");
        disk.finish();
        assert_eq!(disk.problems().count(), 0);
        assert_eq!(sidecar::read(&folder.raw(1)).unwrap(), Some(0));
        assert_eq!(sidecar::read(&folder.raw(2)).unwrap(), Some(REJECT));

        let rows: Vec<serde_json::Value> =
            fs::read_to_string(&log).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(rows.len(), 3);
        let size = fs::metadata(folder.raw(1)).unwrap().len();
        assert_eq!(
            rows[0],
            serde_json::json!({
                "v": 1,
                "at": 1_791_000_000_123u64,
                "session": 1_791_000_000_000u64,
                "file": {
                    "path": fs::canonicalize(folder.raw(1)).unwrap(),
                    "size": size,
                    "modified": FileId::of(&folder.raw(1)).unwrap().modified,
                    "captured": "2026:10:04 12:00:00.123",
                },
                "rating": 3,
                "was": 0,
                "how": "mark",
                "view": "loupe",
                "compared": [],
                "filter": "undecided",
                "dwell_ms": 1500,
                "signals": null,
                "standing": null,
                "suggested": null,
            })
        );
        assert_eq!((&rows[1]["how"], &rows[1]["rating"], &rows[1]["was"]), (&"undo".into(), &0.into(), &3.into()));
        assert_eq!(rows[2]["rating"], REJECT);
        let picks = Mark { filter: Filter::AtLeast(1), ..mark(folder.raw(1), 1, 0) };
        let row = serde_json::to_value(Decision::new(&picks, 0).unwrap()).unwrap();
        assert_eq!(row["filter"], serde_json::json!({"at_least": 1}));
    }

    #[test]
    fn a_frame_passed_over_is_logged_and_its_sidecar_left_alone() {
        use crate::signals::By;
        let folder = Folder::with_raws("disk-pass", 1, &Arw::default());
        let log = folder.0.join("decisions.jsonl");
        let mut disk = Disk::new(Some(log.clone()), 0, || {});
        let signals = Signals { focus: Some(40.0), open: Some(0.5), faces: 1, ..Signals::default() };
        let standing = Standing { of: 3, sharp: Some(0.5), open: None };
        let suggested = Suggestion { rating: REJECT, by: By::Model, confidence: 0.9 };
        disk.write(Mark {
            how: How::Pass,
            signals: Some(signals),
            standing: Some(standing),
            suggested: Some(suggested),
            ..mark(folder.raw(1), 0, 0)
        });
        disk.finish();
        assert!(!sidecar::path_for(&folder.raw(1)).exists());
        let row: serde_json::Value = serde_json::from_str(fs::read_to_string(&log).unwrap().trim()).unwrap();
        assert_eq!((&row["how"], &row["rating"]), (&"pass".into(), &0.into()));
        assert_eq!(row["signals"]["focus"], 40.0);
        assert_eq!(row["standing"], serde_json::json!({"of": 3, "sharp": 0.5, "open": null}));
        assert_eq!(row["suggested"]["rating"], REJECT);
        assert_eq!(row["suggested"]["by"], "model");
    }

    #[test]
    fn a_mark_that_cant_be_written_comes_back_with_whats_on_disk() {
        let folder = Folder::with_raws("disk-fail", 1, &Arw::default());
        sidecar::write(&folder.raw(1), 2).unwrap();
        let xmp = sidecar::path_for(&folder.raw(1));
        let text = fs::read_to_string(&xmp).unwrap();
        // A sidecar this can't place a rating in: it has no description.
        fs::write(&xmp, text.replace("rdf:Description", "rdf:Other")).unwrap();
        let log = folder.0.join("decisions.jsonl");
        let mut disk = Disk::new(Some(log.clone()), 0, || {});
        disk.write(mark(folder.raw(1), 5, 2));
        disk.finish();
        let problems: Vec<_> = disk.problems().collect();
        assert!(
            matches!(&problems[..], [Problem::Sidecar { path, on_disk: 0, .. }] if *path == folder.raw(1)),
            "{problems:?}"
        );
        assert!(!log.exists(), "only marks that were written are logged");
    }
}
