//! Reading `git status --porcelain=v2 -z --branch` into the shape the panel
//! lists: the branch, its upstream and the counts against it, and one entry
//! per changed file.
//!
//! With `-z` every record ends in a NUL and paths are not quoted, so a name
//! with spaces, quotes or non-ASCII letters arrives as it is on disk. A
//! rename or copy record is followed by one more NUL-terminated field holding
//! the path it came from.

/// What a changed file is, compared with the last commit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Conflicted,
}

impl Kind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Kind::Added => "added",
            Kind::Modified => "modified",
            Kind::Deleted => "deleted",
            Kind::Renamed => "renamed",
            Kind::Conflicted => "conflicted",
        }
    }
}

/// One changed file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Change {
    pub(crate) path: String,
    pub(crate) kind: Kind,
    /// The path a renamed file came from.
    pub(crate) from: Option<String>,
    /// False for a file Git does not track yet.
    pub(crate) tracked: bool,
}

/// The branch and the changed files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Status {
    /// The branch `HEAD` is on, also before its first commit. `None` when
    /// `HEAD` is detached.
    pub(crate) branch: Option<String>,
    pub(crate) detached: bool,
    /// The branch it follows, such as `origin/main`.
    pub(crate) upstream: Option<String>,
    /// Commits here that the upstream lacks, when there is an upstream.
    pub(crate) ahead: Option<u32>,
    /// Commits there that this branch lacks, as far as the last fetch knows.
    pub(crate) behind: Option<u32>,
    /// Sorted by path.
    pub(crate) changes: Vec<Change>,
}

/// Reads the output of `git status --porcelain=v2 -z --branch`.
pub(crate) fn parse(raw: &[u8]) -> Status {
    let text = String::from_utf8_lossy(raw);
    let mut status = Status::default();
    let mut records = text.split('\0');
    while let Some(record) = records.next() {
        if let Some(header) = record.strip_prefix("# ") {
            apply_header(&mut status, header);
            continue;
        }
        match record.chars().next() {
            // `1 XY sub mH mI mW hH hI path`
            Some('1') => {
                let fields: Vec<&str> = record.splitn(9, ' ').collect();
                if let (Some(xy), Some(path)) = (fields.get(1), fields.get(8)) {
                    status.changes.push(Change {
                        path: (*path).to_string(),
                        kind: ordinary_kind(xy),
                        from: None,
                        tracked: true,
                    });
                }
            }
            // `2 XY sub mH mI mW hH hI Xscore path`, then the original path.
            Some('2') => {
                let fields: Vec<&str> = record.splitn(10, ' ').collect();
                let from = records.next().map(str::to_string);
                if let Some(path) = fields.get(9) {
                    status.changes.push(Change {
                        path: (*path).to_string(),
                        kind: Kind::Renamed,
                        from,
                        tracked: true,
                    });
                }
            }
            // `u XY sub m1 m2 m3 mW h1 h2 h3 path`
            Some('u') => {
                let fields: Vec<&str> = record.splitn(11, ' ').collect();
                if let Some(path) = fields.get(10) {
                    status.changes.push(Change {
                        path: (*path).to_string(),
                        kind: Kind::Conflicted,
                        from: None,
                        tracked: true,
                    });
                }
            }
            // `? path`
            Some('?') => {
                if let Some(path) = record.get(2..).filter(|path| !path.is_empty()) {
                    status.changes.push(Change {
                        path: path.to_string(),
                        kind: Kind::Added,
                        from: None,
                        tracked: false,
                    });
                }
            }
            _ => {}
        }
    }
    status.changes.sort_by(|a, b| a.path.cmp(&b.path));
    status
}

fn apply_header(status: &mut Status, header: &str) {
    if let Some(head) = header.strip_prefix("branch.head ") {
        if head == "(detached)" {
            status.detached = true;
        } else {
            status.branch = Some(head.to_string());
        }
    } else if let Some(upstream) = header.strip_prefix("branch.upstream ") {
        status.upstream = Some(upstream.to_string());
    } else if let Some(counts) = header.strip_prefix("branch.ab ") {
        let mut parts = counts.split(' ');
        status.ahead = parts
            .next()
            .and_then(|part| part.strip_prefix('+'))
            .and_then(|part| part.parse().ok());
        status.behind = parts
            .next()
            .and_then(|part| part.strip_prefix('-'))
            .and_then(|part| part.parse().ok());
    }
}

/// The kind of an ordinary record from its two status letters, the index and
/// the working tree. A file gone from either is deleted; a file new in the
/// index is added, whatever happened to it since.
fn ordinary_kind(xy: &str) -> Kind {
    let mut letters = xy.chars();
    let index = letters.next().unwrap_or('.');
    let tree = letters.next().unwrap_or('.');
    if index == 'D' || tree == 'D' {
        Kind::Deleted
    } else if index == 'A' {
        Kind::Added
    } else {
        Kind::Modified
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZERO: &str = "0000000000000000000000000000000000000000";

    /// Joins records the way `-z` does.
    fn records(records: &[String]) -> Vec<u8> {
        let mut raw = Vec::new();
        for record in records {
            raw.extend_from_slice(record.as_bytes());
            raw.push(0);
        }
        raw
    }

    fn ordinary(xy: &str, path: &str) -> String {
        format!("1 {xy} N... 100644 100644 100644 {ZERO} {ZERO} {path}")
    }

    #[test]
    fn the_branch_headers_are_read() {
        let status = parse(&records(&[
            format!("# branch.oid {ZERO}"),
            "# branch.head main".to_string(),
            "# branch.upstream origin/main".to_string(),
            "# branch.ab +3 -1".to_string(),
        ]));
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert_eq!((status.ahead, status.behind), (Some(3), Some(1)));
        assert!(!status.detached);
        assert!(status.changes.is_empty());
    }

    #[test]
    fn a_branch_before_its_first_commit_has_no_upstream() {
        let status = parse(&records(&[
            "# branch.oid (initial)".to_string(),
            "# branch.head main".to_string(),
            "? a.txt".to_string(),
        ]));
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.upstream, None);
        assert_eq!((status.ahead, status.behind), (None, None));
        assert_eq!(status.changes.len(), 1);
    }

    #[test]
    fn a_detached_head_has_no_branch() {
        let status = parse(&records(&[
            format!("# branch.oid {ZERO}"),
            "# branch.head (detached)".to_string(),
        ]));
        assert!(status.detached);
        assert_eq!(status.branch, None);
    }

    #[test]
    fn every_kind_of_record_is_read() {
        let status = parse(&records(&[
            "# branch.head main".to_string(),
            ordinary(".M", "modified.txt"),
            ordinary("M.", "staged.txt"),
            ordinary("A.", "added.txt"),
            ordinary("AM", "added-then-edited.txt"),
            ordinary(".D", "deleted.txt"),
            ordinary("D.", "staged-delete.txt"),
            format!("2 R. N... 100644 100644 100644 {ZERO} {ZERO} R100 new name.txt"),
            "old name.txt".to_string(),
            format!("u UU N... 100644 100644 100644 100644 {ZERO} {ZERO} {ZERO} both.txt"),
            "? dir/untracked.txt".to_string(),
            "? caf\u{e9}.txt".to_string(),
        ]));
        let seen: Vec<(&str, &str, bool)> = status
            .changes
            .iter()
            .map(|change| (change.path.as_str(), change.kind.as_str(), change.tracked))
            .collect();
        assert_eq!(
            seen,
            vec![
                ("added-then-edited.txt", "added", true),
                ("added.txt", "added", true),
                ("both.txt", "conflicted", true),
                ("caf\u{e9}.txt", "added", false),
                ("deleted.txt", "deleted", true),
                ("dir/untracked.txt", "added", false),
                ("modified.txt", "modified", true),
                ("new name.txt", "renamed", true),
                ("staged-delete.txt", "deleted", true),
                ("staged.txt", "modified", true),
            ]
        );
        let renamed = status
            .changes
            .iter()
            .find(|change| change.kind == Kind::Renamed)
            .unwrap();
        assert_eq!(renamed.from.as_deref(), Some("old name.txt"));
    }

    #[test]
    fn a_path_with_spaces_stays_whole() {
        let status = parse(&records(&[ordinary(".M", "my chapter/one two.tex")]));
        assert_eq!(status.changes[0].path, "my chapter/one two.tex");
    }

    #[test]
    fn nothing_changed_is_an_empty_list() {
        assert!(parse(b"").changes.is_empty());
        assert_eq!(parse(b""), Status::default());
    }
}
