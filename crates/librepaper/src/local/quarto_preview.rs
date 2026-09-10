//! Author-only managed watchers. Their URLs are never portable bundle artifacts.
use super::{
    protocol::{JobRequest, QuartoExecutionMode, QuartoRenderPolicy, QuartoRenderScope},
    quarto::{find_quarto, terminate_process_group, verify_bound_manifest, BindingStore},
};
use std::{
    collections::HashMap,
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::AsyncReadExt,
    process::{Child, ChildStderr, ChildStdout, Command},
    sync::Mutex as AsyncMutex,
};

/// Bound on the combined stdout/stderr log kept per preview. Truncated from
/// the front so the tail -- the part a reader actually wants to see -- is
/// always what survives.
pub(crate) const PREVIEW_LOG_CAP_BYTES: usize = 16 * 1024;

pub(crate) struct Preview {
    pub origin: String,
    pub project: String,
    pub binding: String,
    pub root: std::path::PathBuf,
    /// Project-relative path to the watched `.qmd`, used to locate the
    /// rendered output Quarto writes alongside (or under `_site`/`_output`/
    /// `docs`) the source tree.
    pub entrypoint: String,
    /// No managed web server runs any more: kept only for wire
    /// compatibility with clients that still read `url` off the response.
    /// Always empty.
    pub url: String,
    pub started: Instant,
    /// The last `PREVIEW_LOG_CAP_BYTES` of combined stdout+stderr from the
    /// watching `quarto preview` process.
    pub log: Arc<AsyncMutex<String>>,
    child: Child,
}

/// Append `chunk` to `buf`, then drop whole characters off the front until
/// `buf` is at most `cap` bytes -- keeping the tail, which is what a reader
/// polling for a render failure actually wants.
fn append_bounded(buf: &mut String, chunk: &str, cap: usize) {
    buf.push_str(chunk);
    if buf.len() > cap {
        let excess = buf.len() - cap;
        let mut boundary = excess;
        while boundary < buf.len() && !buf.is_char_boundary(boundary) {
            boundary += 1;
        }
        buf.drain(..boundary);
    }
}

/// Return at most the last `max_bytes` bytes of `s`, cut on a char boundary.
fn tail_within(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let start = s.len() - max_bytes;
    let mut boundary = start;
    while boundary < s.len() && !s.is_char_boundary(boundary) {
        boundary += 1;
    }
    s[boundary..].to_string()
}

/// The last `max_bytes` of a preview's combined stdout+stderr log.
pub(crate) async fn log_tail(preview: &Preview, max_bytes: usize) -> String {
    let log = preview.log.lock().await;
    tail_within(&log, max_bytes)
}

async fn pump_stdout(mut reader: ChildStdout, log: Arc<AsyncMutex<String>>) {
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let chunk = String::from_utf8_lossy(&buf[..n]);
                let mut guard = log.lock().await;
                append_bounded(&mut guard, &chunk, PREVIEW_LOG_CAP_BYTES);
            }
        }
    }
}

async fn pump_stderr(mut reader: ChildStderr, log: Arc<AsyncMutex<String>>) {
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let chunk = String::from_utf8_lossy(&buf[..n]);
                let mut guard = log.lock().await;
                append_bounded(&mut guard, &chunk, PREVIEW_LOG_CAP_BYTES);
            }
        }
    }
}

/// Candidate rendered outputs for a watched `.qmd`, checked in order and
/// resolved to the first that exists as a regular file within `root`. Quarto
/// may write the render next to the source, or under a project output
/// directory; canonicalizing and checking `starts_with(root)` refuses a
/// symlink or an entrypoint crafted to escape the bound root.
pub(crate) fn resolve_rendered_page(
    root: &std::path::Path,
    entrypoint: &str,
) -> Option<std::path::PathBuf> {
    let rendered = std::path::Path::new(entrypoint).with_extension("html");
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

impl Preview {
    /// Whether the watching `quarto preview` process is still alive.
    pub(crate) fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

#[derive(Default)]
pub(crate) struct Previews(pub HashMap<String, Preview>);

impl Previews {
    pub async fn start(
        &mut self,
        request: &JobRequest,
        bindings: &BindingStore,
    ) -> Result<(String, String), String> {
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
        if self.0.values().any(|p| p.binding == options.binding_id) {
            return Err("Stop the existing preview before starting another watcher.".into());
        }
        if self.0.len() >= 8 {
            return Err("Too many active previews".into());
        }
        let binding = bindings
            .resolve_scoped(&options.binding_id, &request.origin, &request.project)
            .map_err(|_| "Preview binding is not authorized")?;
        if self.0.values().any(|p| p.root == binding.root) {
            return Err("Another managed preview already watches this project directory".into());
        }
        let hosted = BindingStore::is_hosted(&binding);
        // A hosted binding names no fixed entrypoint: each job (and each
        // preview) names its own `.qmd`, already validated as a safe
        // relative path by `options.validate()` above. A granted binding
        // still refuses any entrypoint but the one it was granted for.
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
        super::quarto::add_declared_inputs(&binding.root, &options.data_inputs, &mut inventory)?;
        verify_bound_manifest(&binding.root, &inventory)?;
        if let Some(expected) = options.shared_tree_sha256.as_deref() {
            let actual = super::quarto::inventory_manifest_impl(&binding.root, &request.manifest)?
                .tree_sha256;
            if actual != expected {
                return Err(
                    "preview source inventory is stale; synchronize before previewing".into(),
                );
            }
        }
        let main =
            std::fs::canonicalize(binding.root.join(&entrypoint)).map_err(|e| e.to_string())?;
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
        // A single self-contained HTML file: no separate `_files/` directory
        // for the local app to also locate and serve.
        command.arg("-M").arg("embed-resources:true");
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|e| format!("Start Quarto preview: {e}"))?;
        let log: Arc<AsyncMutex<String>> = Arc::new(AsyncMutex::new(String::new()));
        let stdout = child.stdout.take().ok_or("Quarto preview has no stdout")?;
        let stderr = child.stderr.take().ok_or("Quarto preview has no stderr")?;
        tokio::spawn(pump_stdout(stdout, log.clone()));
        tokio::spawn(pump_stderr(stderr, log.clone()));
        // Do not expose a dead preview when Quarto rejects its arguments or
        // exits during initial startup. Longer initial renders remain pending.
        tokio::time::sleep(Duration::from_millis(250)).await;
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            terminate_process_group(&mut child).await;
            return Err(format!("Quarto preview exited during startup ({status}); check the project with quarto preview locally"));
        }
        let id = crate::util::new_id();
        self.0.insert(
            id.clone(),
            Preview {
                origin: request.origin.clone(),
                project: request.project.clone(),
                binding: options.binding_id.clone(),
                root: binding.root.clone(),
                entrypoint: entrypoint.clone(),
                url: String::new(),
                started: Instant::now(),
                log,
                child,
            },
        );
        Ok((id, String::new()))
    }

    pub async fn stop(&mut self, id: &str) {
        if let Some(mut preview) = self.0.remove(id) {
            terminate_process_group(&mut preview.child).await;
        }
    }

    pub async fn stop_scope(&mut self, origin: &str, project: &str) {
        let ids: Vec<_> = self
            .0
            .iter()
            .filter(|(_, preview)| preview.origin == origin && preview.project == project)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.stop(&id).await;
        }
    }

    pub async fn reap(&mut self, active_scopes: &[(String, String)], bindings: &BindingStore) {
        // Deliberately does not reap a preview merely because its `quarto
        // preview` process has exited: a render failure now ends that
        // process quickly (there is no server left to keep it alive), and
        // the reader still needs `GET previews/{id}` and its `page`/
        // `log_tail` to see why. Such a preview is cleared by an explicit
        // `DELETE`, by its pairing or binding going away, or by the TTL
        // below.
        let expired: Vec<_> = self
            .0
            .iter()
            .filter_map(|(id, p)| {
                let pairing_expired = !active_scopes
                    .iter()
                    .any(|(origin, project)| origin == &p.origin && project == &p.project);
                (pairing_expired
                    || bindings
                        .get_scoped(&p.binding, &p.origin, &p.project)
                        .is_none()
                    || p.started.elapsed() > Duration::from_secs(3600))
                .then(|| id.clone())
            })
            .collect();
        for id in expired {
            self.stop(&id).await;
        }
    }
}
