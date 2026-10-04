//! Pixels: decoding the camera's JPEGs, turning them upright and making
//! filmstrip thumbnails of them.

use std::io::{self, ErrorKind};
use std::path::Path;

use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

use crate::raw::RawFile;

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
    let raw = RawFile::open(raw)?;
    let jpeg = raw.preview().ok_or_else(|| invalid("no preview in the raw"))?;
    Ok(Image::decode_jpeg(&raw.read(jpeg)?)?.oriented(raw.orientation))
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
    fn the_preview_comes_out_upright() {
        let folder = crate::testing::Folder::new("image-preview");
        let path = folder.0.join("DSC00001.ARW");
        crate::testing::Arw { orientation: 6, ..Default::default() }.write(&path);
        let image = preview(&path).unwrap();
        assert_eq!((image.width, image.height), (32, 48));
    }
}
