//! Raws for tests: small TIFFs shaped like a Sony ARW, with whatever
//! embedded JPEGs, orientation, focus and shooting settings a test needs;
//! the same wrapped up as Nikon, Canon and Fuji wrap theirs; and as a JPEG
//! on its own.

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
    /// A makernote of another make's, whole, in place of Sony's.
    pub makernote: Option<Vec<u8>>,
    pub make: &'static str,
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
            makernote: None,
            make: "SONY",
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
        let makernote = match &self.makernote {
            Some(note) => (MAKER_NOTE, 7, note.len() as u32, Value::Bytes(note.clone())),
            None => (MAKER_NOTE, 7, 100, Value::Directory(4)),
        };
        let directories: Vec<Vec<Entry>> = vec![
            vec![
                (IMAGE_WIDTH, 4, 1, long(self.size.0)),
                (IMAGE_LENGTH, 4, 1, long(self.size.1)),
                ascii(MAKE, self.make),
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
                makernote,
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

    /// The same frame as a JPEG on its own, as a camera writes one beside
    /// the raw: the preview is the picture, and the rest is its Exif.
    pub fn jpeg(mut self) -> Vec<u8> {
        let picture = std::mem::take(&mut self.preview);
        with_exif(&picture, &self.bytes())
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

/// A Nikon makernote: a TIFF of its own behind a header, whose preview
/// directory holds `jpeg`.
pub fn nikon_makernote(jpeg: &[u8]) -> Vec<u8> {
    let entry = |tag: u16, value: u32| [&tag.to_le_bytes()[..], &4u16.to_le_bytes(), &1u32.to_le_bytes(), &value.to_le_bytes()].concat();
    // The first directory at 8 points at the preview's at 26, which says
    // the JPEG is at 56 and how long it is.
    let tiff = [
        &b"II\x2a\0\x08\0\0\0"[..],
        &1u16.to_le_bytes(),
        &entry(NIKON_PREVIEW_IFD, 26),
        &[0; 4],
        &2u16.to_le_bytes(),
        &entry(JPEG_OFFSET, 56),
        &entry(JPEG_LENGTH, jpeg.len() as u32),
        &[0; 4],
        jpeg,
    ];
    [&b"Nikon\0\x02\x10\0\0"[..], &tiff.concat()].concat()
}

/// A JPEG with a TIFF for its Exif, as cameras write them.
pub fn with_exif(jpeg: &[u8], tiff: &[u8]) -> Vec<u8> {
    let len = (tiff.len() + 8) as u16;
    [&jpeg[..2], &[0xff, 0xe1], &len.to_be_bytes(), b"Exif\0\0", tiff, &jpeg[2..]].concat()
}

/// A JPEG carrying a colour profile, in one segment.
pub fn with_icc(jpeg: &[u8], icc: &[u8]) -> Vec<u8> {
    let len = (icc.len() + 16) as u16;
    [&jpeg[..2], &[0xff, 0xe2], &len.to_be_bytes(), b"ICC_PROFILE\0\x01\x01", icc, &jpeg[2..]].concat()
}

/// A Fuji RAF holding `jpeg`, a whole JPEG file with its own Exif.
pub fn raf(jpeg: &[u8]) -> Vec<u8> {
    let mut file = b"FUJIFILMCCD-RAW 0201FF129502X-T3".to_vec();
    file.resize(84, 0);
    file.extend(108u32.to_be_bytes());
    file.extend((jpeg.len() as u32).to_be_bytes());
    file.resize(108, 0);
    file.extend(jpeg);
    // Where the raw data would be.
    file.extend([0; 64]);
    file
}

/// A Canon CR3: `settings` is a TIFF with the shooting settings, as its
/// CMT boxes hold them.
pub fn cr3(settings: &[u8], thumbnail: &[u8], preview: &[u8]) -> Vec<u8> {
    let boxed = |kind: &[u8; 4], content: &[u8]| [&((content.len() + 8) as u32).to_be_bytes()[..], kind, content].concat();
    let size = |jpeg: &[u8]| (jpeg.len() as u32).to_be_bytes();
    let thmb = [&[0; 4][..], &160u16.to_be_bytes(), &120u16.to_be_bytes(), &size(thumbnail), &[0, 1, 0, 0], thumbnail];
    let canon = [
        &[0x85; 16][..],
        &boxed(b"CNCV", b"CanonCR3_001/00.09.00/00.00.00"),
        &boxed(b"CMT1", settings),
        &boxed(b"THMB", &thmb.concat()),
    ];
    let prvw = [&[0, 0, 0, 0, 0, 1][..], &1620u16.to_be_bytes(), &1080u16.to_be_bytes(), &[0, 1], &size(preview), preview];
    [
        boxed(b"ftyp", b"crx \0\0\0\x01crx isom"),
        boxed(b"moov", &boxed(b"uuid", &canon.concat())),
        // The XMP packet, then the preview, each in a box named by 16 bytes.
        boxed(b"uuid", &[&[0xbe; 16][..], b"<?xpacket begin='' id='W5M0MpCehiHzreSzNTczkc9d'?>"].concat()),
        boxed(b"uuid", &[&[0xea; 16][..], &[0, 0, 0, 0, 0, 0, 0, 1], &boxed(b"PRVW", &prvw.concat())].concat()),
        boxed(b"mdat", &[0; 64]),
    ]
    .concat()
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
