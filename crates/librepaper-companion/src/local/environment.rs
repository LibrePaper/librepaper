//! Resolve a useful PATH before the companion starts its async runtime.
//!
//! Finder and LaunchAgents do not read the user's interactive shell setup.
//! Resolve that environment once, at synchronous process startup, then let
//! every child inherit the same PATH.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// Initialize the process PATH on macOS. This must run before Tokio or any
/// other threads are started because changing the process environment later
/// is unsafe when other threads may read it.
#[cfg(target_os = "macos")]
/// # Safety
/// Call only during synchronous process startup, before Tokio or any other
/// threads are started. Mutating the process environment while other threads
/// can read it is unsafe.
pub unsafe fn initialize() {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let dirs = resolve_path(&inherited, home_directory(), shell_path());
    if let Ok(path) = std::env::join_paths(dirs) {
        // Called synchronously before the runtime and its worker threads.
        unsafe { std::env::set_var("PATH", path) };
    }
}

/// Other platforms already receive PATH from their normal launch context.
#[cfg(not(target_os = "macos"))]
/// # Safety
/// Call only during synchronous process startup, before any threads are
/// started. This implementation is a no-op outside macOS.
pub unsafe fn initialize() {}

#[cfg(target_os = "macos")]
fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|home| !home.is_empty()).map(PathBuf::from)
}

#[cfg(target_os = "macos")]
fn shell_path() -> Option<OsString> {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let dirs = std::env::split_paths(&inherited).collect::<Vec<_>>();
    if has_user_tool_path(&dirs) {
        return None;
    }
    let shell = std::env::var_os("SHELL").filter(|value| !value.is_empty())?;
    let shell = PathBuf::from(shell);
    let shell = if shell.is_absolute() {
        shell
    } else {
        which::which(&shell).ok()?
    };
    shell.is_file().then(|| login_shell_path(&shell)).flatten()
}

fn has_user_tool_path(dirs: &[PathBuf]) -> bool {
    dirs.iter().any(|dir| {
        matches!(
            dir.to_string_lossy().as_ref(),
            "/opt/homebrew/bin" | "/usr/local/bin"
        ) || dir.ends_with(".local/bin")
            || dir.ends_with(".cargo/bin")
            || dir.ends_with("/texbin")
    })
}

/// Build a stable search path. Existing PATH entries remain first so explicit
/// user choices keep precedence; shell and system path-file entries follow.
fn resolve_path(inherited: &OsStr, home: Option<PathBuf>, shell: Option<OsString>) -> Vec<PathBuf> {
    let mut dirs = std::env::split_paths(inherited).collect::<Vec<_>>();
    if let Some(shell) = shell {
        dirs.extend(std::env::split_paths(&shell));
    }
    if let Some(home) = home {
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".cargo/bin"));
    }
    dirs.extend(path_file_entries(Path::new("/etc/paths")));
    if let Ok(entries) = std::fs::read_dir("/etc/paths.d") {
        let mut files = entries.flatten().map(|entry| entry.path()).collect::<Vec<_>>();
        files.sort();
        for file in files {
            dirs.extend(path_file_entries(&file));
        }
    }
    dirs.extend(
        [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/Library/TeX/texbin",
            "/usr/bin",
            "/bin",
            "/usr/sbin",
            "/sbin",
        ]
        .into_iter()
        .map(PathBuf::from),
    );

    let mut unique = Vec::with_capacity(dirs.len());
    for dir in dirs {
        if !dir.as_os_str().is_empty() && !unique.contains(&dir) {
            unique.push(dir);
        }
    }
    unique
}

fn path_file_entries(path: &Path) -> Vec<PathBuf> {
    if std::fs::metadata(path).is_ok_and(|metadata| metadata.len() > 16 * 1024) {
        return Vec::new();
    }
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && line.starts_with('/'))
        .map(PathBuf::from)
        .collect()
}

#[cfg(target_os = "macos")]
fn login_shell_path(shell: &Path) -> Option<OsString> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    let marker = format!("__LIBREPAPER_PATH_{}_{}__", std::process::id(), unique_marker());
    let script = format!("printf '\\n{marker}_BEGIN\\n%s\\n{marker}_END\\n' \"$PATH\"");
    let mut command = Command::new(shell);
    command
        .args(["-l", "-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let Ok(count) = stdout.read(&mut chunk) else { break };
            if count == 0 {
                break;
            }
            if bytes.len() < MAX_SHELL_OUTPUT {
                let remaining = MAX_SHELL_OUTPUT - bytes.len();
                bytes.extend_from_slice(&chunk[..count.min(remaining)]);
            }
        }
        bytes
    });

    let deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        if let Ok(Some(status)) = child.try_wait() {
            break Some(status);
        }
        if Instant::now() >= deadline {
            crate::local::native::kill_tree(child.id());
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return None;
        }
        thread::sleep(Duration::from_millis(10));
    };
    crate::local::native::kill_tree(child.id());
    if !status.is_some_and(|status| status.success()) {
        let _ = reader.join();
        return None;
    }
    let output = reader.join().ok()?;
    parse_marked_path(&output, &marker)
}

#[cfg(target_os = "macos")]
const MAX_SHELL_OUTPUT: usize = 64 * 1024;

#[cfg(target_os = "macos")]
fn unique_marker() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

fn parse_marked_path(output: &[u8], marker: &str) -> Option<OsString> {
    let text = std::str::from_utf8(output).ok()?;
    let begin = format!("{marker}_BEGIN");
    let end = format!("{marker}_END");
    let start = text.rfind(&begin)?.checked_add(begin.len())?;
    let remainder = text.get(start..)?.trim_start_matches(['\r', '\n']);
    let finish = remainder.find(&end)?;
    let value = remainder[..finish].trim_end_matches(['\r', '\n']);
    if value.is_empty() {
        None
    } else {
        Some(OsString::from(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inherited_entries_keep_precedence_and_duplicates_are_removed() {
        let dirs = resolve_path(
            OsStr::new("/custom/bin:/usr/local/bin:/custom/bin"),
            Some(PathBuf::from("/Users/example")),
            Some(OsString::from("/shell/bin:/usr/local/bin")),
        );
        assert_eq!(dirs[0], PathBuf::from("/custom/bin"));
        assert_eq!(dirs[1], PathBuf::from("/usr/local/bin"));
        assert_eq!(dirs[2], PathBuf::from("/shell/bin"));
        assert_eq!(dirs[3], PathBuf::from("/Users/example/.local/bin"));
        assert_eq!(dirs[4], PathBuf::from("/Users/example/.cargo/bin"));
        assert_eq!(dirs.iter().filter(|path| **path == Path::new("/usr/local/bin")).count(), 1);
    }

    #[test]
    fn sparse_path_gets_deterministic_macos_fallbacks() {
        let dirs = resolve_path(OsStr::new("/usr/bin:/bin"), None, None);
        for expected in [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/Library/TeX/texbin",
            "/usr/bin",
            "/bin",
        ] {
            assert!(dirs.contains(&PathBuf::from(expected)));
        }
    }

    #[test]
    fn parser_ignores_noisy_shell_output_and_uses_last_complete_marker() {
        let marker = "__LIBREPAPER_PATH_test__";
        let output = format!(
            "profile noise\n{marker}_BEGIN\nwrong\n{marker}_END\n{marker}_BEGIN\n/opt/homebrew/bin:/custom/bin\n{marker}_END\n"
        );
        assert_eq!(
            parse_marked_path(output.as_bytes(), marker),
            Some(OsString::from("/opt/homebrew/bin:/custom/bin"))
        );
    }

    #[test]
    fn incomplete_or_empty_markers_are_rejected() {
        let marker = "__LIBREPAPER_PATH_test__";
        assert_eq!(parse_marked_path(b"no marker", marker), None);
        let empty = format!("{marker}_BEGIN\n\n{marker}_END");
        assert_eq!(parse_marked_path(empty.as_bytes(), marker), None);
    }
}
