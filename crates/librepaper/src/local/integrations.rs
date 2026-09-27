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
use std::sync::OnceLock;
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

static STORE: OnceLock<RwLock<BTreeMap<&'static str, Custom>>> = OnceLock::new();
static STORE_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Load the integrations store from disk, initializing the process-wide cache.
/// Missing or unreadable files result in defaults; validation errors are
/// ignored (the tool can still be overridden via env var or PATH at runtime).
/// Called once when the service starts; calling again replaces the in-memory
/// state (useful for tests).
pub fn init(state_home: &Path) {
    let path = state_home.join("librepaper").join("local").join("integrations.json");
    let _ = STORE_PATH.set(path.clone());
    let all = load_or_default(&path);
    let _ = STORE.set(RwLock::new(all));
}

/// Retrieve the custom configuration for an integration. Returns the default
/// (no override) if not initialized.
pub fn get(which: Integration) -> Custom {
    if let Ok(store) = STORE.get_or_init(|| RwLock::new(BTreeMap::new())).read() {
        store.get(which.key()).cloned().unwrap_or_default()
    } else {
        Custom::default()
    }
}

/// Retrieve all integrations as a map from name to custom configuration.
pub fn all() -> BTreeMap<&'static str, Custom> {
    if let Ok(store) = STORE.get_or_init(|| RwLock::new(BTreeMap::new())).read() {
        Integration::ALL
            .iter()
            .map(|i| (i.key(), store.get(i.key()).cloned().unwrap_or_default()))
            .collect()
    } else {
        Integration::ALL
            .iter()
            .map(|i| (i.key(), Custom::default()))
            .collect()
    }
}

/// Update the configuration for an integration, validate it, write it
/// atomically to disk, and update the in-memory cache. The write is done
/// to a temporary file in the same directory as the target, then renamed,
/// so a crash does not corrupt the store.
pub fn set(which: Integration, custom: Custom) -> Result<(), String> {
    custom.validate()?;
    let path = STORE_PATH
        .get()
        .cloned()
        .unwrap_or_else(|| {
            std::env::var_os("XDG_STATE_HOME")
                .or_else(|| std::env::var_os("HOME").map(|h| format!("{h}/.local/state").into()))
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join("librepaper")
                .join("local")
                .join("integrations.json")
        });
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new("."))).map_err(|e| e.to_string())?;
    let mut all = load_or_default(&path);
    all.insert(which.key(), custom);
    write_private_json(&path, &all).map_err(|e| e.to_string())?;
    if let Ok(mut store) = STORE.get_or_init(|| RwLock::new(BTreeMap::new())).write() {
        *store = all;
    }
    Ok(())
}

/// The executable to run. The override environment variable (if set to a
/// file) wins; then the custom path (a directory is joined with the tool's
/// program name); then a PATH search using the standard tools::find lookup.
/// Returns None if none of these sources yields an executable file.
pub fn executable(which: Integration) -> Option<PathBuf> {
    let override_var = which.override_var();
    let configured = std::env::var_os(override_var).map(PathBuf::from);
    if let Some(path) = configured.filter(|path| path.is_file()) {
        return Some(path);
    }
    let custom = get(which);
    if let Some(path) = custom.path {
        let exe = if path.is_dir() {
            path.join(which.program())
        } else {
            path
        };
        if exe.is_file() {
            return Some(exe);
        }
    }
    crate::local::tools::find(override_var, which.as_str())
}

/// Extra command-line arguments to append to every invocation.
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

    fn with_tempdir<F: Fn(&Path)>(f: F) {
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
