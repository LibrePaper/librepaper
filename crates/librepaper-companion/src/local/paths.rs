//! Process and state paths shared by non-CLI entry points.

use std::path::{Path, PathBuf};

pub fn state_home() -> Result<PathBuf, String> {
    let local_app_data = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA")
    } else {
        None
    };
    state_home_from(
        std::env::var_os("XDG_STATE_HOME"),
        std::env::var_os("HOME"),
        local_app_data,
    )
}

fn state_home_from(
    xdg_state: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
    local_app_data: Option<std::ffi::OsString>,
) -> Result<PathBuf, String> {
    if let Some(base) = xdg_state.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(base));
    }
    if let Some(home) = home.filter(|value| !value.is_empty()) {
        return Ok(Path::new(&home).join(".local").join("state"));
    }
    if let Some(base) = local_app_data.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(base).join("LibrePaper").join("State"));
    }
    Err("no home directory to store LibrePaper state".into())
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

    #[test]
    fn windows_state_path_falls_back_to_local_app_data() {
        let local = std::ffi::OsString::from(r"C:\Users\Ada\AppData\Local");
        let result = state_home_from(None, None, Some(local)).unwrap();
        assert_eq!(
            result,
            PathBuf::from(r"C:\Users\Ada\AppData\Local")
                .join("LibrePaper")
                .join("State")
        );
        assert_eq!(
            state_home_from(None, Some("/home/ada".into()), Some("C:\\local".into())).unwrap(),
            PathBuf::from("/home/ada/.local/state"),
            "the existing HOME convention remains preferred"
        );
    }
}
