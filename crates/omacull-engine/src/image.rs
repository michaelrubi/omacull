//! Pixels: decoding the camera's JPEGs (and PNGs), turning them upright,
//! making filmstrip thumbnails of them, and measuring them for the loupe's
//! histogram, clipping and focus peaking.

use std::io::{self, ErrorKind};
use std::path::Path;

use rayon::prelude::*;
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

use crate::color::{self, Space, Tagged};
use crate::raw::{Developed, Info, RawFile};

/// A preview is no bigger than this on its long edge: where a camera
/// embeds only a full-size JPEG, or the picture is a JPEG or a PNG itself,
/// that's shrunk to what the loupe shows whole.
pub(crate) const LARGEST_PREVIEW: usize = 2048;

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

    /// A PNG's pixels and the colours they're in. The profile it carries
    /// is believed, as a JPEG's is, and anything but sRGB and Adobe RGB
    /// converted to sRGB. Without one it's taken for sRGB. What's
    /// transparent is shown on grey.
    pub fn decode_png(png: &[u8]) -> io::Result<(Self, Space)> {
        /// Pixels converted at a time, on every core.
        const PART: usize = 1 << 14;
        let mut decoder = png::Decoder::new(io::Cursor::new(png));
        // A palette's colours as colours, and no fewer than 8 bits.
        decoder.set_transformations(png::Transformations::EXPAND);
        let mut reader = decoder.read_info().map_err(invalid)?;
        // A few bytes can claim to be a picture of any size. No bigger
        // than a JPEG is decoded.
        let size = reader.output_buffer_size().filter(|_| reader.info().width.max(reader.info().height) <= 1 << 14);
        let mut samples = vec![0; size.ok_or_else(|| invalid("PNG too big to decode"))?];
        let frame = reader.next_frame(&mut samples).map_err(invalid)?;
        let (width, height) = (frame.width as usize, frame.height as usize);
        // Grey or colour, with or without alpha, in 8 bits or 16.
        let channels = frame.color_type.samples();
        let wide = frame.bit_depth == png::BitDepth::Sixteen;
        let step = channels * if wide { 2 } else { 1 };
        if frame.buffer_size() != width * height * step || samples.len() < frame.buffer_size() {
            return Err(invalid("PNG decoded to the wrong size"));
        }
        let pixel = |px: &[u8]| -> [u16; 4] {
            let sample = |c: usize| match wide {
                true => u16::from_be_bytes([px[c * 2], px[c * 2 + 1]]),
                false => u16::from(px[c]) * 257,
            };
            let alpha = if channels % 2 == 0 { sample(channels - 1) } else { u16::MAX };
            if channels < 3 { [sample(0), sample(0), sample(0), alpha] } else { [sample(0), sample(1), sample(2), alpha] }
        };
        let (space, profile) = match reader.info().icc_profile.as_deref().and_then(Tagged::read) {
            Some(Tagged::Is(space)) => (space, None),
            Some(Tagged::Other(profile)) => (Space::Srgb, Some(profile)),
            None => (Space::Srgb, None),
        };
        // Colours of its own are converted before 16 bits are cut to 8,
        // which is slow, or as a JPEG's are where there are only 8.
        let deep_to_srgb = profile.as_ref().filter(|_| wide).and_then(|profile| color::deep(profile).ok());
        let to_srgb = profile.as_ref().filter(|_| !wide).and_then(|profile| color::to_srgb(profile).ok());
        let mut rgba = vec![0; width * height * 4];
        let parts = rgba.par_chunks_mut(PART * 4).zip(samples[..frame.buffer_size()].par_chunks(PART * step));
        parts.for_each(|(rgba, samples)| {
            let deep: Vec<[u16; 4]> = samples.chunks_exact(step).map(pixel).collect();
            let (rgba, _) = rgba.as_chunks_mut::<4>();
            let cut = |deep: &[u16; 4]| deep.map(|v| ((u32::from(v) + 128) / 257) as u8);
            match &deep_to_srgb {
                Some(to_srgb) => to_srgb.transform_pixels(&deep, rgba),
                None => rgba.iter_mut().zip(&deep).for_each(|(px, deep)| *px = cut(deep)),
            }
            if let Some(to_srgb) = &to_srgb {
                to_srgb.transform_in_place(rgba);
            }
            for px in rgba.iter_mut().filter(|px| px[3] < 255) {
                let alpha = u32::from(px[3]);
                *px = px.map(|v| ((u32::from(v) * alpha + 128 * (255 - alpha) + 127) / 255) as u8);
                px[3] = 255;
            }
        });
        Ok((Self { width, height, rgba }, space))
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

    /// The part `width` × `height` from (`x`, `y`), kept inside the image.
    pub fn crop(&self, x: usize, y: usize, width: usize, height: usize) -> Self {
        let (x, y) = (x.min(self.width), y.min(self.height));
        let (w, h) = (width.min(self.width - x), height.min(self.height - y));
        let mut rgba = Vec::with_capacity(w * h * 4);
        for row in y..y + h {
            rgba.extend_from_slice(&self.rgba[(row * self.width + x) * 4..(row * self.width + x + w) * 4]);
        }
        Self { width: w, height: h, rgba }
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

/// The JPEG the camera embedded in a raw for looking at it, upright.
pub fn preview(raw: &Path) -> io::Result<Image> {
    Ok(preview_with_info(raw)?.0)
}

/// The preview, the colours it's in, and the rest of what the loupe shows.
/// A JPEG's preview is the JPEG, shrunk, and a PNG's the PNG.
pub fn preview_with_info(raw: &Path) -> io::Result<(Image, Space, Info)> {
    let raw = RawFile::open(raw)?;
    let (image, space) = picture(&raw, LARGEST_PREVIEW)?;
    Ok((image, space, raw.info()))
}

/// The picture to look at a file by, upright and no bigger than `largest`
/// on its long edge, and the colours it's in: a raw's embedded preview, or
/// the whole of a JPEG or a PNG on its own.
pub(crate) fn picture(raw: &RawFile, largest: usize) -> io::Result<(Image, Space)> {
    let bytes = raw.read(raw.preview().ok_or_else(|| invalid("no preview in the raw"))?)?;
    let (mut image, mut space) = match raw.developed {
        // A PNG's colours are its own business, not its Exif's.
        Some(Developed::Png) => Image::decode_png(&bytes)?,
        _ => (Image::decode_jpeg(&bytes)?, if raw.adobe_rgb { Space::AdobeRgb } else { Space::Srgb }),
    };
    if image.width.max(image.height) > largest {
        image = image.shrunk(largest);
    }
    // A profile of its own says what a JPEG is in, whatever its Exif does.
    if let Some(known) = raw.icc().and_then(|icc| color::tagged(&mut image, &icc)) {
        space = known;
    }
    Ok((image.oriented(raw.orientation), space))
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
    fn crops_stay_inside() {
        let image = numbered(10, 8);
        let part = image.crop(3, 2, 4, 3);
        assert_eq!((part.width, part.height, part.pixel(0, 0)[..2].to_vec()), (4, 3, vec![3, 2]));
        assert_eq!(part.pixel(3, 2)[..2], [6, 4]);
        let edge = image.crop(8, 6, 5, 5);
        assert_eq!((edge.width, edge.height), (2, 2));
        assert_eq!(image.crop(20, 20, 5, 5).rgba.len(), 0);
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
    fn a_png_decodes_whatever_its_made_of() {
        use crate::testing::Png;
        let decode = |png: Png| Image::decode_png(&png.bytes()).unwrap();
        // 8 bits a channel and no profile: sRGB, as it is.
        let (image, space) = decode(Png::default());
        assert_eq!((image.width, image.height, image.pixel(5, 5), space), (48, 32, [200, 120, 40, 255], Space::Srgb));
        // 16 bits are cut to 8.
        assert_eq!(decode(Png { deep: true, ..Png::default() }).0, image);
        // What's transparent is shown on grey.
        let see_through = |alpha: u16| decode(Png { rgba: [u16::MAX, 0, 0, alpha], alpha: true, ..Png::default() }).0;
        assert_eq!(see_through(0x8080).pixel(5, 5), [192, 64, 64, 255]);
        assert_eq!(see_through(0).pixel(5, 5), [128, 128, 128, 255]);

        // A profile of its own is believed. Linear: mid-grey is far brighter
        // once it's sRGB, and a shadow that 8 bits of linear would have
        // lost is still there.
        let curve = lcms2::ToneCurve::new(1.0);
        let white = lcms2::CIExyY { x: 0.3127, y: 0.3290, Y: 1.0 };
        let point = |x, y| lcms2::CIExyY { x, y, Y: 1.0 };
        let primaries = lcms2::CIExyYTRIPLE { Red: point(0.64, 0.33), Green: point(0.30, 0.60), Blue: point(0.15, 0.06) };
        let linear = lcms2::Profile::new_rgb(&white, &primaries, &[&curve, &curve, &curve]).unwrap().icc().unwrap();
        let grey = |level: u16, deep: bool, icc: &[u8]| {
            let (image, space) = decode(Png { rgba: [level; 4], deep, icc: Some(icc.to_vec()), ..Png::default() });
            (image.pixel(5, 5)[0], image.pixel(5, 5)[3], space)
        };
        let (mid, alpha, space) = grey(128 * 257, false, &linear);
        assert!((185..=191).contains(&mid) && alpha == 255 && space == Space::Srgb, "{mid} {alpha} {space:?}");
        let (shadow, alpha, _) = grey(200, true, &linear);
        assert!((9..=11).contains(&shadow) && alpha == 255, "{shadow} {alpha}");
        // Adobe RGB by its profile is left as it is.
        let adobe = crate::color::ColorProfile::adobe_rgb();
        assert_eq!(grey(128 * 257, true, adobe.icc()), (128, 255, Space::AdobeRgb));

        // A palette of four colours, one of them transparent.
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, 2, 1);
        encoder.set_color(png::ColorType::Indexed);
        encoder.set_depth(png::BitDepth::Two);
        encoder.set_palette(vec![10, 20, 30, 200, 100, 50]);
        encoder.set_trns(vec![255, 0]);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[0b0001_0000]).unwrap();
        writer.finish().unwrap();
        let (image, _) = Image::decode_png(&out).unwrap();
        assert_eq!((image.pixel(0, 0), image.pixel(1, 0)), ([10, 20, 30, 255], [128, 128, 128, 255]));

        assert!(Image::decode_png(b"not a png").is_err());
        assert!(Image::decode_png(&Png::default().bytes()[..60]).is_err());
        assert!(Image::decode_png(&Png { width: 20_000, height: 1, ..Png::default() }.bytes()).is_err(), "too big");
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
        // A full-size JPEG, where it's all a camera embeds, is shrunk to
        // what the loupe shows whole.
        let full = crate::testing::jpeg(3000, 2000, [90, 90, 90]);
        crate::testing::Arw { preview: full, thumbnail: Vec::new(), orientation: 8, ..Default::default() }.write(&path);
        let image = preview(&path).unwrap();
        assert_eq!((image.width, image.height), (1365, 2048));
    }

    #[test]
    fn a_jpeg_is_its_own_preview_in_the_colours_its_profile_says() {
        use crate::testing::{Arw, jpeg, with_icc};
        let folder = crate::testing::Folder::new("image-jpeg");
        let path = folder.0.join("DSC00001.JPG");
        let camera = Arw { preview: jpeg(3000, 2000, [128, 128, 128]), orientation: 6, ..Default::default() }.jpeg();
        std::fs::write(&path, &camera).unwrap();
        let (image, space, info) = preview_with_info(&path).unwrap();
        assert_eq!((image.width, image.height, space), (1365, 2048, Space::Srgb), "shrunk and upright");
        assert_eq!((info.exif.iso, info.size), (Some(400), Some((2000, 3000))));

        // Its Exif says sRGB; its profile says linear, and is believed.
        let curve = lcms2::ToneCurve::new(1.0);
        let white = lcms2::CIExyY { x: 0.3127, y: 0.3290, Y: 1.0 };
        let point = |x, y| lcms2::CIExyY { x, y, Y: 1.0 };
        let primaries = lcms2::CIExyYTRIPLE { Red: point(0.64, 0.33), Green: point(0.30, 0.60), Blue: point(0.15, 0.06) };
        let linear = lcms2::Profile::new_rgb(&white, &primaries, &[&curve, &curve, &curve]).unwrap().icc().unwrap();
        std::fs::write(&path, with_icc(&camera, &linear)).unwrap();
        let (image, space, _) = preview_with_info(&path).unwrap();
        assert_eq!(space, Space::Srgb);
        assert!((185..=191).contains(&image.pixel(100, 100)[0]), "{:?}", image.pixel(100, 100));

        // Adobe RGB by its profile, whatever the Exif says.
        let adobe = crate::color::ColorProfile::adobe_rgb();
        std::fs::write(&path, with_icc(&camera, adobe.icc())).unwrap();
        let (image, space, _) = preview_with_info(&path).unwrap();
        assert_eq!((space, image.pixel(100, 100)[0]), (Space::AdobeRgb, 128));
    }
}
