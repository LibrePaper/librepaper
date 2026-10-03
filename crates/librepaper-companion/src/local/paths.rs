//! Process and state paths shared by non-CLI entry points.

use std::path::PathBuf;

use etcetera::BaseStrategy;

/// The user home directory, or nothing when the platform reports none.
pub fn home() -> Option<PathBuf> {
    etcetera::choose_base_strategy()
        .ok()
        .map(|strategy| strategy.home_dir().to_path_buf())
}

/// The user configuration base directory (`$XDG_CONFIG_HOME` on Linux).
pub fn config_home() -> Option<PathBuf> {
    etcetera::choose_base_strategy()
        .ok()
        .map(|strategy| strategy.config_dir())
}

/// The user cache base directory (`$XDG_CACHE_HOME` on Linux).
pub fn cache_home() -> Option<PathBuf> {
    etcetera::choose_base_strategy()
        .ok()
        .map(|strategy| strategy.cache_dir())
}

pub fn state_home() -> Result<PathBuf, String> {
    let strategy = etcetera::choose_base_strategy()
        .map_err(|_| "no home directory to store LibrePaper state".to_string())?;
    Ok(strategy.state_dir().unwrap_or_else(|| strategy.data_dir()))
}

/// The state directory to read and write under, following XDG: where the
/// token cache and the local service's pairings live. A machine with no home
/// directory cannot go on, so this exits with the reason.
pub fn state_home_or_die() -> PathBuf {
    state_home().unwrap_or_else(|error| librepaper_base::util::die(&error))
}

/// Resolve the live installation path even when Linux reports the retained
/// inode of a binary that has since been atomically replaced.
pub(crate) fn current_executable() -> Result<PathBuf, String> {
    resolve_executable(
        std::env::current_exe().map_err(|error| format!("could not locate librepaper: {error}"))?,
    )
}

pub(crate) fn resolve_executable(found: PathBuf) -> Result<PathBuf, String> {
    let path = match found
        .to_str()
        .and_then(|text| text.strip_suffix(" (deleted)"))
    {
        Some(live) => PathBuf::from(live),
        None => found,
    };
    if !path.is_file() {
        return Err(format!(
            "the librepaper binary this app is running from is gone ({}); restart it with `librepaper stop` then `librepaper start`",
            path.display()
        ));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaced_and_missing_executables_are_distinguished() {
        let directory = tempfile::tempdir().unwrap();
        let live = directory.path().join("librepaper");
        std::fs::write(&live, b"#!/bin/sh\n").unwrap();
        let marked = PathBuf::from(format!("{} (deleted)", live.display()));
        assert_eq!(resolve_executable(marked).unwrap(), live);
        assert_eq!(resolve_executable(live.clone()).unwrap(), live);

        let missing = directory.path().join("missing");
        let error = resolve_executable(missing.clone()).unwrap_err();
        assert!(error.contains("librepaper start"), "{error}");
        assert!(error.contains(&missing.display().to_string()));
        assert!(resolve_executable(directory.path().to_path_buf()).is_err());
    }
}
