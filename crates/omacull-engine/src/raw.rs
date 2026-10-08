//! What a culler needs from a raw file, read without decoding the raw: the
//! JPEGs the camera embedded, the orientation, where it focused and the
//! shooting settings.
//!
//! An ARW is a TIFF, and so is Nikon's NEF. Only the directories are read,
//! a few hundred bytes each, so opening a 50 MB raw costs a handful of
//! small reads. Canon's CR3 is boxes, as in an MP4, with small TIFFs
//! inside for the settings; Fuji's RAF is a header pointing at a whole
//! JPEG file, with the settings in that JPEG's own Exif. The focus point
//! is only read from Sony's.
//!
//! A JPEG on its own is read the same way: the picture is the whole file,
//! and its Exif holds the settings and, from a camera, a thumbnail. So is
//! a PNG, whose Exif, where it has any, is in a chunk of its own.

use std::fs::File;
use std::io::{self, ErrorKind};
use std::os::unix::fs::FileExt;
use std::path::Path;

/// A JPEG the camera embedded in the raw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Embedded {
    pub offset: u64,
    pub len: u64,
}

/// Where the camera focused, in the coordinates of a `width` × `height`
/// frame (the camera's, not any preview's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Focus {
    pub width: u16,
    pub height: u16,
    pub x: u16,
    pub y: u16,
}

/// The shooting settings, for the readout under the loupe.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Exif {
    pub model: Option<String>,
    pub lens: Option<String>,
    /// Exposure time in seconds, as a fraction.
    pub exposure: Option<(u32, u32)>,
    pub f_number: Option<f32>,
    pub iso: Option<u32>,
    /// In millimetres.
    pub focal_length: Option<f32>,
}

/// What the loupe shows of a frame besides its pixels, upright.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Info {
    pub exif: Exif,
    pub captured: Option<String>,
    /// Where the camera focused, as fractions of the upright frame's width
    /// and height.
    pub focus: Option<[f32; 2]>,
    /// The raw's size in pixels, upright: about what a full decode gives.
    pub size: Option<(u32, u32)>,
}

impl Info {
    /// "1/250 s   f/2.8   ISO 400   85 mm   FE 85mm F1.8 GM   2026-10-04 15:43:16".
    pub fn summary(&self) -> String {
        let e = &self.exif;
        let number = |n: f32| if n.fract().abs() < 0.05 { format!("{n:.0}") } else { format!("{n:.1}") };
        let exposure = e.exposure.filter(|&(_, d)| d > 0).map(|(n, d)| match n {
            // 1/250 s, 0.6 s, 2 s, 1.3 s.
            1 if d > 1 => format!("1/{d} s"),
            _ if n % d == 0 => format!("{} s", n / d),
            _ if d > n * 3 => format!("1/{} s", (d as f32 / n as f32).round()),
            _ => format!("{} s", number(n as f32 / d as f32)),
        });
        let time = self.captured.as_deref().map(|t| {
            // Exif writes the date with colons, and a fraction of a second
            // isn't worth showing.
            let t = t.split('.').next().unwrap_or(t);
            match t.split_once(' ') {
                Some((date, clock)) => format!("{} {clock}", date.replace(':', "-")),
                None => t.to_owned(),
            }
        });
        [
            exposure,
            e.f_number.map(|f| format!("f/{}", number(f))),
            e.iso.map(|iso| format!("ISO {iso}")),
            e.focal_length.map(|mm| format!("{} mm", number(mm))),
            e.lens.clone().or(e.model.clone()),
            time,
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("   ")
    }
}

/// Where a point in a frame stored with this TIFF orientation ends up once
/// the frame is turned upright, both as fractions of width and height.
pub fn upright(orientation: u16, [u, v]: [f32; 2]) -> [f32; 2] {
    match orientation {
        2 => [1.0 - u, v],
        3 => [1.0 - u, 1.0 - v],
        4 => [u, 1.0 - v],
        5 => [v, u],
        6 => [1.0 - v, u],
        7 => [1.0 - v, 1.0 - u],
        8 => [v, 1.0 - u],
        _ => [u, v],
    }
}

/// A picture that isn't a raw: the whole file is the picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Developed {
    Jpeg,
    Png,
}

pub struct RawFile {
    file: File,
    /// Every embedded JPEG, in the order the file lists them. A JPEG or a
    /// PNG on its own is the last of them, whole.
    pub jpegs: Vec<Embedded>,
    /// The TIFF orientation (1 to 8); 1 if the file doesn't say.
    pub orientation: u16,
    /// None when the camera recorded no focus location (manual focus, or
    /// tracking on some bodies).
    pub focus: Option<Focus>,
    /// When the shutter fired, as Exif writes it (`2023:10:25 15:43:16`),
    /// with the fraction of a second after a dot if the camera recorded it.
    pub captured: Option<String>,
    pub exif: Exif,
    /// The embedded JPEGs are Adobe RGB, not sRGB.
    pub adobe_rgb: bool,
    /// The largest image in the file, the raw data, as stored.
    pub size: Option<(u32, u32)>,
    /// Not a raw: a JPEG or a PNG on its own, the picture itself.
    pub developed: Option<Developed>,
}

pub(crate) const MAKE: u16 = 0x010f;
pub(crate) const JPEG_OFFSET: u16 = 0x0201;
pub(crate) const JPEG_LENGTH: u16 = 0x0202;
pub(crate) const ORIENTATION: u16 = 0x0112;
pub(crate) const SUB_IFDS: u16 = 0x014a;
pub(crate) const EXIF_IFD: u16 = 0x8769;
pub(crate) const MAKER_NOTE: u16 = 0x927c;
pub(crate) const SONY_FOCUS_LOCATION: u16 = 0x2027;
pub(crate) const DATE_TIME_ORIGINAL: u16 = 0x9003;
pub(crate) const SUB_SEC_TIME_ORIGINAL: u16 = 0x9291;
pub(crate) const MODEL: u16 = 0x0110;
pub(crate) const IMAGE_WIDTH: u16 = 0x0100;
pub(crate) const IMAGE_LENGTH: u16 = 0x0101;
pub(crate) const EXPOSURE_TIME: u16 = 0x829a;
pub(crate) const F_NUMBER: u16 = 0x829d;
pub(crate) const ISO: u16 = 0x8827;
pub(crate) const FOCAL_LENGTH: u16 = 0x920a;
pub(crate) const LENS_MODEL: u16 = 0xa434;
pub(crate) const COLOR_SPACE: u16 = 0xa001;
pub(crate) const INTEROP_IFD: u16 = 0xa005;
pub(crate) const INTEROP_INDEX: u16 = 0x0001;
/// In Sony's makernote: 0 for manual focus (ExifTool's FocusMode, 0x201b).
pub(crate) const SONY_FOCUS_MODE: u16 = 0x201b;
/// In Nikon's makernote: a directory holding a small JPEG.
pub(crate) const NIKON_PREVIEW_IFD: u16 = 0x0011;
/// A preview at least this many pixels on its long edge fills the loupe.
const PREVIEW_EDGE: u32 = 1400;

fn invalid(what: &str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, what)
}

/// One directory entry. Values of four bytes or fewer are stored in
/// `value` itself; longer ones at the offset it holds.
struct Entry {
    tag: u16,
    kind: u16,
    count: u32,
    value: [u8; 4],
}

/// A TIFF somewhere in a file: the whole of an ARW, or the Exif of a JPEG
/// inside a raw. Its offsets count from `base`.
struct Tiff<'a> {
    file: &'a File,
    big_endian: bool,
    base: u64,
    /// The offset of its first directory.
    first: u64,
}

/// What's been found in a raw so far.
#[derive(Default)]
struct Found {
    jpegs: Vec<Embedded>,
    orientation: Option<u16>,
    focus: Option<Focus>,
    date: Option<String>,
    fraction: Option<String>,
    exif: Exif,
    color_space: Option<u32>,
    interop: Option<String>,
    size: Option<(u32, u32)>,
    make: String,
}

impl<'a> Tiff<'a> {
    /// The TIFF whose header is at `base`.
    fn at(file: &'a File, base: u64) -> io::Result<Self> {
        let mut header = [0; 8];
        file.read_exact_at(&mut header, base)?;
        let big_endian = match &header[..2] {
            b"II" => false,
            b"MM" => true,
            _ => return Err(invalid("not a TIFF-based raw")),
        };
        let mut tiff = Self { file, big_endian, base, first: 0 };
        if tiff.u16(&header[2..]) != 42 {
            return Err(invalid("not a TIFF-based raw"));
        }
        tiff.first = u64::from(tiff.u32(&header[4..]));
        Ok(tiff)
    }

    fn read(&self, bytes: &mut [u8], offset: u64) -> io::Result<()> {
        self.file.read_exact_at(bytes, self.base + offset)
    }

    /// Every directory, and what a culler wants from each.
    fn walk(&self, found: &mut Found) -> io::Result<()> {
        // The main chain of directories, and the sub-directories hanging off
        // them. A corrupt file could chain in a loop, so stop after a few.
        let mut pending = vec![self.first];
        let mut seen = 0;
        while let Some(offset) = pending.pop() {
            seen += 1;
            if offset == 0 || seen > 32 {
                continue;
            }
            let (entries, next) = self.directory(offset)?;
            // Sub-directories and the next in the chain go on a stack, so
            // push the next first to visit the sub-directories before it.
            pending.push(next);
            let (mut at, mut len, mut width, mut height) = (None, None, None, None);
            let ratio = |e: &Entry| self.rational(e).ok().filter(|&(_, d)| d > 0).map(|(n, d)| n as f32 / d as f32);
            let exif = &mut found.exif;
            for e in &entries {
                match e.tag {
                    JPEG_OFFSET => at = Some(self.number(e)),
                    JPEG_LENGTH => len = Some(self.number(e)),
                    ORIENTATION if found.orientation.is_none() => found.orientation = Some(self.number(e) as u16),
                    SUB_IFDS => pending.extend(self.offsets(e)?),
                    EXIF_IFD => pending.push(u64::from(self.u32(&e.value))),
                    MAKE => found.make = self.text(e).unwrap_or_default().to_ascii_uppercase(),
                    MAKER_NOTE if found.make.starts_with("SONY") => {
                        found.focus = sony_focus(self, u64::from(self.u32(&e.value)));
                    }
                    MAKER_NOTE if found.make.starts_with("NIKON") => {
                        found.jpegs.extend(nikon_preview(self.file, self.base + u64::from(self.u32(&e.value))));
                    }
                    DATE_TIME_ORIGINAL if found.date.is_none() => found.date = self.text(e).ok(),
                    SUB_SEC_TIME_ORIGINAL if found.fraction.is_none() => found.fraction = self.text(e).ok(),
                    MODEL if exif.model.is_none() => exif.model = self.text(e).ok().filter(|m| !m.is_empty()),
                    LENS_MODEL => exif.lens = self.text(e).ok().filter(|l| !l.is_empty()),
                    EXPOSURE_TIME => exif.exposure = self.rational(e).ok(),
                    F_NUMBER => exif.f_number = ratio(e),
                    FOCAL_LENGTH => exif.focal_length = ratio(e),
                    ISO => exif.iso = Some(self.number(e)),
                    COLOR_SPACE => found.color_space = Some(self.number(e)),
                    INTEROP_IFD => pending.push(u64::from(self.u32(&e.value))),
                    INTEROP_INDEX if e.kind == 2 => found.interop = self.text(e).ok(),
                    IMAGE_WIDTH => width = Some(self.number(e)),
                    IMAGE_LENGTH => height = Some(self.number(e)),
                    _ => {}
                }
            }
            if let (Some(at), Some(len)) = (at, len)
                && len > 0
            {
                found.jpegs.push(Embedded { offset: self.base + u64::from(at), len: u64::from(len) });
            }
            if let (Some(w), Some(h)) = (width, height)
                && found.size.is_none_or(|(sw, sh)| u64::from(w) * u64::from(h) > u64::from(sw) * u64::from(sh))
            {
                found.size = Some((w, h));
            }
        }
        Ok(())
    }

    fn u16(&self, b: &[u8]) -> u16 {
        let b = [b[0], b[1]];
        if self.big_endian { u16::from_be_bytes(b) } else { u16::from_le_bytes(b) }
    }

    fn u32(&self, b: &[u8]) -> u32 {
        let b = [b[0], b[1], b[2], b[3]];
        if self.big_endian { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) }
    }

    /// The entries of the directory at `offset`, and the offset of the
    /// directory chained after it (0 if none).
    fn directory(&self, offset: u64) -> io::Result<(Vec<Entry>, u64)> {
        let mut count = [0; 2];
        self.read(&mut count, offset)?;
        let count = usize::from(self.u16(&count));
        let mut bytes = vec![0; count * 12 + 4];
        self.read(&mut bytes, offset + 2)?;
        let entries = bytes[..count * 12]
            .as_chunks::<12>()
            .0
            .iter()
            .map(|e| Entry {
                tag: self.u16(e),
                kind: self.u16(&e[2..]),
                count: self.u32(&e[4..]),
                value: [e[8], e[9], e[10], e[11]],
            })
            .collect();
        Ok((entries, u64::from(self.u32(&bytes[count * 12..]))))
    }

    /// An entry's value as one number (a SHORT or a LONG).
    fn number(&self, e: &Entry) -> u32 {
        if e.kind == 3 { u32::from(self.u16(&e.value)) } else { self.u32(&e.value) }
    }

    /// An entry's value as text (ASCII), without the trailing NULs.
    fn text(&self, e: &Entry) -> io::Result<String> {
        let mut bytes = vec![0; e.count.min(64) as usize];
        if e.count <= 4 {
            let len = bytes.len();
            bytes.copy_from_slice(&e.value[..len]);
        } else {
            self.read(&mut bytes, u64::from(self.u32(&e.value)))?;
        }
        Ok(String::from_utf8_lossy(&bytes).trim_end_matches('\0').trim().to_owned())
    }

    /// An entry's value as a fraction (a RATIONAL).
    fn rational(&self, e: &Entry) -> io::Result<(u32, u32)> {
        let mut bytes = [0; 8];
        self.read(&mut bytes, u64::from(self.u32(&e.value)))?;
        Ok((self.u32(&bytes), self.u32(&bytes[4..])))
    }

    /// An entry's value as a list of offsets (LONGs).
    fn offsets(&self, e: &Entry) -> io::Result<Vec<u64>> {
        if e.count <= 1 {
            return Ok(vec![u64::from(self.u32(&e.value))]);
        }
        // No raw has more than a few sub-directories.
        let mut bytes = vec![0; e.count.min(16) as usize * 4];
        self.read(&mut bytes, u64::from(self.u32(&e.value)))?;
        Ok(bytes.as_chunks::<4>().0.iter().map(|b| u64::from(self.u32(b))).collect())
    }
}

impl RawFile {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let mut header = [0; 16];
        file.read_exact_at(&mut header, 0)?;
        let mut found = Found::default();
        let mut developed = None;
        if header.starts_with(&[0xff, 0xd8]) {
            developed = Some(Developed::Jpeg);
            jpeg(&file, &mut found)?;
        } else if header.starts_with(b"\x89PNG\r\n\x1a\n") {
            developed = Some(Developed::Png);
            png(&file, &mut found)?;
        } else if header.starts_with(b"FUJIFILMCCD-RAW") {
            raf(&file, &mut found)?;
        } else if &header[4..8] == b"ftyp" {
            cr3(&file, &mut found)?;
        } else {
            Tiff::at(&file, 0)?.walk(&mut found)?;
        }
        let Found { jpegs, orientation, focus, date, fraction, exif, color_space, interop, size, .. } = found;
        // Adobe RGB is "uncalibrated" in Exif, with R03 for its interop index.
        let adobe_rgb = color_space == Some(0xffff) && interop.as_deref() != Some("R98");
        // Cameras without a clock set write blanks.
        let captured = date.filter(|d| d.starts_with(|c: char| c.is_ascii_digit())).map(|date| match fraction {
            Some(f) if !f.is_empty() => format!("{date}.{f}"),
            _ => date,
        });
        let orientation = orientation.unwrap_or(1);
        Ok(Self { file, jpegs, orientation, focus, captured, exif, adobe_rgb, size, developed })
    }

    /// The colour profile a JPEG on its own carries, if it has one: what
    /// its colours are, whatever its Exif says. (A PNG's is squeezed, and
    /// read as the PNG is decoded.)
    pub fn icc(&self) -> Option<Vec<u8>> {
        if self.developed != Some(Developed::Jpeg) {
            return None;
        }
        // In one or more segments, each numbered after the name.
        let mut parts: Vec<(u8, Vec<u8>)> = jpeg_segments(&self.file, 0)
            .filter(|&(marker, _, len)| marker == 0xe2 && len > 14)
            .filter_map(|(_, at, len)| {
                let mut bytes = vec![0; len as usize];
                self.file.read_exact_at(&mut bytes, at).ok()?;
                bytes.starts_with(b"ICC_PROFILE\0").then(|| (bytes[12], bytes.split_off(14)))
            })
            .collect();
        parts.sort_by_key(|&(number, _)| number);
        (!parts.is_empty()).then(|| parts.into_iter().flat_map(|(_, bytes)| bytes).collect())
    }

    /// What the loupe shows besides the pixels, upright.
    pub fn info(&self) -> Info {
        let turned = self.orientation >= 5;
        Info {
            exif: self.exif.clone(),
            captured: self.captured.clone(),
            focus: self.focus.map(|f| {
                let at = [f32::from(f.x) / f32::from(f.width), f32::from(f.y) / f32::from(f.height)];
                upright(self.orientation, at)
            }),
            size: self.size.map(|(w, h)| if turned { (h, w) } else { (w, h) }),
        }
    }

    /// The embedded JPEG to step through: the smallest that fills the
    /// loupe, or failing that the biggest. Sony embeds one of 1616×1080;
    /// other cameras a full-size one as well, too slow to step through.
    pub fn preview(&self) -> Option<Embedded> {
        // A PNG is the picture; a JPEG in its Exif is only a thumbnail.
        if self.jpegs.len() < 2 || self.developed == Some(Developed::Png) {
            return self.jpegs.last().copied();
        }
        let sized = self.jpegs.iter().map(|&j| (jpeg_size(&self.file, j.offset).map_or(0, |(w, h)| w.max(h)), j));
        let sized: Vec<(u32, Embedded)> = sized.collect();
        let big_enough = sized.iter().filter(|(edge, _)| *edge >= PREVIEW_EDGE).min_by_key(|(edge, j)| (*edge, j.len));
        big_enough.or_else(|| sized.iter().max_by_key(|(edge, j)| (*edge, j.len))).map(|&(_, j)| j)
    }

    /// The smallest embedded JPEG: the camera's own thumbnail.
    pub fn thumbnail(&self) -> Option<Embedded> {
        self.jpegs.iter().copied().min_by_key(|j| j.len)
    }

    pub fn read(&self, jpeg: Embedded) -> io::Result<Vec<u8>> {
        let mut bytes = vec![0; jpeg.len as usize];
        self.file.read_exact_at(&mut bytes, jpeg.offset)?;
        Ok(bytes)
    }
}

/// Sony's makernote is a directory with offsets from the start of the
/// file, in older bodies behind a 12-byte "SONY DSC " header. FocusLocation
/// is four SHORTs: frame width, frame height, x, y; all zero when there's
/// no location.
fn sony_focus(tiff: &Tiff, offset: u64) -> Option<Focus> {
    let mut header = [0; 12];
    tiff.read(&mut header, offset).ok()?;
    let skip = if header.starts_with(b"SONY") { 12 } else { 0 };
    let (entries, _) = tiff.directory(offset + skip).ok()?;
    // Manual focus records the middle of the frame, which means nothing.
    if entries.iter().any(|e| e.tag == SONY_FOCUS_MODE && e.kind == 1 && e.value[0] == 0) {
        return None;
    }
    let e = entries.iter().find(|e| e.tag == SONY_FOCUS_LOCATION && e.kind == 3 && e.count == 4)?;
    let mut bytes = [0; 8];
    tiff.read(&mut bytes, u64::from(tiff.u32(&e.value))).ok()?;
    let [width, height, x, y] = [0, 2, 4, 6].map(|i| tiff.u16(&bytes[i..]));
    (width > 0 && height > 0).then_some(Focus { width, height, x, y })
}

/// Nikon's makernote is a TIFF of its own behind a 10-byte "Nikon" header.
/// Its preview directory holds a small JPEG: the only small one in a NEF.
fn nikon_preview(file: &File, offset: u64) -> Option<Embedded> {
    let mut header = [0; 6];
    file.read_exact_at(&mut header, offset).ok()?;
    if &header != b"Nikon\0" {
        return None;
    }
    let tiff = Tiff::at(file, offset + 10).ok()?;
    let (entries, _) = tiff.directory(tiff.first).ok()?;
    let preview = entries.iter().find(|e| e.tag == NIKON_PREVIEW_IFD)?;
    let (entries, _) = tiff.directory(u64::from(tiff.u32(&preview.value))).ok()?;
    let number = |tag: u16| entries.iter().find(|e| e.tag == tag).map(|e| u64::from(tiff.number(e)));
    let (at, len) = (number(JPEG_OFFSET)?, number(JPEG_LENGTH)?);
    (len > 0).then_some(Embedded { offset: tiff.base + at, len })
}

/// A JPEG's segments from the one at `offset` (its first, after the start
/// marker) on, up to the picture itself: each one's marker, where what it
/// holds starts, and how long that is.
fn jpeg_segments(file: &File, offset: u64) -> impl Iterator<Item = (u8, u64, u64)> + '_ {
    let mut at = offset + 2;
    // A corrupt file could go on for ever; a phone's JPEG can have dozens
    // before its picture.
    (0..1024).map_while(move |_| {
        let mut head = [0; 4];
        file.read_exact_at(&mut head, at).ok()?;
        let len = u64::from(u16::from_be_bytes([head[2], head[3]]));
        // The picture starts at 0xda; anything else out of place isn't a
        // JPEG.
        if head[0] != 0xff || head[1] == 0xda || len < 2 {
            return None;
        }
        let segment = (head[1], at + 4, len - 2);
        at += 2 + len;
        Some(segment)
    })
}

/// A JPEG's size in pixels, from its frame header.
fn jpeg_size(file: &File, offset: u64) -> Option<(u32, u32)> {
    let (_, at, _) = jpeg_segments(file, offset).find(|&(marker, _, _)| matches!(marker, 0xc0..=0xc2))?;
    let mut frame = [0; 5];
    file.read_exact_at(&mut frame, at).ok()?;
    let side = |i: usize| u32::from(u16::from_be_bytes([frame[i], frame[i + 1]]));
    Some((side(3), side(1)))
}

/// Where the Exif of the JPEG at `offset` is: a TIFF, after "Exif" and
/// two zeros.
fn jpeg_exif(file: &File, offset: u64) -> Option<u64> {
    jpeg_segments(file, offset).filter(|&(marker, _, len)| marker == 0xe1 && len > 6).find_map(|(_, at, _)| {
        let mut name = [0; 6];
        file.read_exact_at(&mut name, at).ok()?;
        (&name == b"Exif\0\0").then_some(at + 6)
    })
}

/// A JPEG on its own: the picture is the whole file. Its Exif, if it has
/// one, holds the shooting settings, and a camera's has a thumbnail too.
fn jpeg(file: &File, found: &mut Found) -> io::Result<()> {
    // Exif that can't be read is no reason not to show the picture.
    if let Some(exif) = jpeg_exif(file, 0)
        && let Err(e) = Tiff::at(file, exif).and_then(|tiff| tiff.walk(found))
    {
        log::debug!("unreadable Exif in a JPEG: {e}");
    }
    whole(file, found, jpeg_size(file, 0).ok_or_else(|| invalid("a damaged JPEG"))?)
}

/// A PNG on its own: the picture is the whole file. Its size is in its
/// first chunk, and its Exif, if it has any, in one of its own. That's
/// looked for before the pixels, where it's usually written: they're in
/// thousands of chunks, and after them is the far end of a big file.
fn png(file: &File, found: &mut Found) -> io::Result<()> {
    let (mut at, mut size) = (8, None);
    // A corrupt file could go on for ever.
    for _ in 0..1024 {
        // Each chunk: how long it is, its name, that many bytes and a
        // checksum.
        let mut head = [0; 16];
        if file.read_exact_at(&mut head, at).is_err() {
            break;
        }
        let number = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
        match &head[4..8] {
            b"IHDR" => size = Some((number(&head[8..]), number(&head[12..]))),
            b"eXIf" => {
                // Exif that can't be read is no reason not to show the
                // picture.
                if let Err(e) = Tiff::at(file, at + 8).and_then(|tiff| tiff.walk(found)) {
                    log::debug!("unreadable Exif in a PNG: {e}");
                }
            }
            b"IDAT" | b"IEND" => break,
            _ => {}
        }
        at += 12 + u64::from(number(&head));
    }
    whole(file, found, size.filter(|&(w, h)| w > 0 && h > 0).ok_or_else(|| invalid("a damaged PNG"))?)
}

/// A picture on its own, `width` × `height`: the whole file, whatever
/// size its Exif says the camera's frame was.
fn whole(file: &File, found: &mut Found, (width, height): (u32, u32)) -> io::Result<()> {
    found.jpegs.push(Embedded { offset: 0, len: file.metadata()?.len() });
    found.size = Some((width, height));
    // An export keeps the camera's makernote, but once it's been turned or
    // cropped to another shape the focus point isn't where that says.
    found.focus = found.focus.filter(|f| {
        let (a, b) = (u64::from(width) * u64::from(f.height), u64::from(height) * u64::from(f.width));
        a.abs_diff(b) * 100 <= a.max(b)
    });
    Ok(())
}

/// Fuji's RAF: a header saying where its JPEG is, a whole JPEG file with
/// the shooting settings and a thumbnail in its own Exif, then the raw.
fn raf(file: &File, found: &mut Found) -> io::Result<()> {
    let mut place = [0; 8];
    file.read_exact_at(&mut place, 84)?;
    let number = |b: &[u8]| u64::from(u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let (offset, len) = (number(&place), number(&place[4..]));
    if len == 0 {
        return Err(invalid("no preview in the RAF"));
    }
    found.jpegs.push(Embedded { offset, len });
    if let Some(exif) = jpeg_exif(file, offset) {
        Tiff::at(file, exif)?.walk(found)?;
    }
    Ok(())
}

/// The boxes in a part of a CR3 (as in an MP4), each with its kind, where
/// what it holds starts and where it ends.
fn boxes(file: &File, from: u64, to: u64) -> Vec<([u8; 4], u64, u64)> {
    let mut found = Vec::new();
    let mut at = from;
    // A corrupt file could go on for ever.
    while at + 8 <= to && found.len() < 64 {
        let mut head = [0; 16];
        if file.read_exact_at(&mut head[..8], at).is_err() {
            break;
        }
        let kind = [head[4], head[5], head[6], head[7]];
        let (start, len) = match u32::from_be_bytes([head[0], head[1], head[2], head[3]]) {
            // To the end of what it's in.
            0 => (at + 8, to - at),
            // Too long for four bytes: in the next eight.
            1 if file.read_exact_at(&mut head[8..], at + 8).is_ok() => {
                (at + 16, u64::from_be_bytes(head[8..].try_into().unwrap_or_default()))
            }
            len => (at + 8, u64::from(len)),
        };
        if len < start - at || at + len > to {
            break;
        }
        found.push((kind, start, at + len));
        at += len;
    }
    found
}

/// The JPEG in a box, a few bytes into it.
fn jpeg_in(file: &File, start: u64, end: u64) -> Option<Embedded> {
    let mut head = vec![0; (end - start).min(96) as usize];
    file.read_exact_at(&mut head, start).ok()?;
    let at = start + head.windows(3).position(|w| w == [0xff, 0xd8, 0xff])? as u64;
    Some(Embedded { offset: at, len: end - at })
}

/// Canon's CR3. Under `moov`, Canon's own box holds the shooting settings
/// as small TIFFs (CMT1 the first directory's, CMT2 Exif's) and the
/// thumbnail (THMB); the preview (PRVW) is in a box of its own beside
/// `moov`.
fn cr3(file: &File, found: &mut Found) -> io::Result<()> {
    let len = file.metadata()?.len();
    for (kind, start, end) in boxes(file, 0, len) {
        match &kind {
            b"moov" => {
                // Canon's box is named by 16 bytes before what's in it.
                let canon = boxes(file, start, end).into_iter().filter(|(kind, _, _)| kind == b"uuid");
                for (kind, start, end) in canon.flat_map(|(_, start, end)| boxes(file, start + 16, end)) {
                    match &kind {
                        b"CMT1" | b"CMT2" => Tiff::at(file, start)?.walk(found)?,
                        b"THMB" => found.jpegs.extend(jpeg_in(file, start, end)),
                        _ => {}
                    }
                }
            }
            b"uuid" => found.jpegs.extend(jpeg_in(file, start, end)),
            _ => {}
        }
    }
    if found.jpegs.is_empty() {
        return Err(invalid("no preview in the CR3"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Arw;

    fn open(arw: &Arw) -> io::Result<RawFile> {
        open_bytes(&arw.bytes())
    }

    fn open_bytes(bytes: &[u8]) -> io::Result<RawFile> {
        let path = std::env::temp_dir().join(format!(
            "omacull-raw-{}-{:?}.arw",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&path, bytes)?;
        let raw = RawFile::open(&path);
        std::fs::remove_file(&path)?;
        raw
    }

    #[test]
    fn finds_the_embedded_jpegs_orientation_focus_and_capture_time() {
        let arw = Arw {
            preview: vec![b'P'; 30],
            thumbnail: vec![b'T'; 10],
            orientation: 8,
            focus: [6000, 4000, 3543, 2575],
            ..Arw::default()
        };
        let raw = open(&arw).unwrap();
        assert_eq!(raw.jpegs.len(), 2);
        assert_eq!(raw.read(raw.preview().unwrap()).unwrap(), [b'P'; 30]);
        assert_eq!(raw.read(raw.thumbnail().unwrap()).unwrap(), [b'T'; 10]);
        assert_eq!(raw.orientation, 8);
        assert_eq!(raw.focus, Some(Focus { width: 6000, height: 4000, x: 3543, y: 2575 }));
        assert_eq!(raw.captured.as_deref(), Some("2026:10:04 12:00:00.123"));
    }

    #[test]
    fn finds_the_focus_behind_an_older_bodys_makernote_header() {
        let arw = Arw { focus: [6000, 4000, 1, 2], makernote_header: b"SONY DSC \0\0\0".to_vec(), ..Arw::default() };
        assert_eq!(open(&arw).unwrap().focus, Some(Focus { width: 6000, height: 4000, x: 1, y: 2 }));
    }

    #[test]
    fn no_focus_location_when_the_camera_recorded_zeros() {
        assert_eq!(open(&Arw { focus: [0; 4], ..Arw::default() }).unwrap().focus, None);
    }

    #[test]
    fn capture_time_without_a_fraction_or_a_clock() {
        let whole = Arw { captured: ("2026:10:04 12:00:00", ""), ..Arw::default() };
        assert_eq!(open(&whole).unwrap().captured.as_deref(), Some("2026:10:04 12:00:00"));
        let unset = Arw { captured: ("    :  :     :  :  ", ""), ..Arw::default() };
        assert_eq!(open(&unset).unwrap().captured, None);
    }

    #[test]
    fn reads_the_shooting_settings() {
        let raw = open(&Arw { orientation: 6, ..Arw::default() }).unwrap();
        let exif = Exif {
            model: Some("ILCE-7M3".into()),
            lens: Some("FE 85mm F1.8".into()),
            exposure: Some((1, 250)),
            f_number: Some(2.8),
            iso: Some(400),
            focal_length: Some(85.0),
        };
        assert_eq!(raw.exif, exif);
        assert!(!raw.adobe_rgb);
        assert_eq!(raw.size, Some((6048, 4024)));
        let info = raw.info();
        assert_eq!(info.size, Some((4024, 6048)), "upright");
        assert_eq!(info.summary(), "1/250 s   f/2.8   ISO 400   85 mm   FE 85mm F1.8   2026-10-04 12:00:00");
        assert!(open(&Arw { color_space: 0xffff, ..Arw::default() }).unwrap().adobe_rgb);
    }

    #[test]
    fn exposures_read_as_a_photographer_writes_them() {
        let summary = |exposure, lens| {
            let exif = Exif { exposure: Some(exposure), model: Some("ILCE-7M3".into()), lens, ..Exif::default() };
            let info = Info { exif, ..Info::default() };
            info.summary()
        };
        assert_eq!(summary((1, 4000), None), "1/4000 s   ILCE-7M3");
        assert_eq!(summary((10, 2500), None), "1/250 s   ILCE-7M3");
        assert_eq!(summary((2, 1), Some("Sigma".into())), "2 s   Sigma");
        assert_eq!(summary((13, 10), None), "1.3 s   ILCE-7M3");
        assert_eq!(summary((6, 10), None), "0.6 s   ILCE-7M3");
        assert_eq!(Info::default().summary(), "");
    }

    #[test]
    fn the_focus_point_turns_with_the_frame() {
        // A point near the top-left of the sensor, a quarter across and a
        // tenth down.
        let at = [0.25, 0.1];
        assert_eq!(upright(1, at), [0.25, 0.1]);
        assert_eq!(upright(3, at), [0.75, 0.9]);
        // A quarter turn clockwise takes the top-left to the top-right.
        assert_eq!(upright(6, at), [0.9, 0.25]);
        assert_eq!(upright(8, at), [0.1, 0.75]);
        let raw = open(&Arw { orientation: 8, focus: [6000, 4000, 1500, 400], ..Arw::default() }).unwrap();
        assert_eq!(raw.info().focus, Some([0.1, 0.75]));
    }

    #[test]
    fn manual_focus_has_no_focus_point() {
        assert_eq!(open(&Arw { focus_mode: 0, ..Arw::default() }).unwrap().focus, None);
    }

    #[test]
    fn rejects_files_that_arent_tiff() {
        assert!(open_bytes(b"\xff\xd8\xff\xe1 not a raw, whatever it is").is_err());
        assert!(open_bytes(b"too short").is_err());
    }

    #[test]
    fn the_preview_is_the_smallest_jpeg_that_fills_the_loupe() {
        use crate::testing::{jpeg, nikon_makernote};
        let (full, middling, small) = (jpeg(3000, 2000, [9; 3]), jpeg(1620, 1080, [9; 3]), jpeg(160, 120, [9; 3]));
        // A Nikon's: full size, with a small one in its makernote.
        let nef = Arw {
            make: "NIKON CORPORATION",
            makernote: Some(nikon_makernote(&small)),
            preview: full.clone(),
            thumbnail: Vec::new(),
            ..Arw::default()
        };
        let raw = open(&nef).unwrap();
        assert_eq!(raw.jpegs.len(), 2);
        assert_eq!(raw.read(raw.thumbnail().unwrap()).unwrap(), small, "found in Nikon's makernote");
        assert_eq!(raw.read(raw.preview().unwrap()).unwrap(), full, "all there is to look at");
        assert_eq!((raw.focus, raw.exif.iso), (None, Some(400)), "the focus point is only read from Sony's");
        // With one of a size for the loupe as well, that's the preview.
        let three = Arw { makernote: Some(nikon_makernote(&middling)), thumbnail: small.clone(), ..nef };
        let raw = open(&three).unwrap();
        assert_eq!(raw.jpegs.len(), 3);
        assert_eq!(raw.read(raw.preview().unwrap()).unwrap(), middling);
        assert_eq!(raw.read(raw.thumbnail().unwrap()).unwrap(), small);
    }

    #[test]
    fn reads_a_fuji_raf_through_its_jpegs_exif() {
        use crate::testing::{jpeg, raf, with_exif};
        let thumbnail = jpeg(160, 120, [9; 3]);
        let settings = Arw {
            make: "FUJIFILM",
            model: "X-T3",
            orientation: 6,
            preview: Vec::new(),
            thumbnail: thumbnail.clone(),
            ..Arw::default()
        };
        let preview = with_exif(&jpeg(1920, 1280, [200, 120, 40]), &settings.bytes());
        let raw = open_bytes(&raf(&preview)).unwrap();
        assert_eq!(raw.read(raw.preview().unwrap()).unwrap(), preview);
        assert_eq!(raw.read(raw.thumbnail().unwrap()).unwrap(), thumbnail, "at its place in the Exif, in the file");
        assert_eq!((raw.orientation, raw.exif.model.as_deref()), (6, Some("X-T3")));
        assert_eq!(raw.captured.as_deref(), Some("2026:10:04 12:00:00.123"));
        assert_eq!((raw.exif.exposure, raw.exif.iso, raw.focus), (Some((1, 250)), Some(400), None));
        let decoded = crate::image::Image::decode_jpeg(&raw.read(raw.preview().unwrap()).unwrap()).unwrap();
        assert_eq!((decoded.width, decoded.height), (1920, 1280), "and it's still a JPEG");
        assert!(open_bytes(&raf(&[])).is_err());
    }

    #[test]
    fn reads_a_jpeg_on_its_own_through_its_exif() {
        use crate::testing::{jpeg, with_icc};
        let thumbnail = jpeg(160, 120, [9; 3]);
        let camera = || Arw {
            orientation: 6,
            preview: jpeg(3000, 2000, [200, 120, 40]),
            thumbnail: thumbnail.clone(),
            focus: [6000, 4000, 1500, 400],
            ..Arw::default()
        };
        let file = camera().jpeg();
        let raw = open_bytes(&file).unwrap();
        assert_eq!(raw.developed, Some(Developed::Jpeg));
        assert_eq!(raw.read(raw.preview().unwrap()).unwrap(), file, "the picture is the whole file");
        assert_eq!(raw.read(raw.thumbnail().unwrap()).unwrap(), thumbnail, "at its place in the Exif");
        assert_eq!((raw.orientation, raw.exif.model.as_deref()), (6, Some("ILCE-7M3")));
        assert_eq!(raw.captured.as_deref(), Some("2026:10:04 12:00:00.123"));
        assert_eq!((raw.exif.exposure, raw.exif.iso), (Some((1, 250)), Some(400)));
        // Its size is the picture's, not what the Exif says of the sensor.
        assert_eq!((raw.size, raw.info().size), (Some((3000, 2000)), Some((2000, 3000))));
        assert_eq!(raw.info().focus, Some([0.9, 0.25]), "Sony's focus point, as in its raws");
        assert_eq!(raw.icc(), None);

        // A profile, whole or in parts out of order, as big ones are.
        assert_eq!(open_bytes(&with_icc(&file, b"a profile")).unwrap().icc().unwrap(), b"a profile");
        let part = |n: u8, bytes: &[u8]| [&[0xff, 0xe2, 0, 16 + bytes.len() as u8][..], b"ICC_PROFILE\0", &[n, 2], bytes].concat();
        let parts = [&file[..2], &part(2, b"file"), &part(1, b"pro"), &file[2..]].concat();
        assert_eq!(open_bytes(&parts).unwrap().icc().unwrap(), b"profile");
        assert_eq!(open(&camera()).unwrap().icc(), None, "a raw has none");

        // Exported upright, the camera's focus point no longer fits it.
        let turned = Arw { preview: jpeg(2000, 3000, [9; 3]), ..camera() }.jpeg();
        assert_eq!(open_bytes(&turned).unwrap().focus, None);

        // No Exif at all: still a picture, as it's stored.
        let bare = open_bytes(&jpeg(64, 48, [9; 3])).unwrap();
        assert_eq!((bare.jpegs.len(), bare.orientation, bare.size), (1, 1, Some((64, 48))));
        assert_eq!((bare.captured.as_deref(), bare.info().summary().as_str()), (None, ""));
        assert!(open_bytes(b"\xff\xd8 and nothing more").is_err());
    }

    #[test]
    fn reads_a_png_on_its_own_and_the_exif_it_carries() {
        use crate::testing::{Png, jpeg};
        // As darktable exports them: no Exif at all.
        let file = Png { width: 300, height: 200, ..Png::default() }.bytes();
        let raw = open_bytes(&file).unwrap();
        assert_eq!(raw.developed, Some(Developed::Png));
        assert_eq!(raw.read(raw.preview().unwrap()).unwrap(), file, "the picture is the whole file");
        assert_eq!((raw.orientation, raw.size, raw.captured.as_deref()), (1, Some((300, 200)), None));
        assert_eq!((raw.info().summary().as_str(), raw.focus, raw.icc()), ("", None, None));

        // With Exif, in a chunk of its own, it's read as a JPEG's is.
        let thumbnail = jpeg(160, 120, [9; 3]);
        let exif = Arw {
            orientation: 6,
            preview: Vec::new(),
            thumbnail: thumbnail.clone(),
            focus: [6000, 4000, 1500, 400],
            ..Arw::default()
        };
        let file = Png { width: 300, height: 200, exif: Some(exif.bytes()), ..Png::default() }.bytes();
        let raw = open_bytes(&file).unwrap();
        assert_eq!(raw.read(raw.preview().unwrap()).unwrap(), file, "not the thumbnail in its Exif");
        assert_eq!(raw.read(raw.thumbnail().unwrap()).unwrap(), thumbnail);
        assert_eq!((raw.orientation, raw.exif.iso), (6, Some(400)));
        assert_eq!(raw.captured.as_deref(), Some("2026:10:04 12:00:00.123"));
        // Its size is the picture's, not what the Exif says of the sensor.
        assert_eq!((raw.size, raw.info().size), (Some((300, 200)), Some((200, 300))));
        assert_eq!(raw.info().focus, Some([0.9, 0.25]));

        // Cut short of its first chunk, there's no telling its size.
        assert!(open_bytes(&file[..20]).is_err());
    }

    #[test]
    fn reads_a_canon_cr3_through_its_boxes() {
        use crate::testing::{cr3, jpeg};
        let (thumbnail, preview) = (jpeg(160, 120, [9; 3]), jpeg(1620, 1080, [200, 120, 40]));
        let settings = Arw {
            make: "Canon",
            model: "Canon EOS R6",
            orientation: 8,
            preview: Vec::new(),
            thumbnail: Vec::new(),
            ..Arw::default()
        };
        let raw = open_bytes(&cr3(&settings.bytes(), &thumbnail, &preview)).unwrap();
        assert_eq!(raw.jpegs.len(), 2);
        assert_eq!(raw.read(raw.preview().unwrap()).unwrap(), preview);
        assert_eq!(raw.read(raw.thumbnail().unwrap()).unwrap(), thumbnail);
        assert_eq!((raw.orientation, raw.exif.model.as_deref()), (8, Some("Canon EOS R6")));
        assert_eq!((raw.exif.f_number, raw.exif.lens.as_deref()), (Some(2.8), Some("FE 85mm F1.8")));
        assert_eq!((raw.size, raw.focus), (Some((6048, 4024)), None));
        // Cut short, there's no preview to find.
        let whole = cr3(&settings.bytes(), &thumbnail, &preview);
        assert!(open_bytes(&whole[..60]).is_err());
    }
}
