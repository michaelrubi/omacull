//! Filmstrip thumbnails, cached on disk (`~/.cache/omacull/thumbnails/`).
//!
//! The camera's own thumbnail is 160×120 with black bars, too small and the
//! wrong shape for a filmstrip on a HiDPI screen, so thumbnails are made by
//! shrinking the preview instead. That costs a preview decode the first
//! time a folder is opened, so they're kept: one small JPEG a raw, named
//! for the raw's path, size and modification time, so a changed or moved
//! raw gets a new one. The cache is disposable. Thumbnails are kept in
//! sRGB, whatever the camera's JPEGs were in.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::UNIX_EPOCH;

use crate::color::{Display, Space};
use crate::image::{self, Image};

/// The long edge of a thumbnail, in pixels: sharp on a HiDPI filmstrip.
pub const LONG_EDGE: usize = 320;
const QUALITY: u8 = 85;
/// Bumped when thumbnails are made differently, so old ones aren't used.
const VERSION: u64 = 2;

/// `$XDG_CACHE_HOME/omacull`, or `~/.cache/omacull`.
pub fn cache_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(dir.join("omacull"))
}

/// `$XDG_CACHE_HOME/omacull/thumbnails`, or `~/.cache/omacull/thumbnails`.
pub fn default_dir() -> Option<PathBuf> {
    Some(cache_dir()?.join("thumbnails"))
}

/// FNV-1a: stable between builds, unlike std's hasher.
pub(crate) fn fnv(bytes: &[u8], mut hash: u64) -> u64 {
    for &b in bytes {
        hash = (hash ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
    }
    hash
}

/// Where a raw's thumbnail is cached in `dir`.
/// A name for what's cached of a raw: changes with its path, size and
/// modification time, and with `version`, how it was made.
pub(crate) fn key(raw: &Path, version: u64) -> io::Result<String> {
    let raw = fs::canonicalize(raw)?;
    let meta = fs::metadata(&raw)?;
    let modified = meta.modified()?.duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let mut hash = fnv(raw.as_os_str().as_encoded_bytes(), 0xcbf2_9ce4_8422_2325);
    for part in [meta.len(), modified as u64, (modified >> 64) as u64, version] {
        hash = fnv(&part.to_le_bytes(), hash);
    }
    Ok(format!("{hash:016x}"))
}

fn cached(dir: &Path, raw: &Path) -> io::Result<PathBuf> {
    Ok(dir.join(format!("{}.jpg", key(raw, VERSION)?)))
}

fn make(raw: &Path) -> io::Result<Image> {
    static SRGB: LazyLock<Display> = LazyLock::new(Display::srgb);
    let (preview, space, _) = image::preview_with_info(raw)?;
    let mut thumbnail = preview.shrunk(LONG_EDGE);
    if space == Space::AdobeRgb {
        SRGB.convert(&mut thumbnail, space);
    }
    Ok(thumbnail)
}

/// Keep a thumbnail, replacing the file in one step so a reader never sees
/// half of it.
fn store(path: &Path, thumbnail: &Image) -> io::Result<()> {
    static UNIQUE: AtomicUsize = AtomicUsize::new(0);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{}-{}.tmp", std::process::id(), UNIQUE.fetch_add(1, Ordering::Relaxed)));
    let temporary = PathBuf::from(name);
    let written = fs::write(&temporary, thumbnail.encode_jpeg(QUALITY)?).and_then(|()| fs::rename(&temporary, path));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

/// A raw's thumbnail, in sRGB: from the cache in `dir` if it's there, else
/// made and cached. With no cache it's made every time.
pub fn load(raw: &Path, dir: Option<&Path>) -> io::Result<Image> {
    let Some(dir) = dir else { return make(raw) };
    let path = cached(dir, raw)?;
    if let Ok(jpeg) = fs::read(&path)
        && let Ok(thumbnail) = Image::decode_jpeg(&jpeg)
    {
        return Ok(thumbnail);
    }
    let thumbnail = make(raw)?;
    if let Err(e) = store(&path, &thumbnail) {
        log::warn!("couldn't cache a thumbnail in {}: {e}", dir.display());
    }
    Ok(thumbnail)
}

/// Make sure a raw's thumbnail is in the cache, without decoding it if it
/// already is.
pub fn warm(raw: &Path, dir: &Path) -> io::Result<()> {
    let path = cached(dir, raw)?;
    if path.exists() {
        return Ok(());
    }
    store(&path, &make(raw)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Arw, Folder};

    #[test]
    fn thumbnails_are_cached_and_remade_when_the_raw_changes() {
        let folder = Folder::new("thumbs");
        let cache = folder.0.join("cache");
        let raw = folder.0.join("DSC00001.ARW");
        Arw { preview: crate::testing::jpeg(640, 480, [10, 200, 10]), orientation: 8, ..Arw::default() }.write(&raw);

        let first = load(&raw, Some(&cache)).unwrap();
        assert_eq!((first.width, first.height), (240, 320), "shrunk and upright");
        let entries = || fs::read_dir(&cache).unwrap().map(|e| e.unwrap().path()).collect::<Vec<_>>();
        assert_eq!(entries().len(), 1);
        assert!(entries()[0].extension().is_some_and(|e| e == "jpg"));

        // Served from the cache: the raw's preview isn't read again.
        let size = fs::metadata(&raw).unwrap();
        fs::write(&entries()[0], Image { width: 2, height: 2, rgba: vec![255; 16] }.encode_jpeg(90).unwrap()).unwrap();
        let file = fs::File::options().write(true).open(&raw).unwrap();
        file.set_modified(size.modified().unwrap()).unwrap();
        assert_eq!(load(&raw, Some(&cache)).unwrap().width, 2);

        // A different raw at the same path is a different thumbnail.
        Arw { preview: crate::testing::jpeg(64, 48, [10, 200, 10]), ..Arw::default() }.write(&raw);
        let modified = size.modified().unwrap() + std::time::Duration::from_secs(5);
        fs::File::options().write(true).open(&raw).unwrap().set_modified(modified).unwrap();
        assert_eq!(load(&raw, Some(&cache)).unwrap().width, 64);
        assert_eq!(entries().len(), 2);

        // Warming leaves what's there and adds what isn't.
        warm(&raw, &cache).unwrap();
        assert_eq!(entries().len(), 2);
        let other = folder.0.join("DSC00002.ARW");
        Arw::default().write(&other);
        warm(&other, &cache).unwrap();
        assert_eq!(entries().len(), 3);
        assert!(entries().iter().all(|p| p.extension().is_some_and(|e| e == "jpg")), "no temporary files left");
    }

    #[test]
    fn thumbnails_of_adobe_rgb_previews_are_srgb() {
        let folder = Folder::new("thumbs-adobe");
        let raw = folder.0.join("DSC00001.ARW");
        let leaf = crate::testing::jpeg(64, 48, [110, 160, 90]);
        Arw { preview: leaf.clone(), ..Arw::default() }.write(&raw);
        let srgb = load(&raw, None).unwrap().pixel(10, 10);
        Arw { preview: leaf, color_space: 0xffff, ..Arw::default() }.write(&raw);
        let adobe = load(&raw, None).unwrap().pixel(10, 10);
        // The same numbers mean a more saturated green in Adobe RGB, so
        // in sRGB there's less red in it.
        assert!(adobe[0] + 15 < srgb[0], "{adobe:?} {srgb:?}");
    }

    #[test]
    fn without_a_cache_thumbnails_are_still_made() {
        let folder = Folder::new("thumbs-uncached");
        let raw = folder.0.join("DSC00001.ARW");
        Arw::default().write(&raw);
        assert_eq!(load(&raw, None).unwrap().width, 48);
        assert!(load(&folder.0.join("missing.ARW"), None).is_err());
    }
}
