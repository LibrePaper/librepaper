//! What a path in a document's directory may be, and which of the two kinds
//! of file sits at it.
//!
//! One function, asked by every route that takes a path, by the repair pass
//! that runs over what a peer wrote into the shared document, and by the
//! command line before it sends anything -- so a name refused on the server is
//! refused on the laptop first, with the same reason. The rules are the
//! server's; the laptop and the shell check early only so the refusal is
//! explained where the person is.

use unicode_normalization::UnicodeNormalization;

use crate::config::Configuration;

/// Which kind of file an extension says this is. A text is a file a person
/// edits and the shared document holds; an asset is bytes nobody edits in
/// place, kept in the store and named by their digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Asset,
}

/// The lists and the length, borrowed from the deployment's rules. A struct
/// rather than four arguments, because every caller has a `Configuration` and
/// none of them should be choosing which of its fields the answer depends on.
pub struct Rules<'a> {
    pub text: &'a [String],
    pub asset: &'a [String],
    pub derived: &'a [String],
    pub max_path: usize,
    pub max_segments: usize,
}

impl Configuration {
    pub fn paths(&self) -> Rules<'_> {
        Rules {
            text: &self.text_extensions,
            asset: &self.asset_extensions,
            derived: &self.derived_extensions,
            max_path: self.max_path,
            max_segments: MAX_SEGMENTS,
        }
    }
}

/// At most eight segments. A paper has a `chapters/` and a `fig/`; a tree
/// deeper than this is a filesystem somebody is trying to store here.
pub const MAX_SEGMENTS: usize = 8;

/// The normalised form of a path, which is what is compared and what is
/// stored. NFC because two spellings of the same accented name are the same
/// file to every person who looks at them and to macOS, whatever the bytes
/// say.
pub fn normalise(path: &str) -> String {
    path.trim().nfc().collect()
}

/// The key two paths collide on: normalised, then case-folded, because a sync
/// client on macOS or Windows would write `Fig.png` and `fig.png` to one file
/// and silently lose one of them.
pub fn collision_key(path: &str) -> String {
    normalise(path).to_lowercase()
}

/// Whether this path is one the rules accept, and which kind of file it names.
/// The error is the sentence shown to whoever offered the path, so it says
/// what is wrong with this path rather than restating the rules.
pub fn check(rules: &Rules, path: &str) -> Result<Kind, String> {
    let path = normalise(path);
    if path.is_empty() {
        return Err("a file needs a name".into());
    }
    if path.len() > rules.max_path {
        return Err(format!(
            "{path}: a path may be at most {} bytes",
            rules.max_path
        ));
    }
    if path.starts_with('/') {
        return Err(format!(
            "{path}: a path is relative, so it cannot begin with /"
        ));
    }
    if path.contains('\\') {
        return Err(format!("{path}: paths are separated by /, not by \\"));
    }
    // Anything a filesystem or a URL would read as something other than a
    // name. Control characters are the ones a terminal would obey rather than
    // print.
    if path.chars().any(|c| (c as u32) < 0x20 || c == 0x7f as char) {
        return Err(format!("{path}: a path cannot carry control characters"));
    }
    let segments: Vec<&str> = path.split('/').collect();
    if segments.len() > rules.max_segments {
        return Err(format!(
            "{path}: a path may be at most {} segments deep",
            rules.max_segments
        ));
    }
    for segment in &segments {
        if segment.is_empty() {
            return Err(format!("{path}: a path cannot have an empty segment"));
        }
        if *segment == "." || *segment == ".." {
            return Err(format!("{path}: a path cannot climb with . or .."));
        }
        if segment.starts_with('.') {
            return Err(format!("{path}: a dotfile is not part of a document"));
        }
    }
    kind_of(rules, &path)
}

/// The kind an extension names, or the reason there is none. Split out
/// because the answer for a name with no directory in front of it is the same
/// answer, and the command line asks it that way.
pub fn kind_of(rules: &Rules, path: &str) -> Result<Kind, String> {
    let lower = path.to_lowercase();
    // Derived first, because `.log` and `.out` are plausible-looking names and
    // the reason they are refused is worth saying rather than "not a file
    // type this holds".
    if rules
        .derived
        .iter()
        .any(|end| lower.ends_with(end.as_str()))
    {
        return Err(format!(
            "{path}: this is a file a compiler writes, and the document keeps what a person wrote"
        ));
    }
    if rules.text.iter().any(|end| lower.ends_with(end.as_str())) {
        return Ok(Kind::Text);
    }
    if rules.asset.iter().any(|end| lower.ends_with(end.as_str())) {
        return Ok(Kind::Asset);
    }
    Err(format!(
        "{path}: a document holds texts and figures, and this is neither"
    ))
}

/// The name a file with no usable one is given, so that a path the rules
/// refuse becomes a file somebody can see and rename rather than a key
/// nothing reaches. The id is in it so that two of them seldom collide, and
/// the repair moves one aside when they do.
///
/// Only the letters and digits of the id, and not many of them: the id is a
/// key a peer wrote into the shared document, so it can be `/`, a control
/// character, or a kilobyte long, and a name built from it raw would be one
/// the rules refuse -- which the repair would then report, without effect,
/// after every update for the rest of the document's life.
pub fn placeholder(id: &str) -> String {
    let clean: String = id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(12)
        .collect();
    if clean.is_empty() {
        "unnamed.txt".to_string()
    } else {
        format!("unnamed-{clean}.txt")
    }
}

/// The next spelling of a path that is already taken: `paper.tex` becomes
/// `paper (2).tex`, then `paper (3).tex`. Before the extension rather than
/// after it, because a name that stops being a `.tex` stops being a file the
/// compiler will read.
pub fn suffixed(path: &str, nth: usize) -> String {
    let (stem, extension) = match path.rfind('.') {
        // A leading dot is not an extension, and a dot in a directory name is
        // not this file's.
        Some(dot) if dot > 0 && !path[dot..].contains('/') => (&path[..dot], &path[dot..]),
        _ => (path, ""),
    };
    format!("{stem} ({nth}){extension}")
}
