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
    kind: ArtifactKind,
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
        resolve_rendered_page(root, &self.entrypoint, self.kind)
    }

    fn kind(&self) -> ArtifactKind {
        self.kind
    }
}

/// Candidate rendered outputs for a watched `.qmd`, checked in order and
/// resolved to the first that exists as a regular file within `root`. Quarto
/// may write the render next to the source, or under a project output
/// directory; canonicalizing and checking `starts_with(root)` refuses a
/// symlink or an entrypoint crafted to escape the bound root.
pub(crate) fn resolve_rendered_page(
    root: &Path,
    entrypoint: &str,
    kind: ArtifactKind,
) -> Option<PathBuf> {
    let rendered = Path::new(entrypoint).with_extension(kind.as_str());
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
    let kind = match options.format.as_str() {
        "html" | "revealjs" => ArtifactKind::Html,
        "pdf" => ArtifactKind::Pdf,
        _ => return Err("managed Quarto preview supports html, revealjs, or pdf output".into()),
    };
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
    // HTML must be one self-contained file because the preview endpoint
    // serves one artifact. PDF already has that property.
    if kind == ArtifactKind::Html {
        command.arg("-M").arg("embed-resources:true");
    }
    let confinement = crate::local::confine::detect();
    if !confinement.available {
        return Err(format!(
            "Quarto preview requires filesystem and network confinement: {}",
            confinement.reason
        ));
    }
    let confinement_plan = crate::local::confine::Plan {
        workspace: binding.root.clone(),
        writable: vec![],
        read_only: quarto_installation_roots(command.as_std().get_program().as_ref()),
        network: false,
    };
    if crate::local::confine::wrap(&mut command, &confinement_plan)?
        == crate::local::confine::Applied::None
    {
        return Err("Quarto preview requires supported confinement".into());
    }

    Ok(Plan {
        command,
        adapter: Box::new(QuartoWatch {
            entrypoint: entrypoint.clone(),
            kind,
        }),
        entrypoint,
        root: binding.root,
        binding_id: options.binding_id.clone(),
    })
}

fn quarto_installation_roots(program: &Path) -> Vec<PathBuf> {
    program
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::preview::Previews;
    use crate::local::protocol::{ManifestEntry, QuartoJobOptions};

    #[test]
    fn rendered_page_uses_the_artifact_kind_and_stays_inside_the_root() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("paper.html"), "<html></html>").unwrap();
        std::fs::write(root.path().join("paper.pdf"), b"%PDF-1.7\n%%EOF\n").unwrap();

        assert_eq!(
            resolve_rendered_page(root.path(), "paper.qmd", ArtifactKind::Html),
            Some(std::fs::canonicalize(root.path().join("paper.html")).unwrap())
        );
        assert_eq!(
            resolve_rendered_page(root.path(), "paper.qmd", ArtifactKind::Pdf),
            Some(std::fs::canonicalize(root.path().join("paper.pdf")).unwrap())
        );
        assert_eq!(
            resolve_rendered_page(root.path(), "../paper.qmd", ArtifactKind::Pdf),
            None
        );
    }

    #[tokio::test]
    #[ignore = "requires installed Quarto and a PDF engine"]
    async fn real_quarto_pdf_preview_publishes_complete_pdf_bytes() {
        let state = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let source = b"---\nformat: pdf\n---\n\n# PDF preview\n";
        std::fs::write(project.path().join("paper.qmd"), source).unwrap();
        let bindings = BindingStore::new(state.path());
        let binding = bindings
            .grant(
                "https://paper.example",
                "paper",
                project.path(),
                "paper.qmd",
            )
            .unwrap();
        let request = PreviewRequest {
            protocol: 1,
            kind: "quarto".into(),
            project: "paper".into(),
            origin: "https://paper.example".into(),
            snapshot: "revision".into(),
            generation: 1,
            engine: "quarto".into(),
            quarto: Some(QuartoJobOptions {
                binding_id: binding.id,
                main: "paper.qmd".into(),
                format: "pdf".into(),
                ..Default::default()
            }),
            calepin: None,
            manifest: vec![ManifestEntry {
                path: "paper.qmd".into(),
                sha256: crate::quarto::sha256(source),
                size: source.len() as u64,
            }],
            workspace: None,
            builder: None,
            output: None,
            entrypoint: None,
        };
        let mut previews = Previews::default();
        let (id, _) = previews.start(&request, &bindings).await.unwrap();
        let mut artifact = None;
        for _ in 0..120 {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            artifact = previews.0.get(&id).unwrap().latest.lock().await.clone();
            if artifact.is_some() {
                break;
            }
        }
        previews.stop(&id).await;
        let artifact = artifact.expect("Quarto preview did not publish a PDF");
        assert_eq!(artifact.kind, ArtifactKind::Pdf);
        assert!(super::super::looks_complete(
            ArtifactKind::Pdf,
            "paper.qmd",
            &artifact.bytes
        ));
    }
}
