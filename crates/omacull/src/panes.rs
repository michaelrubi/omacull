//! Frames side by side: the loupe is one pane, compare two, survey three
//! or more. Each pane has its own zoom, kept in step with the others when
//! they're locked together.

use egui::{Rect, pos2, vec2};

use crate::loupe::View;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Loupe,
    /// Two frames; marking one down brings in the next candidate.
    Compare,
    /// Several frames, knocked out one by one until one is left.
    Survey,
}

impl Mode {
    /// As the decision log names it.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Loupe => "loupe",
            Mode::Compare => "compare",
            Mode::Survey => "survey",
        }
    }
}

/// A frame on screen, and how it's shown.
#[derive(Clone, Debug, PartialEq)]
pub struct Pane {
    pub frame: usize,
    pub view: View,
    /// The frame whose focus point the view last went to, when following
    /// them: its preview may land after the pane changes frame.
    pub followed: Option<usize>,
}

impl Pane {
    pub fn new(frame: usize, view: View) -> Self {
        Self { frame, view, followed: None }
    }
}

/// Where frames of these shapes (width over height) go in `area`, in
/// order: in rows, each row's frames the same height and filling its
/// width, as many rows as leave the smallest frame largest. So landscape
/// frames stay large, portrait ones sit side by side, and more frames
/// shrink gracefully.
pub fn layout(aspects: &[f32], area: Rect, gap: f32) -> Vec<Rect> {
    let n = aspects.len();
    let mut best: Option<(f32, Vec<Rect>)> = None;
    for rows in 1..=n {
        // The first rows take one frame more when they don't divide evenly.
        let mut runs = Vec::with_capacity(rows);
        let mut start = 0;
        for row in 0..rows {
            let len = n / rows + usize::from(row < n % rows);
            runs.push(start..start + len);
            start += len;
        }
        let gaps = |count: usize| gap * count.saturating_sub(1) as f32;
        let heights: Vec<f32> = runs
            .iter()
            .map(|run| (area.width() - gaps(run.len())) / aspects[run.clone()].iter().sum::<f32>())
            .collect();
        let scale = ((area.height() - gaps(rows)) / heights.iter().sum::<f32>()).min(1.0);
        let used = heights.iter().sum::<f32>() * scale + gaps(rows);
        let mut y = area.center().y - used / 2.0;
        let mut rects = Vec::with_capacity(n);
        for (run, height) in runs.into_iter().zip(heights) {
            let h = height * scale;
            let width = aspects[run.clone()].iter().sum::<f32>() * h + gaps(run.len());
            let mut x = area.center().x - width / 2.0;
            for i in run {
                let w = aspects[i] * h;
                rects.push(Rect::from_min_size(pos2(x, y), vec2(w, h)));
                x += w + gap;
            }
            y += h + gap;
        }
        let smallest = rects.iter().map(|r| r.area()).fold(f32::INFINITY, f32::min);
        if best.as_ref().is_none_or(|(s, _)| smallest > *s * 1.001) {
            best = Some((smallest, rects));
        }
    }
    best.map(|(_, rects)| rects).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDE: Rect = Rect { min: pos2(0.0, 0.0), max: pos2(1400.0, 760.0) };
    const LANDSCAPE: f32 = 1.5;
    const PORTRAIT: f32 = 2.0 / 3.0;

    fn rows(rects: &[Rect]) -> usize {
        let mut tops: Vec<i32> = rects.iter().map(|r| r.top().round() as i32).collect();
        tops.dedup();
        tops.len()
    }

    #[test]
    fn two_frames_sit_side_by_side() {
        let rects = layout(&[LANDSCAPE, LANDSCAPE], WIDE, 8.0);
        assert_eq!(rows(&rects), 1);
        assert!((rects[0].width() - 696.0).abs() < 1.0, "{rects:?}");
        assert!(rects.iter().all(|r| WIDE.contains_rect(*r)));
    }

    #[test]
    fn four_landscapes_make_two_rows_and_three_portraits_one() {
        let four = layout(&[LANDSCAPE; 4], WIDE, 8.0);
        assert_eq!(rows(&four), 2);
        assert!(four[0].height() > 350.0, "landscape frames stay large: {:?}", four[0]);
        let three = layout(&[PORTRAIT; 3], WIDE, 8.0);
        assert_eq!(rows(&three), 1);
        assert!((three[0].height() - 692.0).abs() < 1.0, "as wide as fits: {:?}", three[0]);
    }

    #[test]
    fn mixed_shapes_and_many_frames_still_fit() {
        let mixed = layout(&[LANDSCAPE, PORTRAIT, LANDSCAPE, PORTRAIT, LANDSCAPE], WIDE, 8.0);
        assert_eq!(mixed.len(), 5);
        let twelve = layout(&[LANDSCAPE; 12], WIDE, 8.0);
        assert_eq!(twelve.len(), 12);
        for r in mixed.iter().chain(&twelve) {
            assert!(WIDE.expand(0.5).contains_rect(*r), "{r:?}");
        }
        // No two overlap.
        for (i, a) in twelve.iter().enumerate() {
            for b in &twelve[i + 1..] {
                assert!(!a.shrink(0.5).intersects(b.shrink(0.5)), "{a:?} {b:?}");
            }
        }
        assert!(layout(&[], WIDE, 8.0).is_empty());
    }
}
