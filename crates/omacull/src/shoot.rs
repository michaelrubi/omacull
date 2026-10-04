//! An open folder on screen: the frames in their panes (the loupe, compare
//! or survey) and the filmstrip, and the decoded frames behind them, kept
//! ahead of the cursor.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{
    Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui, Vec2, pos2,
    vec2,
};
use omacull_engine::color::Display;
use omacull_engine::faces::Face;
use omacull_engine::cull::{Cull, PICK, Rating};
use omacull_engine::loader::{Decoded, Job, Loaded, Loader, Output};
use omacull_engine::raw::Info;
use omacull_engine::sidecar::REJECT;

use crate::faces::{Faces, Finder};
use crate::loupe::{self, Bake, View};
use crate::panes::{self, Mode, Pane};
use crate::state::Show;
use crate::theme::Theme;

/// Previews decoded ahead of the cursor, the way it's going, and behind.
const AHEAD: usize = 4;
const BEHIND: usize = 2;
/// Thumbnails kept further than this from the cursor are let go.
const KEEP_THUMBNAILS: usize = 100;
pub const STRIP_HEIGHT: f32 = 104.0;
/// Full-size frames are uploaded in tiles of this many pixels square, as
/// they come into view: a 24 MP frame is 96 MB, too much to upload at once
/// without a stutter, and bigger than some GPUs take in one texture.
const TILE: usize = 512;
/// The face close-ups beside the loupe.
pub const FACES_WIDTH: f32 = 150.0;
/// Close-ups from the full-size frame are this many pixels square.
const CLOSE_UP: usize = 256;

/// Something decoded and on the GPU, or why it isn't.
enum Slot<T> {
    Ready(T),
    Failed(String),
}

/// A preview, with the overlays baked into its texture.
struct Shown {
    decoded: Decoded,
    texture: TextureHandle,
    baked: Bake,
}

/// A full-size frame, uploaded a tile at a time as they're seen.
struct Full {
    decoded: Decoded,
    tiles: HashMap<(usize, usize), TextureHandle>,
    baked: Bake,
}

fn bake_of(show: Show) -> Bake {
    Bake { clipping: show.clipping, peaking: show.peaking }
}

fn upload(ctx: &egui::Context, name: String, decoded: &Decoded, bake: Bake, region: [usize; 4]) -> TextureHandle {
    upload_with(ctx, name, decoded, bake, region, TextureOptions::LINEAR)
}

fn upload_with(
    ctx: &egui::Context,
    name: String,
    decoded: &Decoded,
    bake: Bake,
    region: [usize; 4],
    filter: TextureOptions,
) -> TextureHandle {
    ctx.load_texture(name, loupe::bake(&decoded.image, &decoded.marks, bake, region), filter)
}

fn whole(decoded: &Decoded) -> [usize; 4] {
    [0, 0, decoded.image.width, decoded.image.height]
}

pub struct Shoot {
    pub cull: Cull,
    loader: Loader,
    previews: HashMap<usize, Slot<Shown>>,
    full: HashMap<usize, Slot<Full>>,
    thumbnails: HashMap<usize, Slot<TextureHandle>>,
    /// Frames whose thumbnail is in the disk cache.
    cached: HashSet<usize>,
    /// What the loader was last asked for.
    wanted: Vec<Job>,
    /// How many thumbnails either side of the current one the filmstrip
    /// showed last time it was drawn.
    reach: usize,
    /// The filmstrip's frames where they were last drawn.
    pub cells: Vec<(usize, Rect)>,
    /// The frame on screen, and since when.
    seen: usize,
    arrived: Instant,
    pub mode: Mode,
    /// The frames on screen. The current frame's pane is the active one,
    /// which marks and zoom keys go to.
    pub panes: Vec<Pane>,
    /// Compare's and survey's panes zoom and pan together.
    pub locked: bool,
    pub faces: Faces,
    /// The face the zoom last went to, in the current frame, by its place
    /// left to right; E again goes to the next.
    eyes: Option<(usize, usize)>,
    /// The current frame's face close-ups, and whether they're from its
    /// full-size frame (else they're cut from the preview as it's drawn).
    close_ups: Option<(usize, Vec<TextureHandle>)>,
    /// The close-ups where they were last drawn, for tests.
    pub close_up_cells: Vec<Rect>,
}

/// How a filmstrip click was meant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    Plain,
    /// Ctrl: in or out of the selection.
    Toggle,
    /// Shift: the selection runs to here.
    Range,
}

/// "★★★" for three stars; a pick is one star.
pub fn stars(rating: Rating) -> String {
    "★".repeat(rating.max(0) as usize)
}

impl Shoot {
    pub fn new(
        cull: Cull,
        cache: Option<PathBuf>,
        faces: (Option<PathBuf>, Finder),
        display: Arc<Display>,
        ctx: &egui::Context,
    ) -> Self {
        let raws: Vec<PathBuf> = cull.frames().iter().map(|f| f.path.clone()).collect();
        let (wake, wake_faces) = (ctx.clone(), ctx.clone());
        let faces = Faces::new(raws.clone(), faces.0, faces.1, move || wake_faces.request_repaint());
        let seen = cull.current();
        Self {
            cull,
            loader: Loader::new(raws, cache, display, move || wake.request_repaint()),
            previews: HashMap::new(),
            full: HashMap::new(),
            thumbnails: HashMap::new(),
            cached: HashSet::new(),
            wanted: Vec::new(),
            reach: 10,
            cells: Vec::new(),
            seen,
            arrived: Instant::now(),
            mode: Mode::Loupe,
            panes: vec![Pane::new(seen, View::default())],
            locked: true,
            faces,
            eyes: None,
            close_ups: None,
            close_up_cells: Vec::new(),
        }
    }

    /// E: zoom to a face's eyes in the current frame. The first time, the
    /// face nearest the pointer if it's on the frame, else the largest;
    /// again, the next face to the right, round to the first.
    pub fn zoom_to_eyes(&mut self, pointer: Option<Pos2>, ppp: f32) -> Result<(), String> {
        let current = self.cull.current();
        let faces = match self.faces.of(current) {
            Some(faces) => faces,
            None => return Err(self.faces.unavailable.clone().unwrap_or_else(|| "Still looking for faces".into())),
        };
        if faces.is_empty() {
            return Err("No faces found in this frame".into());
        }
        let next = match self.eyes {
            Some((frame, k)) if frame == current && self.view().zoomed => (k + 1) % faces.len(),
            _ => {
                let fit = self.fit_rect();
                let at = pointer.filter(|p| fit.contains(*p)).map(|p| ((p - fit.min) / fit.size()).to_pos2());
                let distance = |f: &Face, at: Pos2| {
                    let [x, y] = f.between_eyes();
                    (x - at.x).powi(2) + (y - at.y).powi(2)
                };
                let by = |key: &dyn Fn(&Face) -> f32| {
                    (0..faces.len()).min_by(|&a, &b| key(&faces[a]).total_cmp(&key(&faces[b]))).unwrap_or(0)
                };
                match at {
                    Some(at) => by(&|f| distance(f, at)),
                    None => by(&|f| -f.area()),
                }
            }
        };
        self.go_to_face(next, &faces, ppp);
        Ok(())
    }

    fn go_to_face(&mut self, k: usize, faces: &[Face], ppp: f32) {
        let size = self.full_size(ppp);
        self.view_mut().zoom_to(faces[k].between_eyes(), size);
        self.eyes = Some((self.cull.current(), k));
        self.sync(self.active());
    }

    /// Which pane is the current frame's.
    pub fn active(&self) -> usize {
        self.panes.iter().position(|p| p.frame == self.cull.current()).unwrap_or(0)
    }

    pub fn view(&self) -> &View {
        &self.panes[self.active()].view
    }

    pub fn view_mut(&mut self) -> &mut View {
        let active = self.active();
        &mut self.panes[active].view
    }

    /// The frames on screen besides the current one, for the decision log.
    pub fn others(&self) -> Vec<usize> {
        let current = self.cull.current();
        self.panes.iter().map(|p| p.frame).filter(|&f| f != current).collect()
    }

    /// When locked, the other panes take on `from`'s zoom.
    pub fn sync(&mut self, from: usize) {
        if !self.locked || from >= self.panes.len() {
            return;
        }
        let view = self.panes[from].view.clone();
        for (k, pane) in self.panes.iter_mut().enumerate() {
            if k != from {
                pane.view.follow(&view);
                pane.followed = None;
            }
        }
    }

    fn enter(&mut self, mode: Mode, frames: Vec<usize>) {
        let view = self.view().clone();
        self.panes = frames.into_iter().map(|f| Pane::new(f, view.clone())).collect();
        self.mode = mode;
    }

    /// Back to one frame, the current one, zoomed as it was.
    pub fn back_to_loupe(&mut self) {
        let view = self.view().clone();
        self.panes = vec![Pane::new(self.cull.current(), view)];
        self.mode = Mode::Loupe;
    }

    /// Compare two frames: the first two selected, or the current one and
    /// the next. Again, back to the loupe. Says why not if it can't.
    pub fn compare(&mut self) -> Result<(), &'static str> {
        if self.mode == Mode::Compare {
            self.back_to_loupe();
            return Ok(());
        }
        let current = self.cull.current();
        let selected = self.cull.selection();
        let other = match selected.iter().find(|&&f| f != current) {
            Some(&f) if selected.len() >= 2 => Some(f),
            _ => self.cull.candidate(current, 1, &[current]).or_else(|| self.cull.candidate(current, -1, &[current])),
        };
        let other = other.ok_or("Nothing to compare with")?;
        let mut frames = vec![current, other];
        frames.sort();
        self.enter(Mode::Compare, frames);
        Ok(())
    }

    /// Survey the selected frames, or the current one and the next three.
    /// Again, back to the loupe.
    pub fn survey(&mut self) -> Result<(), &'static str> {
        if self.mode == Mode::Survey {
            self.back_to_loupe();
            return Ok(());
        }
        let current = self.cull.current();
        let mut frames = self.cull.selection();
        // With nothing selected, a stack is surveyed whole.
        if frames.len() < 2
            && let Some(stack) = self.cull.stack(current)
        {
            frames = stack.to_vec();
        }
        if frames.len() < 2 {
            frames = vec![current];
            for direction in [1, -1] {
                while frames.len() < 4 {
                    let ends = (frames.iter().min().copied(), frames.iter().max().copied());
                    let from = if direction > 0 { ends.1 } else { ends.0 }.unwrap_or(current);
                    match self.cull.candidate(from, direction, &frames) {
                        Some(f) => frames.push(f),
                        None => break,
                    }
                }
            }
            frames.sort();
        }
        if frames.len() < 2 {
            return Err("Nothing to survey with");
        }
        if !frames.contains(&current) {
            self.cull.go_to(frames[0]);
        }
        self.enter(Mode::Survey, frames);
        Ok(())
    }

    /// The active frame out of the survey, or out of compare for the next
    /// candidate. With one left, back to the loupe on it.
    pub fn knock_out(&mut self) {
        let active = self.active();
        match self.mode {
            Mode::Loupe => {}
            Mode::Compare => {
                let frames: Vec<usize> = self.panes.iter().map(|p| p.frame).collect();
                let last = *frames.iter().max().unwrap();
                match self.cull.candidate(last, 1, &frames) {
                    Some(next) => {
                        self.panes[active].frame = next;
                        self.panes[active].followed = None;
                        self.cull.go_to(next);
                    }
                    None => {
                        self.cull.go_to(frames[1 - active]);
                        self.back_to_loupe();
                    }
                }
            }
            Mode::Survey => {
                self.panes.remove(active);
                let next = self.panes[active.min(self.panes.len() - 1)].frame;
                self.cull.go_to(next);
                if self.panes.len() == 1 {
                    self.back_to_loupe();
                }
            }
        }
    }

    /// The arrow keys away from the loupe: in survey they move between
    /// panes; in compare they change the active pane's frame.
    pub fn step_panes(&mut self, direction: isize) {
        let active = self.active();
        match self.mode {
            Mode::Loupe => {}
            Mode::Survey => {
                let to = active.saturating_add_signed(direction).min(self.panes.len() - 1);
                self.cull.go_to(self.panes[to].frame);
            }
            Mode::Compare => {
                let frames: Vec<usize> = self.panes.iter().map(|p| p.frame).collect();
                if let Some(next) = self.cull.candidate(frames[active], direction, &frames) {
                    self.panes[active].frame = next;
                    self.panes[active].followed = None;
                    self.cull.go_to(next);
                }
            }
        }
    }

    /// W: the current frame wins, picked unless it's rated already, and
    /// the rest are rejected: the others in the survey, or in the loupe,
    /// the rest of its stack. Returns the marks to make, or why not.
    pub fn winner(&self) -> Result<Vec<(usize, Rating)>, &'static str> {
        let current = self.cull.current();
        let others: Vec<usize> = match self.mode {
            Mode::Survey | Mode::Compare => self.others(),
            Mode::Loupe => match self.cull.stack(current) {
                Some(stack) => stack.iter().copied().filter(|&f| f != current).collect(),
                None => return Err("A winner is chosen in a survey or from a stack"),
            },
        };
        let rating = self.cull.frames()[current].rating.max(PICK);
        Ok(std::iter::once((current, rating)).chain(others.into_iter().map(|f| (f, REJECT))).collect())
    }

    /// Tab: the next pane is the active one.
    pub fn next_pane(&mut self) {
        let next = (self.active() + 1) % self.panes.len();
        self.cull.go_to(self.panes[next].frame);
    }

    /// A frame clicked in the filmstrip.
    pub fn clicked(&mut self, index: usize, click: Click) {
        match click {
            Click::Toggle => self.cull.toggle_selected(index),
            Click::Range => self.cull.select_to(index),
            Click::Plain => {
                self.cull.clear_selection();
                if self.mode == Mode::Compare && !self.panes.iter().any(|p| p.frame == index) {
                    // In compare, the clicked frame takes the active side.
                    let active = self.active();
                    self.panes[active].frame = index;
                    self.panes[active].followed = None;
                }
                self.cull.go_to(index);
            }
        }
    }

    /// Show frames in another monitor's colours: everything is decoded
    /// again.
    pub fn set_display(&mut self, display: Arc<Display>) {
        self.loader.set_display(display);
        self.previews.clear();
        self.full.clear();
        self.thumbnails.clear();
        self.wanted.clear();
    }

    /// How long the current frame has been on screen.
    pub fn dwell(&mut self) -> Duration {
        if self.seen != self.cull.current() {
            self.seen = self.cull.current();
            self.arrived = Instant::now();
        }
        self.arrived.elapsed()
    }

    /// The shooting settings and focus point of the current frame, once
    /// its preview is in.
    pub fn info(&self) -> Option<&Info> {
        self.info_of(self.cull.current())
    }

    fn info_of(&self, frame: usize) -> Option<&Info> {
        match self.previews.get(&frame) {
            Some(Slot::Ready(shown)) => Some(&shown.decoded.info),
            _ => None,
        }
    }

    /// The current frame's size at 100%, in points: the full-size frame's
    /// once it's developed, else what the raw says it will be.
    pub fn full_size(&self, pixels_per_point: f32) -> Vec2 {
        self.full_size_of(self.cull.current(), pixels_per_point)
    }

    fn full_size_of(&self, frame: usize, pixels_per_point: f32) -> Vec2 {
        let current = frame;
        let pixels = match (self.full.get(&current), self.previews.get(&current)) {
            (Some(Slot::Ready(full)), _) => vec2(full.decoded.image.width as f32, full.decoded.image.height as f32),
            (_, Some(Slot::Ready(shown))) => match shown.decoded.info.size {
                Some((w, h)) => vec2(w as f32, h as f32),
                None => vec2(shown.decoded.image.width as f32, shown.decoded.image.height as f32) * 4.0,
            },
            _ => vec2(6000.0, 4000.0),
        };
        pixels / pixels_per_point
    }

    /// Where the current frame is drawn whole, in its pane as last drawn.
    pub fn fit_rect(&self) -> Rect {
        let area = self.view().area;
        self.size_of(self.cull.current()).map_or(area, |size| fit(size, area))
    }

    /// A frame's shape, from its preview or its thumbnail.
    fn size_of(&self, frame: usize) -> Option<Vec2> {
        match (self.previews.get(&frame), self.thumbnails.get(&frame)) {
            (Some(Slot::Ready(shown)), _) => Some(shown.texture.size_vec2()),
            (_, Some(Slot::Ready(texture))) => Some(texture.size_vec2()),
            _ => None,
        }
    }

    #[cfg(test)]
    /// Whether the current frame's preview is decoded and on the GPU.
    pub fn preview_ready(&self) -> bool {
        self.has_preview(self.cull.current())
    }

    #[cfg(test)]
    pub fn has_preview(&self, index: usize) -> bool {
        matches!(self.previews.get(&index), Some(Slot::Ready(_)))
    }

    #[cfg(test)]
    pub fn has_thumbnail(&self, index: usize) -> bool {
        matches!(self.thumbnails.get(&index), Some(Slot::Ready(_)))
    }

    #[cfg(test)]
    /// Whether a full-size frame was tried, and why it failed if it did.
    pub fn full_state(&self, index: usize) -> Option<Result<usize, String>> {
        self.full.get(&index).map(|slot| match slot {
            Slot::Ready(full) => Ok(full.tiles.len()),
            Slot::Failed(e) => Err(e.clone()),
        })
    }

    #[cfg(test)]
    /// A full-size frame as if the loader had developed it.
    pub fn insert_full(&mut self, index: usize, decoded: Decoded) {
        self.full.insert(index, Slot::Ready(Full { decoded, tiles: HashMap::new(), baked: Bake::default() }));
    }

    #[cfg(test)]
    /// The current preview's pixel at (`x`, `y`) as uploaded.
    pub fn preview_bake(&self) -> Option<Bake> {
        match self.previews.get(&self.cull.current()) {
            Some(Slot::Ready(shown)) => Some(shown.baked),
            _ => None,
        }
    }

    #[cfg(test)]
    /// Whether the loader has nothing left to do.
    pub fn idle(&self) -> bool {
        !self.loader.busy()
    }

    /// Take in what has loaded, and ask for what's wanted next.
    pub fn update(&mut self, ctx: &egui::Context, show: Show) {
        self.dwell();
        self.faces.update();
        let current = self.cull.current();
        // The loupe's pane shows the current frame; elsewhere, a frame
        // that isn't on screen (undo went to it) takes back to the loupe.
        match self.mode {
            Mode::Loupe => self.panes[0].frame = current,
            _ if !self.panes.iter().any(|p| p.frame == current) => self.back_to_loupe(),
            _ => {}
        }
        let on_screen: Vec<usize> = self.panes.iter().map(|p| p.frame).collect();
        let mut ring = vec![current];
        ring.extend(on_screen.iter().copied().filter(|&f| f != current));
        ring.extend(self.cull.neighbours(AHEAD, BEHIND).into_iter().filter(|f| !on_screen.contains(f)));
        // Full-size frames for what's on screen; in the loupe, for the
        // neighbours too.
        let mut full_ring = ring[..on_screen.len()].to_vec();
        if self.mode == Mode::Loupe {
            full_ring.extend(self.cull.neighbours(1, 1));
        }
        let near = |i: &usize| i.abs_diff(current) <= KEEP_THUMBNAILS;
        self.previews.retain(|i, _| ring.contains(i));
        self.full.retain(|i, _| full_ring.contains(i));
        self.thumbnails.retain(|i, _| near(i));
        let bake = bake_of(show);

        let loaded: Vec<Loaded> = self.loader.loaded().collect();
        for Loaded { job, result } in loaded {
            // What the cursor has left behind isn't put on the GPU.
            match (job, result) {
                (Job::Preview(i), Ok(Output::Preview(decoded))) if ring.contains(&i) => {
                    let texture = upload(ctx, format!("preview-{i}"), &decoded, bake, whole(&decoded));
                    self.previews.insert(i, Slot::Ready(Shown { decoded, texture, baked: bake }));
                }
                (Job::Full(i), Ok(Output::Full(decoded))) if full_ring.contains(&i) => {
                    self.full.insert(i, Slot::Ready(Full { decoded, tiles: HashMap::new(), baked: bake }));
                }
                (Job::Thumbnail(i), Ok(Output::Thumbnail(image))) if near(&i) => {
                    self.cached.insert(i);
                    let pixels = egui::ColorImage::from_rgba_unmultiplied([image.width, image.height], &image.rgba);
                    let texture = ctx.load_texture(format!("thumbnail-{i}"), pixels, TextureOptions::LINEAR);
                    self.thumbnails.insert(i, Slot::Ready(texture));
                }
                (Job::Preview(i), Err(e)) if ring.contains(&i) => {
                    self.previews.insert(i, Slot::Failed(e.to_string()));
                }
                (Job::Full(i), Err(e)) if full_ring.contains(&i) => {
                    log::warn!("can't develop {}: {e}", self.cull.frames()[i].path.display());
                    self.full.insert(i, Slot::Failed(e.to_string()));
                }
                (Job::Thumbnail(i), Err(e)) if near(&i) => {
                    self.cached.insert(i);
                    self.thumbnails.insert(i, Slot::Failed(e.to_string()));
                }
                (Job::Thumbnail(i) | Job::Cache(i), _) => _ = self.cached.insert(i),
                _ => {}
            }
        }

        // The overlays as they're set now, on the frames on screen; others
        // catch up when they're stepped to. Only they keep tiles on the GPU.
        for &frame in &on_screen {
            if let Some(Slot::Ready(shown)) = self.previews.get_mut(&frame)
                && shown.baked != bake
            {
                shown.texture = upload(ctx, format!("preview-{frame}"), &shown.decoded, bake, whole(&shown.decoded));
                shown.baked = bake;
            }
        }
        for (i, slot) in &mut self.full {
            if let Slot::Ready(full) = slot
                && (!on_screen.contains(i) || full.baked != bake)
            {
                full.tiles.clear();
                full.baked = bake;
            }
        }
        // Following focus points, each pane goes to its own frame's.
        let ppp = ctx.pixels_per_point();
        for k in 0..self.panes.len() {
            let frame = self.panes[k].frame;
            if !self.panes[k].view.follow_focus {
                self.panes[k].followed = None;
            } else if self.panes[k].followed != Some(frame)
                && let Some(focus) = self.info_of(frame).map(|info| info.focus)
            {
                let size = self.full_size_of(frame, ppp);
                self.panes[k].view.arrive(focus, size);
                self.panes[k].followed = Some(frame);
            }
        }

        // The current frame first, then the next one, then the filmstrip,
        // then the rest of the ring, then full-size frames (the current one
        // straight after its preview when zoomed); and while nothing else is
        // wanted, the whole folder's thumbnails into the cache, nearest
        // first.
        let shown = self.cull.shown_indices();
        let at = shown.iter().position(|&i| i == current).unwrap_or(0);
        let reach = at.saturating_sub(self.reach)..(at + self.reach + 1).min(shown.len());
        let mut strip: Vec<usize> = shown[reach].to_vec();
        strip.sort_by_key(|&i| i.abs_diff(current));
        let previews = ring.iter().map(|&i| Job::Preview(i));
        let full = full_ring.iter().map(|&i| Job::Full(i));
        let first = if self.view().zoomed { 1 } else { 0 };
        let thumbnails = strip.into_iter().map(Job::Thumbnail);
        let mut uncached: Vec<usize> = (0..self.cull.frames().len()).filter(|i| !self.cached.contains(i)).collect();
        uncached.sort_by_key(|&i| i.abs_diff(current));
        let wanted: Vec<Job> = previews
            .clone()
            .take(1)
            .chain(full.clone().take(first))
            .chain(previews.clone().skip(1).take(on_screen.len()))
            .chain(thumbnails)
            .chain(previews.skip(1 + on_screen.len()))
            .chain(full.skip(first))
            .chain(uncached.into_iter().map(Job::Cache))
            .filter(|job| match *job {
                Job::Preview(i) => !self.previews.contains_key(&i),
                Job::Full(i) => !self.full.contains_key(&i),
                Job::Thumbnail(i) => !self.thumbnails.contains_key(&i),
                Job::Cache(i) => !self.cached.contains(&i),
            })
            .collect();
        if wanted != self.wanted {
            self.loader.want(wanted.iter().copied());
            self.wanted = wanted;
        }
        // Faces: what's on screen and near it first, then the whole folder.
        let mut order = ring.clone();
        let mut rest: Vec<usize> = (0..self.cull.frames().len()).filter(|i| !ring.contains(i)).collect();
        rest.sort_by_key(|&i| i.abs_diff(current));
        order.extend(rest);
        self.faces.want(order);

        // Close-ups for the current frame, sharp once it's developed.
        let developed = matches!(self.full.get(&current), Some(Slot::Ready(_)));
        let stale = self.close_ups.as_ref().is_some_and(|(frame, _)| *frame != current);
        if stale || (developed && self.close_ups.is_none()) {
            self.close_ups = None;
            if let (Some(Slot::Ready(full)), Some(faces)) = (self.full.get(&current), self.faces.of(current)) {
                let image = &full.decoded.image;
                let textures = faces
                    .iter()
                    .enumerate()
                    .map(|(k, face)| {
                        let [x, y, side] = close_up(face, image.width as f32, image.height as f32);
                        let crop = image.crop(x as usize, y as usize, side as usize, side as usize).shrunk(CLOSE_UP);
                        let pixels = egui::ColorImage::from_rgba_unmultiplied([crop.width, crop.height], &crop.rgba);
                        ctx.load_texture(format!("face-{current}-{k}"), pixels, TextureOptions::LINEAR)
                    })
                    .collect();
                self.close_ups = Some((current, textures));
            }
        }
    }

    /// The current frame's faces, close up, top to bottom in a column.
    /// A click zooms to that face's eyes.
    pub fn face_strip(&mut self, ui: &mut Ui, theme: &Theme) {
        let current = self.cull.current();
        self.close_up_cells.clear();
        let Some(faces) = self.faces.of(current) else {
            let text = self.faces.unavailable.clone().unwrap_or_else(|| "Looking for faces…".into());
            ui.label(egui::RichText::new(text).color(theme.dark_foreground));
            return;
        };
        if faces.is_empty() {
            ui.label(egui::RichText::new("No faces").color(theme.dark_foreground));
            return;
        }
        let side = ui.available_width();
        let mut clicked = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for (k, face) in faces.iter().enumerate() {
                let (rect, response) = ui.allocate_exact_size(vec2(side, side), Sense::click());
                self.close_up_cells.push(rect);
                let painter = ui.painter_at(rect);
                let on = self.eyes == Some((current, k)) && self.view().zoomed;
                match (&self.close_ups, self.previews.get(&current)) {
                    (Some((frame, textures)), _) if *frame == current && k < textures.len() => {
                        let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
                        painter.image(textures[k].id(), rect, uv, Color32::WHITE);
                    }
                    (_, Some(Slot::Ready(shown))) => {
                        // Cut from the preview until the frame is developed.
                        let size = shown.texture.size_vec2();
                        let [x, y, s] = close_up(face, size.x, size.y);
                        let uv = Rect::from_min_size(pos2(x / size.x, y / size.y), vec2(s / size.x, s / size.y));
                        painter.image(shown.texture.id(), rect, uv, Color32::WHITE);
                    }
                    _ => {}
                }
                let stroke = if on { Stroke::new(2.0, theme.accent) } else { Stroke::new(1.0, theme.selection) };
                painter.rect_stroke(rect.shrink(1.0), 2.0, stroke, StrokeKind::Inside);
                if response.clicked() {
                    clicked = Some(k);
                }
                ui.add_space(4.0);
            }
        });
        if let Some(k) = clicked {
            self.go_to_face(k, &faces, ui.ctx().pixels_per_point());
        }
    }

    /// The frames on screen, each in its pane, whole or at 100%, with
    /// what's to be shown over it.
    pub fn loupe(&mut self, ui: &mut Ui, theme: &Theme, show: Show) {
        let whole_area = ui.max_rect();
        let areas = match self.mode {
            Mode::Loupe => vec![whole_area],
            _ => {
                let aspects: Vec<f32> =
                    self.panes.iter().map(|p| self.size_of(p.frame).map_or(1.5, |s| s.x / s.y)).collect();
                // Room round each frame for the active one's outline.
                let rects = panes::layout(&aspects, whole_area.shrink(4.0), 8.0);
                rects.into_iter().map(|r| r.expand(4.0)).collect()
            }
        };
        let active = self.active();
        let pressed = ui.input(|i| i.pointer.primary_pressed());
        for (k, area) in areas.into_iter().enumerate() {
            let response = ui.interact(area, egui::Id::new(("pane", k)), Sense::click_and_drag());
            // A press on another pane makes it the active one, and does no
            // more.
            let activates = k != active && pressed && response.is_pointer_button_down_on();
            if activates {
                self.cull.go_to(self.panes[k].frame);
            }
            self.panes[k].view.area = area.shrink(8.0);
            if !activates && (k == active || self.mode == Mode::Loupe) {
                let frame = self.panes[k].frame;
                let size = self.full_size_of(frame, ui.ctx().pixels_per_point());
                let fit_rect = self.size_of(frame).map_or(area, |s| fit(s, self.panes[k].view.area));
                let before = self.panes[k].view.clone();
                self.panes[k].view.pointer(ui, &response, fit_rect, size);
                if self.panes[k].view != before {
                    self.sync(k);
                }
            }
            self.pane(ui, k, area, theme, show);
        }
    }

    /// One pane: its frame, and what's shown over it.
    fn pane(&mut self, ui: &mut Ui, k: usize, outer: Rect, theme: &Theme, show: Show) {
        let ctx = ui.ctx().clone();
        let ppp = ctx.pixels_per_point();
        let Pane { frame: index, ref view, .. } = self.panes[k];
        let area = view.area;
        let size = self.full_size_of(index, ppp);
        let painter = ui.painter_at(outer);
        let frame = &self.cull.frames()[index];
        let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
        let several = self.panes.len() > 1;
        if several && index == self.cull.current() {
            painter.rect_stroke(outer.shrink(1.0), 2.0, Stroke::new(2.0, theme.accent), StrokeKind::Inside);
        }
        // The thumbnail, enlarged, until the preview is ready.
        let texture = match (self.previews.get(&index), self.thumbnails.get(&index)) {
            (Some(Slot::Ready(shown)), _) => Some(&shown.texture),
            (_, Some(Slot::Ready(t))) => Some(t),
            _ => None,
        };
        let Some(texture) = texture else {
            if let Some(Slot::Failed(e)) = self.previews.get(&index) {
                let name = frame.path.file_name().unwrap_or_default().to_string_lossy();
                let text = format!("Can't show {name}: {e}");
                painter.text(area.center(), Align2::CENTER_CENTER, text, FontId::proportional(14.0), theme.red);
            }
            return;
        };

        let zoomed = view.zoomed;
        let drawn = if zoomed {
            let rect = view.zoomed_rect(size, ppp);
            match self.full.get_mut(&index) {
                Some(Slot::Ready(full)) => {
                    let visible = outer.expand(TILE as f32 / ppp);
                    tiles(&ctx, &painter, full, rect, visible, ppp);
                }
                other => {
                    // The preview, enlarged, until the raw is developed.
                    painter.image(texture.id(), rect, uv, Color32::WHITE);
                    let (text, colour) = match other {
                        // rawler's reasons run long; they're in the log.
                        Some(Slot::Failed(_)) => {
                            ("Can't develop this raw: showing the preview enlarged".to_owned(), theme.red)
                        }
                        _ => ("Developing the raw…".to_owned(), theme.foreground),
                    };
                    let at = area.center_top() + vec2(0.0, 12.0);
                    loupe::plate(&painter, at, Align2::CENTER_TOP, text, colour, area.width());
                }
            }
            rect
        } else {
            let rect = fit(texture.size_vec2(), area);
            painter.image(texture.id(), rect, uv, Color32::WHITE);
            rect
        };

        let frame = &self.cull.frames()[index];
        let info = self.info_of(index);
        if show.focus_point
            && let Some(focus) = info.and_then(|i| i.focus)
        {
            loupe::focus_point(&painter, drawn, focus, theme);
        }
        let badge_at = if zoomed { area.left_top() } else { drawn.left_top() };
        badge(&painter, badge_at + vec2(8.0, 8.0), Align2::LEFT_TOP, frame.rating, 18.0, theme);
        if let Some(Slot::Ready(shown)) = self.previews.get(&index) {
            if show.histogram {
                loupe::histogram(&painter, area, &shown.decoded.histogram);
            }
            let summary = shown.decoded.info.summary();
            if show.info && !summary.is_empty() {
                let at = area.center_bottom() - vec2(0.0, 10.0);
                loupe::plate(&painter, at, Align2::CENTER_BOTTOM, summary, theme.foreground, area.width());
            }
        }
        if several {
            let name = frame.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let at = area.right_top() + vec2(-8.0, 8.0);
            loupe::plate(&painter, at, Align2::RIGHT_TOP, name, theme.foreground, area.width() / 2.0);
        }
    }

    /// The shown frames in a strip, the current one in the middle. Returns
    /// the frame clicked, and how.
    pub fn filmstrip(&mut self, ui: &mut Ui, theme: &Theme) -> Option<(usize, Click)> {
        let (strip, response) = ui.allocate_exact_size(vec2(ui.available_width(), STRIP_HEIGHT), Sense::click());
        let painter = ui.painter_at(strip);
        let cell = vec2(STRIP_HEIGHT * 1.5, STRIP_HEIGHT);
        self.reach = (strip.width() / cell.x / 2.0).ceil() as usize + 1;
        let current = self.cull.current();
        let shown = self.cull.shown_indices();
        let at = shown.iter().position(|&i| i == current).unwrap_or(0);
        self.cells.clear();
        for (k, &i) in shown.iter().enumerate().skip(at.saturating_sub(self.reach)).take(self.reach * 2 + 1) {
            let x = strip.center().x + (k as f32 - at as f32) * cell.x;
            let rect = Rect::from_center_size(pos2(x, strip.center().y), cell);
            self.cells.push((i, rect));
            let rating = self.cull.frames()[i].rating;
            if i == current || self.cull.is_selected(i) {
                painter.rect_filled(rect.shrink(1.0), 0.0, theme.selection);
            }
            if i == current {
                painter.rect_stroke(rect.shrink(2.0), 0.0, Stroke::new(2.0, theme.accent), StrokeKind::Inside);
            } else if self.panes.len() > 1 && self.panes.iter().any(|p| p.frame == i) {
                // On screen in compare or survey.
                painter.rect_stroke(rect.shrink(2.0), 0.0, Stroke::new(1.0, theme.accent), StrokeKind::Inside);
            }
            let inner = rect.shrink(6.0);
            let stack = self.cull.stack(i).map(<[usize]>::len);
            let collapsed = stack.is_some() && !self.cull.expanded(i);
            if collapsed {
                // Cards behind, for the frames it stands for.
                for offset in [4.0, 2.0] {
                    let card = inner.translate(vec2(offset, -offset));
                    painter.rect_filled(card, 1.0, theme.lighter_background);
                    painter.rect_stroke(card, 1.0, Stroke::new(1.0, theme.dark_foreground), StrokeKind::Inside);
                }
            }
            if let Some(Slot::Ready(texture)) = self.thumbnails.get(&i) {
                let image = fit(texture.size_vec2(), inner);
                // Rejects are dimmed, as in Lightroom's grid.
                let tint = if rating == REJECT { Color32::from_gray(80) } else { Color32::WHITE };
                painter.image(texture.id(), image, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), tint);
            }
            badge(&painter, inner.center_bottom() - vec2(0.0, 3.0), Align2::CENTER_BOTTOM, rating, 13.0, theme);
            match stack {
                Some(n) if collapsed => {
                    let at = inner.right_top() + vec2(-3.0, 3.0);
                    loupe::plate(&painter, at, Align2::RIGHT_TOP, format!("{n}"), theme.foreground, inner.width());
                }
                // An open stack's frames are underlined together.
                Some(_) => {
                    let line = [rect.left_bottom() + vec2(0.0, -2.0), rect.right_bottom() + vec2(0.0, -2.0)];
                    painter.line_segment(line, Stroke::new(3.0, theme.accent));
                }
                None => {}
            }
        }
        let pointer = response.interact_pointer_pos().filter(|_| response.clicked())?;
        let modifiers = ui.input(|i| i.modifiers);
        let click = match () {
            () if modifiers.command => Click::Toggle,
            () if modifiers.shift => Click::Range,
            () => Click::Plain,
        };
        self.cells.iter().find(|(_, rect)| rect.contains(pointer)).map(|&(i, _)| (i, click))
    }
}

/// A face's close-up in a frame `width` × `height` pixels: a square round
/// its eyes, twice as wide as the face, kept inside the frame. Left, top
/// and side, in pixels.
fn close_up(face: &Face, width: f32, height: f32) -> [f32; 3] {
    let [x, y] = face.between_eyes();
    let side = ((face.bounds[2] - face.bounds[0]) * 2.0 * width).clamp(1.0, width.min(height));
    let left = (x * width - side / 2.0).clamp(0.0, width - side);
    let top = (y * height - side / 2.0).clamp(0.0, height - side);
    [left, top, side]
}

/// The tiles of a full-size frame drawn at `rect` that fall in `visible`,
/// uploading the ones not on the GPU yet and letting go of the rest.
fn tiles(ctx: &egui::Context, painter: &egui::Painter, full: &mut Full, rect: Rect, visible: Rect, ppp: f32) {
    let (w, h) = (full.decoded.image.width, full.decoded.image.height);
    let step = TILE as f32 / ppp;
    let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
    let mut seen = HashSet::new();
    for ty in 0..h.div_ceil(TILE) {
        for tx in 0..w.div_ceil(TILE) {
            let (tw, th) = (TILE.min(w - tx * TILE), TILE.min(h - ty * TILE));
            let at = rect.min + vec2(tx as f32 * step, ty as f32 * step);
            let tile = Rect::from_min_size(at, vec2(tw as f32, th as f32) / ppp);
            if !tile.intersects(visible) {
                continue;
            }
            seen.insert((tx, ty));
            let texture = full.tiles.entry((tx, ty)).or_insert_with(|| {
                let region = [tx * TILE, ty * TILE, tw, th];
                // At 100% each pixel is a pixel on screen; smoothing would only blur.
                upload_with(ctx, format!("tile-{tx}-{ty}"), &full.decoded, full.baked, region, TextureOptions::NEAREST)
            });
            painter.image(texture.id(), tile, uv, Color32::WHITE);
        }
    }
    full.tiles.retain(|key, _| seen.contains(key));
}

/// A `size` image scaled to fit `area`, centred in it.
fn fit(size: egui::Vec2, area: Rect) -> Rect {
    let scale = (area.width() / size.x).min(area.height() / size.y);
    Rect::from_center_size(area.center(), size * scale)
}

/// A frame's mark, on a dark plate so it reads over any photo. Nothing for
/// an undecided frame.
fn badge(painter: &egui::Painter, at: Pos2, align: Align2, rating: Rating, size: f32, theme: &Theme) {
    let (text, colour) = match rating {
        0 => return,
        REJECT if size > 14.0 => ("✕ Rejected".to_owned(), theme.red),
        REJECT => ("✕".to_owned(), theme.red),
        PICK if size > 14.0 => ("★ Pick".to_owned(), theme.accent),
        stars_ => (stars(stars_), theme.accent),
    };
    let galley = painter.layout_no_wrap(text, FontId::proportional(size), colour);
    let rect = align.anchor_size(at, galley.size()).expand2(vec2(size * 0.35, size * 0.15));
    painter.rect_filled(rect, size * 0.2, Color32::from_black_alpha(170));
    painter.galley(align.anchor_size(at, galley.size()).min, galley, colour);
}
