//! A folder being culled: its raws and their marks, which one is current,
//! which are shown, and the marks to undo and redo.
//!
//! This is only the model. Marks reach the sidecars through
//! [`crate::disk::Disk`], which the app hands every [`Change`].

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use crate::sidecar::{self, REJECT};

/// The raws a folder is culled for. ARW only until other cameras are tried.
const RAW_EXTENSIONS: &[&str] = &["arw"];

/// A mark as darktable stores it in `xmp:Rating`: -1 rejected, 0 not yet
/// decided, 1 a pick (one star), up to 5 stars.
pub type Rating = i32;

pub const PICK: Rating = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub path: PathBuf,
    pub rating: Rating,
}

/// Which frames the filmstrip shows and stepping visits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Filter {
    #[default]
    All,
    Undecided,
    Rejects,
    /// Rated this many stars or more: 1 is picks and up.
    AtLeast(Rating),
}

impl Filter {
    pub fn matches(self, rating: Rating) -> bool {
        match self {
            Filter::All => true,
            Filter::Undecided => rating == 0,
            Filter::Rejects => rating == REJECT,
            Filter::AtLeast(stars) => rating >= stars,
        }
    }

    pub fn label(self) -> String {
        match self {
            Filter::All => "All".into(),
            Filter::Undecided => "Undecided".into(),
            Filter::Rejects => "Rejects".into(),
            Filter::AtLeast(PICK) => "Picks and up".into(),
            Filter::AtLeast(stars) => format!("{stars} stars and up"),
        }
    }

    pub const ALL: [Filter; 8] = [
        Filter::All,
        Filter::Undecided,
        Filter::AtLeast(1),
        Filter::AtLeast(2),
        Filter::AtLeast(3),
        Filter::AtLeast(4),
        Filter::AtLeast(5),
        Filter::Rejects,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Previous,
    Next,
    First,
    Last,
}

/// A mark changing on one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub index: usize,
    pub was: Rating,
    pub now: Rating,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Rated 1 or more.
    pub picks: usize,
    pub rejects: usize,
    pub undecided: usize,
}

pub struct Cull {
    dir: PathBuf,
    frames: Vec<Frame>,
    current: usize,
    filter: Filter,
    /// 1 or -1: the way the cursor last moved, for prefetching.
    direction: isize,
    undo: Vec<Change>,
    redo: Vec<Change>,
    /// Frames picked out in the filmstrip, for compare and survey.
    selected: BTreeSet<usize>,
}

fn is_raw(path: &Path) -> bool {
    let hidden = path.file_name().is_some_and(|n| n.as_encoded_bytes().starts_with(b"."));
    let raw = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| RAW_EXTENSIONS.iter().any(|r| e.eq_ignore_ascii_case(r)));
    raw && !hidden && path.is_file()
}

impl Cull {
    /// The raws in a folder, in name order, with the marks their sidecars
    /// hold. Also returns the sidecars that couldn't be read, which count as
    /// unmarked (and won't be written over: writing them fails too).
    pub fn open(dir: &Path) -> io::Result<(Self, Vec<String>)> {
        let mut paths: Vec<PathBuf> = fs::read_dir(dir)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| is_raw(p))
            .collect();
        if paths.is_empty() {
            return Err(io::Error::new(ErrorKind::NotFound, format!("no raws in {}", dir.display())));
        }
        paths.sort();
        let mut problems = Vec::new();
        let frames = paths
            .into_iter()
            .map(|path| {
                let rating = match sidecar::read(&path) {
                    Ok(rating) => rating.unwrap_or(0).clamp(REJECT, 5),
                    Err(e) => {
                        problems.push(format!("{}: {e}", sidecar::path_for(&path).display()));
                        0
                    }
                };
                Frame { path, rating }
            })
            .collect();
        Ok((Self::new(dir.to_path_buf(), frames), problems))
    }

    pub fn new(dir: PathBuf, frames: Vec<Frame>) -> Self {
        let (undo, redo, selected) = (Vec::new(), Vec::new(), BTreeSet::new());
        Self { dir, frames, current: 0, filter: Filter::All, direction: 1, undo, redo, selected }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }

    pub fn current(&self) -> usize {
        self.current
    }

    pub fn frame(&self) -> &Frame {
        &self.frames[self.current]
    }

    pub fn filter(&self) -> Filter {
        self.filter
    }

    pub fn direction(&self) -> isize {
        self.direction
    }

    /// Shown in the filmstrip: what the filter lets through, and the current
    /// frame, which stays until the cursor leaves it even if a mark took it
    /// out of the filter.
    pub fn shown(&self, index: usize) -> bool {
        index == self.current || self.matches(index)
    }

    /// Whether the filter lets a frame through.
    fn matches(&self, index: usize) -> bool {
        self.filter.matches(self.frames[index].rating)
    }

    pub fn shown_indices(&self) -> Vec<usize> {
        (0..self.frames.len()).filter(|&i| self.shown(i)).collect()
    }

    /// The frames the filter lets through after `from` going `direction`.
    fn onward(&self, from: usize, direction: isize) -> impl Iterator<Item = usize> + '_ {
        let indices: Box<dyn Iterator<Item = usize>> =
            if direction > 0 { Box::new(from + 1..self.frames.len()) } else { Box::new((0..from).rev()) };
        indices.filter(|&i| self.matches(i))
    }

    /// The next `n` frames stepping would visit, the way the cursor last
    /// moved; and the `behind` it would visit going back.
    pub fn neighbours(&self, ahead: usize, behind: usize) -> Vec<usize> {
        let mut near: Vec<usize> = self.onward(self.current, self.direction).take(ahead).collect();
        near.extend(self.onward(self.current, -self.direction).take(behind));
        near
    }

    pub fn go_to(&mut self, index: usize) {
        if index < self.frames.len() && index != self.current {
            self.direction = if index > self.current { 1 } else { -1 };
            self.current = index;
        }
    }

    /// Move the cursor among the shown frames. False if it can't go further.
    pub fn step(&mut self, step: Step) -> bool {
        let to = match step {
            Step::Next => self.onward(self.current, 1).next(),
            Step::Previous => self.onward(self.current, -1).next(),
            Step::First => (0..self.frames.len()).find(|&i| self.matches(i)),
            Step::Last => (0..self.frames.len()).rev().find(|&i| self.matches(i)),
        };
        let moved = to.is_some_and(|to| to != self.current);
        if let Some(to) = to {
            self.go_to(to);
        }
        moved
    }

    /// The next frame the filter lets through after `from`, going
    /// `direction`, that isn't in `skip`: what comes into compare or survey
    /// when a frame leaves it.
    pub fn candidate(&self, from: usize, direction: isize, skip: &[usize]) -> Option<usize> {
        self.onward(from, direction).find(|i| !skip.contains(i))
    }

    /// The selected frames, in order.
    pub fn selection(&self) -> Vec<usize> {
        self.selected.iter().copied().collect()
    }

    pub fn is_selected(&self, index: usize) -> bool {
        self.selected.contains(&index)
    }

    /// Ctrl+click: add a frame to the selection, or take it out, and go to
    /// it. The current frame is in the selection it starts.
    pub fn toggle_selected(&mut self, index: usize) {
        if self.selected.is_empty() {
            self.selected.insert(self.current);
        }
        if !self.selected.remove(&index) {
            self.selected.insert(index);
        }
        self.go_to(index);
    }

    /// Shift+click: select the shown frames from the current one to `index`,
    /// and go there.
    pub fn select_to(&mut self, index: usize) {
        let (from, to) = (self.current.min(index), self.current.max(index));
        self.selected = (from..=to).filter(|&i| self.shown(i)).collect();
        self.go_to(index);
    }

    /// Shift+arrow: step, taking the frames passed into the selection.
    pub fn extend(&mut self, step: Step) -> bool {
        self.selected.insert(self.current);
        let moved = self.step(step);
        self.selected.insert(self.current);
        moved
    }

    pub fn select_all(&mut self) {
        self.selected = self.shown_indices().into_iter().collect();
    }

    pub fn clear_selection(&mut self) {
        self.selected.clear();
    }

    /// Show only what `filter` lets through. If that leaves out the current
    /// frame, the cursor moves to the nearest frame that's in, next first.
    pub fn set_filter(&mut self, filter: Filter) {
        self.filter = filter;
        if !filter.matches(self.frame().rating) {
            let to = self.onward(self.current, 1).next().or_else(|| self.onward(self.current, -1).next());
            if let Some(to) = to {
                self.current = to;
            }
        }
    }

    /// Mark the current frame. None if it already had that mark.
    pub fn mark(&mut self, rating: Rating) -> Option<Change> {
        let change = self.set(self.current, rating)?;
        self.undo.push(change);
        self.redo.clear();
        Some(change)
    }

    fn set(&mut self, index: usize, rating: Rating) -> Option<Change> {
        let frame = &mut self.frames[index];
        let was = std::mem::replace(&mut frame.rating, rating);
        (was != rating).then_some(Change { index, was, now: rating })
    }

    /// Take back the last mark, going to its frame to show what changed.
    pub fn undo(&mut self) -> Option<Change> {
        let change = self.undo.pop()?;
        self.redo.push(change);
        self.go_to(change.index);
        let was = std::mem::replace(&mut self.frames[change.index].rating, change.was);
        Some(Change { index: change.index, was, now: change.was })
    }

    pub fn redo(&mut self) -> Option<Change> {
        let change = self.redo.pop()?;
        self.undo.push(change);
        self.go_to(change.index);
        let was = std::mem::replace(&mut self.frames[change.index].rating, change.now);
        Some(Change { index: change.index, was, now: change.now })
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Put back what a sidecar holds after writing to it failed. Not undoable.
    pub fn restore(&mut self, path: &Path, rating: Rating) {
        if let Some(frame) = self.frames.iter_mut().find(|f| f.path == path) {
            frame.rating = rating;
        }
    }

    pub fn counts(&self) -> Counts {
        let mut counts = Counts::default();
        for frame in &self.frames {
            match frame.rating {
                REJECT => counts.rejects += 1,
                0 => counts.undecided += 1,
                _ => counts.picks += 1,
            }
        }
        counts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Arw, Folder};

    fn cull(ratings: &[Rating]) -> Cull {
        let frames = ratings
            .iter()
            .enumerate()
            .map(|(i, &rating)| Frame { path: PathBuf::from(format!("/shoot/DSC{i:05}.ARW")), rating })
            .collect();
        Cull::new(PathBuf::from("/shoot"), frames)
    }

    #[test]
    fn opening_finds_the_raws_in_name_order_with_their_marks() {
        let folder = Folder::with_raws("cull-open", 3, &Arw::default());
        fs::write(folder.0.join("DSC00000.arw"), Arw::default().bytes()).unwrap();
        fs::write(folder.0.join("._DSC00001.ARW"), b"macOS resource fork").unwrap();
        fs::write(folder.0.join("notes.txt"), b"").unwrap();
        fs::create_dir(folder.0.join("sub.ARW")).unwrap();
        sidecar::write(&folder.raw(2), 4).unwrap();
        sidecar::write(&folder.raw(3), REJECT).unwrap();
        fs::write(sidecar::path_for(&folder.raw(1)), "not a sidecar").unwrap();

        let (cull, problems) = Cull::open(&folder.0).unwrap();
        let names: Vec<_> = cull.frames().iter().map(|f| f.path.file_name().unwrap().to_str().unwrap()).collect();
        assert_eq!(names, ["DSC00000.arw", "DSC00001.ARW", "DSC00002.ARW", "DSC00003.ARW"]);
        let ratings: Vec<_> = cull.frames().iter().map(|f| f.rating).collect();
        assert_eq!(ratings, [0, 0, 4, REJECT]);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("DSC00001.ARW.xmp"), "{problems:?}");
        assert_eq!(cull.counts(), Counts { picks: 1, rejects: 1, undecided: 2 });

        let empty = Folder::new("cull-empty");
        assert!(Cull::open(&empty.0).is_err());
    }

    #[test]
    fn stepping_stops_at_the_ends() {
        let mut c = cull(&[0, 0, 0]);
        assert!(!c.step(Step::Previous));
        assert!(c.step(Step::Next));
        assert!(c.step(Step::Next));
        assert!(!c.step(Step::Next));
        assert_eq!(c.current(), 2);
        assert!(c.step(Step::First));
        assert_eq!(c.current(), 0);
        assert!(!c.step(Step::First));
        assert!(c.step(Step::Last));
        assert_eq!(c.current(), 2);
    }

    #[test]
    fn marks_undo_and_redo() {
        let mut c = cull(&[0, 0, 0]);
        assert_eq!(c.mark(PICK), Some(Change { index: 0, was: 0, now: PICK }));
        assert_eq!(c.mark(PICK), None, "the same mark again changes nothing");
        c.step(Step::Next);
        c.mark(REJECT);
        c.mark(3);
        assert_eq!(c.frames().iter().map(|f| f.rating).collect::<Vec<_>>(), [PICK, 3, 0]);

        c.step(Step::Last);
        assert_eq!(c.undo(), Some(Change { index: 1, was: 3, now: REJECT }));
        assert_eq!(c.current(), 1, "undo goes to the frame it changed");
        assert_eq!(c.undo(), Some(Change { index: 1, was: REJECT, now: 0 }));
        assert_eq!(c.undo(), Some(Change { index: 0, was: PICK, now: 0 }));
        assert_eq!(c.current(), 0);
        assert_eq!(c.undo(), None);
        assert!(!c.can_undo());

        assert_eq!(c.redo(), Some(Change { index: 0, was: 0, now: PICK }));
        assert_eq!(c.redo(), Some(Change { index: 1, was: 0, now: REJECT }));
        assert_eq!(c.current(), 1);
        // A new mark drops what's left to redo.
        c.mark(5);
        assert!(!c.can_redo());
        assert_eq!(c.frames().iter().map(|f| f.rating).collect::<Vec<_>>(), [PICK, 5, 0]);
    }

    #[test]
    fn filters_choose_what_stepping_visits() {
        let mut c = cull(&[0, PICK, REJECT, 3, 0, 5]);
        for (filter, shown) in [
            (Filter::All, vec![0, 1, 2, 3, 4, 5]),
            (Filter::Undecided, vec![0, 4]),
            (Filter::AtLeast(PICK), vec![1, 3, 5]),
            (Filter::AtLeast(3), vec![3, 5]),
            (Filter::Rejects, vec![2]),
        ] {
            c.set_filter(filter);
            assert!(filter.matches(c.frame().rating), "{filter:?} moved the cursor onto a shown frame");
            c.step(Step::First);
            let mut visited = vec![c.current()];
            while c.step(Step::Next) {
                visited.push(c.current());
            }
            assert_eq!(visited, shown, "{filter:?}");
            assert_eq!(c.shown_indices(), shown);
            c.step(Step::First);
            assert_eq!(c.current(), shown[0]);
        }
    }

    #[test]
    fn a_marked_frame_stays_until_the_cursor_leaves_it() {
        let mut c = cull(&[0, 0, 0, 0]);
        c.set_filter(Filter::Undecided);
        c.step(Step::Next);
        c.mark(PICK);
        assert_eq!(c.shown_indices(), [0, 1, 2, 3]);
        c.step(Step::Next);
        assert_eq!(c.current(), 2);
        assert_eq!(c.shown_indices(), [0, 2, 3]);
        c.step(Step::Previous);
        assert_eq!(c.current(), 0, "the pick is skipped");
        // Undo goes back to it, and it's shown again while it's current.
        c.mark(REJECT);
        c.undo();
        c.undo();
        assert_eq!(c.current(), 1);
        assert_eq!(c.frame().rating, 0);
    }

    #[test]
    fn a_filter_that_leaves_out_the_current_frame_moves_the_cursor() {
        let mut c = cull(&[PICK, 0, PICK, 0]);
        c.go_to(2);
        c.set_filter(Filter::Undecided);
        assert_eq!(c.current(), 3, "next first");
        c.set_filter(Filter::AtLeast(PICK));
        assert_eq!(c.current(), 2, "else the one before");
        // Nothing matches: the cursor stays where it is.
        c.set_filter(Filter::Rejects);
        assert_eq!(c.current(), 2);
        assert_eq!(c.shown_indices(), [2]);
        assert!(!c.step(Step::Next));
    }

    #[test]
    fn neighbours_lie_the_way_the_cursor_is_going() {
        let mut c = cull(&[0; 10]);
        c.go_to(5);
        assert_eq!(c.neighbours(3, 1), [6, 7, 8, 4]);
        c.step(Step::Previous);
        assert_eq!(c.neighbours(3, 1), [3, 2, 1, 5]);
        c.set_filter(Filter::AtLeast(PICK));
        assert_eq!(c.neighbours(3, 1), Vec::<usize>::new());
    }

    #[test]
    fn frames_are_selected_by_ctrl_shift_and_arrows() {
        let mut c = cull(&[0, PICK, 0, REJECT, 0]);
        c.toggle_selected(2);
        assert_eq!((c.selection(), c.current()), (vec![0, 2], 2), "starting from the current frame");
        c.toggle_selected(0);
        assert_eq!((c.selection(), c.current()), (vec![2], 0));
        c.set_filter(Filter::Undecided);
        c.select_to(4);
        assert_eq!(c.selection(), [0, 2, 4], "only shown frames");
        c.clear_selection();
        c.extend(Step::Previous);
        assert_eq!((c.selection(), c.current()), (vec![2, 4], 2));
        c.select_all();
        assert_eq!(c.selection(), [0, 2, 4]);
    }

    #[test]
    fn candidates_skip_whats_already_on_screen() {
        let c = cull(&[0, REJECT, 0, 0, 0]);
        assert_eq!(c.candidate(0, 1, &[0, 1, 2]), Some(3));
        assert_eq!(c.candidate(4, 1, &[]), None);
        assert_eq!(c.candidate(4, -1, &[3]), Some(2));
    }

    #[test]
    fn a_failed_write_puts_back_whats_on_disk() {
        let mut c = cull(&[0, 0]);
        c.mark(4);
        c.restore(Path::new("/shoot/DSC00000.ARW"), 2);
        assert_eq!(c.frame().rating, 2);
        c.restore(Path::new("/elsewhere/DSC00000.ARW"), 5);
        assert_eq!(c.frame().rating, 2);
    }
}
