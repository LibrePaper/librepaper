//! The Quarto preview adapter: command construction, option validation and
//! log-line recognition for a managed `quarto preview` watcher. Kept
//! behaviorally identical to the pre-adapter implementation -- an existing
//! Quarto preview client sees no difference on the wire.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tokio::process::Command;

use super::{ArtifactKind, Plan, Session, Watch};
use crate::local::protocol::{
    PreviewRequest, QuartoExecutionMode, QuartoRenderPolicy, QuartoRenderScope,
};
use crate::local::quarto::{find_quarto, verify_bound_manifest, BindingStore};

pub(crate) struct QuartoWatch {
    entrypoint: String,
}

impl Watch for QuartoWatch {
    fn rendering_started(&self, line: &str) -> bool {
        line.contains("processing file:")
            || line.starts_with("pandoc")
            || line.contains("Rendering")
    }

    fn render_finished(&self, line: &str) -> bool {
        line.starts_with("Output created:")
    }

    fn output_path(&self, root: &Path) -> Option<PathBuf> {
        resolve_rendered_page(root, &self.entrypoint)
    }

    fn kind(&self) -> ArtifactKind {
        // A single self-contained HTML file (`-M embed-resources:true` is
        // always passed below): the managed preview surface has never
        // served any other shape, regardless of `--to`.
        ArtifactKind::Html
    }
}

/// Candidate rendered outputs for a watched `.qmd`, checked in order and
/// resolved to the first that exists as a regular file within `root`. Quarto
/// may write the render next to the source, or under a project output
/// directory; canonicalizing and checking `starts_with(root)` refuses a
/// symlink or an entrypoint crafted to escape the bound root.
pub(crate) fn resolve_rendered_page(root: &Path, entrypoint: &str) -> Option<PathBuf> {
    let rendered = Path::new(entrypoint).with_extension("html");
    let candidates = [
        root.join(&rendered),
        root.join("_site").join(&rendered),
        root.join("_output").join(&rendered),
        root.join("docs").join(&rendered),
    ];
    let root_canon = std::fs::canonicalize(root).ok()?;
    for candidate in candidates {
        if let Ok(canon) = std::fs::canonicalize(&candidate) {
            if canon.starts_with(&root_canon) && canon.is_file() {
                return Some(canon);
            }
        }
    }
    None
}

/// Validates a `POST previews` request naming the Quarto engine (the
/// default) and builds the `quarto preview` command to run. `existing` is
/// the live session map, consulted for the same one-binding/one-root
/// exclusions the pre-adapter implementation enforced inline.
pub(crate) fn plan(
    existing: &HashMap<String, Session>,
    request: &PreviewRequest,
    bindings: &BindingStore,
) -> Result<Plan, String> {
    let options = request.quarto.as_ref().ok_or("missing Quarto options")?;
    options.validate()?;
    if options.execution_mode != QuartoExecutionMode::WorkingTree {
        return Err("managed preview supports linked working-tree mode only".into());
    }
    if options.render_scope != QuartoRenderScope::Document {
        return Err("managed preview supports document scope only".into());
    }
    if options.policy != QuartoRenderPolicy::ProjectDefaults {
        return Err("managed preview uses project-default render policy".into());
    }
    if existing.values().any(|p| p.binding == options.binding_id) {
        return Err("Stop the existing preview before starting another watcher.".into());
    }
    if existing.len() >= 8 {
        return Err("Too many active previews".into());
    }
    let binding = bindings
        .resolve_scoped(&options.binding_id, &request.origin, &request.project)
        .map_err(|_| "Preview binding is not authorized")?;
    if existing.values().any(|p| p.root == binding.root) {
        return Err("Another managed preview already watches this project directory".into());
    }
    let hosted = BindingStore::is_hosted(&binding);
    // A hosted binding names no fixed entrypoint: each job (and each
    // preview) names its own `.qmd`, already validated as a safe relative
    // path by `options.validate()` above. A granted binding still refuses
    // any entrypoint but the one it was granted for.
    let entrypoint = if hosted {
        options.main.clone()
    } else {
        binding.entrypoint.clone()
    };
    if !hosted && options.main != binding.entrypoint {
        return Err("quarto entrypoint does not match the granted project binding".into());
    }
    if !request.manifest.iter().any(|f| f.path == entrypoint) {
        return Err("Preview inventory must contain the bound entrypoint".into());
    }
    if std::fs::canonicalize(&binding.root).map_err(|e| e.to_string())? != binding.root {
        return Err("Bound root changed".into());
    }
    let mut inventory = request.manifest.clone();
    crate::local::quarto::add_declared_inputs(&binding.root, &options.data_inputs, &mut inventory)?;
    verify_bound_manifest(&binding.root, &inventory)?;
    if let Some(expected) = options.shared_tree_sha256.as_deref() {
        let actual =
            crate::local::quarto::inventory_manifest_impl(&binding.root, &request.manifest)?
                .tree_sha256;
        if actual != expected {
            return Err("preview source inventory is stale; synchronize before previewing".into());
        }
    }
    let main = std::fs::canonicalize(binding.root.join(&entrypoint)).map_err(|e| e.to_string())?;
    if !main.starts_with(&binding.root) {
        return Err("Preview entrypoint escapes bound root".into());
    }
    let mut command = Command::new(find_quarto().ok_or("Quarto is not installed")?);
    command
        .current_dir(&binding.root)
        .arg("preview")
        .arg(&entrypoint)
        .args(["--no-serve", "--no-browser", "--to", &options.format]);
    if let Some(profile) = &options.profile {
        command.args(["--profile", profile]);
    }
    for (key, value) in &options.parameters {
        command.arg("-P").arg(format!(
            "{key}:{}",
            serde_json::to_string(value).map_err(|e| e.to_string())?
        ));
    }
    // A single self-contained HTML file: no separate `_files/` directory for
    // the local app to also locate and serve.
    command.arg("-M").arg("embed-resources:true");

    Ok(Plan {
        command,
        adapter: Box::new(QuartoWatch {
            entrypoint: entrypoint.clone(),
        }),
        entrypoint,
        root: binding.root,
        binding_id: options.binding_id.clone(),
    })
}
