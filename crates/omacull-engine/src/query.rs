//! Field search: a line of terms that narrows the filmstrip further than the
//! mark filter does, over a frame's shooting settings and what was measured
//! of it. `iso:>=3200 focus:<12 shut:yes` shows the frames that are all of
//! those.
//!
//! The tokenizer and the comparison syntax follow LightCraft's catalogue
//! search (`crates/catalog/src/query.rs`, MIT or Apache-2.0); the fields
//! are Omacull's own.

use crate::cull::Frame;
use crate::sidecar::REJECT;

/// A term that can't be read, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryError(pub String);

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for QueryError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Gt,
    Ge,
    Lt,
    Le,
    Eq,
}

impl Op {
    fn test(self, x: f64, to: f64) -> bool {
        match self {
            Op::Gt => x > to,
            Op::Ge => x >= to,
            Op::Lt => x < to,
            Op::Le => x <= to,
            // Settings are stored as f32: 2.8 isn't exactly 2.8.
            Op::Eq => (x - to).abs() < 1e-4 * to.abs().max(1.0),
        }
    }
}

/// A number a frame has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Number {
    Rating,
    Iso,
    Aperture,
    Focal,
    /// Exposure time in seconds.
    Shutter,
    /// How crisp it is where the camera focused.
    Focus,
    /// How crisp it is at the eyes.
    Eyes,
    /// How open the eyes are, 0 to 1.
    Open,
    /// How much is blown out, 0 to 1.
    Blown,
    /// How much is black, 0 to 1.
    Black,
    Faces,
}

impl Number {
    /// Whether it's something measured from the picture, and so not known
    /// until the background pass has got to the frame.
    fn measured(self) -> bool {
        matches!(self, Number::Focus | Number::Eyes | Number::Open | Number::Blown | Number::Black | Number::Faces)
    }

    fn of(self, frame: &Frame) -> Option<f64> {
        let exif = frame.exif.as_ref();
        let signals = frame.signals.as_ref();
        match self {
            Number::Rating => Some(f64::from(frame.rating)),
            Number::Iso => exif?.iso.map(f64::from),
            Number::Aperture => exif?.f_number.map(f64::from),
            Number::Focal => exif?.focal_length.map(f64::from),
            Number::Shutter => exif?.exposure.filter(|&(_, d)| d > 0).map(|(n, d)| f64::from(n) / f64::from(d)),
            Number::Focus => signals?.focus.map(f64::from),
            Number::Eyes => signals?.eyes.map(f64::from),
            Number::Open => signals?.open.map(f64::from),
            Number::Blown => signals.map(|s| f64::from(s.highlights)),
            Number::Black => signals.map(|s| f64::from(s.shadows)),
            Number::Faces => signals.map(|s| f64::from(s.faces)),
        }
    }
}

/// A yes or no a frame has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flag {
    /// Eyes look shut.
    Shut,
    /// Has a mark suggested.
    Suggested,
    /// Is in a stack.
    Stacked,
}

impl Flag {
    fn measured(self) -> bool {
        self == Flag::Shut
    }

    fn of(self, frame: &Frame, stacked: bool) -> Option<bool> {
        match self {
            Flag::Shut => {
                let signals = frame.signals.as_ref()?;
                signals.open.map(|_| signals.shut())
            }
            Flag::Suggested => Some(frame.rating == 0 && frame.suggested.is_some()),
            Flag::Stacked => Some(stacked),
        }
    }
}

/// Text a frame has, matched as a case-insensitive part of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Text {
    Name,
    Camera,
    Lens,
}

impl Text {
    fn of(self, frame: &Frame) -> String {
        let exif = frame.exif.as_ref();
        match self {
            Text::Name => frame.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            Text::Camera => exif.and_then(|e| e.model.clone()).unwrap_or_default(),
            Text::Lens => exif.and_then(|e| e.lens.clone()).unwrap_or_default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Term {
    Number(Number, Op, f64),
    Flag(Flag, bool),
    Text(Text, String),
    /// A word that's in the file name, camera or lens.
    Word(String),
}

/// What the filmstrip is narrowed to, besides the mark filter. Empty lets
/// everything through.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    source: String,
    terms: Vec<Term>,
}

impl Query {
    /// Read a line of terms. Every term must match: they're ANDed.
    pub fn parse(text: &str) -> Result<Self, QueryError> {
        let terms = tokens(text).map(term).collect::<Result<Vec<_>, _>>()?;
        Ok(Self { source: text.trim().to_owned(), terms })
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// The text it was read from.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Whether it asks about anything measured from the picture: what a
    /// frame hasn't got until the background pass reaches it.
    pub fn needs_measuring(&self) -> bool {
        self.terms.iter().any(|t| match t {
            Term::Number(n, ..) => n.measured(),
            Term::Flag(f, _) => f.measured(),
            _ => false,
        })
    }

    /// Whether a frame has what every term asks for. A frame that lacks the
    /// number a term compares, or hasn't been measured, doesn't match it.
    pub fn matches(&self, frame: &Frame, stacked: bool) -> bool {
        self.terms.iter().all(|term| match term {
            Term::Number(number, op, to) => number.of(frame).is_some_and(|x| op.test(x, *to)),
            Term::Flag(flag, want) => flag.of(frame, stacked) == Some(*want),
            Term::Text(text, part) => text.of(frame).to_lowercase().contains(part),
            Term::Word(word) => {
                [Text::Name, Text::Camera, Text::Lens].into_iter().any(|t| t.of(frame).to_lowercase().contains(word))
            }
        })
    }

    /// The field names, for telling the user what there is to ask for.
    pub const FIELDS: &'static str = "rating, iso, f, focal, shutter, focus, eyes, open, blown, black, faces, \
         shut, suggested, stacked, flag, camera, lens, name";
}

/// The tokens of a line: split at whitespace, except inside double quotes,
/// so `lens:"FE 85mm"` is one.
fn tokens(text: &str) -> impl Iterator<Item = &str> {
    let mut rest = text;
    std::iter::from_fn(move || {
        rest = rest.trim_start();
        if rest.is_empty() {
            return None;
        }
        let mut quoted = false;
        let end = rest
            .char_indices()
            .find(|&(_, c)| {
                quoted ^= c == '"';
                !quoted && c.is_whitespace()
            })
            .map_or(rest.len(), |(i, _)| i);
        let (token, tail) = rest.split_at(end);
        rest = tail;
        Some(token)
    })
}

/// `s` without the double quotes around it (a closing one may be missing).
fn unquote(s: &str) -> &str {
    s.strip_prefix('"').map_or(s, |inner| inner.strip_suffix('"').unwrap_or(inner))
}

fn term(token: &str) -> Result<Term, QueryError> {
    let lower = token.to_lowercase();
    let Some((field, value)) = lower.split_once(':').filter(|_| !lower.starts_with('"')) else {
        return Ok(Term::Word(unquote(&lower).to_owned()));
    };
    let value = unquote(value);
    let bad = |what: &str| QueryError(format!("{field}: {what} (not \"{value}\")"));
    let number = |n: Number| {
        let (op, rest) = split_op(value);
        parse_number(rest).map(|to| Term::Number(n, op, to)).ok_or_else(|| bad("expects a number, like >=3"))
    };
    let flag = |f: Flag| match value {
        "yes" | "true" => Ok(Term::Flag(f, true)),
        "no" | "false" => Ok(Term::Flag(f, false)),
        _ => Err(bad("expects yes or no")),
    };
    let text = |t: Text| match value {
        "" => Err(bad("expects some text")),
        _ => Ok(Term::Text(t, value.to_owned())),
    };
    match field {
        "rating" | "stars" => number(Number::Rating),
        "iso" => number(Number::Iso),
        "f" | "aperture" => number(Number::Aperture),
        "focal" => number(Number::Focal),
        "shutter" | "exposure" => number(Number::Shutter),
        "focus" | "sharp" => number(Number::Focus),
        "eyes" => number(Number::Eyes),
        "open" => number(Number::Open),
        "blown" | "highlights" => number(Number::Blown),
        "black" | "shadows" => number(Number::Black),
        "faces" => number(Number::Faces),
        "shut" => flag(Flag::Shut),
        "suggested" => flag(Flag::Suggested),
        "stacked" | "stack" => flag(Flag::Stacked),
        // What the mark filter's keys say, for those who think in them.
        "flag" => match value {
            "reject" | "rejected" => Ok(Term::Number(Number::Rating, Op::Eq, f64::from(REJECT))),
            "undecided" | "none" => Ok(Term::Number(Number::Rating, Op::Eq, 0.0)),
            "pick" | "picked" => Ok(Term::Number(Number::Rating, Op::Ge, 1.0)),
            _ => Err(bad("expects pick, reject or undecided")),
        },
        "camera" => text(Text::Camera),
        "lens" => text(Text::Lens),
        "name" | "file" => text(Text::Name),
        _ => Err(QueryError(format!("no field called \"{field}\" (there are {})", Query::FIELDS))),
    }
}

/// `>=`, `<=`, `>`, `<` or `=` at the start of a value, and the rest. Two
/// characters first, or `>=3` would read `=3` as the number.
fn split_op(value: &str) -> (Op, &str) {
    [(">=", Op::Ge), ("<=", Op::Le), (">", Op::Gt), ("<", Op::Lt), ("=", Op::Eq)]
        .into_iter()
        .find_map(|(prefix, op)| Some((op, value.strip_prefix(prefix)?)))
        .unwrap_or((Op::Eq, value))
}

/// A number, a fraction like `1/250`, or a percentage like `5%`.
fn parse_number(s: &str) -> Option<f64> {
    let finite = |x: f64| x.is_finite().then_some(x);
    if let Some(percent) = s.strip_suffix('%') {
        return finite(percent.parse::<f64>().ok()? / 100.0);
    }
    if let Some((n, d)) = s.split_once('/') {
        let (n, d) = (n.parse::<f64>().ok()?, d.parse::<f64>().ok()?);
        return finite(n / d).filter(|_| d != 0.0);
    }
    finite(s.parse().ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::Exif;
    use crate::signals::Signals;

    fn frame(rating: i32, exif: Exif, signals: Option<Signals>) -> Frame {
        Frame { exif: Some(exif), signals, ..Frame::new("DSC00001.ARW".into(), rating) }
    }

    fn shot(iso: u32, exposure: (u32, u32)) -> Exif {
        Exif {
            model: Some("ILCE-7M3".into()),
            lens: Some("FE 85mm F1.8".into()),
            exposure: Some(exposure),
            f_number: Some(2.8),
            iso: Some(iso),
            focal_length: Some(85.0),
        }
    }

    fn q(text: &str) -> Query {
        Query::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    #[test]
    fn nothing_asked_lets_everything_through() {
        let f = frame(0, Exif::default(), None);
        assert!(q("").is_empty() && q("").matches(&f, false));
        assert!(q("   ").matches(&f, false));
    }

    #[test]
    fn numbers_compare_the_way_they_are_written() {
        let f = frame(3, shot(3200, (1, 250)), None);
        for (text, yes) in [
            ("iso:>=3200", true),
            ("iso:>3200", false),
            ("iso:3200", true),
            ("iso:=3200", true),
            ("iso:<=3200 iso:>800", true),
            ("f:2.8", true),
            ("aperture:<2", false),
            ("focal:>=85", true),
            ("rating:>=3", true),
            ("rating:4", false),
            ("shutter:1/250", true),
            ("shutter:<1/125", true),
            ("shutter:<1/500", false),
            ("shutter:>0.003", true),
        ] {
            assert_eq!(q(text).matches(&f, false), yes, "{text}");
        }
    }

    #[test]
    fn terms_are_anded_and_words_search_name_camera_and_lens() {
        let f = frame(0, shot(400, (1, 60)), None);
        assert!(q("iso:400 85mm").matches(&f, false));
        assert!(q("dsc0000 ilce").matches(&f, false));
        assert!(!q("iso:400 nikon").matches(&f, false));
        assert!(!q("iso:800 85mm").matches(&f, false));
        assert!(q("lens:\"fe 85mm\"").matches(&f, false));
        assert!(q("camera:7m3 name:.arw").matches(&f, false));
        assert!(q("\"fe 85mm\"").matches(&f, false), "a quoted phrase is one word");
    }

    #[test]
    fn what_a_frame_lacks_matches_nothing() {
        let bare = frame(0, Exif::default(), None);
        for text in ["iso:>0", "iso:<99999", "f:>0", "shutter:>0", "focus:>0", "faces:>=0", "blown:<1"] {
            assert!(!q(text).matches(&bare, false), "{text}");
        }
        let unmeasured = frame(0, shot(100, (1, 60)), None);
        assert!(!q("shut:no").matches(&unmeasured, false), "not yet measured is neither shut nor open");
        assert!(!q("shut:yes").matches(&unmeasured, false));
        assert!(q("iso:100").matches(&unmeasured, false));
    }

    #[test]
    fn signals_are_asked_for_by_what_they_measure() {
        let sharp = Signals { focus: Some(40.0), eyes: Some(30.0), open: Some(0.9), highlights: 0.02, shadows: 0.1, faces: 2, face: 0.1 };
        let blink = Signals { open: Some(0.1), focus: Some(8.0), ..sharp };
        let (a, b) = (frame(0, shot(100, (1, 60)), Some(sharp)), frame(0, shot(100, (1, 60)), Some(blink)));
        for (text, on_a, on_b) in [
            ("focus:<12", false, true),
            ("shut:yes", false, true),
            ("shut:no", true, false),
            ("open:>0.5", true, false),
            ("blown:<5%", true, true),
            ("blown:>5%", false, false),
            ("black:>=10%", true, true),
            ("faces:>=2", true, true),
            ("eyes:>20", true, true),
        ] {
            assert_eq!((q(text).matches(&a, false), q(text).matches(&b, false)), (on_a, on_b), "{text}");
        }
        assert!(q("focus:<12").needs_measuring() && q("shut:yes").needs_measuring());
        assert!(!q("iso:>100 stacked:yes suggested:no rating:2 name:dsc").needs_measuring());
    }

    #[test]
    fn marks_suggestions_and_stacks() {
        let mut f = frame(0, Exif::default(), None);
        f.suggested = Some(1);
        assert!(q("suggested:yes").matches(&f, false) && !q("suggested:no").matches(&f, false));
        assert!(q("stacked:yes").matches(&f, true) && q("stack:no").matches(&f, false));
        f.rating = 2;
        assert!(q("suggested:no").matches(&f, false), "only undecided frames are suggested for");
        assert!(q("flag:pick").matches(&f, false) && !q("flag:reject").matches(&f, false));
        f.rating = REJECT;
        assert!(q("flag:reject rating:<0").matches(&f, false));
        f.rating = 0;
        assert!(q("flag:undecided").matches(&f, false));
    }

    #[test]
    fn a_term_that_cant_be_read_is_an_error_that_says_why() {
        for (text, says) in [
            ("iso:fast", "iso: expects a number"),
            ("iso:>", "iso: expects a number"),
            ("iso:nan", "iso: expects a number"),
            ("shutter:1/0", "shutter: expects a number"),
            ("shut:maybe", "shut: expects yes or no"),
            ("flag:starred", "flag: expects pick"),
            ("lens:", "lens: expects some text"),
            ("colour:red", "no field called \"colour\""),
            ("iso:100 colour:red", "no field called \"colour\""),
        ] {
            let error = Query::parse(text).expect_err(text);
            assert!(error.0.contains(says), "{text}: {error}");
        }
    }

    #[test]
    fn the_source_is_kept_for_showing_and_saving() {
        assert_eq!(q("  iso:>=3200   shut:yes ").source(), "iso:>=3200   shut:yes");
        assert_eq!(q(q("focus:<12 \"85mm\"").source()), q("focus:<12 \"85mm\""));
    }
}
