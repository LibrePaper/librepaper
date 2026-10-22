//! Checks on everything the page hands the sidecar before it reaches a Git
//! command line: object ids, repository paths, branch names and remote URLs.
//! Also the redaction applied to Git's own output before it is returned.

use std::sync::OnceLock;

use super::GitError;

/// Whether `text` is a SHA-1 object id: 40 lowercase hex digits.
pub(super) fn is_id(text: &str) -> bool {
    text.len() == 40 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether `text` is a SHA-256 digest in lowercase hex.
pub(super) fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `text` when it is an object id, else a 400 naming `what`.
pub(super) fn require_id<'a>(text: &'a str, what: &str) -> Result<&'a str, GitError> {
    if is_id(text) {
        Ok(text)
    } else {
        Err(GitError::bad(format!(
            "{what} must be a 40 character lowercase hex Git object id."
        )))
    }
}

/// A repository-relative path the page may put in a tree. It must be a plain
/// relative path with no empty, `.`, `..` or `.git` segment.
pub(super) fn require_repo_path(path: &str) -> Result<(), GitError> {
    let bad = |why: &str| {
        GitError::bad(format!(
            "The path \"{}\" is not allowed: {why}.",
            path.escape_debug()
        ))
    };
    if path.is_empty() {
        return Err(bad("it is empty"));
    }
    if path.contains('\0') || path.contains('\\') {
        return Err(bad("it contains a NUL or backslash"));
    }
    if path.starts_with('/') {
        return Err(bad("it is absolute"));
    }
    for segment in path.split('/') {
        if segment.is_empty() {
            return Err(bad("it has an empty segment"));
        }
        if segment == "." || segment == ".." {
            return Err(bad("it climbs or repeats a directory"));
        }
        if segment.eq_ignore_ascii_case(".git") {
            return Err(bad("it names a .git directory"));
        }
    }
    Ok(())
}

/// Whether `name` is a branch name Git would accept under `refs/heads/`,
/// and one that cannot be read as an option.
pub(super) fn valid_branch(name: &str) -> bool {
    if name.is_empty() || name.len() > 255 || name == "@" {
        return false;
    }
    if name.starts_with('-') || name.starts_with('/') || name.ends_with('/') || name.ends_with('.')
    {
        return false;
    }
    if name.contains("..") || name.contains("@{") || name.contains("//") {
        return false;
    }
    if name
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || "~^:?*[\\".contains(c))
    {
        return false;
    }
    name.split('/')
        .all(|part| !part.starts_with('.') && !part.ends_with(".lock"))
}

/// The remote URL as typed when it is one of the accepted forms, else a 400.
/// Accepted: `https://`, `ssh://`, and scp-style `user@host:path`. A URL
/// that carries a password is refused. A local remote is never a URL: it is
/// a granted folder.
pub(super) fn validate_url(raw: &str) -> Result<String, GitError> {
    let url = raw.trim();
    let refuse = |why: &str| GitError::bad(format!("That remote URL cannot be used: {why}."));
    if url.is_empty() || url.len() > 2048 {
        return Err(refuse("it is empty or too long"));
    }
    if url.starts_with('-') {
        return Err(refuse("it starts with a dash"));
    }
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(refuse("it contains whitespace or control characters"));
    }
    if url.contains("://") {
        let parsed = url::Url::parse(url).map_err(|_| refuse("it is not a valid URL"))?;
        if parsed.scheme() != "https" && parsed.scheme() != "ssh" {
            return Err(refuse(
                "only https://, ssh:// and user@host:path are accepted",
            ));
        }
        if parsed.password().is_some() {
            return Err(refuse(
                "it contains a password; use a credential helper or an SSH key instead",
            ));
        }
        match parsed.host_str() {
            Some(host) if !host.is_empty() && !host.starts_with('-') => {}
            _ => return Err(refuse("it has no usable host")),
        }
        return Ok(url.to_string());
    }
    let Some((user, rest)) = url.split_once('@') else {
        return Err(refuse(
            "only https://, ssh:// and user@host:path are accepted; a folder on this computer is chosen with the folder button",
        ));
    };
    let Some((host, path)) = rest.split_once(':') else {
        return Err(refuse("an SSH address needs the form user@host:path"));
    };
    let user_ok = !user.is_empty()
        && user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    let host_ok = !host.is_empty()
        && !host.starts_with('-')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'));
    if !user_ok || !host_ok || path.is_empty() {
        return Err(refuse("an SSH address needs the form user@host:path"));
    }
    Ok(url.to_string())
}

/// The URL with any user info removed, as shown to people and stored on the
/// server. `git@github.com:o/r.git` becomes `github.com:o/r.git`.
pub(super) fn label_for_url(url: &str) -> String {
    if url.contains("://") {
        if let Ok(mut parsed) = url::Url::parse(url) {
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            return parsed.to_string();
        }
    }
    match url.split_once('@') {
        Some((_, rest)) => rest.to_string(),
        None => url.to_string(),
    }
}

/// Git's version from the `git version` banner, and whether it is new
/// enough (2.38, the first with `merge-tree --write-tree`).
pub(super) fn parse_version(banner: &str) -> Option<(String, bool)> {
    let word = banner
        .trim()
        .strip_prefix("git version")?
        .split_whitespace()
        .next()?;
    let numbers: Vec<u32> = word
        .split('.')
        .map_while(|part| part.parse::<u32>().ok())
        .take(3)
        .collect();
    if numbers.len() < 2 {
        return None;
    }
    let text = numbers
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(".");
    Some((text, (numbers[0], numbers[1]) >= (2, 38)))
}

/// `text` with the user info of any URL removed and cut to a size fit for an
/// error message.
pub(super) fn redact(text: &str) -> String {
    static USER_INFO: OnceLock<regex::Regex> = OnceLock::new();
    let pattern = USER_INFO.get_or_init(|| {
        regex::Regex::new(r"(?i)([a-z][a-z0-9+.\-]*)://[^/\s]*@").expect("user info pattern")
    });
    let cleaned = pattern.replace_all(text, "${1}://");
    let cleaned = cleaned.trim();
    if cleaned.chars().count() <= 2000 {
        return cleaned.to_string();
    }
    let mut cut: String = cleaned.chars().take(2000).collect();
    cut.push_str("...");
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_forty_lowercase_hex() {
        assert!(is_id(&"a".repeat(40)));
        assert!(!is_id(&"A".repeat(40)));
        assert!(!is_id(&"a".repeat(39)));
        assert!(!is_id("HEAD"));
        assert!(is_sha256(&"0".repeat(64)));
        assert!(!is_sha256(&"0".repeat(40)));
    }

    #[test]
    fn repository_paths_are_plain_and_relative() {
        for good in [
            "a.txt",
            "dir/sub/a.tex",
            "dir.git/x",
            ".gitignore",
            ".github/x.yml",
        ] {
            assert!(require_repo_path(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "/abs",
            "../x",
            "a/../b",
            "a//b",
            "a/",
            ".git/config",
            "a/.GIT/x",
            "a\\b",
            "a\0b",
            "./a",
        ] {
            assert!(require_repo_path(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn branch_names_follow_git_rules() {
        for good in ["main", "feature/x-1", "v1.2"] {
            assert!(valid_branch(good), "{good}");
        }
        for bad in [
            "", "-x", "a b", "a..b", "a:b", "a~1", "a^", "a?", "a*", "a[", "a\\b", "x.lock",
            "a/.b", "/a", "a/", "a.", "@", "a@{b", "a//b",
        ] {
            assert!(!valid_branch(bad), "{bad}");
        }
    }

    #[test]
    fn urls_are_limited_to_three_forms_without_passwords() {
        for good in [
            "https://github.com/o/r.git",
            "https://token@github.com/o/r.git",
            "ssh://git@github.com/o/r.git",
            "git@github.com:o/r.git",
        ] {
            assert!(validate_url(good).is_ok(), "{good}");
        }
        for bad in [
            "https://user:secret@github.com/o/r.git",
            "ssh://user:secret@github.com/o/r.git",
            "http://github.com/o/r.git",
            "file:///tmp/r",
            "ext::sh -c id",
            "/tmp/repo",
            "C:\\repo",
            "-oProxyCommand=x",
            "git@-bad:o/r",
            "ssh://-oProxyCommand=x/r",
            "github.com:o/r",
            "https://a b/c",
            "",
        ] {
            assert!(validate_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn labels_drop_user_info() {
        assert_eq!(
            label_for_url("https://token@github.com/o/r.git"),
            "https://github.com/o/r.git"
        );
        assert_eq!(
            label_for_url("ssh://git@github.com/o/r.git"),
            "ssh://github.com/o/r.git"
        );
        assert_eq!(
            label_for_url("git@github.com:o/r.git"),
            "github.com:o/r.git"
        );
    }

    #[test]
    fn version_banners_parse() {
        assert_eq!(
            parse_version("git version 2.43.0\n"),
            Some(("2.43.0".into(), true))
        );
        assert_eq!(
            parse_version("git version 2.39.3 (Apple Git-146)"),
            Some(("2.39.3".into(), true))
        );
        assert_eq!(
            parse_version("git version 2.45.1.windows.1"),
            Some(("2.45.1".into(), true))
        );
        assert_eq!(
            parse_version("git version 2.37.9"),
            Some(("2.37.9".into(), false))
        );
        assert_eq!(
            parse_version("git version 1.9"),
            Some(("1.9".into(), false))
        );
        assert_eq!(parse_version("nonsense"), None);
    }

    #[test]
    fn redaction_removes_credentials() {
        let text = "fatal: unable to access 'https://user:p@ss@example.com/o/r.git/': 401";
        let cleaned = redact(text);
        assert!(!cleaned.contains("p@ss"), "{cleaned}");
        assert!(cleaned.contains("https://example.com/o/r.git/"));
    }
}
