//! Bounded execution and artifact capture for typed local adapters.
use super::{BuildRequest, CommandPlan, Output};
use crate::local::{discovery, native, protocol};
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
    // The engine comes from the request, never from free-form options.
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
    let plan = match super::plan(
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
