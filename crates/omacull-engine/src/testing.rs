//! Raws for tests: small TIFFs shaped like a Sony ARW, with whatever
//! embedded JPEGs, orientation, focus and capture time a test needs.

use std::fs;
use std::path::{Path, PathBuf};

use crate::raw::{
    DATE_TIME_ORIGINAL, EXIF_IFD, JPEG_LENGTH, JPEG_OFFSET, MAKE, MAKER_NOTE, ORIENTATION, SONY_FOCUS_LOCATION,
    SUB_SEC_TIME_ORIGINAL,
};

pub struct Arw {
    /// IFD0's JPEG, the preview.
    pub preview: Vec<u8>,
    /// IFD1's JPEG, the thumbnail.
    pub thumbnail: Vec<u8>,
    pub orientation: u16,
    /// Frame width, frame height, x, y; zeros for none.
    pub focus: [u16; 4],
    /// DateTimeOriginal and SubSecTimeOriginal (at most three characters).
    pub captured: (&'static str, &'static str),
    /// What older bodies put before the makernote's directory.
    pub makernote_header: Vec<u8>,
}

impl Default for Arw {
    fn default() -> Self {
        Self {
            preview: jpeg(48, 32, [200, 120, 40]),
            thumbnail: jpeg(16, 12, [200, 120, 40]),
            orientation: 1,
            focus: [6000, 4000, 3000, 2000],
            captured: ("2026:10:04 12:00:00", "123"),
            makernote_header: Vec::new(),
        }
    }
}

impl Arw {
    /// The file's bytes: IFD0 with the preview, the make, the orientation
    /// and an Exif directory holding the capture time and a Sony makernote,
    /// then IFD1 with the thumbnail.
    pub fn bytes(&self) -> Vec<u8> {
        fn entry(out: &mut Vec<u8>, tag: u16, kind: u16, count: u32, value: u32) {
            out.extend(tag.to_le_bytes());
            out.extend(kind.to_le_bytes());
            out.extend(count.to_le_bytes());
            out.extend(value.to_le_bytes());
        }
        let len = |b: &[u8]| b.len() as u32;
        // Directories are 2 + 12 a entry + 4 bytes; the data follows them.
        let (ifd0, ifd1, exif): (u32, u32, u32) = (8, 8 + 66, 8 + 66 + 30);
        let makernote = exif + 42;
        let focus = makernote + len(&self.makernote_header) + 18;
        let make = focus + 8;
        let date = make + 8;
        let preview = date + 20;
        let thumbnail = preview + len(&self.preview);

        let mut f = b"II\x2a\0".to_vec();
        f.extend(ifd0.to_le_bytes());
        f.extend(5u16.to_le_bytes());
        entry(&mut f, MAKE, 2, 5, make);
        entry(&mut f, JPEG_OFFSET, 4, 1, preview);
        entry(&mut f, JPEG_LENGTH, 4, 1, len(&self.preview));
        entry(&mut f, ORIENTATION, 3, 1, u32::from(self.orientation));
        entry(&mut f, EXIF_IFD, 4, 1, exif);
        f.extend(ifd1.to_le_bytes());

        assert_eq!(f.len() as u32, ifd1);
        f.extend(2u16.to_le_bytes());
        entry(&mut f, JPEG_OFFSET, 4, 1, thumbnail);
        entry(&mut f, JPEG_LENGTH, 4, 1, len(&self.thumbnail));
        f.extend(0u32.to_le_bytes());

        assert_eq!(f.len() as u32, exif);
        f.extend(3u16.to_le_bytes());
        entry(&mut f, MAKER_NOTE, 7, 100, makernote);
        entry(&mut f, DATE_TIME_ORIGINAL, 2, 20, date);
        let mut fraction = [0; 4];
        fraction[..self.captured.1.len()].copy_from_slice(self.captured.1.as_bytes());
        entry(&mut f, SUB_SEC_TIME_ORIGINAL, 2, len(self.captured.1.as_bytes()) + 1, u32::from_le_bytes(fraction));
        f.extend(0u32.to_le_bytes());

        assert_eq!(f.len() as u32, makernote);
        f.extend(&self.makernote_header);
        f.extend(1u16.to_le_bytes());
        entry(&mut f, SONY_FOCUS_LOCATION, 3, 4, focus);
        f.extend(0u32.to_le_bytes());

        assert_eq!(f.len() as u32, focus);
        for v in self.focus {
            f.extend(v.to_le_bytes());
        }
        f.extend(b"SONY\0\0\0\0");
        let mut stamp = [0; 20];
        stamp[..self.captured.0.len()].copy_from_slice(self.captured.0.as_bytes());
        f.extend(stamp);
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
