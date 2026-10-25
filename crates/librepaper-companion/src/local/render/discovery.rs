//! Finding the program a render command starts, and asking it its version.
//!
//! `argv[0]` is an absolute path or a program name looked up on the process
//! PATH, which the service already replaced with the cached login-shell PATH
//! when it started. On Windows the lookup honours `PATHEXT`. Nothing here
//! installs anything or sends a path to the browser.
//!
//! Results are cached under `<state home>/librepaper/local/tools.json` and
//! reused until an explicit rescan, a changed PATH, or a cached program's
//! executable changing on disk.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// How long a `--version` probe is allowed to run. A hung or misbehaving
/// binary must not hang discovery.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// What is known about one program: where it resolves, and the first line of
/// its `--version`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Found {
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub version: Option<String>,
    /// The executable's modification time when it was probed, for invalidation.
    #[serde(default)]
    mtime: Option<i64>,
}

impl Found {
    pub fn found(&self) -> bool {
        self.path.is_some()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Cache {
    #[serde(default)]
    path_fingerprint: String,
    #[serde(default)]
    programs: BTreeMap<String, Found>,
}

fn cache_path() -> PathBuf {
    crate::local::paths::state_home_or_die()
        .join("librepaper")
        .join("local")
        .join("tools.json")
}

fn path_fingerprint(raw: &OsStr) -> String {
    format!("path-v3:{:?}", raw)
}

/// The executable `argv0` names: an absolute path that is an executable file,
/// or a bare program name found in one of the directories of `path`. A
/// relative name with a separator in it resolves to nothing, because there is
/// no directory it could be relative to.
pub fn resolve_in(argv0: &str, path: &OsStr) -> Option<PathBuf> {
    let has_separator = argv0.contains('/') || (cfg!(windows) && argv0.contains('\\'));
    if has_separator && !Path::new(argv0).is_absolute() {
        return None;
    }
    which::which_in(
        argv0,
        Some(path),
        std::env::current_dir().unwrap_or_default(),
    )
    .ok()
}

/// The executable `argv0` names on the process PATH.
pub fn resolve(argv0: &str) -> Option<PathBuf> {
    resolve_in(argv0, &std::env::var_os("PATH").unwrap_or_default())
}

fn mtime_of(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    i64::try_from(modified.duration_since(UNIX_EPOCH).ok()?.as_secs()).ok()
}

/// Whether a cached answer can no longer be trusted: the executable it found
/// changed on disk, or its modification time was never known. A program that
/// was missing stays missing until a rescan or a changed PATH.
fn is_stale(cached: &Found) -> bool {
    match (&cached.path, cached.mtime) {
        (Some(path), Some(mtime)) => mtime_of(path) != Some(mtime),
        (Some(_), None) => true,
        (None, _) => false,
    }
}

async fn probe_program(argv0: &str, path: &OsStr) -> Found {
    let Some(resolved) = resolve_in(argv0, path) else {
        return Found::default();
    };
    // Isolated, because this decides what is installed and a tool that reads
    // its configuration from the ambient environment would make the answer
    // depend on who started the service.
    let version = super::super::tools::version_line(
        &resolved,
        super::super::tools::Probe::isolated(PROBE_TIMEOUT),
    )
    .await;
    Found {
        mtime: mtime_of(&resolved),
        path: Some(resolved),
        version,
    }
}

fn load_cache() -> Option<Cache> {
    let text = std::fs::read_to_string(cache_path()).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_cache(cache: &Cache) {
    if let Ok(text) = serde_json::to_string_pretty(cache) {
        let _ =
            librepaper_base::private_files::publish(&cache_path(), text.as_bytes(), "tool cache");
    }
}

/// Where each of `programs` resolves and its version, using the cache unless
/// `refresh` is set, the PATH changed, or a cached executable changed.
pub async fn report(programs: &[String], refresh: bool) -> BTreeMap<String, Found> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let fingerprint = path_fingerprint(&path);
    let cached = if refresh {
        Cache::default()
    } else {
        load_cache()
            .filter(|cache| cache.path_fingerprint == fingerprint)
            .unwrap_or_default()
    };
    let mut answers = BTreeMap::new();
    let mut changed = refresh;
    for program in programs {
        if answers.contains_key(program) {
            continue;
        }
        let found = match cached.programs.get(program) {
            Some(found) if !is_stale(found) => found.clone(),
            _ => {
                changed = true;
                probe_program(program, &path).await
            }
        };
        answers.insert(program.clone(), found);
    }
    if changed || cached.programs.len() != answers.len() {
        save_cache(&Cache {
            path_fingerprint: fingerprint,
            programs: answers.clone(),
        });
    }
    answers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn executable(directory: &Path, name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = directory.join(name);
        std::fs::write(&path, "#!/bin/sh\necho tool 1.2\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn a_bare_name_is_found_on_the_given_path_only() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let tool = executable(second.path(), "mytool");
        let path = std::env::join_paths([first.path(), second.path()]).unwrap();
        assert_eq!(resolve_in("mytool", &path), Some(tool));
        assert_eq!(resolve_in("absent-tool", &path), None);
        assert_eq!(
            resolve_in("mytool", &std::env::join_paths([first.path()]).unwrap()),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_absolute_path_resolves_and_a_relative_path_with_a_separator_does_not() {
        let directory = tempfile::tempdir().unwrap();
        let tool = executable(directory.path(), "mytool");
        let path = std::env::join_paths([directory.path()]).unwrap();
        assert_eq!(
            resolve_in(tool.to_str().unwrap(), OsStr::new("")),
            Some(tool)
        );
        assert_eq!(resolve_in("./mytool", &path), None);
        assert_eq!(resolve_in("bin/mytool", &path), None);
        assert_eq!(resolve_in("/no/such/tool", &path), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_that_is_not_executable_is_not_a_program() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("notes"), "text").unwrap();
        let path = std::env::join_paths([directory.path()]).unwrap();
        assert_eq!(resolve_in("notes", &path), None);
    }

    #[cfg(windows)]
    #[test]
    fn a_bare_name_resolves_through_pathext() {
        let directory = tempfile::tempdir().unwrap();
        let tool = directory.path().join("mytool.exe");
        std::fs::write(&tool, b"").unwrap();
        let path = std::env::join_paths([directory.path()]).unwrap();
        assert_eq!(
            resolve_in("mytool", &path).map(|found| found.is_file()),
            Some(true)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_probe_reads_the_first_line_of_the_version() {
        let directory = tempfile::tempdir().unwrap();
        executable(directory.path(), "mytool");
        let path = std::env::join_paths([directory.path()]).unwrap();
        let found = probe_program("mytool", &path).await;
        assert!(found.found());
        assert_eq!(found.version.as_deref(), Some("tool 1.2"));
        let missing = probe_program("absent-tool", &path).await;
        assert!(!missing.found());
        assert_eq!(missing.version, None);
    }

    #[cfg(unix)]
    #[test]
    fn a_changed_executable_is_stale_and_a_missing_program_is_not() {
        let directory = tempfile::tempdir().unwrap();
        let tool = executable(directory.path(), "mytool");
        let mut found = Found {
            path: Some(tool),
            version: None,
            mtime: Some(1),
        };
        assert!(is_stale(&found), "the recorded time is not the file's");
        found.mtime = None;
        assert!(
            is_stale(&found),
            "no recorded time means the cache cannot vouch for it"
        );
        found.mtime = mtime_of(found.path.as_ref().unwrap());
        assert!(!is_stale(&found));
        assert!(!is_stale(&Found::default()));
    }

    #[test]
    fn the_fingerprint_changes_when_the_path_changes() {
        assert_ne!(
            path_fingerprint(OsStr::new("/first/bin")),
            path_fingerprint(OsStr::new("/second/bin"))
        );
    }
}
