//! A folder being culled: its raws and JPEGs and their marks, which one
//! is current, which are shown, and the marks to undo and redo.
//!
//! This is only the model. Marks reach the sidecars through
//! [`crate::disk::Disk`], which the app hands every [`Change`].

use std::cmp::Reverse;
use std::collections::BTreeSet;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::image::Image;
use crate::raw::RawFile;
use crate::sidecar::{self, REJECT};
use crate::stacks::{self, Signature, Stacking};

/// The raws a folder is culled for: Sony's, Nikon's, Canon's and Fuji's.
pub const RAW_EXTENSIONS: &[&str] = &["arw", "nef", "cr3", "raf"];
/// And the pictures that are developed already.
pub const JPEG_EXTENSIONS: &[&str] = &["jpg", "jpeg"];

/// What a picture is, going by its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Raw,
    Jpeg,
}

/// What kind of picture a path names, if it names one.
pub fn kind(path: &Path) -> Option<Kind> {
    let extension = path.extension()?.to_str()?;
    let among = |extensions: &[&str]| extensions.iter().any(|e| extension.eq_ignore_ascii_case(e));
    match () {
        () if among(RAW_EXTENSIONS) => Some(Kind::Raw),
        () if among(JPEG_EXTENSIONS) => Some(Kind::Jpeg),
        () => None,
    }
}

/// Which of a folder's pictures are culled. The others aren't in the cull
/// at all: not counted, stacked or suggested for, so a raw and the JPEG
/// the camera wrote beside it are never weighed against each other.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Formats {
    #[default]
    All,
    Raw,
    Jpeg,
}

impl Formats {
    pub const ALL: [Formats; 3] = [Formats::All, Formats::Raw, Formats::Jpeg];

    pub fn label(self) -> &'static str {
        match self {
            Formats::All => "All",
            Formats::Raw => "RAW",
            Formats::Jpeg => "JPEG",
        }
    }

    /// The next one round, for the key that cycles them.
    pub fn next(self) -> Self {
        let at = Self::ALL.iter().position(|&f| f == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }

    /// Whether pictures of this kind are culled.
    pub fn takes(self, kind: Kind) -> bool {
        match self {
            Formats::All => true,
            Formats::Raw => kind == Kind::Raw,
            Formats::Jpeg => kind == Kind::Jpeg,
        }
    }
}

/// A mark as darktable stores it in `xmp:Rating`: -1 rejected, 0 not yet
/// decided, 1 a pick (one star), up to 5 stars.
pub type Rating = i32;

pub const PICK: Rating = 1;

#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub path: PathBuf,
    pub rating: Rating,
    /// When it was taken, in seconds, for stacking by time.
    pub captured: Option<f64>,
    /// What it looks like, roughly, for stacking by look.
    pub signature: Option<Signature>,
    /// The mark Omacull suggests for it, while it has none.
    pub suggested: Option<Rating>,
}

impl Frame {
    pub fn new(path: PathBuf, rating: Rating) -> Self {
        Self { path, rating, captured: None, signature: None, suggested: None }
    }
}

/// Which frames the filmstrip shows and stepping visits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Filter {
    #[default]
    All,
    Undecided,
    Rejects,
    /// Rated this many stars or more: 1 is picks and up.
    AtLeast(Rating),
    /// Undecided, with a mark suggested: what there is to review.
    Suggested,
}

impl Filter {
    pub fn matches(self, frame: &Frame) -> bool {
        match self {
            Filter::All => true,
            Filter::Undecided => frame.rating == 0,
            Filter::Rejects => frame.rating == REJECT,
            Filter::AtLeast(stars) => frame.rating >= stars,
            Filter::Suggested => frame.rating == 0 && frame.suggested.is_some(),
        }
    }

    pub fn label(self) -> String {
        match self {
            Filter::All => "All".into(),
            Filter::Undecided => "Undecided".into(),
            Filter::Rejects => "Rejects".into(),
            Filter::AtLeast(PICK) => "Picks and up".into(),
            Filter::AtLeast(stars) => format!("{stars} stars and up"),
            Filter::Suggested => "Suggested".into(),
        }
    }

    pub const ALL: [Filter; 9] = [
        Filter::All,
        Filter::Undecided,
        Filter::AtLeast(1),
        Filter::AtLeast(2),
        Filter::AtLeast(3),
        Filter::AtLeast(4),
        Filter::AtLeast(5),
        Filter::Rejects,
        Filter::Suggested,
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
    /// How many have 1 to 5 stars.
    pub stars: [usize; 5],
}

pub struct Cull {
    dir: PathBuf,
    frames: Vec<Frame>,
    current: usize,
    filter: Filter,
    /// 1 or -1: the way the cursor last moved, for prefetching.
    direction: isize,
    /// Each step is one or more changes, undone together.
    undo: Vec<Vec<Change>>,
    redo: Vec<Vec<Change>>,
    /// Frames picked out in the filmstrip, for compare and survey.
    selected: BTreeSet<usize>,
    stacking: Stacking,
    /// The stacks made by hand, each in order.
    manual: Vec<Vec<usize>>,
    /// The stacks as the setting makes them, in order.
    stacks: Vec<Vec<usize>>,
    /// Which stack each frame is in.
    stack_of: Vec<Option<usize>>,
    /// Stacks opened out in the filmstrip; the rest show one frame.
    expanded: BTreeSet<usize>,
    /// Which of the folder's pictures were asked for.
    formats: Formats,
    /// How many raws and how many JPEGs the folder holds, in the cull or
    /// not.
    holds: (usize, usize),
}

/// The kind of picture a file is, if it's one to cull: not a hidden file,
/// nor a folder named like one.
pub(crate) fn picture(path: &Path) -> Option<Kind> {
    let hidden = path.file_name().is_some_and(|n| n.as_encoded_bytes().starts_with(b"."));
    kind(path).filter(|_| !hidden && path.is_file())
}

impl Cull {
    /// The pictures in a folder that `formats` takes, in name order, with
    /// the marks their sidecars hold. A folder with none of the kind asked
    /// for is opened on what it has. Also returns the sidecars that
    /// couldn't be read, which count as unmarked (and won't be written
    /// over: writing them fails too).
    pub fn open(dir: &Path, formats: Formats) -> io::Result<(Self, Vec<String>)> {
        let pictures: Vec<(PathBuf, Kind)> = fs::read_dir(dir)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter_map(|p| picture(&p).map(|kind| (p, kind)))
            .collect();
        if pictures.is_empty() {
            return Err(io::Error::new(ErrorKind::NotFound, format!("no raws or JPEGs in {}", dir.display())));
        }
        let raws = pictures.iter().filter(|(_, kind)| *kind == Kind::Raw).count();
        let holds = (raws, pictures.len() - raws);
        let wanted = pictures.iter().any(|&(_, kind)| formats.takes(kind));
        let mut paths: Vec<PathBuf> =
            pictures.into_iter().filter(|&(_, kind)| !wanted || formats.takes(kind)).map(|(p, _)| p).collect();
        paths.sort();
        // Each raw's mark, and its capture time and look for stacking: a
        // few small reads each, so on every core.
        let read: Vec<(Frame, Option<String>)> = paths
            .into_par_iter()
            .map(|path| {
                let (rating, problem) = match sidecar::read(&path) {
                    Ok(rating) => (rating.unwrap_or(0).clamp(REJECT, 5), None),
                    Err(e) => (0, Some(format!("{}: {e}", sidecar::path_for(&path).display()))),
                };
                let raw = RawFile::open(&path).ok();
                let captured = raw.as_ref().and_then(|r| r.captured.as_deref()).and_then(stacks::seconds);
                // Only from a thumbnail: where a raw embeds nothing small,
                // a look at every frame would hold the folder up.
                let signature = raw.as_ref().and_then(|r| {
                    let jpeg = r.read(r.thumbnail().filter(|j| j.len < 1 << 20)?).ok()?;
                    Some(Signature::of(&Image::decode_jpeg(&jpeg).ok()?))
                });
                (Frame { path, rating, captured, signature, suggested: None }, problem)
            })
            .collect();
        let (frames, problems): (Vec<Frame>, Vec<Option<String>>) = read.into_iter().unzip();
        let mut cull = Self::new(dir.to_path_buf(), frames);
        (cull.formats, cull.holds) = (formats, holds);
        Ok((cull, problems.into_iter().flatten().collect()))
    }

    pub fn new(dir: PathBuf, frames: Vec<Frame>) -> Self {
        let (undo, redo, selected) = (Vec::new(), Vec::new(), BTreeSet::new());
        let stack_of = vec![None; frames.len()];
        Self {
            dir,
            frames,
            current: 0,
            filter: Filter::All,
            direction: 1,
            undo,
            redo,
            selected,
            stacking: Stacking::Off,
            manual: Vec::new(),
            stacks: Vec::new(),
            stack_of,
            expanded: BTreeSet::new(),
            formats: Formats::All,
            holds: (0, 0),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Which of the folder's pictures were asked for.
    pub fn formats(&self) -> Formats {
        self.formats
    }

    /// How many raws and how many JPEGs the folder holds, whichever of
    /// them are being culled.
    pub fn holds(&self) -> (usize, usize) {
        self.holds
    }

    /// The pictures of the folder left out of the cull: of the other
    /// format, when it holds both and one was asked for.
    pub fn left_out(&self) -> usize {
        (self.holds.0 + self.holds.1).saturating_sub(self.frames.len())
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
        index == self.current || self.visible(index)
    }

    /// What the filter lets through, and in a collapsed stack only the
    /// frame that stands for it.
    fn visible(&self, index: usize) -> bool {
        self.matches(index)
            && match self.stack_of[index] {
                Some(stack) if !self.expanded.contains(&stack) => self.representative(stack) == Some(index),
                _ => true,
            }
    }

    /// The frame a collapsed stack shows: its best-rated frame the filter
    /// lets through; of equals, the one suggested for keeping, else the
    /// first. None if the filter lets none through.
    pub fn representative(&self, stack: usize) -> Option<usize> {
        let members = self.stacks.get(stack)?;
        let key = |&i: &usize| {
            let frame = &self.frames[i];
            (frame.rating, frame.suggested.is_some_and(|rating| rating > 0), Reverse(i))
        };
        members.iter().copied().filter(|&i| self.matches(i)).max_by_key(key)
    }

    /// Whether the filter lets a frame through.
    fn matches(&self, index: usize) -> bool {
        self.filter.matches(&self.frames[index])
    }

    /// The marks Omacull suggests, for undecided frames: these and no
    /// others.
    pub fn set_suggested(&mut self, suggested: impl IntoIterator<Item = (usize, Rating)>) {
        for frame in &mut self.frames {
            frame.suggested = None;
        }
        for (index, rating) in suggested {
            self.frames[index].suggested = Some(rating);
        }
    }

    pub fn shown_indices(&self) -> Vec<usize> {
        (0..self.frames.len()).filter(|&i| self.shown(i)).collect()
    }

    /// The frames the filter lets through after `from` going `direction`.
    fn onward(&self, from: usize, direction: isize) -> impl Iterator<Item = usize> + '_ {
        let indices: Box<dyn Iterator<Item = usize>> =
            if direction > 0 { Box::new(from + 1..self.frames.len()) } else { Box::new((0..from).rev()) };
        indices.filter(|&i| self.visible(i))
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
            Step::First => (0..self.frames.len()).find(|&i| self.visible(i)),
            Step::Last => (0..self.frames.len()).rev().find(|&i| self.visible(i)),
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
        if !filter.matches(self.frame()) {
            let to = self.onward(self.current, 1).next().or_else(|| self.onward(self.current, -1).next());
            if let Some(to) = to {
                self.current = to;
            }
        }
    }

    /// Mark the current frame. None if it already had that mark.
    pub fn mark(&mut self, rating: Rating) -> Option<Change> {
        let change = self.set(self.current, rating)?;
        self.undo.push(vec![change]);
        self.redo.clear();
        Some(change)
    }

    /// Mark several frames at once, undone as one step. Returns what
    /// changed.
    pub fn mark_many(&mut self, marks: &[(usize, Rating)]) -> Vec<Change> {
        let changes: Vec<Change> = marks.iter().filter_map(|&(index, rating)| self.set(index, rating)).collect();
        if !changes.is_empty() {
            self.undo.push(changes.clone());
            self.redo.clear();
        }
        changes
    }

    fn set(&mut self, index: usize, rating: Rating) -> Option<Change> {
        let frame = &mut self.frames[index];
        let was = std::mem::replace(&mut frame.rating, rating);
        (was != rating).then_some(Change { index, was, now: rating })
    }

    /// Take back the last step, going to its (first) frame to show what
    /// changed. Returns the changes made, none if there was nothing to undo.
    pub fn undo(&mut self) -> Vec<Change> {
        let Some(step) = self.undo.pop() else { return Vec::new() };
        self.go_to(step[0].index);
        let done = step
            .iter()
            .rev()
            .map(|c| {
                let was = std::mem::replace(&mut self.frames[c.index].rating, c.was);
                Change { index: c.index, was, now: c.was }
            })
            .collect();
        self.redo.push(step);
        done
    }

    pub fn redo(&mut self) -> Vec<Change> {
        let Some(step) = self.redo.pop() else { return Vec::new() };
        self.go_to(step[0].index);
        let done = step
            .iter()
            .map(|c| {
                let was = std::mem::replace(&mut self.frames[c.index].rating, c.now);
                Change { index: c.index, was, now: c.now }
            })
            .collect();
        self.undo.push(step);
        done
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

    pub fn stacking(&self) -> Stacking {
        self.stacking
    }

    /// Stack the folder another way.
    pub fn set_stacking(&mut self, stacking: Stacking) {
        self.stacking = stacking;
        self.restack();
    }

    /// The stacks made by hand.
    pub fn manual_stacks(&self) -> &[Vec<usize>] {
        &self.manual
    }

    /// The stacks made by hand, as remembered.
    pub fn set_manual_stacks(&mut self, stacks: Vec<Vec<usize>>) {
        let count = self.frames.len();
        self.manual = stacks
            .into_iter()
            .map(|mut s| {
                s.retain(|&i| i < count);
                s.sort();
                s.dedup();
                s
            })
            .filter(|s| s.len() > 1)
            .collect();
        self.restack();
    }

    /// Ctrl+G: stack the selected frames by hand (taking them out of other
    /// stacks), which switches stacking to by hand. False with fewer than
    /// two selected.
    pub fn stack_selection(&mut self) -> bool {
        let selected = self.selection();
        if selected.len() < 2 {
            return false;
        }
        for stack in &mut self.manual {
            stack.retain(|i| !selected.contains(i));
        }
        self.manual.retain(|s| s.len() > 1);
        self.manual.push(selected);
        self.manual.sort();
        self.selected.clear();
        self.set_stacking(Stacking::Manual);
        true
    }

    /// Take the current frame's stack apart, if it was made by hand.
    pub fn unstack(&mut self) -> bool {
        let before = self.manual.len();
        let current = self.current;
        self.manual.retain(|s| !s.contains(&current));
        let done = self.manual.len() != before;
        self.restack();
        done
    }

    fn restack(&mut self) {
        self.stacks = match self.stacking {
            Stacking::Off => Vec::new(),
            Stacking::Manual => self.manual.clone(),
            auto => {
                let frames: Vec<_> = self.frames.iter().map(|f| (f.captured, f.signature.as_ref())).collect();
                stacks::group(&frames, auto)
            }
        };
        self.stack_of = vec![None; self.frames.len()];
        for (s, stack) in self.stacks.iter().enumerate() {
            for &i in stack {
                self.stack_of[i] = Some(s);
            }
        }
        self.expanded.clear();
    }

    /// The stack a frame is in, if any.
    pub fn stack(&self, index: usize) -> Option<&[usize]> {
        self.stack_of.get(index).copied().flatten().map(|s| self.stacks[s].as_slice())
    }

    pub fn stack_count(&self) -> usize {
        self.stacks.len()
    }

    /// Every stack, in order.
    pub fn stacks(&self) -> &[Vec<usize>] {
        &self.stacks
    }

    /// Whether the stack a frame is in shows all its frames.
    pub fn expanded(&self, index: usize) -> bool {
        self.stack_of.get(index).copied().flatten().is_some_and(|s| self.expanded.contains(&s))
    }

    /// Open out the current frame's stack, or close it up, going to the
    /// frame that stands for it. False if it isn't in one.
    pub fn toggle_stack(&mut self) -> bool {
        let Some(stack) = self.stack_of[self.current] else { return false };
        if !self.expanded.remove(&stack) {
            self.expanded.insert(stack);
        } else if let Some(to) = self.representative(stack) {
            self.go_to(to);
        }
        true
    }

    /// The first frame not yet decided, from the start.
    pub fn first_undecided(&self) -> Option<usize> {
        self.frames.iter().position(|f| f.rating == 0)
    }

    pub fn counts(&self) -> Counts {
        let mut counts = Counts::default();
        for frame in &self.frames {
            match frame.rating {
                REJECT => counts.rejects += 1,
                0 => counts.undecided += 1,
                stars => {
                    counts.picks += 1;
                    counts.stars[stars.clamp(1, 5) as usize - 1] += 1;
                }
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
            .map(|(i, &rating)| Frame::new(PathBuf::from(format!("/shoot/DSC{i:05}.ARW")), rating))
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

        let (cull, problems) = Cull::open(&folder.0, Formats::All).unwrap();
        let names: Vec<_> = cull.frames().iter().map(|f| f.path.file_name().unwrap().to_str().unwrap()).collect();
        assert_eq!(names, ["DSC00000.arw", "DSC00001.ARW", "DSC00002.ARW", "DSC00003.ARW"]);
        let ratings: Vec<_> = cull.frames().iter().map(|f| f.rating).collect();
        assert_eq!(ratings, [0, 0, 4, REJECT]);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("DSC00001.ARW.xmp"), "{problems:?}");
        assert_eq!(cull.counts(), Counts { picks: 1, rejects: 1, undecided: 2, stars: [0, 0, 0, 1, 0] });
        assert_eq!(cull.first_undecided(), Some(0));

        let empty = Folder::new("cull-empty");
        assert!(Cull::open(&empty.0, Formats::All).is_err());
    }

    #[test]
    fn other_cameras_raws_are_culled_too() {
        use crate::testing::{cr3, jpeg, raf};
        let folder = Folder::new("cull-formats");
        let preview = jpeg(48, 32, [200, 120, 40]);
        let settings = Arw { preview: Vec::new(), thumbnail: Vec::new(), ..Arw::default() };
        Arw::default().write(&folder.0.join("DSC_0001.NEF"));
        fs::write(folder.0.join("IMG_0002.CR3"), cr3(&settings.bytes(), &jpeg(16, 12, [9; 3]), &preview)).unwrap();
        fs::write(folder.0.join("DSCF0003.RAF"), raf(&preview)).unwrap();
        fs::write(folder.0.join("IMG_0004.JPG"), &preview).unwrap();
        let (cull, problems) = Cull::open(&folder.0, Formats::Raw).unwrap();
        assert_eq!((cull.frames().len(), problems.len()), (3, 0));
        for frame in cull.frames() {
            let image = crate::image::preview(&frame.path).unwrap();
            assert_eq!((image.width, image.height), (48, 32), "{}", frame.path.display());
        }
        // The RAF's JPEG has no Exif: no capture time, and no harm.
        let captured: Vec<bool> = cull.frames().iter().map(|f| f.captured.is_some()).collect();
        assert_eq!(captured, [false, true, true], "by name: the RAF, the NEF, the CR3");
    }

    #[test]
    fn jpegs_are_culled_too_or_one_format_alone() {
        use crate::testing::jpeg;
        let folder = Folder::with_raws("cull-jpegs", 2, &Arw::default());
        let camera = |seconds: &'static str| Arw { captured: (seconds, "5"), ..Arw::default() }.jpeg();
        fs::write(folder.0.join("DSC00001.JPG"), camera("2026:10:04 12:00:00")).unwrap();
        fs::write(folder.0.join("DSC00002.jpeg"), camera("2026:10:04 12:00:07")).unwrap();
        fs::write(folder.0.join("phone.jpg"), jpeg(64, 48, [9; 3])).unwrap();
        fs::write(folder.0.join(".DSC00003.JPG"), jpeg(64, 48, [9; 3])).unwrap();
        fs::write(folder.0.join("scan.png"), b"").unwrap();
        // Each has a sidecar of its own.
        sidecar::write(&folder.raw(1), 3).unwrap();
        sidecar::write(&folder.0.join("DSC00001.JPG"), REJECT).unwrap();

        let names = |cull: &Cull| -> Vec<String> {
            cull.frames().iter().map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned()).collect()
        };
        let (all, problems) = Cull::open(&folder.0, Formats::All).unwrap();
        assert_eq!(names(&all), ["DSC00001.ARW", "DSC00001.JPG", "DSC00002.ARW", "DSC00002.jpeg", "phone.jpg"]);
        assert_eq!(all.frames().iter().map(|f| f.rating).collect::<Vec<_>>(), [3, REJECT, 0, 0, 0]);
        assert_eq!((problems.len(), all.holds(), all.left_out()), (0, (2, 3), 0));
        // A JPEG's capture time is read like a raw's, where it has one.
        let captured: Vec<_> = all.frames().iter().map(|f| f.captured).collect();
        assert_eq!(captured[1].unwrap() + 7.0, captured[3].unwrap());
        assert_eq!((captured[0].is_some(), captured[4]), (true, None));
        assert!(all.frames()[1].signature.is_some(), "and its look, from the thumbnail in its Exif");

        let (raws, _) = Cull::open(&folder.0, Formats::Raw).unwrap();
        assert_eq!((names(&raws), raws.holds(), raws.left_out()), (vec!["DSC00001.ARW".into(), "DSC00002.ARW".into()], (2, 3), 3));
        assert_eq!(raws.counts(), Counts { picks: 1, undecided: 1, stars: [0, 0, 1, 0, 0], ..Counts::default() });
        let (jpegs, _) = Cull::open(&folder.0, Formats::Jpeg).unwrap();
        assert_eq!((names(&jpegs).len(), jpegs.left_out(), jpegs.counts().rejects), (3, 2, 1));

        // A folder with none of what was asked for opens on what it has.
        for name in ["DSC00001.ARW", "DSC00002.ARW"] {
            fs::remove_file(folder.0.join(name)).unwrap();
        }
        let (only, _) = Cull::open(&folder.0, Formats::Raw).unwrap();
        assert_eq!((only.frames().len(), only.holds(), only.left_out()), (3, (0, 3), 0));
        assert_eq!((Formats::All.next(), Formats::Jpeg.next()), (Formats::Raw, Formats::All));
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
        assert_eq!(c.undo(), [Change { index: 1, was: 3, now: REJECT }]);
        assert_eq!(c.current(), 1, "undo goes to the frame it changed");
        assert_eq!(c.undo(), [Change { index: 1, was: REJECT, now: 0 }]);
        assert_eq!(c.undo(), [Change { index: 0, was: PICK, now: 0 }]);
        assert_eq!(c.current(), 0);
        assert_eq!(c.undo(), []);
        assert!(!c.can_undo());

        assert_eq!(c.redo(), [Change { index: 0, was: 0, now: PICK }]);
        assert_eq!(c.redo(), [Change { index: 1, was: 0, now: REJECT }]);
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
            assert!(filter.matches(c.frame()), "{filter:?} moved the cursor onto a shown frame");
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

    /// Frames taken at these seconds past noon, all of one scene.
    fn burst(times: &[f64]) -> Cull {
        let mut c = cull(&vec![0; times.len()]);
        for (frame, &t) in c.frames.iter_mut().zip(times) {
            frame.captured = Some(t);
        }
        c
    }

    #[test]
    fn collapsed_stacks_show_one_frame_the_best_rated() {
        let mut c = burst(&[0.0, 0.1, 0.2, 10.0, 20.0, 20.3]);
        c.set_stacking(Stacking::Time);
        assert_eq!(c.stack_count(), 2);
        assert_eq!(c.shown_indices(), [0, 3, 4]);
        assert!(c.step(Step::Next));
        assert_eq!(c.current(), 3, "stepping goes stack to stack");
        // A rating makes a frame the one that stands for its stack.
        c.go_to(1);
        c.mark(4);
        c.go_to(3);
        assert_eq!(c.shown_indices(), [1, 3, 4]);
        c.set_filter(Filter::Undecided);
        assert_eq!(c.shown_indices(), [0, 3, 4], "the best the filter lets through");
        c.set_filter(Filter::All);

        c.go_to(1);
        assert!(c.toggle_stack());
        assert_eq!(c.shown_indices(), [0, 1, 2, 3, 4]);
        assert!(c.expanded(1));
        c.go_to(2);
        assert!(c.toggle_stack());
        assert_eq!(c.current(), 1, "closing it goes to the frame that stands for it");
        c.go_to(3);
        assert!(!c.toggle_stack(), "not in a stack");
        c.set_stacking(Stacking::Off);
        assert_eq!(c.shown_indices().len(), 6);
    }

    #[test]
    fn suggestions_are_what_there_is_to_review_and_stand_for_their_stack() {
        let mut c = burst(&[0.0, 0.1, 0.2, 10.0, 20.0]);
        c.set_stacking(Stacking::Time);
        c.set_suggested([(0, REJECT), (1, PICK), (2, REJECT), (4, 3)]);
        assert_eq!(c.shown_indices(), [0, 1, 3, 4], "the cursor's frame, and the one suggested for keeping");
        c.go_to(3);
        assert_eq!(c.shown_indices(), [1, 3, 4]);
        c.set_filter(Filter::Suggested);
        assert_eq!(c.current(), 4, "moved onto a frame with a suggestion");
        assert_eq!(c.shown_indices(), [1, 4]);
        // Marked, a frame has nothing left to review.
        c.mark(3);
        c.step(Step::Previous);
        assert_eq!(c.shown_indices(), [1]);
        // A rating still counts for more than a suggestion.
        c.set_filter(Filter::All);
        c.go_to(2);
        c.mark(PICK);
        c.go_to(3);
        assert_eq!(c.shown_indices(), [2, 3, 4]);
        c.set_suggested([]);
        assert!(c.frames().iter().all(|f| f.suggested.is_none()));
    }

    #[test]
    fn stacks_are_made_by_hand_from_the_selection() {
        let mut c = cull(&[0; 6]);
        c.toggle_selected(1);
        c.toggle_selected(4);
        assert!(c.stack_selection());
        assert_eq!((c.stacking(), c.manual_stacks()), (Stacking::Manual, &[vec![0, 1, 4]][..]));
        assert!(c.selection().is_empty());
        assert_eq!(c.shown_indices(), [0, 2, 3, 4, 5], "the current frame stays");
        c.go_to(0);
        assert_eq!(c.shown_indices(), [0, 2, 3, 5]);
        assert!(!c.stack_selection(), "nothing selected");
        c.go_to(0);
        assert!(c.unstack());
        assert!(c.manual_stacks().is_empty());
        c.set_manual_stacks(vec![vec![5, 2, 2], vec![9, 3], vec![1]]);
        assert_eq!(c.manual_stacks(), [vec![2, 5]], "tidied: in order, in range, two or more");
    }

    #[test]
    fn marking_many_is_one_step_to_undo() {
        let mut c = cull(&[0, 0, 0, 0]);
        c.go_to(2);
        let changes = c.mark_many(&[(2, PICK), (0, REJECT), (1, REJECT), (3, 0)]);
        assert_eq!(changes.len(), 3, "the last was already unmarked");
        assert_eq!(c.frames().iter().map(|f| f.rating).collect::<Vec<_>>(), [REJECT, REJECT, PICK, 0]);
        c.go_to(3);
        assert_eq!(c.undo().len(), 3);
        assert_eq!(c.current(), 2);
        assert!(c.frames().iter().all(|f| f.rating == 0));
        assert_eq!(c.redo().len(), 3);
        assert_eq!(c.frames()[2].rating, PICK);
    }

    #[test]
    fn opening_reads_capture_times_for_stacking() {
        let folder = Folder::new("cull-times");
        for (i, (time, colour)) in [("12:00:00", 40), ("12:00:00", 40), ("12:00:05", 220)].iter().enumerate() {
            let captured: &'static str = Box::leak(format!("2026:10:04 {time}").into_boxed_str());
            let thumbnail = crate::testing::jpeg(16, 12, [*colour; 3]);
            Arw { captured: (captured, "100"), thumbnail, ..Arw::default() }.write(&folder.raw(i + 1));
        }
        let (mut c, _) = Cull::open(&folder.0, Formats::All).unwrap();
        assert!(c.frames().iter().all(|f| f.captured.is_some() && f.signature.is_some()));
        c.set_stacking(Stacking::Time);
        assert_eq!(c.stack(0), Some(&[0, 1][..]));
        assert_eq!(c.stack(2), None);
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
