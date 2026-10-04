//! An open folder on screen: the loupe and the filmstrip, and the decoded
//! frames behind them, kept ahead of the cursor.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{
    Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui, pos2, vec2,
};
use omacull_engine::cull::{Cull, PICK, Rating};
use omacull_engine::image::Image;
use omacull_engine::loader::{Job, Loaded, Loader};
use omacull_engine::sidecar::REJECT;

use crate::theme::Theme;

/// Previews decoded ahead of the cursor, the way it's going, and behind.
const AHEAD: usize = 4;
const BEHIND: usize = 2;
/// Thumbnails kept further than this from the cursor are let go.
const KEEP_THUMBNAILS: usize = 100;
pub const STRIP_HEIGHT: f32 = 104.0;

/// A decoded frame on the GPU, or why there isn't one.
enum Slot {
    Ready(TextureHandle),
    Failed(String),
}

pub struct Shoot {
    pub cull: Cull,
    loader: Loader,
    previews: HashMap<usize, Slot>,
    thumbnails: HashMap<usize, Slot>,
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
}

/// "★★★" for three stars; a pick is one star.
pub fn stars(rating: Rating) -> String {
    "★".repeat(rating.max(0) as usize)
}

impl Shoot {
    pub fn new(cull: Cull, cache: Option<PathBuf>, ctx: &egui::Context) -> Self {
        let raws = cull.frames().iter().map(|f| f.path.clone()).collect();
        let ctx = ctx.clone();
        let seen = cull.current();
        Self {
            cull,
            loader: Loader::new(raws, cache, move || ctx.request_repaint()),
            previews: HashMap::new(),
            thumbnails: HashMap::new(),
            cached: HashSet::new(),
            wanted: Vec::new(),
            reach: 10,
            cells: Vec::new(),
            seen,
            arrived: Instant::now(),
        }
    }

    /// How long the current frame has been on screen.
    pub fn dwell(&mut self) -> Duration {
        if self.seen != self.cull.current() {
            self.seen = self.cull.current();
            self.arrived = Instant::now();
        }
        self.arrived.elapsed()
    }

    #[cfg(test)]
    /// Whether the current frame's preview is decoded and on the GPU.
    pub fn preview_ready(&self) -> bool {
        matches!(self.previews.get(&self.cull.current()), Some(Slot::Ready(_)))
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
    /// Whether the loader has nothing left to do.
    pub fn idle(&self) -> bool {
        !self.loader.busy()
    }

    /// Take in what has loaded, and ask for what's wanted next.
    pub fn update(&mut self, ctx: &egui::Context) {
        self.dwell();
        let current = self.cull.current();
        let mut ring = vec![current];
        ring.extend(self.cull.neighbours(AHEAD, BEHIND));
        let near = |i: &usize| i.abs_diff(current) <= KEEP_THUMBNAILS;
        self.previews.retain(|i, _| ring.contains(i));
        self.thumbnails.retain(|i, _| near(i));

        let loaded: Vec<Loaded> = self.loader.loaded().collect();
        for Loaded { job, result } in loaded {
            let slot = |result: std::io::Result<Option<Image>>, name: String| match result {
                Ok(Some(image)) => Slot::Ready(ctx.load_texture(
                    name,
                    egui::ColorImage::from_rgba_unmultiplied([image.width, image.height], &image.rgba),
                    TextureOptions::LINEAR,
                )),
                Ok(None) => Slot::Failed("nothing decoded".into()),
                Err(e) => Slot::Failed(e.to_string()),
            };
            // What the cursor has left behind isn't put on the GPU.
            match job {
                Job::Preview(i) if ring.contains(&i) => {
                    self.previews.insert(i, slot(result, format!("preview-{i}")));
                }
                Job::Thumbnail(i) if near(&i) => {
                    self.cached.insert(i);
                    self.thumbnails.insert(i, slot(result, format!("thumbnail-{i}")));
                }
                Job::Preview(_) => {}
                Job::Thumbnail(i) | Job::Cache(i) => _ = self.cached.insert(i),
            }
        }

        // The current frame first, then the next one, then the filmstrip,
        // then the rest of the ring; and while nothing else is wanted, the
        // whole folder's thumbnails into the cache, nearest first.
        let shown = self.cull.shown_indices();
        let at = shown.iter().position(|&i| i == current).unwrap_or(0);
        let reach = at.saturating_sub(self.reach)..(at + self.reach + 1).min(shown.len());
        let mut strip: Vec<usize> = shown[reach].to_vec();
        strip.sort_by_key(|&i| i.abs_diff(current));
        let previews = ring.iter().map(|&i| Job::Preview(i));
        let thumbnails = strip.into_iter().map(Job::Thumbnail);
        let mut uncached: Vec<usize> = (0..self.cull.frames().len()).filter(|i| !self.cached.contains(i)).collect();
        uncached.sort_by_key(|&i| i.abs_diff(current));
        let wanted: Vec<Job> = previews
            .clone()
            .take(2)
            .chain(thumbnails)
            .chain(previews.skip(2))
            .chain(uncached.into_iter().map(Job::Cache))
            .filter(|job| match *job {
                Job::Preview(i) => !self.previews.contains_key(&i),
                Job::Thumbnail(i) => !self.thumbnails.contains_key(&i),
                Job::Cache(i) => !self.cached.contains(&i),
            })
            .collect();
        if wanted != self.wanted {
            self.loader.want(wanted.iter().copied());
            self.wanted = wanted;
        }
    }

    /// The current frame, as large as fits.
    pub fn loupe(&self, ui: &mut Ui, theme: &Theme) {
        let area = ui.max_rect().shrink(8.0);
        let painter = ui.painter_at(ui.max_rect());
        let current = self.cull.current();
        let frame = self.cull.frame();
        // The thumbnail, enlarged, until the preview is ready.
        let texture = match (self.previews.get(&current), self.thumbnails.get(&current)) {
            (Some(Slot::Ready(t)), _) | (_, Some(Slot::Ready(t))) => Some(t),
            _ => None,
        };
        if let Some(texture) = texture {
            let rect = fit(texture.size_vec2(), area);
            painter.image(texture.id(), rect, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
            badge(&painter, rect.left_top() + vec2(8.0, 8.0), Align2::LEFT_TOP, frame.rating, 18.0, theme);
        } else if let Some(Slot::Failed(e)) = self.previews.get(&current) {
            let name = frame.path.file_name().unwrap_or_default().to_string_lossy();
            let text = format!("Can't show {name}: {e}");
            painter.text(area.center(), Align2::CENTER_CENTER, text, FontId::proportional(14.0), theme.red);
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
