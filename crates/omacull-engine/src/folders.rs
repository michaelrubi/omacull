//! The folders round the one being culled, for the folder tree: each with
//! how many pictures it holds, and whether it has folders of its own.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::cull::picture;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    pub path: PathBuf,
    pub name: String,
    /// The raws, JPEGs and PNGs in it.
    pub pictures: usize,
    /// It has folders in it to open out.
    pub nested: bool,
}

fn hidden(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n.as_encoded_bytes().starts_with(b"."))
}

/// The folders in `dir`, by name, leaving out hidden ones.
pub fn list(dir: &Path) -> io::Result<Vec<Folder>> {
    let mut folders: Vec<Folder> = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir() && !hidden(p))
        .map(|path| {
            let (mut pictures, mut nested) = (0, false);
            for entry in fs::read_dir(&path).into_iter().flatten().filter_map(Result::ok) {
                let inner = entry.path();
                if picture(&inner).is_some() {
                    pictures += 1;
                } else if !nested && inner.is_dir() && !hidden(&inner) {
                    nested = true;
                }
            }
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            Folder { path, name, pictures, nested }
        })
        .collect();
    folders.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(folders)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Arw, Folder as Temp};

    #[test]
    fn folders_are_listed_with_their_pictures() {
        let root = Temp::new("folders");
        let shoot = root.0.join("2026-10-04 wedding");
        fs::create_dir_all(shoot.join("selects")).unwrap();
        for i in 1..=3 {
            Arw::default().write(&shoot.join(format!("DSC{i:05}.ARW")));
        }
        fs::write(shoot.join("DSC00001.ARW.xmp"), "").unwrap();
        fs::write(shoot.join("DSC00001.JPG"), crate::testing::jpeg(8, 8, [0; 3])).unwrap();
        fs::create_dir_all(root.0.join("2026-09-01 empty")).unwrap();
        fs::create_dir_all(root.0.join(".thumbnails")).unwrap();
        fs::write(root.0.join("notes.txt"), "").unwrap();

        let listed = list(&root.0).unwrap();
        let summary: Vec<_> = listed.iter().map(|f| (f.name.as_str(), f.pictures, f.nested)).collect();
        assert_eq!(summary, [("2026-09-01 empty", 0, false), ("2026-10-04 wedding", 4, true)]);
        assert!(list(&root.0.join("missing")).is_err());
    }
}
