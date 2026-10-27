//! What a relative path may be, in three named policies over one structure.
//!
//! `RelPath` is structure only: it says the path cannot leave the directory
//! it is joined to. `admit` is what a project may contain, the rules every
//! route that takes a path asks, the projection that reads what a peer wrote
//! into the shared document, and the command line before it sends anything,
//! so a name refused on the server is refused on the laptop first, with the
//! same reason. The browser mirrors it (`web/src/lib/paths.js`). `RepoPath`
//! is what may be named to Git, and `portable` is whether a project path can
//! be written to a local folder on every desktop OS.

use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

/// A path that a policy refused, and why. Displayed as one sentence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathRefusal {
    pub path: String,
    pub reason: String,
}

impl PathRefusal {
    fn new(path: &str, reason: impl Into<String>) -> PathRefusal {
        PathRefusal {
            path: path.to_owned(),
            reason: reason.into(),
        }
    }
}

impl fmt::Display for PathRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.reason)
    }
}

impl std::error::Error for PathRefusal {}

/// Structure only: relative, `/`-separated, no empty, `.` or `..` segment, no
/// `\`, no NUL or control characters, no leading `/`, and no Windows drive
/// prefix (`C:`). Never trims and never normalises. It says nothing about what
/// a project admits, so `asset:figures/plot.png` is one.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RelPath(String);

impl RelPath {
    /// Whether `path` has the structure, for the callers that only branch.
    pub fn is_valid(path: &str) -> bool {
        RelPath::try_from(path).is_ok()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }

    /// Where this path is below `root`. Every part of it must be a plain name:
    /// a root, a drive or `..` would lead out of the folder, and joining an
    /// absolute path replaces the folder altogether.
    pub fn under(&self, root: &Path) -> Result<PathBuf, PathRefusal> {
        let path = Path::new(&self.0);
        let plain = path
            .components()
            .all(|part| matches!(part, Component::Normal(_)));
        let target = root.join(path);
        if !plain || !target.starts_with(root) {
            return Err(PathRefusal::new(&self.0, "leaves the folder"));
        }
        Ok(target)
    }
}

impl TryFrom<&str> for RelPath {
    type Error = PathRefusal;

    fn try_from(path: &str) -> Result<RelPath, PathRefusal> {
        let refuse = |reason: &str| Err(PathRefusal::new(path, reason));
        if path.is_empty() {
            return refuse("a path needs a name");
        }
        if path.starts_with('/') {
            return refuse("a path is relative, so it cannot begin with /");
        }
        if path.contains('\\') {
            return refuse("paths are separated by /, not by \\");
        }
        if path.contains('\0') || path.chars().any(char::is_control) {
            return refuse("a path cannot carry control characters");
        }
        let bytes = path.as_bytes();
        if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            return refuse("a path cannot begin with a drive");
        }
        if path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return refuse("a path cannot have an empty or relative segment");
        }
        Ok(RelPath(path.to_owned()))
    }
}

/// What may be named to Git for a commit or a diff: the structure, and no
/// segment that is Git's own directory, whatever its case. Dotfiles such as
/// `.gitignore` and `.github/workflows/x.yml` are fine.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RepoPath(RelPath);

impl RepoPath {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl TryFrom<&str> for RepoPath {
    type Error = PathRefusal;

    fn try_from(path: &str) -> Result<RepoPath, PathRefusal> {
        let path = RelPath::try_from(path)?;
        if path
            .segments()
            .any(|part| part.eq_ignore_ascii_case(".git"))
        {
            return Err(PathRefusal::new(
                path.as_str(),
                "Git's own directory cannot be named",
            ));
        }
        Ok(RepoPath(path))
    }
}

/// The extra refusals a local folder needs on top of admission: a name that
/// begins or ends with a space, and a colon, which Windows reads as a drive.
/// `notes:2026.md` is a legal project file that cannot be written to a
/// Windows folder, so a pull reports it and skips it. Asked of the path as it
/// was spelled, before admission trims it.
pub fn portable(path: &str) -> Result<(), PathRefusal> {
    if path.trim() != path {
        return Err(PathRefusal::new(
            &format!("{path:?}"),
            "a name cannot begin or end with a space",
        ));
    }
    if path.contains(':') {
        return Err(PathRefusal::new(
            path,
            "a name cannot carry a colon, which Windows reads as a drive",
        ));
    }
    Ok(())
}

/// Which kind of file an extension says this is. A text is a file a person
/// edits and the shared document holds; an asset is bytes nobody edits in
/// place, kept in the store and named by their digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Text,
    Asset,
}

impl Kind {
    /// The spelling a tree entry and a projection digest use.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Asset => "asset",
        }
    }
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

/// At most eight segments. A paper has a `chapters/` and a `fig/`; a tree
/// deeper than this is a filesystem somebody is trying to store here.
pub const MAX_SEGMENTS: usize = 8;

/// A path in the composed form two spellings of one accented name share,
/// without trimming. The form a folder walk compares before admission.
pub fn nfc(path: &str) -> String {
    path.nfc().collect()
}

/// The normalised form of a path, which is what is compared and what is
/// stored. NFC because two spellings of the same accented name are the same
/// file to every person who looks at them and to macOS, whatever the bytes
/// say.
pub fn normalise(path: &str) -> String {
    nfc(path.trim())
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
pub fn admit(rules: &Rules, path: &str) -> Result<Kind, String> {
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
    let mut allowed = rules.text.to_vec();
    for extension in rules.asset {
        if !allowed.contains(extension) {
            allowed.push(extension.clone());
        }
    }
    let allowed = if allowed.is_empty() {
        "none".to_owned()
    } else {
        allowed.join(", ")
    };
    let error = format!("{path}: allowed file extensions are {allowed}");
    // Derived extensions are refused even when their names look plausible.
    if rules
        .derived
        .iter()
        .any(|end| lower.ends_with(end.as_str()))
    {
        return Err(error);
    }
    if rules.text.iter().any(|end| lower.ends_with(end.as_str())) {
        return Ok(Kind::Text);
    }
    if rules.asset.iter().any(|end| lower.ends_with(end.as_str())) {
        return Ok(Kind::Asset);
    }
    Err(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|item| (*item).to_owned()).collect()
    }

    fn rules<'a>(text: &'a [String], asset: &'a [String]) -> Rules<'a> {
        Rules {
            text,
            asset,
            derived: &[],
            max_path: 200,
            max_segments: MAX_SEGMENTS,
        }
    }

    #[test]
    fn the_policies_part_where_the_table_says() {
        let text = strings(&[".tex", ".md"]);
        let asset = strings(&[".png"]);
        let rules = rules(&text, &asset);
        let long = format!("{}.tex", "a".repeat(201));

        // main.tex
        assert!(RelPath::try_from("main.tex").is_ok());
        assert_eq!(admit(&rules, "main.tex"), Ok(Kind::Text));
        assert!(RepoPath::try_from("main.tex").is_ok());
        assert!(portable("main.tex").is_ok());

        // .gitignore
        assert!(RelPath::try_from(".gitignore").is_ok());
        assert!(admit(&rules, ".gitignore").is_err());
        assert!(RepoPath::try_from(".gitignore").is_ok());
        assert!(RepoPath::try_from(".github/workflows/x.yml").is_ok());

        // .git/config
        assert!(RelPath::try_from(".git/config").is_ok());
        assert!(admit(&rules, ".git/config").is_err());
        assert!(RepoPath::try_from(".git/config").is_err());
        assert!(RepoPath::try_from("a/.GIT/x").is_err());

        // notes:2026.md
        assert!(RelPath::try_from("notes:2026.md").is_ok());
        assert_eq!(admit(&rules, "notes:2026.md"), Ok(Kind::Text));
        assert!(RepoPath::try_from("notes:2026.md").is_ok());
        assert!(portable("notes:2026.md").is_err());

        // asset:figures/plot.png
        assert!(RelPath::try_from("asset:figures/plot.png").is_ok());

        // C:/x
        assert!(RelPath::try_from("C:/x").is_err());
        assert!(admit(&rules, "C:/x").is_err());
        assert!(RepoPath::try_from("C:/x").is_err());

        // " a.tex ": structure and repository keep it verbatim, admission trims.
        assert!(RelPath::try_from(" a.tex ").is_ok());
        assert_eq!(admit(&rules, " a.tex "), Ok(Kind::Text));
        assert!(RepoPath::try_from(" a.tex ").is_ok());
        assert!(portable(" a.tex ").is_err());
        assert!(portable(&normalise(" a.tex ")).is_ok());

        // 201 bytes
        assert!(RelPath::try_from(long.as_str()).is_ok());
        assert!(admit(&rules, &long).is_err());
        assert!(RepoPath::try_from(long.as_str()).is_ok());
    }

    #[test]
    fn a_structure_has_no_way_out() {
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../b",
            "a/./b",
            "a\\b",
            "a\u{0}b",
            "a//b",
            "x\n",
            "C:/Windows/system32",
            "c:x",
        ] {
            assert!(RelPath::try_from(bad).is_err(), "{bad:?} was allowed");
        }
        let path = RelPath::try_from("chapters/01.tex").unwrap();
        assert_eq!(path.as_str(), "chapters/01.tex");
        assert_eq!(path.segments().collect::<Vec<_>>(), ["chapters", "01.tex"]);
    }

    #[test]
    fn a_path_is_under_a_folder_only_when_every_part_is_a_plain_name() {
        let dir = Path::new("folder");
        let path = RelPath::try_from("chapters/one.tex").unwrap();
        assert_eq!(
            path.under(dir).unwrap(),
            dir.join("chapters").join("one.tex")
        );
    }

    #[test]
    fn the_rules_are_applied_to_the_trimmed_composed_form() {
        let text = strings(&[".tex"]);
        let rules = rules(&text, &[]);
        assert_eq!(normalise("cafe\u{301}.tex"), "caf\u{e9}.tex");
        assert_eq!(normalise(" a.tex "), "a.tex");
        assert_eq!(collision_key("Fig.TEX"), "fig.tex");
        assert_eq!(admit(&rules, "cafe\u{301}.tex"), Ok(Kind::Text));
        let derived = strings(&[".pdf"]);
        let with_derived = Rules {
            derived: &derived,
            ..rules
        };
        assert!(admit(&with_derived, "paper.pdf").is_err());
        let shallow = Rules {
            max_segments: 2,
            ..with_derived
        };
        assert!(admit(&shallow, "a/b/c.tex").is_err());
        assert_eq!(admit(&shallow, "a/b.tex"), Ok(Kind::Text));
    }
}
