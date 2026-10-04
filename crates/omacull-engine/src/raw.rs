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
}

const MAKE: u16 = 0x010f;
const JPEG_OFFSET: u16 = 0x0201;
const JPEG_LENGTH: u16 = 0x0202;
const ORIENTATION: u16 = 0x0112;
const SUB_IFDS: u16 = 0x014a;
const EXIF_IFD: u16 = 0x8769;
const MAKER_NOTE: u16 = 0x927c;
const SONY_FOCUS_LOCATION: u16 = 0x2027;

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
                    _ => {}
                }
            }
            if let (Some(at), Some(len)) = (at, len)
                && len > 0
            {
                jpegs.push(Embedded { offset: u64::from(at), len: u64::from(len) });
            }
        }
        Ok(Self { file, jpegs, orientation: orientation.unwrap_or(1), focus })
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

    /// A little-endian TIFF shaped like an ARW: IFD0 with a preview, an
    /// orientation and an Exif directory holding a Sony makernote, then
    /// IFD1 with a thumbnail.
    fn arw(focus: [u16; 4]) -> Vec<u8> {
        arw_with(focus, b"")
    }

    fn arw_with(focus: [u16; 4], makernote_header: &[u8]) -> Vec<u8> {
        fn entry(out: &mut Vec<u8>, tag: u16, kind: u16, count: u32, value: u32) {
            out.extend(tag.to_le_bytes());
            out.extend(kind.to_le_bytes());
            out.extend(count.to_le_bytes());
            out.extend(value.to_le_bytes());
        }
        let mut f = b"II\x2a\0\x08\0\0\0".to_vec();
        // IFD0 at 8: five entries, then IFD1 at 74.
        f.extend(5u16.to_le_bytes());
        entry(&mut f, MAKE, 2, 5, 190);
        entry(&mut f, JPEG_OFFSET, 4, 1, 200);
        entry(&mut f, JPEG_LENGTH, 4, 1, 30);
        entry(&mut f, ORIENTATION, 3, 1, 8);
        entry(&mut f, EXIF_IFD, 4, 1, 104);
        f.extend(74u32.to_le_bytes());
        // IFD1 at 74: the thumbnail.
        assert_eq!(f.len(), 74);
        f.extend(2u16.to_le_bytes());
        entry(&mut f, JPEG_OFFSET, 4, 1, 230);
        entry(&mut f, JPEG_LENGTH, 4, 1, 10);
        f.extend(0u32.to_le_bytes());
        // Exif directory at 104: the makernote at 122.
        assert_eq!(f.len(), 104);
        f.extend(1u16.to_le_bytes());
        entry(&mut f, MAKER_NOTE, 7, 100, 122);
        f.extend(0u32.to_le_bytes());
        // Makernote at 122: a directory with FocusLocation at 180.
        assert_eq!(f.len(), 122);
        f.extend(makernote_header);
        f.extend(1u16.to_le_bytes());
        entry(&mut f, SONY_FOCUS_LOCATION, 3, 4, 180);
        f.extend(0u32.to_le_bytes());
        f.resize(180, 0);
        for v in focus {
            f.extend(v.to_le_bytes());
        }
        f.resize(190, 0);
        f.extend(b"SONY\0");
        f.resize(200, 0);
        f.extend([b'P'; 30]);
        f.extend([b'T'; 10]);
        f
    }

    fn open(bytes: &[u8]) -> io::Result<RawFile> {
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
    fn finds_the_embedded_jpegs_orientation_and_focus() {
        let raw = open(&arw([6000, 4000, 3543, 2575])).unwrap();
        assert_eq!(raw.jpegs, [Embedded { offset: 200, len: 30 }, Embedded { offset: 230, len: 10 }]);
        assert_eq!(raw.read(raw.preview().unwrap()).unwrap(), [b'P'; 30]);
        assert_eq!(raw.read(raw.thumbnail().unwrap()).unwrap(), [b'T'; 10]);
        assert_eq!(raw.orientation, 8);
        assert_eq!(raw.focus, Some(Focus { width: 6000, height: 4000, x: 3543, y: 2575 }));
    }

    #[test]
    fn finds_the_focus_behind_an_older_bodys_makernote_header() {
        let raw = open(&arw_with([6000, 4000, 1, 2], b"SONY DSC \0\0\0")).unwrap();
        assert_eq!(raw.focus, Some(Focus { width: 6000, height: 4000, x: 1, y: 2 }));
    }

    #[test]
    fn no_focus_location_when_the_camera_recorded_zeros() {
        assert_eq!(open(&arw([0; 4])).unwrap().focus, None);
    }

    #[test]
    fn rejects_files_that_arent_tiff() {
        assert!(open(b"\xff\xd8\xff\xe1 not a raw").is_err());
    }
}
