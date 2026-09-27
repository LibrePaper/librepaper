//! Custom executable paths and command-line arguments for Quarto and Calepin.
//!
//! The local companion lets authors point to custom-built or vendored versions
//! of these tools, and pass extra arguments the tools should always receive.
//! Configuration lives in `<state_home>/librepaper/local/integrations.json`,
//! alongside the pairing and connection stores.
//!
//! An executable can be given as a file path or a directory (which is joined
//! with the tool name). Paths must be absolute; relative paths are rejected.
//! Environment variables (one per tool) win outright; then custom paths;
//! then a PATH search.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use super::pairing::{read_json, write_private_json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Integration {
    Quarto,
    Calepin,
}

impl Integration {
    pub const ALL: [Integration; 2] = [Integration::Quarto, Integration::Calepin];

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "quarto" => Ok(Self::Quarto),
            "calepin" => Ok(Self::Calepin),
            _ => Err(format!(
                "unknown integration: {value} (supported: quarto, calepin)"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Quarto => "quarto",
            Self::Calepin => "calepin",
        }
    }

    fn program(self) -> &'static str {
        match self {
            Self::Quarto => {
                if cfg!(windows) {
                    "quarto.exe"
                } else {
                    "quarto"
                }
            }
            Self::Calepin => {
                if cfg!(windows) {
                    "calepin.exe"
                } else {
                    "calepin"
                }
            }
        }
    }

    fn override_var(self) -> &'static str {
        match self {
            Self::Quarto => "LIBREPAPER_QUARTO_PATH",
            Self::Calepin => "LIBREPAPER_CALEPIN_PATH",
        }
    }

    fn key(self) -> &'static str {
        self.as_str()
    }
}

/// Execution configuration for a single integration.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Custom {
    /// Absolute path to the executable, or a directory to join with the
    /// program name. Omitted means use PATH lookup.
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// Extra command-line arguments to append to every invocation.
    #[serde(default)]
    pub args: Vec<String>,
}

impl Custom {
    /// Validate the custom configuration. Paths must be absolute; arg count
    /// is capped at 64; each arg is non-empty, at most 4096 bytes, and
    /// contains no NUL bytes.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(path) = &self.path {
            if !path.is_absolute() {
                return Err(format!("path must be absolute: {}", path.display()));
            }
        }
        if self.args.len() > 64 {
            return Err(format!(
                "too many args: {} (maximum 64)",
                self.args.len()
            ));
        }
        for (i, arg) in self.args.iter().enumerate() {
            if arg.is_empty() {
                return Err(format!("arg #{} is empty", i));
            }
            if arg.len() > 4096 {
                return Err(format!(
                    "arg #{} is too long: {} bytes (maximum 4096)",
                    i,
                    arg.len()
                ));
            }
            if arg.contains('\0') {
                return Err(format!("arg #{} contains NUL byte", i));
            }
        }
        Ok(())
    }
}

/// The store file and what it holds. `None` until the service calls [`init`].
struct State {
    path: PathBuf,
    entries: BTreeMap<&'static str, Custom>,
}

static STATE: RwLock<Option<State>> = RwLock::new(None);

/// Loads `<state_home>/librepaper/local/integrations.json`. A missing or
/// unreadable file, or an entry that no longer validates, reads as the
/// default. Called when the service starts; a later call replaces the state.
pub fn init(state_home: &Path) {
    let path = state_home.join("librepaper").join("local").join("integrations.json");
    let entries = load_or_default(&path);
    *STATE.write().unwrap_or_else(|poison| poison.into_inner()) = Some(State { path, entries });
}

/// The configuration for one integration; the default before [`init`].
pub fn get(which: Integration) -> Custom {
    STATE
        .read()
        .unwrap_or_else(|poison| poison.into_inner())
        .as_ref()
        .and_then(|state| state.entries.get(which.key()).cloned())
        .unwrap_or_default()
}

/// Every integration's configuration, defaults included.
pub fn all() -> BTreeMap<&'static str, Custom> {
    Integration::ALL.iter().map(|&which| (which.key(), get(which))).collect()
}

/// Validates `custom`, writes the store, then updates memory. An empty
/// configuration removes the entry.
pub fn set(which: Integration, custom: Custom) -> Result<(), String> {
    custom.validate()?;
    let mut guard = STATE.write().unwrap_or_else(|poison| poison.into_inner());
    let state = guard.as_mut().ok_or("integrations store is not initialised")?;
    let mut entries = state.entries.clone();
    if custom == Custom::default() {
        entries.remove(which.key());
    } else {
        entries.insert(which.key(), custom);
    }
    if let Some(parent) = state.path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    write_private_json(&state.path, &entries).map_err(|e| e.to_string())?;
    state.entries = entries;
    Ok(())
}

/// The executable to run. The override environment variable wins (tests
/// and packaging use it); then the custom path, a directory being joined
/// with the program name; then a PATH search. A custom path that names no
/// file is `None` rather than a silent fall back to some other program.
pub fn executable(which: Integration) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(which.override_var()).map(PathBuf::from) {
        if path.is_file() {
            return Some(path);
        }
    }
    if let Some(path) = get(which).path {
        let exe = if path.is_dir() { path.join(which.program()) } else { path };
        return exe.is_file().then_some(exe);
    }
    crate::local::tools::find(which.override_var(), which.as_str())
}

/// Extra command-line arguments appended to every invocation.
pub fn extra_args(which: Integration) -> Vec<String> {
    get(which).args
}

fn load_or_default(path: &Path) -> BTreeMap<&'static str, Custom> {
    read_json::<BTreeMap<String, Custom>>(path)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(key, value)| {
            Integration::parse(&key)
                .ok()
                .and_then(|i| value.validate().ok().map(|_| (i.key(), value)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The store is process-wide, so tests that touch it take turns.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_tempdir<F: Fn(&Path)>(f: F) {
        let _turn = SERIAL.lock().unwrap_or_else(|poison| poison.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        f(dir.path());
    }

    #[test]
    fn validate_rejects_relative_paths() {
        let custom = Custom {
            path: Some(PathBuf::from("relative/path")),
            args: vec![],
        };
        assert!(custom.validate().is_err());
    }

    #[test]
    fn validate_rejects_too_many_args() {
        let custom = Custom {
            path: None,
            args: vec!["arg".to_string(); 65],
        };
        assert!(custom.validate().is_err());
    }

    #[test]
    fn validate_rejects_empty_args() {
        let custom = Custom {
            path: None,
            args: vec!["good".to_string(), "".to_string()],
        };
        assert!(custom.validate().is_err());
    }

    #[test]
    fn validate_rejects_args_with_nul() {
        let custom = Custom {
            path: None,
            args: vec!["good".to_string(), "bad\0arg".to_string()],
        };
        assert!(custom.validate().is_err());
    }

    #[test]
    fn validate_rejects_oversized_args() {
        let custom = Custom {
            path: None,
            args: vec!["x".repeat(4097)],
        };
        assert!(custom.validate().is_err());
    }

    #[test]
    fn validate_accepts_absolute_path_and_bounded_args() {
        let custom = Custom {
            path: Some(PathBuf::from("/absolute/path")),
            args: vec!["arg1".to_string(), "arg2".to_string()],
        };
        assert!(custom.validate().is_ok());
    }

    #[test]
    fn set_and_get_round_trip() {
        with_tempdir(|dir| {
            init(dir);
            let custom = Custom {
                path: Some(PathBuf::from("/usr/bin/quarto")),
                args: vec!["--quiet".to_string()],
            };
            set(Integration::Quarto, custom.clone()).expect("set");
            assert_eq!(get(Integration::Quarto), custom);
            assert_eq!(get(Integration::Calepin), Custom::default());
        });
    }

    #[test]
    fn set_persists_across_init() {
        with_tempdir(|dir| {
            init(dir);
            let custom = Custom {
                path: Some(PathBuf::from("/usr/bin/calepin")),
                args: vec![],
            };
            set(Integration::Calepin, custom.clone()).expect("set");
            init(dir);
            assert_eq!(get(Integration::Calepin), custom);
        });
    }

    #[test]
    fn all_returns_every_integration() {
        with_tempdir(|dir| {
            init(dir);
            let q = Custom {
                path: Some(PathBuf::from("/usr/bin/quarto")),
                args: vec![],
            };
            let c = Custom {
                path: Some(PathBuf::from("/usr/bin/calepin")),
                args: vec!["--flag".to_string()],
            };
            set(Integration::Quarto, q.clone()).expect("set quarto");
            set(Integration::Calepin, c.clone()).expect("set calepin");
            let all_map = all();
            assert_eq!(all_map.get("quarto"), Some(&q));
            assert_eq!(all_map.get("calepin"), Some(&c));
        });
    }

    #[test]
    fn directory_path_joins_program_name() {
        with_tempdir(|dir| {
            init(dir);
            std::fs::create_dir_all(dir.join("bin")).expect("mkdir");
            let bin_dir = dir.join("bin");
            #[cfg(not(windows))]
            {
                std::fs::write(bin_dir.join("quarto"), "#!/bin/sh").expect("write");
            }
            #[cfg(windows)]
            {
                std::fs::write(bin_dir.join("quarto.exe"), "").expect("write");
            }
            let custom = Custom {
                path: Some(bin_dir),
                args: vec![],
            };
            set(Integration::Quarto, custom).expect("set");
            let exe = executable(Integration::Quarto);
            assert!(exe.is_some());
            assert_eq!(exe.unwrap().file_name().unwrap(), Integration::Quarto.program());
        });
    }

    #[test]
    fn extra_args_returns_configured_args() {
        with_tempdir(|dir| {
            init(dir);
            let custom = Custom {
                path: None,
                args: vec!["--verbose".to_string(), "--config=custom.toml".to_string()],
            };
            set(Integration::Calepin, custom).expect("set");
            assert_eq!(extra_args(Integration::Calepin), vec![
                "--verbose".to_string(),
                "--config=custom.toml".to_string()
            ]);
        });
    }
}
