//! Reading `git status --porcelain=v2 -z --branch` into the shape a pull
//! needs: the branch, its upstream, and one entry per changed file.
//!
//! With `-z` every record ends in a NUL and paths are not quoted, so a name
//! with spaces, quotes or non-ASCII letters arrives as it is on disk. A
//! rename or copy record is followed by one more NUL-terminated field holding
//! the path it came from.

/// One changed file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Change {
    pub(crate) path: String,
    /// False for a file Git does not track yet.
    pub(crate) tracked: bool,
}

/// The branch and the changed files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Status {
    /// The branch `HEAD` is on, also before its first commit. `None` when
    /// `HEAD` is detached.
    pub(crate) branch: Option<String>,
    /// The branch it follows, such as `origin/main`.
    pub(crate) upstream: Option<String>,
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
                if let Some(path) = record.splitn(9, ' ').nth(8) {
                    status.changes.push(tracked(path));
                }
            }
            // `2 XY sub mH mI mW hH hI Xscore path`, then the original path.
            Some('2') => {
                let _original = records.next();
                if let Some(path) = record.splitn(10, ' ').nth(9) {
                    status.changes.push(tracked(path));
                }
            }
            // `u XY sub m1 m2 m3 mW h1 h2 h3 path`
            Some('u') => {
                if let Some(path) = record.splitn(11, ' ').nth(10) {
                    status.changes.push(tracked(path));
                }
            }
            // `? path`
            Some('?') => {
                if let Some(path) = record.get(2..).filter(|path| !path.is_empty()) {
                    status.changes.push(Change {
                        path: path.to_string(),
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

fn tracked(path: &str) -> Change {
    Change {
        path: path.to_string(),
        tracked: true,
    }
}

fn apply_header(status: &mut Status, header: &str) {
    if let Some(head) = header.strip_prefix("branch.head ") {
        if head != "(detached)" {
            status.branch = Some(head.to_string());
        }
    } else if let Some(upstream) = header.strip_prefix("branch.upstream ") {
        status.upstream = Some(upstream.to_string());
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
        assert_eq!(status.changes.len(), 1);
    }

    #[test]
    fn a_detached_head_has_no_branch() {
        let status = parse(&records(&[
            format!("# branch.oid {ZERO}"),
            "# branch.head (detached)".to_string(),
        ]));
        assert_eq!(status.branch, None);
    }

    #[test]
    fn every_kind_of_record_is_read() {
        let status = parse(&records(&[
            "# branch.head main".to_string(),
            ordinary(".M", "modified.txt"),
            ordinary("A.", "added.txt"),
            ordinary(".D", "deleted.txt"),
            format!("2 R. N... 100644 100644 100644 {ZERO} {ZERO} R100 new name.txt"),
            "old name.txt".to_string(),
            format!("u UU N... 100644 100644 100644 100644 {ZERO} {ZERO} {ZERO} both.txt"),
            "? dir/untracked.txt".to_string(),
            "? caf\u{e9}.txt".to_string(),
        ]));
        let seen: Vec<(&str, bool)> = status
            .changes
            .iter()
            .map(|change| (change.path.as_str(), change.tracked))
            .collect();
        assert_eq!(
            seen,
            vec![
                ("added.txt", true),
                ("both.txt", true),
                ("caf\u{e9}.txt", false),
                ("deleted.txt", true),
                ("dir/untracked.txt", false),
                ("modified.txt", true),
                ("new name.txt", true),
            ]
        );
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
