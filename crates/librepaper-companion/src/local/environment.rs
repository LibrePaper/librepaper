//! Resolve a useful PATH before the companion starts its async runtime.
//!
//! Finder, LaunchAgents and Linux desktop launchers do not read shell startup
//! files. Resolve the login-shell PATH once, synchronously, then let children
//! inherit it.

#[cfg(any(target_os = "macos", target_os = "linux", all(test, unix)))]
use std::ffi::{OsStr, OsString};
#[cfg(any(target_os = "macos", target_os = "linux", all(test, unix)))]
use std::path::{Path, PathBuf};

#[cfg(any(target_os = "macos", target_os = "linux", all(unix, test)))]
const MAX_SHELL_OUTPUT: usize = 64 * 1024;

/// Initialize the process PATH on macOS and Linux. This must run before threads start.
///
/// The login shell runs at most once a day, here at startup, before any thread
/// exists. Its PATH is cached in `state/librepaper/local/login-path`; a start
/// within 24 hours of that file's last write uses the cache without running the
/// shell. Otherwise the shell is run synchronously and the cache is rewritten. A
/// tool installed since the last run is found by the next day's start, or at
/// once after deleting the cache file.
///
/// # Safety
/// Call only during synchronous startup before Tokio or any other threads are
/// started. Changing the process environment while other threads may read it
/// is unsafe, and `getpwuid` in the login-shell lookup is not thread safe.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub unsafe fn initialize() {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let cache = cache_file();
    let shell = match cache.as_deref().and_then(read_cache) {
        Some(path) => Some(path),
        None => {
            let fresh = shell_path();
            if let (Some(file), Some(fresh)) = (cache.as_deref(), fresh.as_deref()) {
                write_cache(file, fresh);
            }
            fresh
        }
    };
    let dirs = resolve_path(&inherited, home_directory(), shell);
    if let Ok(path) = std::env::join_paths(dirs) {
        unsafe { std::env::set_var("PATH", path) };
    }
}

/// The PATH initializer is a no-op outside macOS and Linux.
///
/// # Safety
/// Call only during synchronous startup before any threads are started.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub unsafe fn initialize() {}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

// The login shell's PATH from its last run, rewritten at most once a day.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn cache_file() -> Option<PathBuf> {
    crate::local::paths::state_home()
        .ok()
        .map(|home| home.join("librepaper/local/login-path"))
}

/// The cached PATH, or None when the file is missing, empty, or its last write
/// is 24 hours old or more (or its age cannot be read, or lies in the future).
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn read_cache(path: &Path) -> Option<OsString> {
    use std::os::unix::ffi::OsStringExt;
    let age = std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .elapsed()
        .ok()?;
    if age >= std::time::Duration::from_secs(24 * 60 * 60) {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    (!bytes.is_empty()).then(|| OsString::from_vec(bytes))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn write_cache(path: &Path, value: &OsStr) {
    use std::os::unix::ffi::OsStrExt;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Written aside and renamed, so a start that reads it meanwhile never sees half a PATH.
    let partial = path.with_extension(format!("{}.partial", std::process::id()));
    if std::fs::write(&partial, value.as_bytes()).is_ok()
        && std::fs::rename(&partial, path).is_err()
    {
        let _ = std::fs::remove_file(&partial);
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn shell_path() -> Option<OsString> {
    let configured = std::env::var_os("SHELL");
    let account = account_login_shell();
    let shell = select_shell(
        configured.as_deref(),
        account.as_deref(),
        &[
            Path::new("/bin/zsh"),
            Path::new("/bin/bash"),
            Path::new("/bin/sh"),
        ],
    )?;
    login_shell_path(&shell, std::time::Duration::from_secs(2))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn account_login_shell() -> Option<OsString> {
    // SAFETY: getpwuid returns a process-global record; copy its shell before
    // making any other libc calls. Initialization is synchronous and single-threaded.
    unsafe {
        let record = libc::getpwuid(libc::getuid());
        if record.is_null() || (*record).pw_shell.is_null() {
            return None;
        }
        Some(OsString::from(
            std::ffi::CStr::from_ptr((*record).pw_shell)
                .to_string_lossy()
                .into_owned(),
        ))
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", all(test, unix)))]
fn select_shell(
    configured: Option<&OsStr>,
    account: Option<&OsStr>,
    fallbacks: &[&Path],
) -> Option<PathBuf> {
    configured
        .into_iter()
        .chain(account)
        .map(PathBuf::from)
        .chain(fallbacks.iter().map(|path| path.to_path_buf()))
        .find(|path| path.is_absolute() && path.is_file())
}

/// Where tools usually live when no PATH names them. Homebrew and MacTeX
/// folders exist only on macOS.
#[cfg(any(target_os = "macos", target_os = "linux", all(test, unix)))]
const FALLBACK_DIRS: &[&str] = if cfg!(target_os = "macos") {
    &[
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/Library/TeX/texbin",
        "/usr/bin",
        "/bin",
        "/usr/sbin",
        "/sbin",
    ]
} else {
    &["/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"]
};

/// Merge PATH sources while preserving inherited precedence and dropping
/// relative entries, which would otherwise make discovery depend on CWD.
#[cfg(any(target_os = "macos", target_os = "linux", all(test, unix)))]
fn resolve_path(inherited: &OsStr, home: Option<PathBuf>, shell: Option<OsString>) -> Vec<PathBuf> {
    let mut dirs = std::env::split_paths(inherited)
        .filter(|dir| dir.is_absolute())
        .collect::<Vec<_>>();
    if let Some(shell) = shell {
        dirs.extend(std::env::split_paths(&shell).filter(|dir| dir.is_absolute()));
    }
    if let Some(home) = home.filter(|home| home.is_absolute()) {
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".cargo/bin"));
    }
    dirs.extend(path_file_entries(Path::new("/etc/paths")));
    if let Ok(entries) = std::fs::read_dir("/etc/paths.d") {
        let mut files = entries
            .flatten()
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        files.sort();
        for file in files {
            dirs.extend(path_file_entries(&file));
        }
    }
    dirs.extend(FALLBACK_DIRS.iter().map(PathBuf::from));

    let mut unique = Vec::with_capacity(dirs.len());
    for dir in dirs {
        if !unique.contains(&dir) {
            unique.push(dir);
        }
    }
    unique
}

#[cfg(any(target_os = "macos", target_os = "linux", all(test, unix)))]
fn path_file_entries(path: &Path) -> Vec<PathBuf> {
    use std::io::Read;

    const MAX_PATH_FILE_BYTES: usize = 16 * 1024;
    if !std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file()) {
        return Vec::new();
    }
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let mut bytes = Vec::new();
    if file
        .take((MAX_PATH_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > MAX_PATH_FILE_BYTES
    {
        return Vec::new();
    }
    String::from_utf8(bytes)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && line.starts_with('/'))
        .map(PathBuf::from)
        .collect()
}

/// Run only a fixed PATH-printing snippet in the user's interactive login
/// shell. stdout is read without blocking and retained up to 64 KiB. On
/// timeout the shell process group is killed before the child is reaped.
#[cfg(any(target_os = "macos", target_os = "linux", all(unix, test)))]
fn login_shell_path(shell: &Path, timeout: std::time::Duration) -> Option<OsString> {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::Instant;

    let marker = format!(
        "__LIBREPAPER_PATH_{}_{}__",
        std::process::id(),
        unique_marker()
    );
    let script = format!("printf '\\n{marker}_BEGIN\\n%s\\n{marker}_END\\n' \"$PATH\"");
    let mut child = Command::new(shell)
        .args(["-l", "-i", "-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .ok()?;
    let stdout = child.stdout.as_ref()?;
    let fd = stdout.as_raw_fd();
    // SAFETY: fcntl only reads/updates flags on this live pipe descriptor.
    let old_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if old_flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, old_flags | libc::O_NONBLOCK) } < 0
    {
        let pid = child.id();
        crate::local::native::kill_tree(Some(pid));
        let _ = child.wait();
        return None;
    }

    let pid = child.id();
    let deadline = Instant::now() + timeout;
    let mut output = Vec::with_capacity(MAX_SHELL_OUTPUT);
    let mut chunk = [0u8; 4096];
    let status = loop {
        let count = match read_nonblocking(fd, &mut chunk) {
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => 0,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => {
                crate::local::native::kill_tree(Some(pid));
                let _ = child.wait();
                return None;
            }
        };
        if count > 0 {
            let remaining = MAX_SHELL_OUTPUT.saturating_sub(output.len());
            if count > remaining {
                crate::local::native::kill_tree(Some(pid));
                let _ = child.wait();
                return None;
            }
            output.extend_from_slice(&chunk[..count]);
        }

        match child.try_wait() {
            Ok(Some(exit)) => {
                // Drain bytes already in the pipe, but never wait for EOF:
                // a shell profile may leave a detached descendant holding it.
                loop {
                    if Instant::now() >= deadline {
                        return None;
                    }
                    let count = match read_nonblocking(fd, &mut chunk) {
                        Ok(0) => break,
                        Ok(count) => count,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => return None,
                    };
                    let remaining = MAX_SHELL_OUTPUT.saturating_sub(output.len());
                    if count > remaining {
                        return None;
                    }
                    output.extend_from_slice(&chunk[..count]);
                }
                break exit;
            }
            Ok(None) => {}
            Err(_) => {
                crate::local::native::kill_tree(Some(pid));
                let _ = child.wait();
                return None;
            }
        }
        if Instant::now() >= deadline {
            crate::local::native::kill_tree(Some(pid));
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };

    if !status.success() {
        return None;
    }
    parse_marked_path(&output, &marker)
}

#[cfg(any(target_os = "macos", target_os = "linux", all(unix, test)))]
fn read_nonblocking(fd: std::os::fd::RawFd, buffer: &mut [u8]) -> std::io::Result<usize> {
    // SAFETY: caller keeps the descriptor open and buffer points to writable memory.
    let count = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
    if count >= 0 {
        Ok(count as usize)
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", all(unix, test)))]
fn unique_marker() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

#[cfg(any(target_os = "macos", target_os = "linux", all(test, unix)))]
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn inherited_entries_keep_precedence_absolute_paths_only_and_deduplicate() {
        let dirs = resolve_path(
            OsStr::new("relative:/custom/bin:/usr/local/bin:/custom/bin"),
            Some(PathBuf::from("/Users/example")),
            Some(OsString::from("/shell/bin:/usr/local/bin")),
        );
        assert_eq!(dirs[0], PathBuf::from("/custom/bin"));
        assert_eq!(dirs[1], PathBuf::from("/usr/local/bin"));
        assert_eq!(dirs[2], PathBuf::from("/shell/bin"));
        assert_eq!(dirs[3], PathBuf::from("/Users/example/.local/bin"));
        assert_eq!(dirs[4], PathBuf::from("/Users/example/.cargo/bin"));
        assert_eq!(
            dirs.iter()
                .filter(|path| **path == Path::new("/usr/local/bin"))
                .count(),
            1
        );
        assert!(!dirs.contains(&PathBuf::from("relative")));
    }

    #[test]
    fn sparse_path_gets_deterministic_fallbacks() {
        let dirs = resolve_path(OsStr::new("/usr/bin:/bin"), None, None);
        for expected in FALLBACK_DIRS {
            assert!(dirs.contains(&PathBuf::from(expected)));
        }
        assert_eq!(
            dirs.contains(&PathBuf::from("/opt/homebrew/bin")),
            cfg!(target_os = "macos")
        );
    }

    #[test]
    fn absent_shell_uses_account_shell_then_fallback() {
        assert_eq!(
            select_shell(None, Some(OsStr::new("/bin/sh")), &[Path::new("/bin/bash")]),
            Some(PathBuf::from("/bin/sh"))
        );
        assert_eq!(
            select_shell(None, None, &[Path::new("/bin/sh")]),
            Some(PathBuf::from("/bin/sh"))
        );
    }

    #[test]
    fn parser_handles_noisy_shell_output_and_rejects_incomplete_markers() {
        let marker = "__LIBREPAPER_PATH_test__";
        let output = format!(
            "profile noise\n{marker}_BEGIN\nwrong\n{marker}_END\n{marker}_BEGIN\n/opt/homebrew/bin:/custom/bin\n{marker}_END\n"
        );
        assert_eq!(
            parse_marked_path(output.as_bytes(), marker),
            Some(OsString::from("/opt/homebrew/bin:/custom/bin"))
        );
        assert_eq!(parse_marked_path(b"no marker", marker), None);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn cached_path_round_trips_and_empty_cache_reads_as_none() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("nested/login-path");
        assert_eq!(read_cache(&file), None);

        write_cache(&file, OsStr::new("/opt/bin:/usr/bin"));
        assert_eq!(read_cache(&file), Some(OsString::from("/opt/bin:/usr/bin")));

        std::fs::write(&file, b"").unwrap();
        assert_eq!(read_cache(&file), None);

        write_cache(&file, OsStr::new("/opt/bin:/usr/bin"));
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 86400))
            .unwrap();
        assert_eq!(read_cache(&file), None);
    }

    #[cfg(unix)]
    fn fake_shell(directory: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = directory.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn login_shell_lookup_accepts_noisy_success_and_rejects_failure() {
        let directory = tempfile::tempdir().unwrap();
        let noisy = fake_shell(directory.path(), "noisy", "echo profile-noise; eval \"$4\"");
        assert!(login_shell_path(&noisy, std::time::Duration::from_secs(1)).is_some());
        let failing = fake_shell(directory.path(), "failing", "exit 3");
        assert_eq!(
            login_shell_path(&failing, std::time::Duration::from_secs(1)),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn login_shell_lookup_caps_output_and_times_out_without_waiting_for_pipe_eof() {
        let directory = tempfile::tempdir().unwrap();
        let oversized = fake_shell(
            directory.path(),
            "oversized",
            "head -c 70000 /dev/zero; eval \"$4\"",
        );
        assert_eq!(
            login_shell_path(&oversized, std::time::Duration::from_secs(1)),
            None
        );

        let hanging = fake_shell(directory.path(), "hanging", "sleep 10 & wait");
        assert_eq!(
            login_shell_path(&hanging, std::time::Duration::from_millis(100)),
            None
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn login_shell_lookup_does_not_wait_for_detached_stdout_descendants() {
        let directory = tempfile::tempdir().unwrap();
        let detached = fake_shell(
            directory.path(),
            "detached",
            "setsid sleep 3 & eval \"$4\"; exit 0",
        );
        let started = std::time::Instant::now();
        assert!(login_shell_path(&detached, std::time::Duration::from_secs(2)).is_some());
        assert!(started.elapsed() < std::time::Duration::from_millis(1500));
    }
}
