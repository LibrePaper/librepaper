//! The Calepin preview adapter: command construction, option validation and
//! log-line recognition for a managed `calepin watch` watcher over a Typst
//! document.
//!
//! `calepin watch <input.typ> [OUTPUT] --format html|pdf [-- <typst watch
//! args>]` delegates to `typst watch`. For HTML it writes a self-contained
//! page (images embedded as data URIs) and, unless told `--no-serve`,
//! starts its own web server we never want; `--no-serve` is forwarded
//! through Calepin's own `--` argument passthrough. It logs `compiling ...`
//! then `compiled successfully in Nms` or `compiled with warnings in Nms`
//! (ANSI-coloured -- stripped before this module ever sees a line).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use super::{ArtifactKind, Plan, Session, Watch};
use crate::local::protocol::{PreviewRequest, Tool};
use crate::local::quarto::{verify_bound_manifest, BindingStore};

pub(crate) struct CalepinWatch {
    entrypoint: String,
    kind: ArtifactKind,
}

impl Watch for CalepinWatch {
    fn rendering_started(&self, line: &str) -> bool {
        line.contains("compiling")
    }

    fn render_finished(&self, line: &str) -> bool {
        line.contains("compiled successfully in") || line.contains("compiled with warnings in")
    }

    fn output_path(&self, root: &Path) -> Option<PathBuf> {
        Some(root.join(sibling_output(&self.entrypoint, self.kind)))
    }

    fn kind(&self) -> ArtifactKind {
        self.kind
    }
}

/// Where Calepin is told to write its output: beside the entrypoint, same
/// stem, extension matching the requested format.
fn sibling_output(entrypoint: &str, kind: ArtifactKind) -> PathBuf {
    let extension = match kind {
        ArtifactKind::Html => "html",
        ArtifactKind::Pdf => "pdf",
    };
    Path::new(entrypoint).with_extension(extension)
}

/// Validates a `POST previews` request naming the Calepin engine and builds
/// the `calepin watch` command to run. `existing` is the live session map,
/// consulted for the same one-binding/one-root exclusions the Quarto
/// adapter enforces.
pub(crate) fn plan(
    existing: &HashMap<String, Session>,
    request: &PreviewRequest,
    bindings: &BindingStore,
) -> Result<Plan, String> {
    let options = request.calepin.as_ref().ok_or("missing Calepin options")?;
    options.validate()?;
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
    // A hosted binding names no fixed entrypoint: each preview names its
    // own `.typ`, already validated as a safe relative path by
    // `options.validate()` above. A granted binding still refuses any
    // entrypoint but the one it was granted for.
    let entrypoint = if hosted {
        options.main.clone()
    } else {
        binding.entrypoint.clone()
    };
    if !hosted && options.main != binding.entrypoint {
        return Err("calepin entrypoint does not match the granted project binding".into());
    }
    if !request.manifest.iter().any(|f| f.path == entrypoint) {
        return Err("Preview inventory must contain the bound entrypoint".into());
    }
    if std::fs::canonicalize(&binding.root).map_err(|e| e.to_string())? != binding.root {
        return Err("Bound root changed".into());
    }
    verify_bound_manifest(&binding.root, &request.manifest)?;
    let main =
        std::fs::canonicalize(binding.root.join(&entrypoint)).map_err(|e| e.to_string())?;
    if !main.starts_with(&binding.root) {
        return Err("Preview entrypoint escapes bound root".into());
    }
    let kind = match options.format.as_str() {
        "pdf" => ArtifactKind::Pdf,
        _ => ArtifactKind::Html,
    };
    let output = sibling_output(&entrypoint, kind);
    let mut command = Command::new(find_calepin().ok_or("Calepin is not installed")?);
    command
        .current_dir(&binding.root)
        .arg("watch")
        .arg(&entrypoint)
        .arg(&output)
        .args(["--format", &options.format]);
    if kind == ArtifactKind::Html {
        // typst's own HTML watch mode starts a web server unless told
        // otherwise; `--serve`/`--port` are website-only and never used
        // here. This app renders the file itself.
        command.arg("--").arg("--no-serve");
    }

    Ok(Plan {
        command,
        adapter: Box::new(CalepinWatch {
            entrypoint: entrypoint.clone(),
            kind,
        }),
        entrypoint,
        root: binding.root,
        binding_id: options.binding_id.clone(),
    })
}

/// Discover Calepin without exposing its path. A configured path is
/// accepted for tests and app packaging; normal operation searches PATH
/// explicitly -- the same shape as `local::quarto::find_quarto`.
pub(crate) fn find_calepin() -> Option<PathBuf> {
    let configured = std::env::var_os("LIBREPAPER_CALEPIN_PATH").map(PathBuf::from);
    configured
        .filter(|path| path.is_file())
        .or_else(|| executable(if cfg!(windows) { "calepin.exe" } else { "calepin" }))
}

fn executable(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())?
        .into_iter()
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}

/// `GET capabilities`'s `calepin` entry: whether the tool was found and its
/// version string, probed the same bounded-timeout way as the other tool
/// probes in this codebase (`local::discovery::probe_version`,
/// `local::quarto::bounded_version_output`).
pub(crate) async fn discover() -> Tool {
    let Some(path) = find_calepin() else {
        return Tool {
            available: false,
            note: "calepin not found: install Calepin or add it to PATH".into(),
            ..Default::default()
        };
    };
    let version = version_of(&path).await;
    Tool {
        available: version.is_some(),
        version,
        note: String::new(),
    }
}

async fn version_of(path: &Path) -> Option<String> {
    let (stdout, stderr) = bounded_version_output(path).await?;
    let value = if stdout.trim().is_empty() {
        stderr.trim().to_owned()
    } else {
        stdout.trim().to_owned()
    };
    (!value.is_empty()).then(|| value.lines().next().unwrap_or(&value).to_string())
}

async fn bounded_version_output(path: &Path) -> Option<(String, String)> {
    let mut command = Command::new(path);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take().map(|pipe| tokio::spawn(read_bounded(pipe)));
    let stderr = child.stderr.take().map(|pipe| tokio::spawn(read_bounded(pipe)));
    let status = match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
        Ok(Ok(status)) => status,
        _ => {
            let _ = child.kill().await;
            let _ = join_streams(stdout, stderr).await;
            return None;
        }
    };
    let (stdout, stderr) = join_streams(stdout, stderr).await;
    if !status.success() && stdout.is_empty() && stderr.is_empty() {
        return None;
    }
    Some((
        String::from_utf8_lossy(&stdout).into_owned(),
        String::from_utf8_lossy(&stderr).into_owned(),
    ))
}

async fn read_bounded<R: AsyncRead + Unpin>(mut reader: R) -> Vec<u8> {
    const MAX_PROBE_BYTES: usize = 64 * 1024;
    let mut retained = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                let remaining = MAX_PROBE_BYTES.saturating_sub(retained.len());
                if remaining > 0 {
                    retained.extend_from_slice(&chunk[..count.min(remaining)]);
                }
            }
        }
    }
    retained
}

async fn join_streams(
    stdout: Option<tokio::task::JoinHandle<Vec<u8>>>,
    stderr: Option<tokio::task::JoinHandle<Vec<u8>>>,
) -> (Vec<u8>, Vec<u8>) {
    let stdout = match stdout {
        Some(task) => task.await.unwrap_or_default(),
        None => Vec::new(),
    };
    let stderr = match stderr {
        Some(task) => task.await.unwrap_or_default(),
        None => Vec::new(),
    };
    (stdout, stderr)
}
