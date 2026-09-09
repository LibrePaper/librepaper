//! The execution-engine boundary for the local service.
//!
//! The service owns admission, authentication, queueing, cancellation and
//! persistence.  This module owns the small amount of engine selection needed
//! to hand a request to the implementation that knows its source inventory,
//! invocation policy, output capture and publication exclusions. Quarto is
//! the only computation engine currently supported by this adapter surface;
//! TeX and Biber continue through the existing native runner.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::Value;
use tokio::process::Command;
use tokio::sync::{mpsc, watch};

use super::protocol::{
    Capabilities, JobOutcome, JobRequest, JobStatus, QuartoJobOptions, Workspace,
};
use super::{native, quarto, quarto_capture};

/// The engine selected for a local job.  The `Native` arm is the compatibility
/// path for the pre-adapter TeX/Biber jobs; it does not grant another engine
/// access to the Quarto execution surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobAdapter {
    Native,
    Quarto,
}

/// Selects an adapter from the explicit wire job kind and engine field.
/// Unknown kinds never fall through to native execution.  Calepin is named
/// here so adding its wire vocabulary cannot accidentally enable execution
/// before its adapter, grants and capture policy exist.
pub fn select(request: &JobRequest) -> Result<JobAdapter, String> {
    match request.kind.as_str() {
        "tex"
            if matches!(
                request.engine.as_str(),
                "" | "pdflatex" | "xelatex" | "lualatex"
            ) =>
        {
            Ok(JobAdapter::Native)
        }
        "biber" if request.engine.is_empty() => Ok(JobAdapter::Native),
        "quarto" if request.engine.is_empty() || request.engine == "quarto" => {
            Ok(JobAdapter::Quarto)
        }
        "quarto" => Err(format!(
            "unsupported engine {:?} for quarto job",
            request.engine
        )),
        "calepin" => Err("unsupported execution engine: calepin".into()),
        "tex" | "biber" => Err(format!(
            "unsupported engine {:?} for {} job",
            request.engine, request.kind
        )),
        other => Err(format!("unknown job kind: {other}")),
    }
}

/// Dispatches an admitted job while preserving the shared runner contract.
/// The caller still supplies the same workspace, cancellation receiver and
/// progress channel used by the service's queue worker.
pub async fn run(
    request: JobRequest,
    workspace: Workspace,
    cancel: watch::Receiver<bool>,
    progress: mpsc::UnboundedSender<JobStatus>,
    bindings: &super::quarto::BindingStore,
) -> JobOutcome {
    let id = workspace
        .root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    match select(&request) {
        Ok(JobAdapter::Native) => native::run_job(request, workspace, cancel, progress).await,
        Ok(JobAdapter::Quarto) => {
            quarto::run_job_with_bindings(request, workspace, cancel, progress, bindings).await
        }
        Err(error) => unsupported(&request, &id, &error),
    }
}

/// Capabilities remain the existing browser-facing shape.  Keeping discovery
/// behind this function means the service has no engine-specific capability
/// branch and cannot accidentally advertise a future engine.
pub async fn capabilities(refresh: bool) -> Capabilities {
    super::discovery::discover(refresh).await
}

/// Quarto policy support is version-gated by the adapter.  An unavailable or
/// unparsable version stays conservative and permits project defaults only.
pub fn quarto_supported_policies(version: Option<&str>) -> Vec<String> {
    quarto::supported_policies(version)
}

fn unsupported(request: &JobRequest, id: &str, error: &str) -> JobOutcome {
    JobOutcome {
        status: JobStatus {
            id: id.to_string(),
            kind: request.kind.clone(),
            status: "failed".into(),
            stage: "finished".into(),
            error: Some(error.into()),
            snapshot: request.snapshot.clone(),
            generation: request.generation,
            ..Default::default()
        },
        files: Default::default(),
    }
}

/// A checked Quarto command plan.  Paths are supplied only by the local
/// service when the plan is applied; browser values remain typed options and
/// cannot become an executable or shell fragment.
#[derive(Clone, Debug, PartialEq)]
pub struct QuartoInvocationPlan {
    pub main: String,
    pub format: String,
    pub render_scope: super::protocol::QuartoRenderScope,
    pub policy: super::protocol::QuartoRenderPolicy,
    pub profile: Option<String>,
    pub parameters: Vec<(String, Value)>,
}

impl QuartoInvocationPlan {
    pub fn from_options(options: &QuartoJobOptions) -> Result<Self, String> {
        options.validate()?;
        Ok(Self {
            main: options.main.clone(),
            format: options.format.clone(),
            render_scope: options.render_scope,
            policy: options.policy,
            profile: options.profile.clone(),
            parameters: options
                .parameters
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
        })
    }

    /// Appends the fixed argument vector used by the Quarto adapter.  The
    /// caller has already canonicalised the project root and checked the
    /// entrypoint against its binding.
    pub fn apply(&self, command: &mut Command, output: &Path, filter: &Path) {
        command.arg("render");
        if self.render_scope == super::protocol::QuartoRenderScope::Document {
            command.arg(&self.main);
        }
        command
            .arg("--to")
            .arg(&self.format)
            .arg("--no-execute-daemon")
            .arg("--output-dir")
            .arg(output)
            .arg("--lua-filter")
            .arg(filter);
        match self.policy {
            super::protocol::QuartoRenderPolicy::ProjectDefaults => {}
            super::protocol::QuartoRenderPolicy::RefreshComputations => {
                command.arg("--cache-refresh");
            }
            super::protocol::QuartoRenderPolicy::Frozen => {
                // `--use-freezer` selects the saved computation result. The
                // adapter's preflight verifies the complete cache and source
                // identity before this process starts; adding `--no-execute`
                // here makes Quarto drop cached display nodes in 1.10.18.
                command.arg("--use-freezer");
            }
        }
        if let Some(profile) = &self.profile {
            command.arg("--profile").arg(profile);
        }
        for (name, value) in &self.parameters {
            let value = serde_json::to_string(value).unwrap_or_else(|_| "null".into());
            command.arg("-P").arg(format!("{name}:{value}"));
        }
    }
}

/// Delegates source inventory to the Quarto implementation while keeping the
/// adapter as the only caller-facing engine boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceInventory {
    pub tree_sha256: String,
    pub files: Vec<String>,
}

pub fn quarto_source_inventory(
    root: &Path,
    manifest: &[super::protocol::ManifestEntry],
) -> Result<SourceInventory, String> {
    quarto::inventory_manifest_impl(root, manifest)
}

pub fn quarto_source_tree_inventory(root: &Path) -> Result<SourceInventory, String> {
    quarto::inventory_tree_impl(root)
}

pub fn quarto_computation_fingerprint(
    source: &str,
    entrypoint: &str,
    format: &str,
    profiles: &[String],
    parameters_sha256: Option<&str>,
    dependencies: &[String],
) -> String {
    let mut document = crate::quarto::parse_qmd(source, entrypoint);
    document.dependencies = dependencies.to_vec();
    crate::quarto::computation_fingerprint_for_format(
        &document,
        entrypoint,
        format,
        profiles,
        parameters_sha256,
    )
}

pub fn quarto_capture(
    manifest_path: &Path,
    source_path: &str,
    source: &str,
    output_root: &Path,
) -> Result<quarto_capture::Capture, String> {
    quarto_capture::collect(manifest_path, source_path, source, output_root)
}

/// The files selected for a Quarto publication.  Quarto's source and
/// editorial inputs are shareable by default; generated output and local
/// environments require an explicit `.librepaper-share.json` include.
pub fn quarto_shared_paths(
    root: &Path,
    main: &str,
    files: Vec<String>,
) -> Result<Vec<String>, String> {
    use std::io::Read;

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Share {
        #[serde(default)]
        include: Vec<String>,
    }

    let policy = root.join(".librepaper-share.json");
    let read_policy = std::fs::File::open(&policy).and_then(|file| {
        let mut bytes = Vec::new();
        file.take(64 * 1024 + 1).read_to_end(&mut bytes)?;
        Ok(bytes)
    });
    let include = match read_policy {
        Ok(bytes) => {
            if bytes.len() > 64 * 1024 {
                return Err(".librepaper-share.json exceeds 64 KiB".into());
            }
            serde_json::from_slice::<Share>(&bytes)
                .map_err(|error| format!("invalid .librepaper-share.json: {error}"))?
                .include
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(format!("cannot read .librepaper-share.json: {error}")),
    };
    let config = crate::config::Configuration::default();
    for path in &include {
        crate::document::paths::check(&config.paths(), path)?;
        if !files.contains(path) {
            return Err(format!(
                "shared file {path:?} is missing, ignored, or outside the project"
            ));
        }
    }
    let quarto_stems: BTreeSet<_> = files
        .iter()
        .filter(|file| crate::document::render::is_quarto(file))
        .map(|file| Path::new(file).with_extension(""))
        .collect();
    let mut selected = Vec::new();
    for file in files {
        let path = Path::new(&file);
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let generated_sibling = matches!(
            extension.as_str(),
            "md" | "html" | "htm" | "ipynb" | "pdf" | "docx" | "tex"
        ) && quarto_stems.contains(&path.with_extension(""));
        let generated = generated_sibling
            || file.split('/').any(|part| {
                part == "_freeze"
                    || part == "_site"
                    || part == "_book"
                    || part == "site_libs"
                    || part == "node_modules"
                    || part == "renv"
                    || part == "venv"
                    || part == "env"
                    || part.ends_with("_files")
                    || part.ends_with("_cache")
            });
        let editorial = matches!(
            extension.as_str(),
            "qmd"
                | "md"
                | "bib"
                | "csl"
                | "yml"
                | "yaml"
                | "css"
                | "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "svg"
                | "webp"
                | "pdf"
                | "r"
                | "py"
                | "jl"
                | "lua"
        );
        if file == main || include.contains(&file) || (editorial && !generated) {
            selected.push(file);
        } else {
            eprintln!("not shared: {file} (Quarto output, local input, or environment)");
        }
    }
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(kind: &str, engine: &str) -> JobRequest {
        JobRequest {
            protocol: 1,
            kind: kind.into(),
            project: "project".into(),
            origin: "https://example.test".into(),
            snapshot: "snapshot".into(),
            generation: 1,
            engine: engine.into(),
            main: String::new(),
            stem: String::new(),
            quarto: None,
            manifest: Vec::new(),
            options: Default::default(),
        }
    }

    #[test]
    fn explicit_job_gate_keeps_quarto_narrow() {
        assert_eq!(select(&request("quarto", "")), Ok(JobAdapter::Quarto));
        assert_eq!(select(&request("quarto", "quarto")), Ok(JobAdapter::Quarto));
        assert_eq!(select(&request("tex", "")), Ok(JobAdapter::Native));
        assert_eq!(
            select(&request("calepin", "")),
            Err("unsupported execution engine: calepin".into())
        );
        assert!(select(&request("future", "")).is_err());
    }

    #[test]
    fn invocation_plan_preserves_typed_policy_and_parameters() {
        let mut options = QuartoJobOptions {
            binding_id: "binding".into(),
            main: "paper.qmd".into(),
            ..Default::default()
        };
        options
            .parameters
            .insert("answer".into(), serde_json::json!(42));
        let plan = QuartoInvocationPlan::from_options(&options).expect("valid options");
        assert_eq!(plan.main, "paper.qmd");
        assert_eq!(
            plan.parameters,
            vec![("answer".into(), serde_json::json!(42))]
        );
    }

    #[test]
    fn policy_capabilities_are_version_gated() {
        assert_eq!(
            quarto_supported_policies(Some("Quarto 1.2.9")),
            vec!["project-defaults"]
        );
        assert!(quarto_supported_policies(Some("Quarto 1.3.0")).contains(&"frozen".into()));
        assert_eq!(quarto_supported_policies(None), vec!["project-defaults"]);
    }
}
