//! Raws for tests: small TIFFs shaped like a Sony ARW, with whatever
//! embedded JPEGs, orientation, focus and shooting settings a test needs.

use std::fs;
use std::path::{Path, PathBuf};

use crate::raw::*;

pub struct Arw {
    /// IFD0's JPEG, the preview.
    pub preview: Vec<u8>,
    /// IFD1's JPEG, the thumbnail.
    pub thumbnail: Vec<u8>,
    pub orientation: u16,
    /// Frame width, frame height, x, y; zeros for none.
    pub focus: [u16; 4],
    /// Sony's focus mode: 0 is manual.
    pub focus_mode: u8,
    /// DateTimeOriginal and SubSecTimeOriginal.
    pub captured: (&'static str, &'static str),
    /// What older bodies put before the makernote's directory.
    pub makernote_header: Vec<u8>,
    pub model: &'static str,
    pub lens: &'static str,
    pub exposure: (u32, u32),
    pub f_number: (u32, u32),
    pub iso: u16,
    pub focal_length: (u32, u32),
    /// Exif's ColorSpace: 1 sRGB, 0xffff Adobe RGB.
    pub color_space: u16,
    /// The raw data's size, as stored.
    pub size: (u32, u32),
}

impl Default for Arw {
    fn default() -> Self {
        Self {
            preview: jpeg(48, 32, [200, 120, 40]),
            thumbnail: jpeg(16, 12, [200, 120, 40]),
            orientation: 1,
            focus: [6000, 4000, 3000, 2000],
            focus_mode: 2,
            captured: ("2026:10:04 12:00:00", "123"),
            makernote_header: Vec::new(),
            model: "ILCE-7M3",
            lens: "FE 85mm F1.8",
            exposure: (1, 250),
            f_number: (28, 10),
            iso: 400,
            focal_length: (850, 10),
            color_space: 1,
            size: (6048, 4024),
        }
    }
}

/// A value in a directory under construction.
enum Value {
    /// Four bytes or fewer, stored in the entry.
    Inline([u8; 4]),
    /// Stored after the directories.
    Bytes(Vec<u8>),
    /// The offset of another directory, by its index.
    Directory(usize),
    /// The offset of the preview or the thumbnail.
    Preview,
    Thumbnail,
}

fn short(v: u16) -> Value {
    let [a, b] = v.to_le_bytes();
    Value::Inline([a, b, 0, 0])
}

fn long(v: u32) -> Value {
    Value::Inline(v.to_le_bytes())
}

type Entry = (u16, u16, u32, Value);

fn rational(tag: u16, (n, d): (u32, u32)) -> Entry {
    (tag, 5, 1, Value::Bytes([n.to_le_bytes(), d.to_le_bytes()].concat()))
}

fn ascii(tag: u16, text: &str) -> Entry {
    let mut bytes = text.as_bytes().to_vec();
    bytes.push(0);
    let count = bytes.len() as u32;
    if bytes.len() <= 4 {
        bytes.resize(4, 0);
        (tag, 2, count, Value::Inline(bytes.try_into().unwrap()))
    } else {
        (tag, 2, count, Value::Bytes(bytes))
    }
}

impl Arw {
    /// The file's bytes: IFD0 with the preview, the make, the orientation
    /// and an Exif directory holding the shooting settings, an interop
    /// directory and a Sony makernote; then IFD1 with the thumbnail.
    pub fn bytes(&self) -> Vec<u8> {
        // Directories by index: 0 IFD0, 1 IFD1, 2 Exif, 3 interop, 4 makernote.
        let mut stamp = self.captured.0.as_bytes().to_vec();
        stamp.resize(20, 0);
        let focus = self.focus.iter().flat_map(|v| v.to_le_bytes()).collect();
        let directories: Vec<Vec<Entry>> = vec![
            vec![
                (IMAGE_WIDTH, 4, 1, long(self.size.0)),
                (IMAGE_LENGTH, 4, 1, long(self.size.1)),
                ascii(MAKE, "SONY"),
                ascii(MODEL, self.model),
                (ORIENTATION, 3, 1, short(self.orientation)),
                (JPEG_OFFSET, 4, 1, Value::Preview),
                (JPEG_LENGTH, 4, 1, long(self.preview.len() as u32)),
                (EXIF_IFD, 4, 1, Value::Directory(2)),
            ],
            vec![
                (JPEG_OFFSET, 4, 1, Value::Thumbnail),
                (JPEG_LENGTH, 4, 1, long(self.thumbnail.len() as u32)),
            ],
            vec![
                rational(EXPOSURE_TIME, self.exposure),
                rational(F_NUMBER, self.f_number),
                (ISO, 3, 1, short(self.iso)),
                (DATE_TIME_ORIGINAL, 2, 20, Value::Bytes(stamp)),
                ascii(SUB_SEC_TIME_ORIGINAL, self.captured.1),
                rational(FOCAL_LENGTH, self.focal_length),
                (MAKER_NOTE, 7, 100, Value::Directory(4)),
                (COLOR_SPACE, 3, 1, short(self.color_space)),
                (INTEROP_IFD, 4, 1, Value::Directory(3)),
                ascii(LENS_MODEL, self.lens),
            ],
            vec![ascii(INTEROP_INDEX, if self.color_space == 1 { "R98" } else { "R03" })],
            vec![
                (SONY_FOCUS_MODE, 1, 1, Value::Inline([self.focus_mode, 0, 0, 0])),
                (SONY_FOCUS_LOCATION, 3, 4, Value::Bytes(focus)),
            ],
        ];

        // Lay out the directories, then their data, then the JPEGs. The
        // makernote's directory comes after its header.
        let mut at = 8;
        let mut offsets = Vec::new();
        for (i, entries) in directories.iter().enumerate() {
            if i == 4 {
                at += self.makernote_header.len();
            }
            offsets.push(at as u32);
            at += 2 + 12 * entries.len() + 4;
        }
        let mut data: Vec<u8> = Vec::new();
        let mut f = b"II\x2a\0".to_vec();
        f.extend(offsets[0].to_le_bytes());
        let data_start = at;
        let bytes = |e: &Entry| if let Value::Bytes(b) = &e.3 { b.len() } else { 0 };
        let data_len: usize = directories.iter().flatten().map(bytes).sum();
        let preview = (data_start + data_len) as u32;
        let thumbnail = preview + self.preview.len() as u32;
        for (i, entries) in directories.iter().enumerate() {
            if i == 4 {
                f.extend(&self.makernote_header);
            }
            assert_eq!(f.len() as u32, offsets[i]);
            f.extend((entries.len() as u16).to_le_bytes());
            for (tag, kind, count, value) in entries {
                f.extend(tag.to_le_bytes());
                f.extend(kind.to_le_bytes());
                f.extend(count.to_le_bytes());
                let value = match value {
                    Value::Inline(bytes) => *bytes,
                    Value::Bytes(bytes) => {
                        let offset = (data_start + data.len()) as u32;
                        data.extend(bytes);
                        offset.to_le_bytes()
                    }
                    Value::Directory(d) => offsets[*d].to_le_bytes(),
                    Value::Preview => preview.to_le_bytes(),
                    Value::Thumbnail => thumbnail.to_le_bytes(),
                };
                f.extend(value);
            }
            let next = if i == 0 { offsets[1] } else { 0 };
            f.extend(next.to_le_bytes());
        }
        f.extend(data);
        assert_eq!(f.len() as u32, preview);
        f.extend(&self.preview);
        f.extend(&self.thumbnail);
        f
    }

    pub fn write(&self, path: &Path) {
        fs::write(path, self.bytes()).unwrap();
    }
}

/// A JPEG of one colour.
pub fn jpeg(width: u16, height: u16, rgb: [u8; 3]) -> Vec<u8> {
    let pixels: Vec<u8> = (0..usize::from(width) * usize::from(height)).flat_map(|_| rgb).collect();
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, 90)
        .encode(&pixels, width, height, jpeg_encoder::ColorType::Rgb)
        .unwrap();
    out
}

/// A folder of its own for a test, removed when dropped.
pub struct Folder(pub PathBuf);

impl Folder {
    pub fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("omacull-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    /// `count` raws named `DSC00001.ARW` on, all made from `arw`.
    pub fn with_raws(name: &str, count: usize, arw: &Arw) -> Self {
        let folder = Self::new(name);
        let bytes = arw.bytes();
        for i in 1..=count {
            fs::write(folder.0.join(format!("DSC{i:05}.ARW")), &bytes).unwrap();
        }
        folder
    }

    pub fn raw(&self, i: usize) -> PathBuf {
        self.0.join(format!("DSC{i:05}.ARW"))
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
