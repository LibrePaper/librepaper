//! Local renders: write the project's files to a folder, run one command
//! there, read one result file.
//!
//! The sidecar knows nothing about any particular tool. The page names a
//! format and an output, `tool` turns that pair into a command and a result
//! path, and this module runs the pipeline:
//!
//! 1. Write the job's files into the persistent workspace (`workspace`).
//! 2. Run the expanded command there through the confined runner, which owns
//!    cancellation, the deadline and the captured log.
//! 3. Read the result file this run wrote, and for a PDF the SyncTeX file
//!    beside it.
//!
//! Exit status 0 with no result file is a failure that names the path the
//! command was expected to write. A result left in the workspace by an
//! earlier run does not count, and a command that fails after writing its
//! result still returns it beside the error.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use tokio::process::Command;
use tokio::sync::{mpsc, watch};

use crate::local::bindings::BindingStore;
use crate::local::native::{self, RunOutcome};
use crate::local::protocol::{
    self, Capabilities, JobOutcome, JobRequest, JobStatus, Provenance, RendererReport, Tool,
    MAX_RESULT_BYTES,
};

pub mod discovery;
pub mod records;
pub mod tool;
pub mod workspace;

pub use tool::{Format, Key, Output, Record};

/// `GET capabilities` and `POST capabilities/rescan`: the platform, Zotero,
/// and every render command with whether its program was found.
pub async fn capabilities(refresh: bool) -> Capabilities {
    let zotero_client = crate::local::zotero::Client::local();
    let (renderers, zotero) = tokio::join!(reports(refresh), zotero_client.probe());
    Capabilities {
        platform: std::env::consts::OS.to_string(),
        zotero: match zotero {
            Ok(()) => Tool {
                available: true,
                version: Some("api-v3".into()),
                note: "Zotero desktop local API is available".into(),
            },
            Err(error) => Tool {
                available: false,
                version: None,
                note: error.to_string(),
            },
        },
        renderers,
    }
}

/// Every render command in use, keyed `<format>.<output>`, with what the
/// program it starts resolves to.
pub async fn reports(refresh: bool) -> BTreeMap<String, RendererReport> {
    let resolved = records::all();
    let programs: Vec<String> = resolved
        .values()
        .filter_map(|resolved| resolved.record.command.first().cloned())
        .collect();
    let found = discovery::report(&programs, refresh).await;
    resolved
        .into_iter()
        .map(|(key, resolved)| {
            let program = resolved
                .record
                .command
                .first()
                .and_then(|program| found.get(program))
                .cloned()
                .unwrap_or_default();
            (
                key.to_string(),
                RendererReport {
                    found: program.found(),
                    version: program.version,
                    edited: resolved.edited,
                    command: resolved.record.command,
                    result: resolved.record.result,
                },
            )
        })
        .collect()
}

/// Runs one admitted job. `staged` holds the uploaded files, which are
/// written into the workspace the binding names before the command starts.
/// A job already running in that workspace has been canceled by admission,
/// and the queue runs one job at a time, so two commands never share a folder.
pub async fn run(
    request: JobRequest,
    staged: PathBuf,
    bindings: &BindingStore,
    mut cancel: watch::Receiver<bool>,
    progress: mpsc::UnboundedSender<JobStatus>,
) -> JobOutcome {
    let key = match Key::parse_parts(&request.format, &request.output) {
        Ok(key) => key,
        Err(error) => return failed(&request, &error),
    };
    let expanded = match records::get(key).record.expand(&request.entrypoint) {
        Ok(expanded) => expanded,
        Err(error) => return failed(&request, &error),
    };
    let program = &expanded.command[0];
    let Some(executable) = discovery::resolve(program) else {
        return failed(
            &request,
            &format!(
                "{program} was not found. Install it, or change the command in Settings > Rendering."
            ),
        );
    };
    // The binding is checked again here rather than only at admission: the
    // queue delay between the two is long enough for the folder to be moved,
    // replaced or revoked, and what runs is what is on disk now.
    let binding = match bindings.validate_scoped_entrypoint(
        &request.binding_id,
        &request.origin,
        &request.project,
        &request.entrypoint,
        true,
    ) {
        Ok(binding) => binding,
        Err(error) => return failed(&request, &error),
    };
    if !request
        .manifest
        .iter()
        .any(|entry| entry.path == request.entrypoint)
    {
        return failed(
            &request,
            "the input inventory must include the entry file",
        );
    }
    let root = match std::fs::canonicalize(&binding.root) {
        Ok(root) if root == binding.root && root.is_dir() => root,
        _ => {
            return failed(
                &request,
                "the project folder changed or is no longer canonical",
            )
        }
    };
    // The editor is the source of truth for the files the manifest names. In
    // a granted folder every other file is left alone, and a file changed
    // outside LibrePaper since the last write is kept as `<name>.local`; the
    // notes say which were kept, and lead the log.
    let mut notes: Vec<String> = Vec::new();
    if BindingStore::is_hosted(&binding) {
        if let Err(error) = workspace::sync_hosted_workspace(&staged, &root, &request.manifest) {
            return failed(&request, &error);
        }
    } else {
        match workspace::sync_granted_folder(
            &staged,
            &root,
            &request.manifest,
            &bindings.written_record(&binding.id),
        ) {
            Ok(kept) => notes = kept,
            Err(error) => return failed(&request, &error),
        }
    }

    let _ = progress.send(JobStatus {
        status: "running".into(),
        stage: "build".into(),
        snapshot: request.snapshot.clone(),
        generation: request.generation,
        provenance: provenance(&request),
        ..Default::default()
    });

    let mut command = Command::new(&executable);
    command
        .args(&expanded.command[1..])
        .current_dir(&root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    native::hide_console(command.as_std_mut());
    #[cfg(unix)]
    command.process_group(0);
    let deadline =
        Instant::now() + Duration::from_secs(request.options.deadline_seconds.clamp(1, 3600));
    let started = SystemTime::now();
    let (outcome, output) = native::run_confined_logged(command, &mut cancel, deadline).await;

    let mut log: Vec<u8> = notes.iter().flat_map(|note| format!("{note}\n").into_bytes()).collect();
    log.extend_from_slice(&output);
    let mut status = JobStatus {
        stage: "finished".into(),
        snapshot: request.snapshot.clone(),
        generation: request.generation,
        log_tail: String::from_utf8_lossy(&log).into_owned(),
        provenance: provenance(&request),
        ..Default::default()
    };
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    match outcome {
        RunOutcome::Exited(0) => match read_result(&root, &expanded.result, started) {
            Ok(Some(bytes)) if !bytes.is_empty() => {
                attach_result(
                    &mut files,
                    &root,
                    key.output == Output::Pdf,
                    &expanded.stem,
                    started,
                    bytes,
                );
                status.status = "done".into();
            }
            Ok(Some(_)) => {
                status.status = "failed".into();
                status.error = Some(format!(
                    "the command exited with status 0 but {} is empty",
                    expanded.result
                ));
            }
            Ok(None) => {
                status.status = "failed".into();
                status.error = Some(format!(
                    "the command exited with status 0 but wrote no {}",
                    expanded.result
                ));
            }
            Err(error) => {
                status.status = "failed".into();
                status.error = Some(error);
            }
        },
        RunOutcome::Exited(code) => {
            status.exit = code;
            status.status = "failed".into();
            status.error = Some(format!("the command exited with status {code}"));
            // LaTeX in nonstop mode can exit nonzero and still write a usable
            // PDF, so a fresh result is shown beside the error.
            let result = read_result(&root, &expanded.result, started)
                .ok()
                .flatten()
                .filter(|bytes| !bytes.is_empty());
            if let Some(bytes) = result {
                attach_result(
                    &mut files,
                    &root,
                    key.output == Output::Pdf,
                    &expanded.stem,
                    started,
                    bytes,
                );
            }
        }
        RunOutcome::Canceled => status.status = "canceled".into(),
        RunOutcome::TimedOut => {
            status.status = "failed".into();
            status.error = Some("the render deadline was exceeded".into());
        }
        RunOutcome::SpawnFailed(error) => {
            status.status = "failed".into();
            status.error = Some(error);
        }
    }
    files.insert("log".into(), log);
    status.outputs = files
        .iter()
        .map(|(name, bytes)| (name.clone(), protocol::OutputEntry::from_bytes(bytes)))
        .collect();
    JobOutcome { status, files }
}

fn provenance(request: &JobRequest) -> Provenance {
    Provenance {
        backend: "local".into(),
        format: request.format.clone(),
        output: request.output.clone(),
    }
}

/// A job that failed before its command started: no log, only the reason.
fn failed(request: &JobRequest, error: &str) -> JobOutcome {
    JobOutcome {
        status: JobStatus {
            status: "failed".into(),
            stage: "finished".into(),
            error: Some(error.into()),
            snapshot: request.snapshot.clone(),
            generation: request.generation,
            provenance: provenance(request),
            ..Default::default()
        },
        files: BTreeMap::new(),
    }
}

/// The bytes of `relative` under `root` when this run wrote it. A file last
/// modified before `started` was left by an earlier run and counts as absent.
fn read_result(root: &Path, relative: &str, started: SystemTime) -> Result<Option<Vec<u8>>, String> {
    match std::fs::metadata(root.join(relative)).and_then(|metadata| metadata.modified()) {
        Ok(modified) if modified < started => Ok(None),
        Ok(_) => read_inside(root, relative),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("could not read {relative}: {error}")),
    }
}

/// The bytes of `relative` under `root`, or none when nothing is there. A
/// path that resolves outside the workspace, or to something that is not a
/// regular file, is an error rather than an absent result.
fn read_inside(root: &Path, relative: &str) -> Result<Option<Vec<u8>>, String> {
    let path = match std::fs::canonicalize(root.join(relative)) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("could not read {relative}: {error}")),
    };
    if !path.starts_with(root) {
        return Err(format!("{relative} is outside the workspace"));
    }
    if !path.is_file() {
        return Err(format!("{relative} is not a file"));
    }
    crate::local::service::read_bounded_public(&path, MAX_RESULT_BYTES)
        .map(Some)
        .map_err(|error| format!("could not read {relative}: {error}"))
}

/// Adds the result to `files`, and for a PDF the SyncTeX file beside it.
fn attach_result(
    files: &mut BTreeMap<String, Vec<u8>>,
    root: &Path,
    pdf: bool,
    stem: &str,
    started: SystemTime,
    bytes: Vec<u8>,
) {
    files.insert("artifact".into(), bytes);
    if pdf {
        if let Some(bytes) = read_synctex(root, stem, started) {
            files.insert("synctex".into(), bytes);
        }
    }
}

/// The SyncTeX file a TeX run leaves beside its PDF: compressed when the
/// engine wrote it so, else plain. Each candidate needs the same freshness.
fn read_synctex(root: &Path, stem: &str, started: SystemTime) -> Option<Vec<u8>> {
    ["synctex.gz", "synctex"].iter().find_map(|extension| {
        read_result(root, &format!("{stem}.{extension}"), started)
            .ok()
            .flatten()
    })
}

#[cfg(all(test, unix))]
mod tests;
