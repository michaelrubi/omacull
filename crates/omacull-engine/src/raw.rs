//! What a culler needs from a raw file, read without decoding the raw: the
//! JPEGs the camera embedded, the orientation, where it focused and the
//! shooting settings.
//!
//! An ARW is a TIFF. Only the directories are read, a few hundred bytes
//! each, so opening a 50 MB raw costs a handful of small reads.

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

pub struct RawFile {
    file: File,
    /// Every embedded JPEG, in the order the file lists them.
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

struct Tiff<'a> {
    file: &'a File,
    big_endian: bool,
}

impl Tiff<'_> {
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
        self.file.read_exact_at(&mut count, offset)?;
        let count = usize::from(self.u16(&count));
        let mut bytes = vec![0; count * 12 + 4];
        self.file.read_exact_at(&mut bytes, offset + 2)?;
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
            self.file.read_exact_at(&mut bytes, u64::from(self.u32(&e.value)))?;
        }
        Ok(String::from_utf8_lossy(&bytes).trim_end_matches('\0').trim().to_owned())
    }

    /// An entry's value as a fraction (a RATIONAL).
    fn rational(&self, e: &Entry) -> io::Result<(u32, u32)> {
        let mut bytes = [0; 8];
        self.file.read_exact_at(&mut bytes, u64::from(self.u32(&e.value)))?;
        Ok((self.u32(&bytes), self.u32(&bytes[4..])))
    }

    /// An entry's value as a list of offsets (LONGs).
    fn offsets(&self, e: &Entry) -> io::Result<Vec<u64>> {
        if e.count <= 1 {
            return Ok(vec![u64::from(self.u32(&e.value))]);
        }
        // No raw has more than a few sub-directories.
        let mut bytes = vec![0; e.count.min(16) as usize * 4];
        self.file.read_exact_at(&mut bytes, u64::from(self.u32(&e.value)))?;
        Ok(bytes.as_chunks::<4>().0.iter().map(|b| u64::from(self.u32(b))).collect())
    }
}

impl RawFile {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let mut header = [0; 8];
        file.read_exact_at(&mut header, 0)?;
        let big_endian = match &header[..2] {
            b"II" => false,
            b"MM" => true,
            _ => return Err(invalid("not a TIFF-based raw")),
        };
        let tiff = Tiff { file: &file, big_endian };
        if tiff.u16(&header[2..]) != 42 {
            return Err(invalid("not a TIFF-based raw"));
        }

        let (mut jpegs, mut orientation, mut focus) = (Vec::new(), None, None);
        let (mut date, mut fraction) = (None, None);
        let (mut exif, mut color_space, mut interop) = (Exif::default(), None, None);
        let mut size: Option<(u32, u32)> = None;
        let mut sony = false;
        // The main chain of directories, and the sub-directories hanging off
        // them. A corrupt file could chain in a loop, so stop after a few.
        let mut pending = vec![u64::from(tiff.u32(&header[4..]))];
        let mut seen = 0;
        while let Some(offset) = pending.pop() {
            seen += 1;
            if offset == 0 || seen > 32 {
                continue;
            }
            let (entries, next) = tiff.directory(offset)?;
            // Sub-directories and the next in the chain go on a stack, so
            // push the next first to visit the sub-directories before it.
            pending.push(next);
            let (mut at, mut len, mut width, mut height) = (None, None, None, None);
            let ratio = |e: &Entry| tiff.rational(e).ok().filter(|&(_, d)| d > 0).map(|(n, d)| n as f32 / d as f32);
            for e in &entries {
                match e.tag {
                    JPEG_OFFSET => at = Some(tiff.number(e)),
                    JPEG_LENGTH => len = Some(tiff.number(e)),
                    ORIENTATION if orientation.is_none() => orientation = Some(tiff.number(e) as u16),
                    SUB_IFDS => pending.extend(tiff.offsets(e)?),
                    EXIF_IFD => pending.push(u64::from(tiff.u32(&e.value))),
                    MAKE => {
                        let mut make = [0; 4];
                        sony = file.read_exact_at(&mut make, u64::from(tiff.u32(&e.value))).is_ok() && &make == b"SONY";
                    }
                    MAKER_NOTE if sony => focus = sony_focus(&tiff, u64::from(tiff.u32(&e.value))),
                    DATE_TIME_ORIGINAL if date.is_none() => date = tiff.text(e).ok(),
                    SUB_SEC_TIME_ORIGINAL if fraction.is_none() => fraction = tiff.text(e).ok(),
                    MODEL if exif.model.is_none() => exif.model = tiff.text(e).ok().filter(|m| !m.is_empty()),
                    LENS_MODEL => exif.lens = tiff.text(e).ok().filter(|l| !l.is_empty()),
                    EXPOSURE_TIME => exif.exposure = tiff.rational(e).ok(),
                    F_NUMBER => exif.f_number = ratio(e),
                    FOCAL_LENGTH => exif.focal_length = ratio(e),
                    ISO => exif.iso = Some(tiff.number(e)),
                    COLOR_SPACE => color_space = Some(tiff.number(e)),
                    INTEROP_IFD => pending.push(u64::from(tiff.u32(&e.value))),
                    INTEROP_INDEX if e.kind == 2 => interop = tiff.text(e).ok(),
                    IMAGE_WIDTH => width = Some(tiff.number(e)),
                    IMAGE_LENGTH => height = Some(tiff.number(e)),
                    _ => {}
                }
            }
            if let (Some(at), Some(len)) = (at, len)
                && len > 0
            {
                jpegs.push(Embedded { offset: u64::from(at), len: u64::from(len) });
            }
            if let (Some(w), Some(h)) = (width, height)
                && size.is_none_or(|(sw, sh)| u64::from(w) * u64::from(h) > u64::from(sw) * u64::from(sh))
            {
                size = Some((w, h));
            }
        }
        // Adobe RGB is "uncalibrated" in Exif, with R03 for its interop index.
        let adobe_rgb = color_space == Some(0xffff) && interop.as_deref() != Some("R98");
        // Cameras without a clock set write blanks.
        let captured = date.filter(|d| d.starts_with(|c: char| c.is_ascii_digit())).map(|date| match fraction {
            Some(f) if !f.is_empty() => format!("{date}.{f}"),
            _ => date,
        });
        let orientation = orientation.unwrap_or(1);
        Ok(Self { file, jpegs, orientation, focus, captured, exif, adobe_rgb, size })
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

    /// The biggest embedded JPEG: the one to show.
    pub fn preview(&self) -> Option<Embedded> {
        self.jpegs.iter().copied().max_by_key(|j| j.len)
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
    tiff.file.read_exact_at(&mut header, offset).ok()?;
    let skip = if header.starts_with(b"SONY") { 12 } else { 0 };
    let (entries, _) = tiff.directory(offset + skip).ok()?;
    // Manual focus records the middle of the frame, which means nothing.
    if entries.iter().any(|e| e.tag == SONY_FOCUS_MODE && e.kind == 1 && e.value[0] == 0) {
        return None;
    }
    let e = entries.iter().find(|e| e.tag == SONY_FOCUS_LOCATION && e.kind == 3 && e.count == 4)?;
    let mut bytes = [0; 8];
    tiff.file.read_exact_at(&mut bytes, u64::from(tiff.u32(&e.value))).ok()?;
    let [width, height, x, y] = [0, 2, 4, 6].map(|i| tiff.u16(&bytes[i..]));
    (width > 0 && height > 0).then_some(Focus { width, height, x, y })
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
        assert!(open_bytes(b"\xff\xd8\xff\xe1 not a raw").is_err());
    }
}
