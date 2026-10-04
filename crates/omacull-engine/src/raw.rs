//! What a culler needs from a raw file, read without decoding the raw: the
//! JPEGs the camera embedded, the orientation and where it focused.
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
            let (mut at, mut len) = (None, None);
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
                    _ => {}
                }
            }
            if let (Some(at), Some(len)) = (at, len)
                && len > 0
            {
                jpegs.push(Embedded { offset: u64::from(at), len: u64::from(len) });
            }
        }
        // Cameras without a clock set write blanks.
        let captured = date.filter(|d| d.starts_with(|c: char| c.is_ascii_digit())).map(|date| match fraction {
            Some(f) if !f.is_empty() => format!("{date}.{f}"),
            _ => date,
        });
        Ok(Self { file, jpegs, orientation: orientation.unwrap_or(1), focus, captured })
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
    fn rejects_files_that_arent_tiff() {
        assert!(open_bytes(b"\xff\xd8\xff\xe1 not a raw").is_err());
    }
}
