//! Process and state paths shared by non-CLI entry points.

use std::path::{Path, PathBuf};

pub(crate) fn state_home() -> Result<PathBuf, String> {
    if let Some(base) = std::env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(base));
    }
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .ok_or("no home directory to store LibrePaper state")?;
    Ok(Path::new(&home).join(".local").join("state"))
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
            "the librepaper binary this app is running from is gone ({}); restart it with `librepaper local stop` then `librepaper local launch`",
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
        assert!(error.contains("librepaper local launch"), "{error}");
        assert!(error.contains(&missing.display().to_string()));
        assert!(resolve_executable(directory.path().to_path_buf()).is_err());
    }
}
