//! Copies of pictures in another folder, each with its sidecar: a cull's
//! keepers on their own, to hand on.
//!
//! The pictures are copied, never moved, and nothing in the folder they go
//! to is replaced: a picture already there is left as it is, with whatever
//! has been done to its sidecar since.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use crate::sidecar;

/// Copy a picture into `to`, with its sidecar if it has one. False if a
/// file of its name was there already, in which case nothing is copied.
pub fn copy(picture: &Path, to: &Path) -> io::Result<bool> {
    let name = picture.file_name().ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "not a file"))?;
    let copy = to.join(name);
    if copy.try_exists()? {
        return Ok(false);
    }
    fs::create_dir_all(to)?;
    // The sidecar first: a picture is never there without its marks, and so
    // never taken for copied without them.
    let (from, sidecar) = (sidecar::path_for(picture), sidecar::path_for(&copy));
    if from.try_exists()? && !sidecar.try_exists()? {
        place(&from, &sidecar)?;
    }
    place(picture, &copy)?;
    Ok(true)
}

/// A copy of `from` at `to`, with its date, there whole or not at all.
fn place(from: &Path, to: &Path) -> io::Result<()> {
    let mut name = to.as_os_str().to_owned();
    name.push(".omacull-tmp");
    let temporary = PathBuf::from(name);
    let placed = (|| {
        fs::copy(from, &temporary)?;
        let file = fs::File::options().write(true).open(&temporary)?;
        // Without the date it's still a copy.
        let _ = file.set_modified(fs::metadata(from)?.modified()?);
        file.sync_all()?;
        fs::rename(&temporary, to)
    })();
    if placed.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    placed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Arw, Folder};

    const DARKTABLE: &str = include_str!("../testdata/darktable.ARW.xmp");

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> =
            fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[test]
    fn a_picture_is_copied_with_its_sidecar_as_darktable_wrote_it() {
        let folder = Folder::with_raws("export-copy", 2, &Arw::default());
        let to = folder.0.join("selects");
        fs::write(sidecar::path_for(&folder.raw(1)), DARKTABLE).unwrap();
        let then = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        fs::File::options().write(true).open(folder.raw(1)).unwrap().set_modified(then).unwrap();

        assert!(copy(&folder.raw(1), &to).unwrap());
        assert!(copy(&folder.raw(2), &to).unwrap());
        assert_eq!(names(&to), ["DSC00001.ARW", "DSC00001.ARW.xmp", "DSC00002.ARW"], "and nothing else");
        assert_eq!(fs::read(to.join("DSC00001.ARW")).unwrap(), fs::read(folder.raw(1)).unwrap());
        assert_eq!(fs::read_to_string(to.join("DSC00001.ARW.xmp")).unwrap(), DARKTABLE);
        assert_eq!(fs::metadata(to.join("DSC00001.ARW")).unwrap().modified().unwrap(), then, "its date too");
        // The originals are where they were.
        assert_eq!(names(&folder.0), ["DSC00001.ARW", "DSC00001.ARW.xmp", "DSC00002.ARW", "selects"]);
    }

    #[test]
    fn nothing_already_there_is_replaced() {
        let folder = Folder::with_raws("export-there", 1, &Arw::default());
        let to = folder.0.join("selects");
        sidecar::write(&folder.raw(1), 3).unwrap();
        assert!(copy(&folder.raw(1), &to).unwrap());
        // Marked again in both places since: the copy keeps its own.
        sidecar::write(&to.join("DSC00001.ARW"), 5).unwrap();
        sidecar::write(&folder.raw(1), 1).unwrap();
        assert!(!copy(&folder.raw(1), &to).unwrap());
        assert_eq!(sidecar::read(&to.join("DSC00001.ARW")).unwrap(), Some(5));
        // Nor a sidecar waiting there for a picture that isn't yet.
        fs::remove_file(to.join("DSC00001.ARW")).unwrap();
        assert!(copy(&folder.raw(1), &to).unwrap());
        assert_eq!(sidecar::read(&to.join("DSC00001.ARW")).unwrap(), Some(5));
        // Into the folder it's in, there's nothing to do.
        assert!(!copy(&folder.raw(1), &folder.0).unwrap());
    }

    #[test]
    fn a_picture_that_cant_be_copied_leaves_nothing_behind() {
        let folder = Folder::new("export-missing");
        let to = folder.0.join("selects");
        assert!(copy(&folder.raw(1), &to).is_err());
        assert_eq!(names(&to), [] as [&str; 0]);
    }
}
