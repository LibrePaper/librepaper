//! Bounded execution and artifact capture for typed local adapters.
use super::{BuildRequest, CommandPlan, Output};
use crate::local::{discovery, native, presets, protocol};
pub(crate) use native::RunOutcome as Outcome;
use protocol::{JobOutcome, JobRequest, JobStatus, Workspace};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};

pub(crate) async fn run_plan_logged(
    plan: &CommandPlan,
    _workspace: &Path,
    mut cancel: watch::Receiver<bool>,
    deadline: Instant,
) -> (Outcome, Vec<u8>) {
    let mut command = Command::new(&plan.executable);
    command.args(&plan.args).current_dir(&plan.cwd).env_clear();
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
    if let Some(path) = plan
        .environment
        .get("PATH")
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("PATH"))
    {
        let paths: Vec<_> = std::env::split_paths(&path)
            .filter(|path| path.is_absolute())
            // Profile symlinks can live outside the sandbox's mounted roots;
            // their canonical Nix store directories remain readable inside it.
            .map(|path| path.canonicalize().unwrap_or(path))
            .collect();
        if let Ok(path) = std::env::join_paths(paths) {
            command.env("PATH", path);
        }
    }
    command.env("openin_any", "p").env("openout_any", "p").envs(
        plan.environment
            .iter()
            .filter(|(key, _)| key.as_str() != "PATH"),
    );
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
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
    let builder_name = request.builder.as_str();
    let builder = match super::BuilderId::parse(builder_name) {
        Ok(value) => value,
        Err(error) => return failed(&request, &id, &error),
    };
    let output = match request.output.as_str() {
        "pdf" => Output::Pdf,
        "html" => Output::Html,
        "docx" => Output::Docx,
        _ => return failed(&request, &id, "unsupported output"),
    };
    let entrypoint = request.entrypoint.as_str();
    let mut options = BTreeMap::new();
    for (key, value) in request.inputs.options() {
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
        if request.preset_revision != Some(preset.semantic_revision) {
            return failed(
                &request,
                &id,
                "preset changed or was not pinned at admission",
            );
        }
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
    // Only a preset can still supply an engine: `engine` left the typed
    // option vocabulary with the direct-TeX builders that were its only
    // users, and `BuildRequestV2::validate_shape` refuses it on a request
    // that names no preset.
    let engine = options.remove("engine");
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
    status.provenance = protocol::BuildProvenance {
        backend: "local".into(),
        version: tools.version(builder_name),
        builder: builder.as_str().into(),
        engine: engine.unwrap_or_default(),
        preset: request.preset.clone(),
        snapshot: request.snapshot.clone(),
        main_path: request
            .source
            .as_ref()
            .map(|source| source.main_path.clone())
            .unwrap_or_else(|| entrypoint.to_owned()),
        input_manifest_sha256: request
            .source
            .as_ref()
            .map(|source| source.manifest_sha256.clone())
            .unwrap_or_default(),
        confinement: "none".into(),
        ..Default::default()
    };
    let _ = progress.send(status.clone());
    let deadline =
        Instant::now() + Duration::from_secs(request.options.deadline_seconds.clamp(1, 3600));
    if let Some(preset_id) = request.preset.as_deref() {
        let still_authorized = store
            .resolve_scoped(
                &request.origin,
                &request.project,
                preset_id,
                presets::WorkspaceMode::Snapshot,
                presets::Operation::Build,
                entrypoint,
            )
            .is_ok_and(|(_, current)| request.preset_revision == Some(current.semantic_revision));
        if !still_authorized {
            return failed(
                &request,
                &status.id,
                "preset changed or was revoked before execution",
            );
        }
    }
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
    // Only a failure produces diagnostics now. The other arm of this
    // condition asked for them on every successful direct-TeX build, because
    // `latexmk` reports overfull boxes and undefined references through a
    // log a caller has to read even when it exits zero. No direct-TeX
    // builder is nameable on the wire any more -- LaTeX is built in the
    // browser -- so what is left is the native builders, which say what went
    // wrong by failing.
    if status.status == "failed" {
        let log = status
            .log_tail
            .replace(&format!("{}/", project.display()), "");
        status.diagnostics = super::diagnostics::normalize(
            &log,
            &request
                .manifest
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>(),
        );
    }
    status.outputs = files
        .iter()
        .map(|(key, bytes)| (key.clone(), protocol::OutputEntry::from_bytes(bytes)))
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
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn preset_runtime_checks_scope_and_captures_verified_artifact() {
        let root = tempfile::tempdir().unwrap();
        let program = root.path().join("typst");
        std::fs::write(
            &program,
            "#!/bin/sh\ntouch spawn-marker\nprintf '%s' '%PDF-fixture' > out/main.pdf\nprintf 'compiler log'\n",
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let tools = discovery::ToolPaths::fixture(BTreeMap::from([("typst".into(), program)]));
        let store = presets::PresetStore::new(root.path());
        let preset: presets::Preset = serde_json::from_value(serde_json::json!({"id":"test","display_name":"Test","base_adapter":"typst","source_formats":["typst"],"semantic_revision":1})).unwrap();
        store.create(preset).unwrap();
        let request: JobRequest = serde_json::from_value(serde_json::json!({
            "protocol": 2, "kind": "build", "project": "p",
            "origin": "https://example.test", "snapshot": "s", "generation": 1,
            "builder": "typst", "workspace": {"mode": "snapshot"},
            "entrypoint": "main.typ", "output": "pdf", "inputs": {"engine": "native"},
            "preset": "test", "preset_revision": 1, "manifest": [],
        }))
        .unwrap();
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
        assert!(!root.path().join("refused/project/spawn-marker").exists());
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
        let mut recovered_legacy = request.clone();
        recovered_legacy.preset_revision = None;
        let legacy = run(
            recovered_legacy,
            stage("legacy-without-revision"),
            tools.clone(),
            rx.clone(),
            tx.clone(),
            &store,
        )
        .await;
        assert_eq!(legacy.status.status, "failed");
        assert!(legacy.files.is_empty());
        assert!(!root
            .path()
            .join("legacy-without-revision/project/spawn-marker")
            .exists());
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
        assert!(root.path().join("built/project/spawn-marker").exists());
        assert_eq!(
            built.status.outputs["pdf"].sha256,
            hex::encode(Sha256::digest(b"%PDF-fixture"))
        );
        assert_eq!(built.status.provenance.preset.as_deref(), Some("test"));
        assert_eq!(built.files["log"], b"compiler log");
        store.revoke(&grant.id).unwrap();
        assert_eq!(
            run(
                request.clone(),
                stage("revoked"),
                tools.clone(),
                rx.clone(),
                tx.clone(),
                &store
            )
            .await
            .status
            .status,
            "failed"
        );
        assert!(!root.path().join("revoked/project/spawn-marker").exists());
    }
}
