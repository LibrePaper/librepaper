//! Bounded execution and artifact capture for typed local adapters.
use super::{BuildRequest, CommandPlan, Output};
use crate::local::{confine, discovery, native, presets, protocol};
pub(crate) use native::RunOutcome as Outcome;
use protocol::{JobOutcome, JobRequest, JobStatus, Workspace};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};

pub(crate) async fn run_plan_logged(
    plan: &CommandPlan,
    workspace: &Path,
    mut cancel: watch::Receiver<bool>,
    deadline: Instant,
) -> (Outcome, Vec<u8>) {
    let mut command = Command::new(&plan.executable);
    command
        .args(&plan.args)
        .current_dir(&plan.cwd)
        .env_clear()
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for key in [
        "HOME",
        "USERPROFILE",
        "SYSTEMROOT",
        "XDG_CACHE_HOME",
        "XDG_CONFIG_HOME",
        "TEXMFHOME",
        "TEXMFCNF",
        "FONTCONFIG_FILE",
        "FONTCONFIG_PATH",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        let paths: Vec<_> = std::env::split_paths(&path)
            .filter(|path| path.is_absolute())
            .collect();
        if let Ok(path) = std::env::join_paths(paths) {
            command.env("PATH", path);
        }
    }
    command
        .env("openin_any", "p")
        .env("openout_any", "p")
        .envs(&plan.environment);
    #[cfg(unix)]
    command.process_group(0);
    if confine::detect().available {
        if let Err(error) = confine::wrap(
            &mut command,
            &confine::Plan {
                workspace: workspace.into(),
                writable: vec![],
                read_only: vec![plan.executable.clone()],
                network: false,
            },
        ) {
            return (
                Outcome::SpawnFailed(format!("confinement required: {error}")),
                Vec::new(),
            );
        }
    }
    native::run_confined_logged(command, &mut cancel, deadline).await
}

pub async fn run(
    request: JobRequest,
    workspace: Workspace,
    tools: discovery::ToolPaths,
    cancel: watch::Receiver<bool>,
    progress: mpsc::UnboundedSender<JobStatus>,
    store: &presets::PresetStore,
) -> JobOutcome {
    let id = workspace
        .root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let builder_name = request.builder.as_deref().unwrap_or_default();
    let builder = match super::BuilderId::parse(builder_name) {
        Ok(value) => value,
        Err(error) => return failed(&request, &id, &error),
    };
    let output = match request.output.as_deref() {
        Some("pdf") => Output::Pdf,
        Some("html") => Output::Html,
        Some("docx") => Output::Docx,
        _ => return failed(&request, &id, "unsupported output"),
    };
    let entrypoint = request.entrypoint.as_deref().unwrap_or(&request.main);
    let mut options = BTreeMap::new();
    for (key, value) in request.builder_options.as_ref().into_iter().flatten() {
        let value = match value {
            serde_json::Value::String(value) => value.clone(),
            serde_json::Value::Bool(value) => value.to_string(),
            _ => return failed(&request, &id, "unsupported typed builder option"),
        };
        options.insert(key.clone(), value);
    }
    let preset = if let Some(preset_id) = request.preset.as_deref() {
        let (_, preset) = match store.resolve_scoped(
            &request.origin,
            &request.project,
            preset_id,
            presets::WorkspaceMode::Snapshot,
            presets::Operation::Build,
            entrypoint,
        ) {
            Ok(value) => value,
            Err(error) => return failed(&request, &id, &error),
        };
        if preset.base_adapter != builder_name {
            return failed(&request, &id, "preset adapter does not match request");
        }
        options = match presets::PresetStore::effective_options(&preset, &options) {
            Ok(value) => value,
            Err(error) => return failed(&request, &id, &error),
        };
        Some(preset)
    } else {
        None
    };
    let engine = options
        .remove("engine")
        .or_else(|| (!request.engine.is_empty()).then(|| request.engine.clone()));
    let project = workspace.project();
    if project.join("out").exists() {
        return failed(
            &request,
            &id,
            "project contains reserved output directory: out",
        );
    }
    if let Err(error) = std::fs::create_dir(project.join("out")) {
        return failed(&request, &id, &error.to_string());
    }
    let mut plan = match super::plan(
        &BuildRequest {
            builder,
            engine: engine.clone(),
            entrypoint: entrypoint.into(),
            output,
            options,
        },
        &tools.all(),
        &project,
    ) {
        Ok(value) => value,
        Err(error) => return failed(&request, &id, &error),
    };
    if let Some(preset) = &preset {
        plan.environment.extend(preset.environment.clone());
        if let Some(wrapper) = &preset.wrapper {
            let wrapper = std::path::PathBuf::from(wrapper);
            if !wrapper.is_absolute() || !wrapper.is_file() {
                return failed(&request, &id, "preset wrapper is unavailable");
            }
            let executable = std::mem::replace(&mut plan.executable, wrapper);
            plan.args
                .insert(0, executable.to_string_lossy().into_owned());
        }
    }
    let mut status = JobStatus {
        id,
        kind: "build".into(),
        status: "running".into(),
        stage: "build".into(),
        snapshot: request.snapshot.clone(),
        generation: request.generation,
        ..Default::default()
    };
    status.provenance = protocol::Provenance {
        backend: "local".into(),
        version: tools.version(if builder_name == "tex" {
            engine.as_deref().unwrap_or("pdflatex")
        } else {
            builder_name
        }),
        builder: builder.as_str().into(),
        engine: engine.unwrap_or_default(),
        preset: request.preset.clone(),
        snapshot: request.snapshot.clone(),
        confinement: confine::detect().kind,
        ..Default::default()
    };
    let _ = progress.send(status.clone());
    let deadline =
        Instant::now() + Duration::from_secs(request.options.deadline_seconds.clamp(1, 3600));
    let (outcome, log) = run_plan_logged(&plan, &workspace.root, cancel, deadline).await;
    status.stage = "finished".into();
    status.log_tail = String::from_utf8_lossy(&log).into_owned();
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    if !log.is_empty() {
        files.insert("log".into(), log);
    }
    match outcome {
        Outcome::Exited(0) => {
            let bytes = std::fs::canonicalize(&plan.output).and_then(|path| {
                if !path.starts_with(&project) || !path.is_file() {
                    return Err(std::io::Error::other("output escaped workspace"));
                }
                crate::local::service::read_bounded_public(&path, protocol::MAX_PDF_BYTES)
            });
            match bytes {
                Ok(bytes)
                    if !bytes.is_empty()
                        && (output != Output::Pdf || bytes.starts_with(b"%PDF-"))
                        && (output != Output::Docx || bytes.starts_with(b"PK")) =>
                {
                    files.insert(output.extension().into(), bytes);
                    status.status = "done".into();
                }
                _ => {
                    status.status = "failed".into();
                    status.error = Some("builder output is missing, invalid, or too large".into());
                }
            }
        }
        Outcome::Exited(code) => {
            status.exit = code;
            status.status = "failed".into();
            status.error = Some(format!("builder exited with status {code}"));
        }
        Outcome::Canceled => status.status = "canceled".into(),
        Outcome::TimedOut => {
            status.status = "failed".into();
            status.error = Some("build deadline exceeded".into());
        }
        Outcome::SpawnFailed(error) => {
            status.status = "failed".into();
            status.error = Some(error);
        }
    }
    if status.status == "failed" || matches!(builder_name, "tex" | "latexmk" | "tectonic") {
        let log = status
            .log_tail
            .replace(&format!("{}/", project.display()), "");
        status.diagnostics = super::diagnostics::normalize(
            builder_name,
            &log,
            entrypoint,
            &request
                .manifest
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>(),
        );
    }
    status.outputs = files
        .iter()
        .map(|(key, bytes)| {
            (
                key.clone(),
                protocol::OutputEntry {
                    size: bytes.len() as u64,
                    sha256: hex::encode(Sha256::digest(bytes)),
                },
            )
        })
        .collect();
    JobOutcome { status, files }
}

fn failed(request: &JobRequest, id: &str, error: &str) -> JobOutcome {
    JobOutcome {
        status: JobStatus {
            id: id.into(),
            kind: "build".into(),
            status: "failed".into(),
            stage: "finished".into(),
            error: Some(error.into()),
            snapshot: request.snapshot.clone(),
            generation: request.generation,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn preset_runtime_checks_scope_and_captures_verified_artifact() {
        let root = tempfile::tempdir().unwrap();
        let program = root.path().join("typst");
        std::fs::write(
            &program,
            "#!/bin/sh\nprintf '%s' '%PDF-fixture' > out/main.pdf\nprintf 'compiler log'\n",
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let tools = discovery::ToolPaths::fixture(BTreeMap::from([("typst".into(), program)]));
        let store = presets::PresetStore::new(root.path());
        let preset: presets::Preset = serde_json::from_value(serde_json::json!({"id":"test","display_name":"Test","base_adapter":"typst","source_formats":["typst"],"semantic_revision":1})).unwrap();
        store.create(preset).unwrap();
        let request: JobRequest = serde_json::from_value(serde_json::json!({"protocol":2,"kind":"build","project":"p","origin":"https://example.test","snapshot":"s","generation":1,"builder":"typst","workspace":{"mode":"snapshot"},"entrypoint":"main.typ","main":"main.typ","output":"pdf","preset":"test","manifest":[]})).unwrap();
        let stage = |name: &str| {
            let workspace = Workspace {
                root: root.path().join(name),
            };
            std::fs::create_dir_all(workspace.project()).unwrap();
            std::fs::write(workspace.project().join("main.typ"), "= Test").unwrap();
            workspace
        };
        let (_cancel, rx) = watch::channel(false);
        let (tx, _) = mpsc::unbounded_channel();
        let refused = run(
            request.clone(),
            stage("refused"),
            tools.clone(),
            rx.clone(),
            tx.clone(),
            &store,
        )
        .await;
        assert_eq!(refused.status.status, "failed");
        assert!(refused.files.is_empty());
        let grant = store
            .grant(
                "https://example.test",
                "p",
                "test",
                presets::WorkspaceMode::Snapshot,
                presets::Operation::Build,
                "main.typ",
                1,
            )
            .unwrap();
        let built = run(
            request.clone(),
            stage("built"),
            tools.clone(),
            rx.clone(),
            tx.clone(),
            &store,
        )
        .await;
        assert_eq!(built.status.status, "done", "{:?}", built.status);
        assert_eq!(built.files["pdf"], b"%PDF-fixture");
        assert_eq!(
            built.status.outputs["pdf"].sha256,
            hex::encode(Sha256::digest(b"%PDF-fixture"))
        );
        assert_eq!(built.status.provenance.preset.as_deref(), Some("test"));
        assert_eq!(built.files["log"], b"compiler log");
        store.revoke(&grant.id).unwrap();
        assert_eq!(
            run(request, stage("revoked"), tools, rx, tx, &store)
                .await
                .status
                .status,
            "failed"
        );
    }
}
