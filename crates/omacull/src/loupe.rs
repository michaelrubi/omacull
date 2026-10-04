//! The loupe's zoom and what it draws over a frame: 100% zoom held for a
//! look or toggled, at the pointer or the focus point, panned by dragging;
//! and the histogram, the shooting settings, the focus point, and the
//! clipping and focus peaking colours baked into the frame's pixels.

use egui::{Align2, Color32, ColorImage, FontId, Painter, Pos2, Rect, Response, Shape, Stroke, Ui, Vec2, pos2, vec2};
use omacull_engine::image::{Histogram, Image, mark};

use crate::theme::Theme;

/// Held longer than this (seconds), the zoom key or a click is a look that
/// ends when it's let go; shorter, it toggles.
pub const HOLD: f64 = 0.3;

/// A press of the zoom key or the mouse that may become a hold.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Hold {
    /// egui's time when it started.
    since: f64,
    /// Whether it zoomed in.
    from_fit: bool,
    dragged: bool,
    /// The key, not the mouse.
    key: bool,
}

/// How the loupe shows the current frame: whole, or at 100%.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub zoomed: bool,
    /// The point in the middle of the loupe at 100%, as fractions of the
    /// frame's width and height. Kept when stepping, so the same spot can
    /// be checked across a burst.
    pub center: [f32; 2],
    /// Zoomed to the focus point, and so to each frame's as it's stepped to.
    pub follow_focus: bool,
    hold: Option<Hold>,
    /// Where the frame was last drawn.
    pub area: Rect,
}

impl Default for View {
    fn default() -> Self {
        Self { zoomed: false, center: [0.5, 0.5], follow_focus: false, hold: None, area: Rect::NOTHING }
    }
}

impl View {
    /// `center` moved as little as it takes for a frame of `size` (points,
    /// at 100%) to cover the loupe, or to sit in its middle where it's
    /// smaller.
    fn clamped(&self, [u, v]: [f32; 2], size: Vec2) -> [f32; 2] {
        let axis = |c: f32, view: f32, size: f32| {
            let half = view / 2.0 / size;
            if half >= 0.5 { 0.5 } else { c.clamp(half, 1.0 - half) }
        };
        [axis(u, self.area.width(), size.x), axis(v, self.area.height(), size.y)]
    }

    /// Zoom in, keeping the point under `at` where it is (`fit` is where the
    /// whole frame is drawn); elsewhere, where it was last zoomed.
    fn zoom_in(&mut self, at: Option<Pos2>, fit: Rect, size: Vec2) {
        if let Some(at) = at.filter(|p| self.area.contains(*p) && fit.is_positive()) {
            let u = ((at - fit.min) / fit.size()).clamp(Vec2::ZERO, Vec2::splat(1.0));
            let from_middle = (at - self.area.center()) / size;
            self.center = [u.x - from_middle.x, u.y - from_middle.y];
        }
        self.center = self.clamped(self.center, size);
        self.zoomed = true;
        self.follow_focus = false;
    }

    /// The zoom key went down: zoom in, or out if zoomed. Key repeats
    /// while it's held do nothing.
    pub fn key_down(&mut self, at: Option<Pos2>, fit: Rect, size: Vec2, time: f64) {
        if self.hold.is_some_and(|h| h.key) {
            return;
        }
        if self.zoomed {
            self.zoomed = false;
            self.follow_focus = false;
        } else {
            self.zoom_in(at, fit, size);
            self.hold = Some(Hold { since: time, from_fit: true, dragged: false, key: true });
        }
    }

    /// The zoom key isn't down: if it was held, the look is over.
    pub fn key_up(&mut self, time: f64) {
        if let Some(hold) = self.hold.filter(|h| h.key) {
            self.hold = None;
            if time - hold.since >= HOLD {
                self.zoomed = false;
            }
        }
    }

    /// Zoom to a focus point, and follow it from frame to frame. False if
    /// there's none to go to.
    pub fn zoom_to_focus(&mut self, focus: Option<[f32; 2]>, size: Vec2) -> bool {
        let Some(focus) = focus else { return false };
        self.center = self.clamped(focus, size);
        self.zoomed = true;
        self.follow_focus = true;
        true
    }

    /// Zoom to a point of the frame, as fractions of its width and height.
    pub fn zoom_to(&mut self, at: [f32; 2], size: Vec2) {
        self.center = self.clamped(at, size);
        self.zoomed = true;
        self.follow_focus = false;
    }

    /// The next frame: its focus point, if following them.
    pub fn arrive(&mut self, focus: Option<[f32; 2]>, size: Vec2) {
        if self.follow_focus
            && let Some(focus) = focus
        {
            self.center = self.clamped(focus, size);
        }
    }

    /// Take on another pane's zoom, locked to it: whole or at 100%, at the
    /// same spot, or following focus points.
    pub fn follow(&mut self, other: &View) {
        self.zoomed = other.zoomed;
        self.center = other.center;
        self.follow_focus = other.follow_focus;
    }

    /// Move the frame by `delta` points.
    pub fn pan(&mut self, delta: Vec2, size: Vec2) {
        self.center = self.clamped([self.center[0] - delta.x / size.x, self.center[1] - delta.y / size.y], size);
        self.follow_focus = false;
    }

    /// The mouse on the loupe: press to zoom in at the pointer, hold for a
    /// look, drag to pan, click again to zoom out; the wheel pans.
    pub fn pointer(&mut self, ui: &Ui, response: &Response, fit: Rect, size: Vec2) {
        let (pressed, released, time, scroll) = ui.input(|i| {
            (i.pointer.primary_pressed(), i.pointer.primary_released(), i.time, i.smooth_scroll_delta)
        });
        if pressed && response.is_pointer_button_down_on() {
            let from_fit = !self.zoomed;
            if from_fit {
                self.zoom_in(response.interact_pointer_pos(), fit, size);
            }
            self.hold = Some(Hold { since: time, from_fit, dragged: false, key: false });
        }
        if self.zoomed && response.dragged() {
            self.pan(response.drag_delta(), size);
            if let Some(hold) = &mut self.hold {
                hold.dragged = true;
            }
        }
        if released && let Some(hold) = self.hold.filter(|h| !h.key) {
            self.hold = None;
            // A quick click toggles; a hold, or a drag, from whole was a look.
            let quick = time - hold.since < HOLD && !hold.dragged;
            if hold.from_fit != quick {
                self.zoomed = false;
            }
        }
        if self.zoomed && response.hovered() && scroll != Vec2::ZERO {
            self.pan(scroll, size);
        }
    }

    /// Where a frame of `size` (points, at 100%) is drawn when zoomed, on
    /// whole pixels so it's sharp.
    pub fn zoomed_rect(&self, size: Vec2, pixels_per_point: f32) -> Rect {
        let [u, v] = self.clamped(self.center, size);
        let min = self.area.center() - vec2(u * size.x, v * size.y);
        let min = (min.to_vec2() * pixels_per_point).round() / pixels_per_point;
        Rect::from_min_size(min.to_pos2(), size)
    }
}

/// Which overlays are baked into a frame's pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bake {
    pub clipping: bool,
    pub peaking: bool,
}

const HIGHLIGHT: Color32 = Color32::from_rgb(255, 0, 0);
const SHADOW: Color32 = Color32::from_rgb(0, 90, 255);
const SHARP: Color32 = Color32::from_rgb(0, 255, 0);

/// A part of a frame (`x`, `y`, `width`, `height`) ready to upload, with
/// sharp edges and clipped pixels coloured if asked.
pub fn bake(image: &Image, marks: &[u8], bake: Bake, [x0, y0, w, h]: [usize; 4]) -> ColorImage {
    let mut pixels = Vec::with_capacity(w * h);
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            let i = y * image.width + x;
            let m = marks.get(i).copied().unwrap_or(0);
            let p = &image.rgba[i * 4..i * 4 + 3];
            pixels.push(match () {
                () if bake.peaking && m & mark::SHARP != 0 => SHARP,
                () if bake.clipping && m & mark::HIGHLIGHT != 0 => HIGHLIGHT,
                () if bake.clipping && m & mark::SHADOW != 0 => SHADOW,
                () => Color32::from_rgb(p[0], p[1], p[2]),
            });
        }
    }
    ColorImage::new([w, h], pixels)
}

/// The red, green and blue histograms, top right of the loupe.
pub fn histogram(painter: &Painter, area: Rect, histogram: &Histogram) {
    let rect = Rect::from_min_size(pos2(area.right() - 268.0, area.top() + 12.0), vec2(256.0, 100.0));
    painter.rect_filled(rect.expand(4.0), 4.0, Color32::from_black_alpha(170));
    // Scaled to the tallest level but the ends, which pile up when clipped.
    let tallest = histogram.0.iter().flat_map(|c| c[1..255].iter()).copied().max().unwrap_or(1).max(1) as f32;
    let colours = [
        Color32::from_rgba_unmultiplied(255, 70, 70, 150),
        Color32::from_rgba_unmultiplied(70, 255, 70, 150),
        Color32::from_rgba_unmultiplied(80, 120, 255, 150),
    ];
    for (counts, colour) in histogram.0.iter().zip(colours) {
        let mut points = vec![rect.left_bottom()];
        for (level, &n) in counts.iter().enumerate() {
            let height = (n as f32 / tallest).min(1.0) * rect.height();
            points.push(pos2(rect.left() + level as f32, rect.bottom() - height));
        }
        points.push(rect.right_bottom());
        painter.add(Shape::line(points, Stroke::new(1.0, colour)));
    }
}

/// Text on a dark plate, anchored at `at`, cut short with … to fit
/// `max_width`.
pub fn plate(painter: &Painter, at: Pos2, align: Align2, text: String, colour: Color32, max_width: f32) -> Rect {
    let mut job = egui::text::LayoutJob::simple_singleline(text, FontId::proportional(13.0), colour);
    job.wrap = egui::text::TextWrapping::truncate_at_width((max_width - 12.0).max(20.0));
    let galley = painter.layout_job(job);
    let rect = align.anchor_size(at, galley.size());
    painter.rect_filled(rect.expand2(vec2(6.0, 3.0)), 4.0, Color32::from_black_alpha(170));
    painter.galley(rect.min, galley, colour);
    rect
}

/// Where the camera focused: a square outlined to read on any photo.
pub fn focus_point(painter: &Painter, frame: Rect, [u, v]: [f32; 2], theme: &Theme) {
    let at = frame.min + vec2(u * frame.width(), v * frame.height());
    let square = Rect::from_center_size(at, Vec2::splat(28.0));
    painter.rect_stroke(square, 2.0, Stroke::new(3.5, Color32::from_black_alpha(160)), egui::StrokeKind::Middle);
    painter.rect_stroke(square, 2.0, Stroke::new(1.5, theme.accent), egui::StrokeKind::Middle);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A loupe 800 × 600 points, and a 6000 × 4000 frame at 100% on a
    /// screen of one pixel a point.
    fn view() -> (View, Rect, Vec2) {
        let view = View { area: Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0)), ..View::default() };
        let fit = Rect::from_min_size(pos2(0.0, 33.0), vec2(800.0, 533.33));
        (view, fit, vec2(6000.0, 4000.0))
    }

    #[test]
    fn zooming_keeps_the_point_under_the_pointer() {
        let (mut view, fit, size) = view();
        // A quarter across, half down the frame as it's shown whole.
        let at = pos2(200.0, fit.center().y);
        view.key_down(Some(at), fit, size, 0.0);
        assert!(view.zoomed);
        let rect = view.zoomed_rect(size, 1.0);
        let under = (at - rect.min) / rect.size();
        assert!((under.x - 0.25).abs() < 0.001 && (under.y - 0.5).abs() < 0.001, "{under:?}");
    }

    #[test]
    fn the_frame_covers_the_loupe_at_its_edges() {
        let (mut view, fit, size) = view();
        // Zoomed at the frame's top-left corner.
        view.key_down(Some(fit.min), fit, size, 0.0);
        let rect = view.zoomed_rect(size, 1.0);
        assert_eq!(rect.min, Pos2::ZERO, "the top-left corner, not past it");
        view.pan(vec2(-1e6, -1e6), size);
        assert_eq!(view.zoomed_rect(size, 1.0).max, pos2(800.0, 600.0));
        // A frame smaller than the loupe sits in its middle.
        let small = vec2(400.0, 300.0);
        assert_eq!(view.zoomed_rect(small, 1.0).center(), pos2(400.0, 300.0));
    }

    #[test]
    fn a_tap_toggles_and_a_hold_is_a_look() {
        let (mut view, fit, size) = view();
        view.key_down(None, fit, size, 0.0);
        view.key_down(None, fit, size, 0.1);
        assert!(view.zoomed, "repeats while held do nothing");
        view.key_up(0.1);
        assert!(view.zoomed, "a tap stays zoomed");
        view.key_down(None, fit, size, 1.0);
        assert!(!view.zoomed, "and the next press zooms out");
        view.key_up(1.1);

        view.key_down(None, fit, size, 2.0);
        view.key_up(2.0 + HOLD + 0.01);
        assert!(!view.zoomed, "a hold is a look");
    }

    #[test]
    fn the_spot_is_kept_and_focus_points_followed() {
        let (mut view, fit, size) = view();
        view.key_down(Some(pos2(600.0, 300.0)), fit, size, 0.0);
        let spot = view.center;
        view.arrive(Some([0.1, 0.1]), size);
        assert_eq!(view.center, spot, "the same spot on the next frame");
        assert!(view.zoom_to_focus(Some([0.5, 0.4]), size));
        view.arrive(Some([0.3, 0.6]), size);
        assert_eq!(view.center, [0.3, 0.6]);
        view.pan(vec2(10.0, 0.0), size);
        assert!(!view.follow_focus, "panning stops following");
        assert!(!view.zoom_to_focus(None, size));
    }

    #[test]
    fn baking_colours_marked_pixels() {
        let image = Image { width: 2, height: 2, rgba: [10, 20, 30, 255].repeat(4) };
        let marks = [0, mark::HIGHLIGHT, mark::SHADOW, mark::SHARP | mark::HIGHLIGHT];
        let plain = bake(&image, &marks, Bake::default(), [0, 0, 2, 2]);
        assert!(plain.pixels.iter().all(|&p| p == Color32::from_rgb(10, 20, 30)));
        let both = bake(&image, &marks, Bake { clipping: true, peaking: true }, [0, 0, 2, 2]);
        assert_eq!(both.pixels, [Color32::from_rgb(10, 20, 30), HIGHLIGHT, SHADOW, SHARP]);
        let tile = bake(&image, &marks, Bake { clipping: true, peaking: false }, [1, 1, 1, 1]);
        assert_eq!((tile.size, tile.pixels[0]), ([1, 1], HIGHLIGHT));
    }
}
