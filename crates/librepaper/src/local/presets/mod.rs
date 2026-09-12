//! Companion-local build presets.
//!
//! Presets are deliberately owned by the companion.  Only their opaque id and
//! descriptive metadata should cross the browser bridge; executable paths,
//! complete argument vectors, and environment values stay in this store.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::protocol::safe_relative_path;

const MAX_PRESETS: usize = 256;
const MAX_GRANTS: usize = 1024;
const MAX_TEXT: usize = 4096;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Preset {
    pub id: String,
    pub display_name: String,
    pub base_adapter: String,
    #[serde(default)]
    pub source_formats: BTreeSet<String>,
    #[serde(default)]
    pub options: BTreeMap<String, String>,
    #[serde(default)]
    pub option_schema: BTreeMap<String, OptionType>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub wrapper: Option<String>,
    /// Incremented whenever execution meaning changes.  Grants carry the
    /// value they approved and become invalid after an edit.
    pub semantic_revision: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "values", rename_all = "kebab-case")]
pub enum OptionType {
    Boolean,
    String,
    Enum(Vec<String>),
}

/// Safe metadata suitable for capability responses.  Never serialize the
/// complete [`Preset`] to the browser: options, environment and wrapper are
/// companion-local implementation details.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PresetSummary {
    pub id: String,
    pub display_name: String,
    pub base_adapter: String,
    pub source_formats: BTreeSet<String>,
    pub semantic_revision: u64,
}

impl From<&Preset> for PresetSummary {
    fn from(preset: &Preset) -> Self {
        Self {
            id: preset.id.clone(),
            display_name: preset.display_name.clone(),
            base_adapter: preset.base_adapter.clone(),
            source_formats: preset.source_formats.clone(),
            semantic_revision: preset.semantic_revision,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PresetGrant {
    pub id: String,
    pub origin: String,
    pub project: String,
    pub preset_id: String,
    pub workspace: WorkspaceMode,
    pub operation: Operation,
    pub entrypoint: String,
    pub semantic_revision: u64,
    pub granted_at: i64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceMode {
    Snapshot,
    Bound,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Operation {
    Build,
    Preview,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    presets: BTreeMap<String, Preset>,
    #[serde(default)]
    grants: BTreeMap<String, PresetGrant>,
}

#[derive(Clone, Debug)]
pub struct PresetStore {
    path: PathBuf,
}

impl PresetStore {
    pub fn new(state_home: impl AsRef<Path>) -> Self {
        Self {
            path: state_home
                .as_ref()
                .join("librepaper")
                .join("local")
                .join("presets.json"),
        }
    }

    pub fn list(&self) -> Vec<PresetSummary> {
        self.load()
            .map(|state| state.presets.values().map(PresetSummary::from).collect())
            .unwrap_or_default()
    }

    pub fn get(&self, id: &str) -> Option<Preset> {
        self.load().ok()?.presets.get(id).cloned()
    }

    pub fn grants(&self) -> Result<Vec<PresetGrant>, String> {
        Ok(self.load()?.grants.into_values().collect())
    }

    pub fn create(&self, mut preset: Preset) -> Result<Preset, String> {
        validate_preset(&preset)?;
        let _lock = self.lock()?;
        let mut state = self.load()?;
        if state.presets.len() >= MAX_PRESETS {
            return Err("too many local presets".into());
        }
        if preset.id.is_empty() {
            preset.id = random_id("preset");
        }
        if state.presets.contains_key(&preset.id) {
            return Err("preset id already exists".into());
        }
        preset.semantic_revision = 1;
        state.presets.insert(preset.id.clone(), preset.clone());
        self.save(&state)?;
        Ok(preset)
    }

    /// Replace the semantic contents of a preset.  Grants are not silently
    /// updated: their old revision remains, so `resolve_grant` rejects them.
    pub fn update(&self, id: &str, mut next: Preset) -> Result<Preset, String> {
        validate_preset(&next)?;
        let _lock = self.lock()?;
        let mut state = self.load()?;
        let previous = state.presets.get(id).ok_or("preset not found")?;
        if next.id != id {
            return Err("preset id cannot change".into());
        }
        next.semantic_revision = previous
            .semantic_revision
            .checked_add(1)
            .ok_or("preset revision exhausted")?;
        state.presets.insert(id.to_owned(), next.clone());
        self.save(&state)?;
        Ok(next)
    }

    pub fn remove(&self, id: &str) -> Result<(), String> {
        let _lock = self.lock()?;
        let mut state = self.load()?;
        if state.presets.remove(id).is_none() {
            return Err("preset not found".into());
        }
        state.grants.retain(|_, grant| grant.preset_id != id);
        self.save(&state)
    }

    #[allow(clippy::too_many_arguments)] // Each authorization dimension remains explicit.
    pub fn grant(
        &self,
        origin: &str,
        project: &str,
        preset_id: &str,
        workspace: WorkspaceMode,
        operation: Operation,
        entrypoint: &str,
        now: i64,
    ) -> Result<PresetGrant, String> {
        validate_scope(origin, project, entrypoint)?;
        let _lock = self.lock()?;
        let mut state = self.load()?;
        let preset = state.presets.get(preset_id).ok_or("preset not found")?;
        if workspace == WorkspaceMode::Bound && entrypoint.is_empty() {
            return Err("bound presets require an entrypoint".into());
        }
        if state.grants.len() >= MAX_GRANTS {
            return Err("too many preset grants".into());
        }
        let grant = PresetGrant {
            id: random_id("grant"),
            origin: origin.to_owned(),
            project: project.to_owned(),
            preset_id: preset_id.to_owned(),
            workspace,
            operation,
            entrypoint: entrypoint.to_owned(),
            semantic_revision: preset.semantic_revision,
            granted_at: now,
        };
        state.grants.insert(grant.id.clone(), grant.clone());
        self.save(&state)?;
        Ok(grant)
    }

    pub fn revoke(&self, grant_id: &str) -> Result<bool, String> {
        let _lock = self.lock()?;
        let mut state = self.load()?;
        let removed = state.grants.remove(grant_id).is_some();
        if removed {
            self.save(&state)?;
        }
        Ok(removed)
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn resolve_grant(
        &self,
        grant_id: &str,
        origin: &str,
        project: &str,
        preset_id: &str,
        workspace: WorkspaceMode,
        operation: Operation,
        entrypoint: &str,
    ) -> Result<PresetGrant, String> {
        self.resolve_for(
            grant_id, origin, project, preset_id, workspace, operation, entrypoint,
        )
        .map(|(grant, _)| grant)
    }

    /// Resolve authorization and the local command definition together at
    /// admission time. Callers must use this immediately before spawning;
    /// editing or revoking either record then denies a subsequent request.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn resolve_for(
        &self,
        grant_id: &str,
        origin: &str,
        project: &str,
        preset_id: &str,
        workspace: WorkspaceMode,
        operation: Operation,
        entrypoint: &str,
    ) -> Result<(PresetGrant, Preset), String> {
        let state = self.load()?;
        let grant = state.grants.get(grant_id).ok_or("preset grant not found")?;
        let preset = state.presets.get(preset_id).ok_or("preset not found")?;
        if grant.origin != origin
            || grant.project != project
            || grant.preset_id != preset_id
            || grant.workspace != workspace
            || grant.operation != operation
            || grant.entrypoint != entrypoint
        {
            return Err("preset grant scope does not match request".into());
        }
        if grant.semantic_revision != preset.semantic_revision {
            return Err("preset changed; grant must be renewed".into());
        }
        Ok((grant.clone(), preset.clone()))
    }

    /// Resolve the current grant for a request that carries only the preset
    /// id. A request cannot select an arbitrary grant id from another scope.
    pub fn resolve_scoped(
        &self,
        origin: &str,
        project: &str,
        preset_id: &str,
        workspace: WorkspaceMode,
        operation: Operation,
        entrypoint: &str,
    ) -> Result<(PresetGrant, Preset), String> {
        let state = self.load()?;
        let preset = state.presets.get(preset_id).ok_or("preset not found")?;
        let grant = state
            .grants
            .values()
            .find(|grant| {
                grant.origin == origin
                    && grant.project == project
                    && grant.preset_id == preset_id
                    && grant.workspace == workspace
                    && grant.operation == operation
                    && grant.entrypoint == entrypoint
                    && grant.semantic_revision == preset.semantic_revision
            })
            .ok_or("preset execution is not granted for this scope")?;
        Ok((grant.clone(), preset.clone()))
    }

    /// Validate browser supplied values against a local preset schema and
    /// return the effective local options. The browser can override declared
    /// values only; it cannot add command arguments or environment entries.
    pub fn effective_options(
        preset: &Preset,
        overrides: &BTreeMap<String, String>,
    ) -> Result<BTreeMap<String, String>, String> {
        let mut effective = preset.options.clone();
        for (name, value) in overrides {
            let Some(kind) = preset.option_schema.get(name) else {
                return Err(format!("preset option is not declared: {name}"));
            };
            match kind {
                OptionType::Boolean if !matches!(value.as_str(), "true" | "false") => {
                    return Err(format!("preset option {name} must be boolean"));
                }
                OptionType::Enum(values) if !values.iter().any(|candidate| candidate == value) => {
                    return Err(format!("preset option {name} is outside its enum"));
                }
                _ => {}
            }
            effective.insert(name.clone(), value.clone());
        }
        Ok(effective)
    }

    fn load(&self) -> Result<State, String> {
        let Ok(text) = fs::read_to_string(&self.path) else {
            return if self.path.exists() {
                Err("local preset store is unreadable".into())
            } else {
                Ok(State::default())
            };
        };
        serde_json::from_str(&text).map_err(|_| "local preset store is corrupt".into())
    }

    fn save(&self, state: &State) -> Result<(), String> {
        let parent = self.path.parent().ok_or("invalid preset store path")?;
        fs::create_dir_all(parent).map_err(|e| format!("could not create preset store: {e}"))?;
        let text = serde_json::to_vec_pretty(state).map_err(|e| e.to_string())?;
        let temporary = self.path.with_extension(format!(
            "json.tmp-{}",
            hex::encode(crate::auth::random_bytes(8))
        ));
        let mut file_options = fs::OpenOptions::new();
        file_options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            file_options.mode(0o600);
        }
        let mut file = file_options
            .open(&temporary)
            .map_err(|e| format!("could not save presets: {e}"))?;
        std::io::Write::write_all(&mut file, &text)
            .and_then(|_| file.sync_all())
            .map_err(|e| format!("could not save presets: {e}"))?;
        let result = fs::rename(&temporary, &self.path)
            .map_err(|e| format!("could not commit presets: {e}"));
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn lock(&self) -> Result<std::fs::File, String> {
        let parent = self.path.parent().ok_or("invalid preset store path")?;
        fs::create_dir_all(parent).map_err(|e| format!("could not create preset store: {e}"))?;
        let lock_path = self.path.with_extension("json.lock");
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(|e| format!("could not lock preset store: {e}"))?;
        fs2::FileExt::lock_exclusive(&lock)
            .map_err(|e| format!("could not lock preset store: {e}"))?;
        Ok(lock)
    }
}

fn validate_preset(preset: &Preset) -> Result<(), String> {
    if preset.id.len() > 128 || (!preset.id.is_empty() && !safe_id(&preset.id)) {
        return Err("invalid preset id".into());
    }
    bounded_text(&preset.display_name, "display name")?;
    bounded_text(&preset.base_adapter, "base adapter")?;
    if preset.base_adapter.is_empty() {
        return Err("base adapter is required".into());
    }
    for (name, value) in preset.options.iter().chain(preset.environment.iter()) {
        bounded_text(name, "preset key")?;
        bounded_text(value, "preset value")?;
        if name.contains('=') || name.contains('\0') {
            return Err("invalid preset key".into());
        }
    }
    for (name, kind) in &preset.option_schema {
        bounded_text(name, "option name")?;
        let Some(value) = preset.options.get(name) else {
            return Err(format!("missing default for preset option {name}"));
        };
        match kind {
            OptionType::Boolean if !matches!(value.as_str(), "true" | "false") => {
                return Err(format!("preset option {name} must be boolean"));
            }
            OptionType::Enum(values) if !values.iter().any(|candidate| candidate == value) => {
                return Err(format!("preset option {name} is outside its enum"));
            }
            _ => {}
        }
    }
    if preset
        .options
        .keys()
        .any(|name| !preset.option_schema.contains_key(name))
    {
        return Err("preset options must have a typed schema".into());
    }
    if let Some(wrapper) = &preset.wrapper {
        bounded_text(wrapper, "wrapper")?;
        if !Path::new(wrapper).is_absolute() {
            return Err("preset wrapper must be an absolute local path".into());
        }
    }
    Ok(())
}

fn validate_scope(origin: &str, project: &str, entrypoint: &str) -> Result<(), String> {
    bounded_text(origin, "origin")?;
    bounded_text(project, "project")?;
    if entrypoint.is_empty() || !safe_relative_path(entrypoint) {
        return Err("entrypoint must be a safe project-relative path".into());
    }
    Ok(())
}

fn bounded_text(value: &str, what: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > MAX_TEXT || value.chars().any(char::is_control) {
        return Err(format!("invalid preset {what}"));
    }
    Ok(())
}

fn safe_id(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn random_id(prefix: &str) -> String {
    format!("{prefix}-{}", hex::encode(crate::auth::random_bytes(12)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset() -> Preset {
        Preset {
            id: String::new(),
            display_name: "local latex".into(),
            base_adapter: "latexmk".into(),
            source_formats: ["latex".into()].into_iter().collect(),
            options: BTreeMap::new(),
            option_schema: BTreeMap::new(),
            environment: BTreeMap::new(),
            wrapper: None,
            semantic_revision: 0,
        }
    }

    #[test]
    fn editing_a_preset_invalidates_its_grant() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = PresetStore::new(root.path());
        let created = store.create(preset()).expect("create");
        let grant = store
            .grant(
                "https://example.test",
                "paper",
                &created.id,
                WorkspaceMode::Snapshot,
                Operation::Build,
                "paper.tex",
                1,
            )
            .expect("grant");
        let mut edited = created.clone();
        edited.display_name = "changed".into();
        store.update(&created.id, edited).expect("update");
        assert!(store
            .resolve_grant(
                &grant.id,
                "https://example.test",
                "paper",
                &created.id,
                WorkspaceMode::Snapshot,
                Operation::Build,
                "paper.tex"
            )
            .is_err());
    }

    #[test]
    fn browser_summary_does_not_include_local_command_details() {
        let mut value = preset();
        value.id = "safe".into();
        value.environment.insert("SECRET".into(), "hidden".into());
        let summary = PresetSummary::from(&value);
        let encoded = serde_json::to_value(summary).expect("encode");
        assert!(encoded.get("environment").is_none());
    }
}
