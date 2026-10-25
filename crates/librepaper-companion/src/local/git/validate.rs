//! Checks on everything a page hands the Git routes before it reaches a Git
//! command line: branch names, remote names and URLs, the name a clone gets,
//! and the identity written into a repository. Also the redaction applied to
//! Git's own output before it is returned.

/// Git's version from the `git version` banner, and whether it is new enough
/// (2.38).
pub(crate) fn parse_version(banner: &str) -> Option<(String, bool)> {
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

/// Whether `name` is a branch name Git would accept under `refs/heads/`,
/// and one that cannot be read as an option.
pub(crate) fn valid_branch(name: &str) -> bool {
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

/// Whether `name` can name a remote: plain letters, digits, dots, dashes and
/// underscores, never leading with a dash or a dot.
pub(crate) fn valid_remote_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && !name.starts_with('.')
        && !name.ends_with('.')
        && !name.ends_with(".lock")
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The remote URL as typed when it is one of the accepted forms, else the
/// reason it is refused. Accepted: `https://`, `ssh://`, and scp-style
/// `user@host:path`. A URL that carries a password is refused. A folder on
/// this computer is never a URL.
pub(crate) fn validate_url(raw: &str) -> Result<String, String> {
    let url = raw.trim();
    let refuse = |why: &str| format!("That URL cannot be used: {why}.");
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
            "only https://, ssh:// and user@host:path are accepted",
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

/// `url` as the panel shows a remote. An https URL loses its user info, since
/// a token often sits there; any other URL loses only a password.
pub(crate) fn display_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let Some((info, host)) = authority.rsplit_once('@') else {
        return url.to_string();
    };
    let keep = if scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http") {
        String::new()
    } else {
        let user = info.split(':').next().unwrap_or_default();
        format!("{user}@")
    };
    format!("{scheme}://{keep}{host}{tail}")
}

/// The folder name a clone of `url` gets: the last part of the address
/// without a trailing `.git`. `None` when that is not a usable folder name.
pub(crate) fn clone_name(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/');
    let last = trimmed.rsplit(['/', ':']).next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    let usable = !name.is_empty()
        && name.len() <= 100
        && !name.starts_with('-')
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'));
    usable.then(|| name.to_string())
}

/// Whether `text` can be a name or an email written into a repository's
/// configuration: not empty, short, one line, no angle brackets (Git strips
/// them from an identity) and not readable as an option.
pub(crate) fn valid_identity_field(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 200
        && !text.starts_with('-')
        && !text.chars().any(|c| c.is_control() || c == '<' || c == '>')
}

/// `text` with the user info of any URL removed and cut to a size fit for an
/// error message.
pub(crate) fn redact(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let (head, tail) = rest.split_at(at + 3);
        cleaned.push_str(head);
        let end = tail
            .find(|c: char| c == '/' || c.is_whitespace())
            .unwrap_or(tail.len());
        let (authority, after) = tail.split_at(end);
        let host = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        cleaned.push_str(host);
        rest = after;
    }
    cleaned.push_str(rest);
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
            parse_version("git version 2.38.0"),
            Some(("2.38.0".into(), true))
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
    fn remote_names_are_plain() {
        for good in ["origin", "up-stream", "fork_2", "a.b"] {
            assert!(valid_remote_name(good), "{good}");
        }
        for bad in [
            "", "-x", ".x", "a b", "a/b", "a..b", "x.lock", "a:", "o'r", "a.",
        ] {
            assert!(!valid_remote_name(bad), "{bad}");
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
            "git://github.com/o/r.git",
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
    fn a_shown_url_loses_what_is_secret() {
        assert_eq!(
            display_url("https://token@github.com/o/r.git"),
            "https://github.com/o/r.git"
        );
        assert_eq!(
            display_url("https://user:secret@github.com/o/r.git"),
            "https://github.com/o/r.git"
        );
        assert_eq!(
            display_url("ssh://git:secret@github.com/o/r.git"),
            "ssh://git@github.com/o/r.git"
        );
        assert_eq!(
            display_url("ssh://git@github.com/o/r.git"),
            "ssh://git@github.com/o/r.git"
        );
        assert_eq!(
            display_url("git@github.com:o/r.git"),
            "git@github.com:o/r.git"
        );
        assert_eq!(display_url("/tmp/remote.git"), "/tmp/remote.git");
    }

    #[test]
    fn a_clone_is_named_for_the_last_part_of_the_address() {
        for (url, name) in [
            ("https://github.com/o/paper.git", "paper"),
            ("https://github.com/o/paper", "paper"),
            ("https://github.com/o/paper.git/", "paper"),
            ("git@github.com:o/my-paper.git", "my-paper"),
            ("git@github.com:paper.git", "paper"),
            ("ssh://git@host:22/o/p_1.git", "p_1"),
        ] {
            assert_eq!(clone_name(url).as_deref(), Some(name), "{url}");
        }
        for url in ["https://github.com/o/.git", "https://github.com/o/..", ""] {
            assert_eq!(clone_name(url), None, "{url}");
        }
    }

    #[test]
    fn an_identity_is_one_plain_line() {
        assert!(valid_identity_field("Ada Lovelace"));
        assert!(valid_identity_field("ada@example.org"));
        for bad in ["", "-x", "a\nb", "Ada <ada@example.org>"] {
            assert!(!valid_identity_field(bad), "{bad:?}");
        }
        assert!(!valid_identity_field(&"a".repeat(201)));
    }

    #[test]
    fn redaction_removes_credentials() {
        let text = "fatal: unable to access 'https://user:p@ss@example.com/o/r.git/': 401";
        let cleaned = redact(text);
        assert!(!cleaned.contains("p@ss"), "{cleaned}");
        assert!(cleaned.contains("https://example.com/o/r.git/"));
        assert_eq!(redact("no urls here\n"), "no urls here");
        let long = "x".repeat(3000);
        assert!(redact(&long).ends_with("..."));
    }
}
