//! Faces found in a frame, and kept in a cache (`~/.cache/omacull/faces/`)
//! so a folder is looked through once. Finding them is `omacull-ai`'s
//! job; this is what the rest of Omacull knows of them.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::thumbs;

/// Bumped when faces are found differently, so old findings aren't used.
/// The signals are measured from them: bump theirs too.
const VERSION: u64 = 2;

/// A face in a frame, in fractions of the upright frame's width and height.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Face {
    /// How sure the detector is, 0 to 1.
    pub score: f32,
    /// Left, top, right, bottom.
    pub bounds: [f32; 4],
    /// The eye on the left of the picture, then the other.
    pub eyes: [[f32; 2]; 2],
    /// How open the eyes are, 0 (shut) to 1, the two together. None if it
    /// couldn't be told.
    #[serde(default)]
    pub open: Option<f32>,
}

impl Face {
    /// Halfway between the eyes: where to zoom to see them both.
    pub fn between_eyes(&self) -> [f32; 2] {
        let [[x0, y0], [x1, y1]] = self.eyes;
        [(x0 + x1) / 2.0, (y0 + y1) / 2.0]
    }

    pub fn area(&self) -> f32 {
        let [l, t, r, b] = self.bounds;
        (r - l).max(0.0) * (b - t).max(0.0)
    }
}

/// `$XDG_CACHE_HOME/omacull/faces`, or `~/.cache/omacull/faces`.
pub fn default_dir() -> Option<PathBuf> {
    Some(thumbs::cache_dir()?.join("faces"))
}

fn cached(dir: &Path, raw: &Path) -> io::Result<PathBuf> {
    Ok(dir.join(format!("{}.json", thumbs::key(raw, VERSION)?)))
}

/// The faces found in a raw before, if they're cached.
pub fn load(raw: &Path, dir: &Path) -> Option<Vec<Face>> {
    let text = fs::read(cached(dir, raw).ok()?).ok()?;
    serde_json::from_slice(&text).ok()
}

/// Keep the faces found in a raw, none being worth keeping too.
pub fn store(raw: &Path, dir: &Path, faces: &[Face]) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = cached(dir, raw)?;
    let mut temporary = path.clone().into_os_string();
    temporary.push(format!(".{}.tmp", std::process::id()));
    fs::write(&temporary, serde_json::to_vec(faces)?)?;
    fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Arw, Folder};

    #[test]
    fn faces_are_cached_by_raw() {
        let folder = Folder::with_raws("faces", 2, &Arw::default());
        let cache = folder.0.join("cache");
        let face = Face { score: 0.9, bounds: [0.2, 0.1, 0.4, 0.5], eyes: [[0.25, 0.2], [0.35, 0.22]], open: Some(0.8) };
        assert_eq!(load(&folder.raw(1), &cache), None);
        store(&folder.raw(1), &cache, &[face]).unwrap();
        store(&folder.raw(2), &cache, &[]).unwrap();
        assert_eq!(load(&folder.raw(1), &cache), Some(vec![face]));
        assert_eq!(load(&folder.raw(2), &cache), Some(vec![]), "none found is remembered too");
        let [x, y] = face.between_eyes();
        assert!((x - 0.3).abs() < 1e-6 && (y - 0.21).abs() < 1e-6);
        assert!((face.area() - 0.08).abs() < 1e-6);
    }
}
