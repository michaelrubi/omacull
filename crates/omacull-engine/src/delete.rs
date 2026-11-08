//! Rejects removed from the disk for good, each with its sidecars: the
//! one place Omacull deletes a picture, and only when the user has said
//! yes to it.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::Path;

use crate::sidecar::{self, Naming};

/// Delete a picture, then its sidecars, named either way (they'd be left
/// for no picture). The picture first: if it can't be deleted, its marks
/// stay with it.
pub fn delete(picture: &Path) -> io::Result<()> {
    fs::remove_file(picture)?;
    for naming in [Naming::Darktable, Naming::Adobe] {
        match fs::remove_file(sidecar::named(picture, naming)) {
            Err(e) if e.kind() != ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Arw, Folder};

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> =
            fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[test]
    fn a_picture_goes_with_its_sidecars_and_nothing_else() {
        let folder = Folder::with_raws("delete-sidecars", 2, &Arw::default());
        for name in ["DSC00001.ARW.xmp", "DSC00001.xmp", "DSC00001.JPG", "DSC00001.JPG.xmp", "DSC00002.ARW.xmp"] {
            fs::write(folder.0.join(name), "").unwrap();
        }
        delete(&folder.raw(1)).unwrap();
        // The JPEG beside it is a picture of its own, with its own marks.
        assert_eq!(names(&folder.0), ["DSC00001.JPG", "DSC00001.JPG.xmp", "DSC00002.ARW", "DSC00002.ARW.xmp"]);
        delete(&folder.0.join("DSC00001.JPG")).unwrap();
        assert_eq!(names(&folder.0), ["DSC00002.ARW", "DSC00002.ARW.xmp"]);
    }

    #[test]
    fn a_picture_that_cant_be_deleted_keeps_its_sidecar() {
        let folder = Folder::new("delete-missing");
        fs::write(folder.0.join("DSC00001.ARW.xmp"), "").unwrap();
        assert!(delete(&folder.raw(1)).is_err());
        assert_eq!(names(&folder.0), ["DSC00001.ARW.xmp"]);
    }
}
