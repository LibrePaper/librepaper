//! Preferences shared by the local dashboard, CLI and desktop tray helper.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Per-user companion settings. Unknown values are retained when this
/// version updates a setting so a newer dashboard field is not discarded.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct LocalSettings {
    /// Extra directories searched for local document tools.
    pub tool_paths: Vec<PathBuf>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

pub fn path(state_home: &Path) -> PathBuf {
    state_home.join("librepaper/local/settings.json")
}

/// Missing settings mean tools from PATH.
pub fn load(state_home: &Path) -> Result<LocalSettings, String> {
    let path = path(state_home);
    match std::fs::read(&path) {
        Ok(bytes) => {
            let mut settings: LocalSettings = serde_json::from_slice(&bytes).map_err(|error| {
                format!("could not read local settings {}: {error}", path.display())
            })?;
            // This used to control tray startup. Discard it instead of
            // persisting it as an unknown flattened setting.
            settings.extra.remove("tray_enabled");
            Ok(settings)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(LocalSettings::default()),
        Err(error) => Err(format!(
            "could not read local settings {}: {error}",
            path.display()
        )),
    }
}

pub fn save(state_home: &Path, settings: &LocalSettings) -> Result<(), String> {
    let path = path(state_home);
    let bytes = serde_json::to_vec_pretty(settings)
        .map_err(|error| format!("could not encode local settings: {error}"))?;
    librepaper_base::private_files::publish(&path, &bytes, "local settings")
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
    fn legacy_tray_preference_is_ignored_and_unknown_fields_survive_updates() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("librepaper/local")).unwrap();
        std::fs::write(path(temp.path()), r#"{"tray_enabled":false,"future":{"x":1}}"#).unwrap();
        let mut settings = load(temp.path()).unwrap();
        assert!(!settings.extra.contains_key("tray_enabled"));
        settings
            .extra
            .insert("future".into(), serde_json::json!({"x": 1}));
        save(temp.path(), &settings).unwrap();
        let saved = std::fs::read_to_string(path(temp.path())).unwrap();
        assert!(!saved.contains("tray_enabled"));
        assert_eq!(load(temp.path()).unwrap().extra["future"]["x"], 1);
    }

    #[test]
    fn missing_settings_default_to_path_tools() {
        let temp = tempfile::tempdir().unwrap();
        let settings = load(temp.path()).unwrap();
        assert!(settings.tool_paths.is_empty());
        assert!(settings.extra.is_empty());
    }
}
