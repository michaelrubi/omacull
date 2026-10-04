//! Signals: what can be measured of a frame with no training. How crisp it
//! is where the camera focused and at the eyes, whether the eyes are open,
//! and how much of it is clipped. They're shown as small indicators and
//! used to suggest the best of a stack; nothing is marked on their word.
//!
//! Measured on the camera's preview, upright, and kept in a cache
//! (`~/.cache/omacull/signals/`) like the faces, so a folder is measured
//! once.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cull::Rating;
use crate::faces::Face;
use crate::image::Image;
use crate::thumbs;

/// Bumped when signals are measured differently, so old ones aren't used.
const VERSION: u64 = 1;

/// The focus point's sharpness is measured in a square this much of the
/// frame's long edge across.
const FOCUS_WINDOW: f32 = 0.125;
/// Faces this much smaller than the largest aren't who the frame is of,
/// and their blinks don't count.
const BYSTANDER: f32 = 0.25;

/// What was measured of a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Signals {
    /// How crisp the crispest edges are where the camera focused: the step
    /// in levels from one pixel to the next. None with no focus point.
    pub focus: Option<f32>,
    /// The same at the eyes of the largest face. None with no face.
    pub eyes: Option<f32>,
    /// How open the eyes are, 0 (shut) to 1: the least open of the faces
    /// the frame is of. None if it couldn't be told of any.
    pub open: Option<f32>,
    /// How much of the frame is blown out, 0 to 1.
    pub highlights: f32,
    /// How much of it is black.
    pub shadows: f32,
    /// How many faces were found.
    pub faces: u32,
    /// How much of the frame the largest face takes up.
    pub face: f32,
}

impl Signals {
    /// The eyes look shut.
    pub fn shut(&self) -> bool {
        self.open.is_some_and(|open| open < 0.3)
    }
}

/// How a frame stands among the frames it was shot with: its stack, or its
/// burst.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Standing {
    /// How many frames it was measured against, itself included.
    pub of: u32,
    /// Its sharpness as a share of the sharpest's.
    pub sharp: Option<f32>,
    /// How open its eyes are as a share of the most open.
    pub open: Option<f32>,
}

impl Default for Standing {
    /// A frame on its own.
    fn default() -> Self {
        Self { of: 1, sharp: None, open: None }
    }
}

/// Where a suggestion came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum By {
    /// The best of a stack, by its signals.
    Signals,
    /// The model trained on the user's own decisions.
    Model,
}

/// A mark Omacull would make, for the user to take or leave.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Suggestion {
    pub rating: Rating,
    pub by: By,
    /// How sure it is, 0 to 1.
    pub confidence: f32,
}

/// How crisp the crispest edges are in a part of a frame (left, top,
/// right, bottom, in pixels): the step in levels from one pixel to the
/// next that the crispest tenth of it averages (Sobel, as focus peaking
/// measures it). None if the part is too small to say.
fn sharpness(image: &Image, [l, t, r, b]: [f32; 4]) -> Option<f32> {
    let (w, h) = (image.width, image.height);
    if w < 3 || h < 3 {
        return None;
    }
    // A pixel is measured against the ones either side of it.
    let inside = |v: f32, size: usize| (v.round().max(1.0) as usize).min(size - 1);
    let (l, t, r, b) = (inside(l, w), inside(t, h), inside(r, w), inside(b, h));
    if r < l + 8 || b < t + 8 {
        return None;
    }
    let luma = |x: usize, y: usize| {
        let p = image.pixel(x, y);
        ((u32::from(p[0]) * 54 + u32::from(p[1]) * 183 + u32::from(p[2]) * 19) >> 8) as i32
    };
    let mut gradients = Vec::with_capacity((r - l) * (b - t));
    for y in t..b {
        for x in l..r {
            let at = |dx: usize, dy: usize| luma(x + dx - 1, y + dy - 1);
            let gx = at(2, 0) + 2 * at(2, 1) + at(2, 2) - at(0, 0) - 2 * at(0, 1) - at(0, 2);
            let gy = at(0, 2) + 2 * at(1, 2) + at(2, 2) - at(0, 0) - 2 * at(1, 0) - at(2, 0);
            gradients.push(gx.unsigned_abs() + gy.unsigned_abs());
        }
    }
    let crispest = (gradients.len() / 10).max(1);
    gradients.select_nth_unstable_by(crispest - 1, |a, b| b.cmp(a));
    let sum: u64 = gradients[..crispest].iter().map(|&g| u64::from(g)).sum();
    // A step of one level reads as 4 to Sobel.
    Some(sum as f32 / crispest as f32 / 4.0)
}

/// Measure an upright preview: `focus` is where the camera focused, as
/// fractions of its width and height, and `faces` the faces found in it.
pub fn measure(preview: &Image, focus: Option<[f32; 2]>, faces: &[Face]) -> Signals {
    let (w, h) = (preview.width as f32, preview.height as f32);
    let focus = focus.and_then(|[u, v]| {
        let half = w.max(h) * FOCUS_WINDOW / 2.0;
        sharpness(preview, [u * w - half, v * h - half, u * w + half, v * h + half])
    });
    let largest = faces.iter().max_by(|a, b| a.area().total_cmp(&b.area()));
    // A box round both eyes: twice as wide as they are apart.
    let eyes = largest.and_then(|face| {
        let [[x0, y0], [x1, y1]] = face.eyes.map(|[x, y]| [x * w, y * h]);
        let (x, y, apart) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0, (x1 - x0).hypot(y1 - y0));
        sharpness(preview, [x - apart, y - apart / 2.0, x + apart, y + apart / 2.0])
    });
    let subject = largest.map_or(0.0, Face::area) * BYSTANDER;
    let open = faces.iter().filter(|face| face.area() >= subject).filter_map(|face| face.open).min_by(f32::total_cmp);
    let (mut blown, mut black) = (0usize, 0usize);
    for p in preview.rgba.as_chunks::<4>().0 {
        let brightest = p[0].max(p[1]).max(p[2]);
        blown += usize::from(brightest >= 254);
        black += usize::from(brightest <= 2);
    }
    let pixels = (preview.width * preview.height).max(1) as f32;
    Signals {
        focus,
        eyes,
        open,
        highlights: blown as f32 / pixels,
        shadows: black as f32 / pixels,
        faces: faces.len() as u32,
        face: largest.map_or(0.0, Face::area),
    }
}

/// What each frame of a group is compared by for sharpness: at the eyes if
/// every frame has them, else where the camera focused if every frame has
/// that, else whichever each has.
fn sharps(group: &[&Signals]) -> Vec<Option<f32>> {
    if group.iter().all(|s| s.eyes.is_some()) {
        group.iter().map(|s| s.eyes).collect()
    } else if group.iter().all(|s| s.focus.is_some()) {
        group.iter().map(|s| s.focus).collect()
    } else {
        group.iter().map(|s| s.eyes.or(s.focus)).collect()
    }
}

/// How each frame of a group stands in it.
pub fn standings(group: &[&Signals]) -> Vec<Standing> {
    let shares = |values: Vec<Option<f32>>| -> Vec<Option<f32>> {
        let best = values.iter().flatten().copied().fold(0.0, f32::max);
        values.into_iter().map(|v| v.map(|v| if best > 0.0 { v / best } else { 1.0 })).collect()
    };
    let sharp = shares(sharps(group));
    let open = shares(group.iter().map(|s| s.open).collect());
    let of = group.len() as u32;
    sharp.into_iter().zip(open).map(|(sharp, open)| Standing { of, sharp, open }).collect()
}

/// The frame of a group the signals favour, and by how much over the next
/// (0 to 1): the sharpest, marked down for eyes less open than the others'
/// and for what's blown out. The first of equals.
pub fn winner(group: &[&Signals]) -> Option<(usize, f32)> {
    let scores: Vec<f32> = standings(group)
        .iter()
        .zip(group)
        .map(|(standing, signals)| {
            standing.sharp.unwrap_or(1.0) * (0.4 + 0.6 * standing.open.unwrap_or(1.0)) - signals.highlights
        })
        .collect();
    let best = (0..scores.len()).max_by(|&a, &b| scores[a].total_cmp(&scores[b]).then(b.cmp(&a)))?;
    let next = scores.iter().enumerate().filter(|&(i, _)| i != best).map(|(_, &s)| s).reduce(f32::max);
    Some((best, next.map_or(1.0, |next| (scores[best] - next).clamp(0.0, 1.0))))
}

/// `$XDG_CACHE_HOME/omacull/signals`, or `~/.cache/omacull/signals`.
pub fn default_dir() -> Option<PathBuf> {
    Some(thumbs::cache_dir()?.join("signals"))
}

fn cached(dir: &Path, raw: &Path) -> io::Result<PathBuf> {
    Ok(dir.join(format!("{}.json", thumbs::key(raw, VERSION)?)))
}

/// What was measured of a raw before, if it's cached.
pub fn load(raw: &Path, dir: &Path) -> Option<Signals> {
    let text = fs::read(cached(dir, raw).ok()?).ok()?;
    serde_json::from_slice(&text).ok()
}

pub fn store(raw: &Path, dir: &Path, signals: &Signals) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = cached(dir, raw)?;
    let mut temporary = path.clone().into_os_string();
    temporary.push(format!(".{}.tmp", std::process::id()));
    fs::write(&temporary, serde_json::to_vec(signals)?)?;
    fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Arw, Folder};

    /// A grey frame with whatever `paint` puts on it.
    fn frame(w: usize, h: usize, paint: impl Fn(usize, usize) -> Option<u8>) -> Image {
        let paint = &paint;
        let rgba = (0..h)
            .flat_map(|y| (0..w).flat_map(move |x| [paint(x, y).unwrap_or(128); 3].into_iter().chain([255])))
            .collect();
        Image { width: w, height: h, rgba }
    }

    /// Stripes four pixels wide, each edge spread over `soft` pixels.
    fn stripes(soft: usize) -> Image {
        frame(400, 300, |x, _| {
            let across = x % 8;
            let ramp = |from: usize| ((across - from + 1) * 200 / (soft + 1)) as u8;
            Some(match across {
                a if a < soft => 228 - ramp(0),
                a if a < 4 => 28,
                a if a < 4 + soft => 28 + ramp(4),
                _ => 228,
            })
        })
    }

    #[test]
    fn crisp_edges_measure_sharper_than_soft_ones() {
        let at = Some([0.5, 0.5]);
        let crisp = measure(&stripes(0), at, &[]).focus.unwrap();
        let soft = measure(&stripes(3), at, &[]).focus.unwrap();
        assert!(crisp > 150.0, "a step of 200 levels: {crisp}");
        assert!(soft < crisp * 0.6, "{soft} against {crisp}");
        let flat = measure(&frame(400, 300, |_, _| None), at, &[]);
        assert_eq!(flat.focus, Some(0.0));
        assert_eq!(measure(&stripes(0), None, &[]).focus, None, "no focus point, nothing measured there");
        assert_eq!((flat.eyes, flat.open, flat.faces), (None, None, 0), "nor at eyes with no face");
    }

    #[test]
    fn a_blink_by_anyone_the_frame_is_of_counts() {
        let face = |left: f32, size: f32, open| Face {
            score: 0.9,
            bounds: [left, 0.2, left + size, 0.2 + size],
            eyes: [[left + size * 0.3, 0.2 + size * 0.4], [left + size * 0.7, 0.2 + size * 0.4]],
            open,
        };
        let image = stripes(0);
        let two = measure(&image, None, &[face(0.1, 0.3, Some(0.9)), face(0.5, 0.4, Some(0.1))]);
        assert_eq!((two.open, two.shut(), two.faces), (Some(0.1), true, 2));
        assert!((two.face - 0.16).abs() < 1e-5, "the largest face's share of the frame");
        assert!(two.eyes.unwrap() > 150.0, "sharpness at the largest face's eyes");
        // Someone small in the background blinking doesn't; nor a face
        // whose eyes couldn't be told.
        let bystander = measure(&image, None, &[face(0.1, 0.4, Some(0.9)), face(0.6, 0.1, Some(0.0))]);
        assert_eq!((bystander.open, bystander.shut()), (Some(0.9), false));
        assert_eq!(measure(&image, None, &[face(0.1, 0.4, None)]).open, None);
    }

    #[test]
    fn clipping_is_a_share_of_the_frame() {
        let image = frame(100, 100, |x, _| match x {
            0..10 => Some(255),
            10..35 => Some(0),
            _ => None,
        });
        let signals = measure(&image, None, &[]);
        assert!((signals.highlights - 0.10).abs() < 1e-6 && (signals.shadows - 0.25).abs() < 1e-6, "{signals:?}");
    }

    fn signals(eyes: Option<f32>, focus: Option<f32>, open: Option<f32>) -> Signals {
        Signals { eyes, focus, open, ..Signals::default() }
    }

    #[test]
    fn the_sharpest_frame_with_open_eyes_wins_its_group() {
        let a = signals(Some(30.0), Some(50.0), Some(0.9));
        let b = signals(Some(40.0), Some(20.0), Some(0.9));
        let blink = signals(Some(44.0), Some(20.0), Some(0.1));
        let group = [&a, &b, &blink];
        let standings = standings(&group);
        assert_eq!(standings[2], Standing { of: 3, sharp: Some(1.0), open: Some(0.1 / 0.9) });
        assert!((standings[0].sharp.unwrap() - 30.0 / 44.0).abs() < 1e-6, "by the eyes, when every frame has them");
        let (best, margin) = winner(&group).unwrap();
        assert_eq!(best, 1, "the blink is sharpest, and loses");
        assert!(margin > 0.1 && margin < 1.0, "{margin}");

        // Without eyes in every frame, by the focus point.
        let no_face = signals(None, Some(60.0), None);
        assert_eq!(winner(&[&a, &no_face]).unwrap().0, 1);
        // Blown highlights count against a frame; equals go to the first.
        let blown = Signals { highlights: 0.2, ..a };
        assert_eq!(winner(&[&blown, &a]).unwrap().0, 1);
        assert_eq!(winner(&[&a, &a]), Some((0, 0.0)));
        assert_eq!(winner(&[&a]), Some((0, 1.0)));
        assert_eq!(winner(&[]), None);
    }

    #[test]
    fn signals_are_cached_by_raw() {
        let folder = Folder::with_raws("signals", 1, &Arw::default());
        let cache = folder.0.join("cache");
        let measured = signals(Some(31.5), None, Some(0.75));
        assert_eq!(load(&folder.raw(1), &cache), None);
        store(&folder.raw(1), &cache, &measured).unwrap();
        assert_eq!(load(&folder.raw(1), &cache), Some(measured));
    }
}
