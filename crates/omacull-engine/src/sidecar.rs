//! Ratings in darktable's XMP sidecars (`DSC01234.ARW.xmp`).
//!
//! darktable keeps its whole edit in the sidecar, so a rating is changed by
//! replacing the characters of its value and nothing else: the file is never
//! parsed and written back out.

use std::fs;
use std::io::{self, ErrorKind, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};

use quick_xml::events::Event;
use quick_xml::name::{QName, ResolveResult};
use quick_xml::reader::NsReader;

/// `xmp:Rating` as darktable reads it: -1 rejected, 0 unrated, 1 to 5 stars.
pub const REJECT: i32 = -1;

const XMP_NS: &str = "http://ns.adobe.com/xap/1.0/";
const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// The sidecar darktable reads for this raw.
pub fn path_for(raw: &Path) -> PathBuf {
    let mut name = raw.as_os_str().to_owned();
    name.push(".xmp");
    name.into()
}

fn invalid(what: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, what)
}

/// Where the rating is in a sidecar, or where it would go.
enum Place {
    /// The characters of the value.
    Value(Range<usize>),
    /// No rating: an attribute can go at `at`, in the first
    /// `rdf:Description` tag. `declared` if `xmp:` means XMP there.
    Missing { at: usize, declared: bool },
}

/// Where `part`, a slice of `xmp` handed out by the reader, sits in it.
fn span(xmp: &str, part: &str) -> io::Result<Range<usize>> {
    (part.as_ptr() as usize)
        .checked_sub(xmp.as_ptr() as usize)
        .map(|start| start..start + part.len())
        .filter(|range| range.end <= xmp.len())
        .ok_or_else(|| invalid("sidecar has escapes where the rating is"))
}

fn locate(xmp: &str) -> io::Result<Place> {
    let is = |result: &ResolveResult, namespace: &str| matches!(result, ResolveResult::Bound(n) if n.0 == namespace);
    let mut reader = NsReader::from_str(xmp);
    let mut missing = None;
    loop {
        let (namespace, event) = reader.read_resolved_event().map_err(invalid)?;
        let (in_rdf, in_xmp) = (is(&namespace, RDF_NS), is(&namespace, XMP_NS));
        match event {
            Event::Start(e) | Event::Empty(e) if in_rdf && e.local_name().as_ref() == "Description" => {
                for attribute in e.attributes() {
                    let attribute = attribute.map_err(invalid)?;
                    let (namespace, name) = reader.resolver().resolve_attribute(attribute.key);
                    if is(&namespace, XMP_NS) && name.as_ref() == "Rating" {
                        return span(xmp, &attribute.value).map(Place::Value);
                    }
                }
                if missing.is_none() {
                    let at = span(xmp, e.name().as_ref())?.end;
                    let declared = match reader.resolver().resolve_attribute(QName("xmp:Rating")).0 {
                        ResolveResult::Bound(n) if n.0 == XMP_NS => true,
                        ResolveResult::Unknown(_) => false,
                        _ => return Err(invalid("sidecar uses the xmp: prefix for something else")),
                    };
                    missing = Some(Place::Missing { at, declared });
                }
            }
            // Other writers spell it <xmp:Rating>3</xmp:Rating>.
            Event::Start(e) if in_xmp && e.local_name().as_ref() == "Rating" => {
                return match reader.read_event().map_err(invalid)? {
                    Event::Text(text) => span(xmp, &text).map(Place::Value),
                    _ => Err(invalid("sidecar has a rating that isn't a number")),
                };
            }
            Event::Empty(e) if in_xmp && e.local_name().as_ref() == "Rating" => {
                return Err(invalid("sidecar has an empty rating"));
            }
            Event::Eof => return missing.ok_or_else(|| invalid("sidecar has no rdf:Description")),
            _ => {}
        }
    }
}

/// The rating in a sidecar's text, if it has one.
pub fn rating(xmp: &str) -> io::Result<Option<i32>> {
    match locate(xmp)? {
        Place::Value(range) => xmp[range].trim().parse().map(Some).map_err(invalid),
        Place::Missing { .. } => Ok(None),
    }
}

/// A sidecar's text with its rating changed and every other byte as it was.
pub fn with_rating(xmp: &str, rating: i32) -> io::Result<String> {
    let (range, value) = match locate(xmp)? {
        Place::Value(range) => (range, rating.to_string()),
        Place::Missing { at, declared } => {
            let namespace = if declared { "" } else { " xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"" };
            (at..at, format!("{namespace} xmp:Rating=\"{rating}\""))
        }
    };
    Ok([&xmp[..range.start], &value, &xmp[range.end..]].concat())
}

/// A sidecar for a raw that has none, holding only a rating. darktable
/// reads the rating from it on import and starts the edit from scratch.
pub fn fresh(rating: i32) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Omacull\">\n \
         <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
         <rdf:Description rdf:about=\"\"\n    \
         xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n   \
         xmp:Rating=\"{rating}\"/>\n \
         </rdf:RDF>\n\
         </x:xmpmeta>\n"
    )
}

/// The rating in a raw's sidecar; None if it has no sidecar, or one
/// without a rating.
pub fn read(raw: &Path) -> io::Result<Option<i32>> {
    match fs::read_to_string(path_for(raw)) {
        Ok(xmp) => rating(&xmp),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Set a raw's rating in its sidecar, making the sidecar if there's a mark
/// to record. The sidecar is replaced in one step, so it's never half
/// written.
pub fn write(raw: &Path, rating: i32) -> io::Result<()> {
    if !(REJECT..=5).contains(&rating) {
        return Err(io::Error::new(ErrorKind::InvalidInput, "rating must be -1 to 5"));
    }
    let path = path_for(raw);
    let (xmp, permissions) = match fs::read_to_string(&path) {
        Ok(old) => {
            let new = with_rating(&old, rating)?;
            if new == old {
                return Ok(());
            }
            (new, Some(fs::metadata(&path)?.permissions()))
        }
        Err(e) if e.kind() == ErrorKind::NotFound && rating == 0 => return Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => (fresh(rating), None),
        Err(e) => return Err(e),
    };
    let mut name = path.clone().into_os_string();
    name.push(".omacull-tmp");
    let temporary = PathBuf::from(name);
    let written = (|| {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(xmp.as_bytes())?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        }
        file.sync_all()?;
        fs::rename(&temporary, &path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sidecar darktable 5.6 wrote, with a 14-step history.
    const DARKTABLE: &str = include_str!("../testdata/darktable.ARW.xmp");

    /// A folder of its own for a test, removed when dropped.
    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("omacull-sidecar-{}-{name}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn sidecars_are_named_as_darktable_names_them() {
        assert_eq!(path_for(Path::new("/shoot/DSC01234.ARW")), Path::new("/shoot/DSC01234.ARW.xmp"));
    }

    #[test]
    fn reads_the_rating_darktable_wrote() {
        assert_eq!(rating(DARKTABLE).unwrap(), Some(0));
    }

    #[test]
    fn changing_the_rating_changes_nothing_else_in_a_darktable_sidecar() {
        let at = DARKTABLE.find("xmp:Rating=\"0\"").unwrap() + "xmp:Rating=\"".len();
        for (value, text) in [(REJECT, "-1"), (3, "3"), (5, "5")] {
            let changed = with_rating(DARKTABLE, value).unwrap();
            assert_eq!(rating(&changed).unwrap(), Some(value));
            assert_eq!(changed[..at], DARKTABLE[..at]);
            assert_eq!(changed[at..at + text.len()], *text);
            assert_eq!(changed[at + text.len()..], DARKTABLE[at + 1..]);
            // And back again is the file darktable wrote, byte for byte.
            assert_eq!(with_rating(&changed, 0).unwrap(), DARKTABLE);
        }
    }

    #[test]
    fn a_fresh_sidecar_holds_the_rating() {
        for value in REJECT..=5 {
            assert_eq!(rating(&fresh(value)).unwrap(), Some(value));
        }
    }

    #[test]
    fn a_sidecar_without_a_rating_gains_one_as_an_attribute() {
        let without = DARKTABLE.replace("   xmp:Rating=\"0\"\n", "");
        assert_eq!(rating(&without).unwrap(), None);
        let with = with_rating(&without, 4).unwrap();
        assert_eq!(rating(&with).unwrap(), Some(4));
        assert_eq!(with.replace(" xmp:Rating=\"4\"", ""), without);

        // With no xmp: prefix either, that's declared too.
        let bare = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                    xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
                    <rdf:Description rdf:about=\"\"/></rdf:RDF></x:xmpmeta>";
        let with = with_rating(bare, 2).unwrap();
        assert_eq!(rating(&with).unwrap(), Some(2));
        assert!(with.contains("<rdf:Description xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" xmp:Rating=\"2\" rdf:about=\"\"/>"));
    }

    #[test]
    fn a_rating_written_as_an_element_is_read_and_changed_in_place() {
        let element = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                       xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
                       <rdf:Description rdf:about=\"\" xmlns:xap=\"http://ns.adobe.com/xap/1.0/\">\
                       <xap:Rating>2</xap:Rating></rdf:Description></rdf:RDF></x:xmpmeta>";
        assert_eq!(rating(element).unwrap(), Some(2));
        let changed = with_rating(element, REJECT).unwrap();
        assert_eq!(changed, element.replace(">2<", ">-1<"));
    }

    #[test]
    fn text_that_isnt_a_sidecar_is_refused() {
        assert!(with_rating("not xml at all", 3).is_err());
        assert!(with_rating("<a><b></a>", 3).is_err());
    }

    #[test]
    fn writing_makes_a_sidecar_only_when_theres_a_mark() {
        let folder = Folder::new("fresh");
        let raw = folder.0.join("DSC00001.ARW");
        assert_eq!(read(&raw).unwrap(), None);
        write(&raw, 0).unwrap();
        assert!(!path_for(&raw).exists());
        write(&raw, REJECT).unwrap();
        assert_eq!(read(&raw).unwrap(), Some(REJECT));
        write(&raw, 0).unwrap();
        assert_eq!(read(&raw).unwrap(), Some(0));
        assert!(write(&raw, 6).is_err());
        // Nothing left behind but the sidecar.
        let names: Vec<_> = fs::read_dir(&folder.0).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["DSC00001.ARW.xmp"]);
    }

    #[test]
    fn writing_to_a_darktable_sidecar_keeps_its_edit() {
        let folder = Folder::new("darktable");
        let raw = folder.0.join("MJR00685.ARW");
        fs::write(path_for(&raw), DARKTABLE).unwrap();
        write(&raw, 5).unwrap();
        let written = fs::read_to_string(path_for(&raw)).unwrap();
        assert_eq!(written, DARKTABLE.replace("xmp:Rating=\"0\"", "xmp:Rating=\"5\""));
        // The same rating again doesn't touch the file.
        let before = fs::metadata(path_for(&raw)).unwrap().modified().unwrap();
        write(&raw, 5).unwrap();
        assert_eq!(fs::metadata(path_for(&raw)).unwrap().modified().unwrap(), before);
    }
}
