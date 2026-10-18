// Removes the home folder and the OS user name from text that leaves the
// machine, such as a diagnostics report.

// Replaces the home folder with ~, then the user name with <user>.
pub fn scrub(text: &str, home: Option<&str>, user: Option<&str>) -> String {
    let mut text = text.to_string();
    if let Some(home) = home {
        let home = home.trim_end_matches(&['/', '\\'][..]);
        if home.len() >= 2 {
            text = text.replace(home, "~");
            if cfg!(windows) {
                let forward = home.replace('\\', "/");
                text = text.replace(&forward, "~");
            }
        }
    }
    if let Some(user) = user {
        if user.chars().count() >= 3 {
            text = replace_user(&text, user);
        }
    }
    text
}

// Scrubs the home folder and user name of this computer.
pub fn scrub_here(text: &str) -> String {
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE"));
    let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME"));
    scrub(text, home.ok().as_deref(), user.ok().as_deref())
}

// Replaces each whole word match of the user name, ignoring ASCII case. A
// match must not touch an ASCII letter or digit on either side.
fn replace_user(text: &str, user: &str) -> String {
    let bytes = text.as_bytes();
    let name = user.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut i = 0;
    while i + name.len() <= bytes.len() {
        let end = i + name.len();
        let whole = bytes[i..end].eq_ignore_ascii_case(name)
            && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
            && (end == bytes.len() || !bytes[end].is_ascii_alphanumeric());
        if whole {
            // The user name is valid UTF-8 and only ASCII letters fold, so both ends are char boundaries.
            out.push_str(&text[last..i]);
            out.push_str("<user>");
            last = end;
            i = end;
        } else {
            i += 1;
        }
    }
    out.push_str(&text[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::scrub;

    #[test]
    fn replaces_a_unix_home_with_a_tilde() {
        assert_eq!(
            scrub("/home/alice/notes", Some("/home/alice"), None),
            "~/notes"
        );
    }

    #[test]
    fn replaces_a_windows_home_with_a_tilde() {
        assert_eq!(
            scrub("C:\\Users\\alice\\AppData", Some("C:\\Users\\alice"), None),
            "~\\AppData"
        );
    }

    #[test]
    fn drops_a_trailing_separator_from_home() {
        assert_eq!(
            scrub("/home/alice/notes", Some("/home/alice/"), None),
            "~/notes"
        );
    }

    #[cfg(windows)]
    #[test]
    fn replaces_a_forward_slash_home_on_windows() {
        assert_eq!(
            scrub("C:/Users/alice/AppData", Some("C:\\Users\\alice"), None),
            "~/AppData"
        );
    }

    #[test]
    fn replaces_a_bare_user_name() {
        assert_eq!(
            scrub("owner alice here", None, Some("alice")),
            "owner <user> here"
        );
        assert_eq!(scrub("alice", None, Some("alice")), "<user>");
    }

    #[test]
    fn replaces_the_user_name_in_a_path_segment() {
        assert_eq!(
            scrub("/data/alice/x", None, Some("alice")),
            "/data/<user>/x"
        );
    }

    #[test]
    fn leaves_the_user_name_inside_a_word_alone() {
        assert_eq!(
            scrub("malice alice2 Alice", None, Some("alice")),
            "malice alice2 <user>"
        );
    }

    #[test]
    fn leaves_text_alone_without_a_usable_home_or_user() {
        assert_eq!(scrub("/home/alice", Some(""), None), "/home/alice");
        assert_eq!(scrub("/home/alice", Some("/"), None), "/home/alice");
        assert_eq!(scrub("ab ab", None, Some("ab")), "ab ab");
        assert_eq!(scrub("ab ab", None, Some("")), "ab ab");
    }
}
