//! Preferences shared by the local dashboard, CLI and desktop tray helper.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Per-user companion preferences. Unknown values are retained when this
/// version updates one setting so a newer dashboard field is not discarded.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct LocalSettings {
    /// Show the optional tray helper while the companion is running.
    pub tray_enabled: bool,
    /// Extra directories searched for local document tools.
    pub tool_paths: Vec<PathBuf>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

pub fn path(state_home: &Path) -> PathBuf {
    state_home.join("librepaper/local/settings.json")
}

/// Missing preferences mean a fresh install: tray disabled and tools from PATH.
pub fn load(state_home: &Path) -> Result<LocalSettings, String> {
    let path = path(state_home);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| format!("could not read local settings {}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(LocalSettings::default()),
        Err(error) => Err(format!("could not read local settings {}: {error}", path.display())),
    }
}

pub fn save(state_home: &Path, settings: &LocalSettings) -> Result<(), String> {
    let path = path(state_home);
    let bytes = serde_json::to_vec_pretty(settings)
        .map_err(|error| format!("could not encode local settings: {error}"))?;
    librepaper_base::private_files::publish(&path, &bytes, "local settings")
}

pub fn tray_enabled(state_home: &Path) -> bool {
    load(state_home).is_ok_and(|settings| settings.tray_enabled)
}

/// Tool paths supplied on the current command line or environment take
/// precedence; otherwise use the dashboard's persisted list.
pub fn tool_paths(state_home: &Path, override_paths: Vec<PathBuf>) -> Result<Vec<PathBuf>, String> {
    if !override_paths.is_empty() {
        return Ok(override_paths);
    }
    Ok(load(state_home)?.tool_paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_settings_default_to_tray_off_and_unknown_fields_survive_updates() {
        let temp = tempfile::tempdir().unwrap();
        let mut settings = load(temp.path()).unwrap();
        assert!(!settings.tray_enabled);
        settings.extra.insert("future".into(), serde_json::json!({"x": 1}));
        save(temp.path(), &settings).unwrap();
        assert_eq!(load(temp.path()).unwrap().extra["future"]["x"], 1);
    }
}
