//! An open folder on screen: the loupe and the filmstrip, and the decoded
//! frames behind them, kept ahead of the cursor.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{
    Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui, Vec2, pos2,
    vec2,
};
use omacull_engine::color::Display;
use omacull_engine::cull::{Cull, PICK, Rating};
use omacull_engine::loader::{Decoded, Job, Loaded, Loader, Output};
use omacull_engine::raw::Info;
use omacull_engine::sidecar::REJECT;

use crate::loupe::{self, Bake, View};
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
    pub view: View,
    /// The frame whose focus point the view last went to, when following
    /// them: its preview may land after the cursor does.
    followed: Option<usize>,
}

/// "★★★" for three stars; a pick is one star.
pub fn stars(rating: Rating) -> String {
    "★".repeat(rating.max(0) as usize)
}

impl Shoot {
    pub fn new(cull: Cull, cache: Option<PathBuf>, display: Arc<Display>, ctx: &egui::Context) -> Self {
        let raws = cull.frames().iter().map(|f| f.path.clone()).collect();
        let ctx = ctx.clone();
        let seen = cull.current();
        Self {
            cull,
            loader: Loader::new(raws, cache, display, move || ctx.request_repaint()),
            previews: HashMap::new(),
            full: HashMap::new(),
            thumbnails: HashMap::new(),
            cached: HashSet::new(),
            wanted: Vec::new(),
            reach: 10,
            cells: Vec::new(),
            seen,
            arrived: Instant::now(),
            view: View::default(),
            followed: None,
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
        match self.previews.get(&self.cull.current()) {
            Some(Slot::Ready(shown)) => Some(&shown.decoded.info),
            _ => None,
        }
    }

    /// The current frame's size at 100%, in points: the full-size frame's
    /// once it's developed, else what the raw says it will be.
    pub fn full_size(&self, pixels_per_point: f32) -> Vec2 {
        let current = self.cull.current();
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

    /// Where the current frame is drawn whole, in the loupe as last drawn.
    pub fn fit_rect(&self) -> Rect {
        let current = self.cull.current();
        let size = match (self.previews.get(&current), self.thumbnails.get(&current)) {
            (Some(Slot::Ready(shown)), _) => shown.texture.size_vec2(),
            (_, Some(Slot::Ready(texture))) => texture.size_vec2(),
            _ => return self.view.area,
        };
        fit(size, self.view.area)
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
        let current = self.cull.current();
        let mut ring = vec![current];
        ring.extend(self.cull.neighbours(AHEAD, BEHIND));
        let mut full_ring = vec![current];
        full_ring.extend(self.cull.neighbours(1, 1));
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

        // The overlays as they're set now, on the frame on screen; others
        // catch up when they're stepped to. Only it keeps tiles on the GPU.
        if let Some(Slot::Ready(shown)) = self.previews.get_mut(&current)
            && shown.baked != bake
        {
            shown.texture = upload(ctx, format!("preview-{current}"), &shown.decoded, bake, whole(&shown.decoded));
            shown.baked = bake;
        }
        for (&i, slot) in &mut self.full {
            if let Slot::Ready(full) = slot
                && (i != current || full.baked != bake)
            {
                full.tiles.clear();
                full.baked = bake;
            }
        }
        if !self.view.follow_focus {
            self.followed = None;
        } else if self.followed != Some(current)
            && let Some(info) = self.info()
        {
            let (focus, size) = (info.focus, self.full_size(ctx.pixels_per_point()));
            self.view.arrive(focus, size);
            self.followed = Some(current);
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
        let first = if self.view.zoomed { 1 } else { 0 };
        let thumbnails = strip.into_iter().map(Job::Thumbnail);
        let mut uncached: Vec<usize> = (0..self.cull.frames().len()).filter(|i| !self.cached.contains(i)).collect();
        uncached.sort_by_key(|&i| i.abs_diff(current));
        let wanted: Vec<Job> = previews
            .clone()
            .take(1)
            .chain(full.clone().take(first))
            .chain(previews.clone().skip(1).take(1))
            .chain(thumbnails)
            .chain(previews.skip(2))
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
    }

    /// The current frame, whole or at 100%, with what's to be shown over it.
    pub fn loupe(&mut self, ui: &mut Ui, theme: &Theme, show: Show) {
        let ctx = ui.ctx().clone();
        let ppp = ctx.pixels_per_point();
        self.view.area = ui.max_rect().shrink(8.0);
        let area = self.view.area;
        let response = ui.interact(ui.max_rect(), egui::Id::new("loupe"), Sense::click_and_drag());
        let (fit_rect, size) = (self.fit_rect(), self.full_size(ppp));
        self.view.pointer(ui, &response, fit_rect, size);

        let painter = ui.painter_at(ui.max_rect());
        let current = self.cull.current();
        let frame = self.cull.frame();
        let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
        // The thumbnail, enlarged, until the preview is ready.
        let texture = match (self.previews.get(&current), self.thumbnails.get(&current)) {
            (Some(Slot::Ready(shown)), _) => Some(&shown.texture),
            (_, Some(Slot::Ready(t))) => Some(t),
            _ => None,
        };
        let Some(texture) = texture else {
            if let Some(Slot::Failed(e)) = self.previews.get(&current) {
                let name = frame.path.file_name().unwrap_or_default().to_string_lossy();
                let text = format!("Can't show {name}: {e}");
                painter.text(area.center(), Align2::CENTER_CENTER, text, FontId::proportional(14.0), theme.red);
            }
            return;
        };

        let drawn = if self.view.zoomed {
            let rect = self.view.zoomed_rect(size, ppp);
            match self.full.get_mut(&current) {
                Some(Slot::Ready(full)) => {
                    let visible = ui.max_rect().expand(TILE as f32 / ppp);
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
                    loupe::plate(&painter, area.center_top() + vec2(0.0, 12.0), Align2::CENTER_TOP, text, colour);
                }
            }
            rect
        } else {
            let rect = fit(texture.size_vec2(), area);
            painter.image(texture.id(), rect, uv, Color32::WHITE);
            rect
        };

        let info = self.info();
        if show.focus_point
            && let Some(focus) = info.and_then(|i| i.focus)
        {
            loupe::focus_point(&painter, drawn, focus, theme);
        }
        let badge_at = if self.view.zoomed { area.left_top() } else { drawn.left_top() };
        badge(&painter, badge_at + vec2(8.0, 8.0), Align2::LEFT_TOP, frame.rating, 18.0, theme);
        if let Some(Slot::Ready(shown)) = self.previews.get(&current) {
            if show.histogram {
                loupe::histogram(&painter, area, &shown.decoded.histogram);
            }
            let summary = shown.decoded.info.summary();
            if show.info && !summary.is_empty() {
                let at = area.center_bottom() - vec2(0.0, 10.0);
                loupe::plate(&painter, at, Align2::CENTER_BOTTOM, summary, theme.foreground);
            }
        }
    }

    /// The shown frames in a strip, the current one in the middle. Returns
    /// the frame clicked.
    pub fn filmstrip(&mut self, ui: &mut Ui, theme: &Theme) -> Option<usize> {
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
            if i == current {
                painter.rect_filled(rect.shrink(1.0), 0.0, theme.selection);
                painter.rect_stroke(rect.shrink(2.0), 0.0, Stroke::new(2.0, theme.accent), StrokeKind::Inside);
            }
            let inner = rect.shrink(6.0);
            if let Some(Slot::Ready(texture)) = self.thumbnails.get(&i) {
                let image = fit(texture.size_vec2(), inner);
                // Rejects are dimmed, as in Lightroom's grid.
                let tint = if rating == REJECT { Color32::from_gray(80) } else { Color32::WHITE };
                painter.image(texture.id(), image, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), tint);
            }
            badge(&painter, inner.center_bottom() - vec2(0.0, 3.0), Align2::CENTER_BOTTOM, rating, 13.0, theme);
        }
        let pointer = response.interact_pointer_pos().filter(|_| response.clicked())?;
        self.cells.iter().find(|(_, rect)| rect.contains(pointer)).map(|&(i, _)| i)
    }
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
