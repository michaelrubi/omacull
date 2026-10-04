//! The raw itself, developed at full size, for 100% zoom: the camera's
//! preview is only 1616×1080.
//!
//! rawler develops a plain, flat rendering: demosaiced, white balanced,
//! sRGB, with no tone curve. Next to the camera's JPEG that looks dull and
//! dark, and zooming in would change the picture as well as its size. So
//! each channel's tones are matched to the preview's: the developed
//! picture's levels are mapped so that as many of its pixels are darker
//! than each level as are in the preview. Shapes, noise and focus are the
//! raw's; brightness, contrast and colour are near the camera's.

use std::io;
use std::path::Path;

use rayon::prelude::*;

use crate::color::Space;
use crate::image::{self, Image};
use crate::raw::Info;

/// Levels the developed picture's values are counted in.
const STEPS: usize = 4096;

/// A raw developed at full size, upright, its tones matched to the
/// camera's preview, which is returned too, with the colours they're in
/// and what else the loupe shows.
pub fn full(raw: &Path) -> io::Result<(Image, Image, Space, Info)> {
    let (preview, space, info) = image::preview_with_info(raw)?;
    let other = |e: rawler::RawlerError| io::Error::other(e.to_string());
    let decoded = rawler::decode_file(raw).map_err(other)?;
    let developed = rawler::imgop::develop::RawDevelop::default().develop_intermediate(&decoded).map_err(other)?;
    let rawler::imgop::develop::Intermediate::ThreeColor(rgb) = developed else {
        return Err(io::Error::other("the raw didn't develop to colour"));
    };
    let orientation = crate::raw::RawFile::open(raw)?.orientation;
    let image = matched(&rgb.data, rgb.width, rgb.height, &preview).oriented(orientation);
    Ok((image, preview, space, info))
}

/// Developed pixels (0 to 1, sRGB-encoded) in 8 bits, each channel's
/// levels mapped to match `reference`'s.
pub fn matched(rgb: &[[f32; 3]], width: usize, height: usize, reference: &Image) -> Image {
    let step = |v: f32| ((v.clamp(0.0, 1.0) * (STEPS - 1) as f32).round() as usize).min(STEPS - 1);
    let wanted = image::Histogram::of(reference).0;
    let luts: [Vec<u8>; 3] = std::array::from_fn(|c| {
        // Count every fourth pixel: plenty for the shape of the histogram.
        let mut have = vec![0u64; STEPS];
        for px in rgb.iter().step_by(4) {
            have[step(px[c])] += 1;
        }
        lut(&have, &wanted[c])
    });
    let mut rgba = vec![0u8; width * height * 4];
    rgba.par_chunks_mut(4).zip(rgb.par_iter()).for_each(|(out, px)| {
        for c in 0..3 {
            out[c] = luts[c][step(px[c])];
        }
        out[3] = 255;
    });
    Image { width, height, rgba }
}

/// For each of `have`'s levels, the level of `want` that as large a share
/// of pixels lies below. Matching cumulative histograms.
fn lut(have: &[u64], want: &[u32; 256]) -> Vec<u8> {
    let have_total = have.iter().sum::<u64>() as f64;
    let want_total = want.iter().map(|&n| u64::from(n)).sum::<u64>() as f64;
    if have_total == 0.0 || want_total == 0.0 {
        // Nothing to match: just the levels in 8 bits.
        return (0..have.len()).map(|i| (i * 255 / (have.len() - 1)) as u8).collect();
    }
    let mut below = Vec::with_capacity(256);
    let mut sum = 0.0;
    for &n in want {
        sum += f64::from(n);
        below.push(sum / want_total);
    }
    let mut sum = 0.0;
    have.iter()
        .map(|&n| {
            // The middle of this level's share.
            let share = (sum + n as f64 / 2.0) / have_total;
            sum += n as f64;
            below.iter().position(|&b| b >= share).unwrap_or(255) as u8
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tones_are_matched_to_the_preview() {
        // A dull, dark development of a picture the camera rendered
        // brighter and with more contrast.
        let (w, h) = (64, 64);
        let developed: Vec<[f32; 3]> = (0..w * h).map(|i| [0.2 + 0.2 * (i % w) as f32 / w as f32; 3]).collect();
        let reference = Image {
            width: w,
            height: h,
            rgba: (0..w * h).flat_map(|i| [((i % w) * 255 / (w - 1)) as u8; 3].into_iter().chain([255])).collect(),
        };
        let out = matched(&developed, w, h, &reference);
        assert_eq!((out.width, out.height), (w, h));
        let (left, right) = (out.pixel(0, 5)[0], out.pixel(w - 1, 5)[0]);
        assert!(left < 10 && right > 245, "stretched to the preview's range: {left}..{right}");
        let middle = out.pixel(w / 2, 5)[0];
        assert!((115..140).contains(&middle), "{middle}");
        // Brighter stays brighter.
        assert!((1..w).all(|x| out.pixel(x, 5)[0] >= out.pixel(x - 1, 5)[0]));
    }

    #[test]
    fn with_nothing_to_match_levels_are_kept() {
        let out = matched(&[[0.0, 0.5, 1.0]], 1, 1, &Image::default());
        assert_eq!(out.rgba, [0, 127, 255, 255]);
    }

    #[test]
    fn a_raw_that_cant_be_decoded_says_so() {
        let folder = crate::testing::Folder::with_raws("develop", 1, &crate::testing::Arw::default());
        assert!(full(&folder.raw(1)).is_err(), "the fake raws hold no raw data");
    }
}
