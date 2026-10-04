//! Pixels: decoding the camera's JPEGs, turning them upright, making
//! filmstrip thumbnails of them, and measuring them for the loupe's
//! histogram, clipping and focus peaking.

use std::io::{self, ErrorKind};
use std::path::Path;

use rayon::prelude::*;
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

use crate::color::Space;
use crate::raw::{Info, RawFile};

/// 8-bit RGBA, row by row, as it's uploaded to the GPU.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

fn invalid(what: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, what)
}

impl Image {
    pub fn decode_jpeg(jpeg: &[u8]) -> io::Result<Self> {
        let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
        let mut decoder = JpegDecoder::new_with_options(ZCursor::new(jpeg), options);
        let rgba = decoder.decode().map_err(|e| invalid(e.to_string()))?;
        let info = decoder.info().ok_or_else(|| invalid("JPEG without a size"))?;
        let (width, height) = (usize::from(info.width), usize::from(info.height));
        if rgba.len() != width * height * 4 {
            return Err(invalid("JPEG decoded to the wrong size"));
        }
        Ok(Self { width, height, rgba })
    }

    pub fn encode_jpeg(&self, quality: u8) -> io::Result<Vec<u8>> {
        let size = |n: usize| u16::try_from(n).map_err(|_| invalid("too big for a JPEG"));
        let mut out = Vec::new();
        jpeg_encoder::Encoder::new(&mut out, quality)
            .encode(&self.rgba, size(self.width)?, size(self.height)?, jpeg_encoder::ColorType::Rgba)
            .map_err(|e| invalid(e.to_string()))?;
        Ok(out)
    }

    pub fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        let at = (y * self.width + x) * 4;
        [self.rgba[at], self.rgba[at + 1], self.rgba[at + 2], self.rgba[at + 3]]
    }

    /// Turned upright for a TIFF orientation: 1 is as stored, 6 needs a
    /// quarter turn clockwise, 8 one anticlockwise, 3 a half turn; 2, 4, 5
    /// and 7 are those mirrored.
    pub fn oriented(self, orientation: u16) -> Self {
        let (w, h) = (self.width, self.height);
        // Where each output pixel comes from, for an output `ow` wide.
        let source: fn(usize, usize, usize, usize) -> (usize, usize) = match orientation {
            2 => |x, y, w, _| (w - 1 - x, y),
            3 => |x, y, w, h| (w - 1 - x, h - 1 - y),
            4 => |x, y, _, h| (x, h - 1 - y),
            5 => |x, y, _, _| (y, x),
            6 => |x, y, _, h| (y, h - 1 - x),
            7 => |x, y, w, h| (w - 1 - y, h - 1 - x),
            8 => |x, y, w, _| (w - 1 - y, x),
            _ => return self,
        };
        let (ow, oh) = if orientation >= 5 { (h, w) } else { (w, h) };
        let mut rgba = Vec::with_capacity(self.rgba.len());
        for y in 0..oh {
            for x in 0..ow {
                let (sx, sy) = source(x, y, w, h);
                rgba.extend(self.pixel(sx, sy));
            }
        }
        Self { width: ow, height: oh, rgba }
    }

    /// Shrunk so its long edge is at most `long_edge`, each pixel the
    /// average of the ones it covers.
    pub fn shrunk(&self, long_edge: usize) -> Self {
        let scale = long_edge as f64 / self.width.max(self.height) as f64;
        if scale >= 1.0 {
            return self.clone();
        }
        let ow = ((self.width as f64 * scale).round() as usize).max(1);
        let oh = ((self.height as f64 * scale).round() as usize).max(1);
        let span = |o: usize, of: usize, from: usize| (o * from / of, ((o + 1) * from / of).max(o * from / of + 1));
        let mut rgba = Vec::with_capacity(ow * oh * 4);
        for oy in 0..oh {
            let (y0, y1) = span(oy, oh, self.height);
            for ox in 0..ow {
                let (x0, x1) = span(ox, ow, self.width);
                let mut sum = [0u32; 4];
                for y in y0..y1 {
                    let row = &self.rgba[(y * self.width + x0) * 4..(y * self.width + x1) * 4];
                    for px in row.as_chunks::<4>().0 {
                        for c in 0..4 {
                            sum[c] += u32::from(px[c]);
                        }
                    }
                }
                let n = ((y1 - y0) * (x1 - x0)) as u32;
                rgba.extend(sum.map(|s| ((s + n / 2) / n) as u8));
            }
        }
        Self { width: ow, height: oh, rgba }
    }
}

/// The biggest JPEG the camera embedded in a raw, upright.
pub fn preview(raw: &Path) -> io::Result<Image> {
    Ok(preview_with_info(raw)?.0)
}

/// The preview, the colours it's in, and the rest of what the loupe shows.
pub fn preview_with_info(raw: &Path) -> io::Result<(Image, Space, Info)> {
    let raw = RawFile::open(raw)?;
    let jpeg = raw.preview().ok_or_else(|| invalid("no preview in the raw"))?;
    let image = Image::decode_jpeg(&raw.read(jpeg)?)?.oriented(raw.orientation);
    let space = if raw.adobe_rgb { Space::AdobeRgb } else { Space::Srgb };
    Ok((image, space, raw.info()))
}

/// How many pixels have each level, red, green and blue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Histogram(pub [[u32; 256]; 3]);

impl Histogram {
    pub fn of(image: &Image) -> Self {
        let mut counts = [[0; 256]; 3];
        for px in image.rgba.as_chunks::<4>().0 {
            for c in 0..3 {
                counts[c][usize::from(px[c])] += 1;
            }
        }
        Self(counts)
    }
}

/// What [`marks`] says of a pixel, as bits.
pub mod mark {
    /// Black, or as near as makes no difference, in every channel.
    pub const SHADOW: u8 = 1;
    /// White in at least one channel.
    pub const HIGHLIGHT: u8 = 2;
    /// On a crisp edge: in focus.
    pub const SHARP: u8 = 4;
}

/// Edges at least this crisp are marked sharp, whatever the picture: the
/// steepness of a step of 40 levels from one pixel to the next (Sobel,
/// |gx| + |gy|).
const SHARP_FLOOR: u32 = 160;
/// And of the picture's own edges, the crispest this part are.
const SHARP_SHARE: f64 = 0.015;

/// For every pixel, whether it's clipped and whether it's on a sharp edge:
/// for the clipping and focus peaking overlays. Measured on the pixels as
/// the camera rendered them, before any conversion for the monitor.
pub fn marks(image: &Image) -> Vec<u8> {
    let (w, h) = (image.width, image.height);
    let luma: Vec<u8> = image
        .rgba
        .as_chunks::<4>()
        .0
        .par_iter()
        .map(|p| ((u32::from(p[0]) * 54 + u32::from(p[1]) * 183 + u32::from(p[2]) * 19) >> 8) as u8)
        .collect();
    let gradient = |x: usize, y: usize| -> u32 {
        let at = |dx: usize, dy: usize| i32::from(luma[(y + dy - 1) * w + x + dx - 1]);
        let gx = at(2, 0) + 2 * at(2, 1) + at(2, 2) - at(0, 0) - 2 * at(0, 1) - at(0, 2);
        let gy = at(0, 2) + 2 * at(1, 2) + at(2, 2) - at(0, 0) - 2 * at(1, 0) - at(2, 0);
        gx.unsigned_abs() + gy.unsigned_abs()
    };
    // How crisp the crispest edges are, from every fourth row.
    let inner = |y: usize| y > 0 && y + 1 < h;
    let mut counts = vec![0u64; 2041];
    for y in (1..h.saturating_sub(1)).step_by(4) {
        for x in 1..w - 1 {
            counts[gradient(x, y) as usize] += 1;
        }
    }
    let total: u64 = counts.iter().sum();
    let mut above = 0;
    let crispest = (0..counts.len())
        .rev()
        .find(|&g| {
            above += counts[g];
            above as f64 > total as f64 * SHARP_SHARE
        })
        .unwrap_or(0) as u32;
    let threshold = crispest.max(SHARP_FLOOR);

    let mut marks = vec![0u8; w * h];
    marks.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
        for (x, m) in row.iter_mut().enumerate() {
            let p = image.pixel(x, y);
            let brightest = p[0].max(p[1]).max(p[2]);
            *m = if brightest >= 254 { mark::HIGHLIGHT } else if brightest <= 2 { mark::SHADOW } else { 0 };
            if inner(y) && x > 0 && x + 1 < w && gradient(x, y) >= threshold {
                *m |= mark::SHARP;
            }
        }
    });
    marks
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `w` × `h` image whose pixels say where they are: red is x, green y.
    fn numbered(w: usize, h: usize) -> Image {
        let rgba = (0..h).flat_map(|y| (0..w).flat_map(move |x| [x as u8, y as u8, 0, 255])).collect();
        Image { width: w, height: h, rgba }
    }

    /// Where the pixels of the top row came from.
    fn top_row(image: &Image) -> Vec<(u8, u8)> {
        (0..image.width).map(|x| image.pixel(x, 0)).map(|p| (p[0], p[1])).collect()
    }

    #[test]
    fn orientations_turn_and_mirror() {
        // 3 wide, 2 high:  (0,0) (1,0) (2,0)
        //                  (0,1) (1,1) (2,1)
        type Expected = (u16, (usize, usize), &'static [(u8, u8)]);
        let expect: [Expected; 8] = [
            (1, (3, 2), &[(0, 0), (1, 0), (2, 0)]),
            (2, (3, 2), &[(2, 0), (1, 0), (0, 0)]),
            (3, (3, 2), &[(2, 1), (1, 1), (0, 1)]),
            (4, (3, 2), &[(0, 1), (1, 1), (2, 1)]),
            (5, (2, 3), &[(0, 0), (0, 1)]),
            // A quarter turn clockwise: the left column becomes the top row,
            // read from the bottom up.
            (6, (2, 3), &[(0, 1), (0, 0)]),
            (7, (2, 3), &[(2, 1), (2, 0)]),
            (8, (2, 3), &[(2, 0), (2, 1)]),
        ];
        for (orientation, size, row) in expect {
            let turned = numbered(3, 2).oriented(orientation);
            assert_eq!((turned.width, turned.height), size, "orientation {orientation}");
            assert_eq!(top_row(&turned), row, "orientation {orientation}");
        }
    }

    #[test]
    fn shrinking_averages_and_keeps_the_aspect() {
        let mut image = numbered(300, 200);
        image.rgba.iter_mut().step_by(4).enumerate().for_each(|(i, r)| *r = if i % 2 == 0 { 0 } else { 200 });
        let small = image.shrunk(30);
        assert_eq!((small.width, small.height), (30, 20));
        assert_eq!(small.pixel(7, 7)[0], 100);
        // Never enlarged.
        assert_eq!(numbered(10, 5).shrunk(30), numbered(10, 5));
    }

    #[test]
    fn a_jpeg_round_trips() {
        let image = Image { width: 16, height: 8, rgba: [90, 160, 30, 255].repeat(16 * 8) };
        let back = Image::decode_jpeg(&image.encode_jpeg(90).unwrap()).unwrap();
        assert_eq!((back.width, back.height), (16, 8));
        let [r, g, b, a] = back.pixel(3, 3);
        assert!(r.abs_diff(90) < 4 && g.abs_diff(160) < 4 && b.abs_diff(30) < 4 && a == 255, "{:?}", back.pixel(3, 3));
        assert!(Image::decode_jpeg(b"not a jpeg").is_err());
    }

    #[test]
    fn histograms_count_each_channel() {
        let image = Image { width: 2, height: 1, rgba: vec![0, 128, 255, 255, 0, 0, 255, 255] };
        let Histogram([r, g, b]) = Histogram::of(&image);
        assert_eq!((r[0], g[128], g[0], b[255]), (2, 1, 1, 2));
        assert_eq!(r.iter().sum::<u32>(), 2);
    }

    #[test]
    fn marks_find_clipping_and_crisp_edges() {
        // Black on the left, white on the right, mid-grey between with a
        // soft ramp into it: one crisp edge, at x = 20.
        let (w, h) = (40, 10);
        let level = |x: usize| match x {
            0..5 => 0,
            5..10 => 60 + (x as u8 - 5) * 4,
            10..20 => 80,
            20..30 => 200,
            _ => 255,
        };
        let rgba = (0..h).flat_map(|_| (0..w).flat_map(|x| [level(x), level(x), level(x), 255])).collect();
        let image = Image { width: w, height: h, rgba };
        let found = marks(&image);
        let row = |y: usize| &found[y * w..(y + 1) * w];
        assert_eq!(row(5)[2], mark::SHADOW);
        assert_eq!(row(5)[35], mark::HIGHLIGHT);
        assert_eq!(row(5)[15], 0);
        assert_eq!(row(5)[7] & mark::SHARP, 0, "the soft ramp isn't sharp");
        assert_ne!(row(5)[20] & mark::SHARP, 0, "the step is");
        assert_eq!(row(0)[20] & mark::SHARP, 0, "the border isn't measured");

        // A picture with nothing crisp in it has nothing marked sharp.
        let ramp = (0..h).flat_map(|_| (0..w).flat_map(|x| [x as u8 * 3; 4])).collect();
        let soft = Image { width: w, height: h, rgba: ramp };
        assert!(marks(&soft).iter().all(|m| m & mark::SHARP == 0));
    }

    #[test]
    fn the_preview_comes_out_upright() {
        let folder = crate::testing::Folder::new("image-preview");
        let path = folder.0.join("DSC00001.ARW");
        crate::testing::Arw { orientation: 6, ..Default::default() }.write(&path);
        let image = preview(&path).unwrap();
        assert_eq!((image.width, image.height), (32, 48));
        let adobe = crate::testing::Arw { color_space: 0xffff, ..Default::default() };
        adobe.write(&path);
        assert_eq!(preview_with_info(&path).unwrap().1, Space::AdobeRgb);
    }
}
