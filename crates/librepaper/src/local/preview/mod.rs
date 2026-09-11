//! Author-only managed watchers. Their URLs are never portable bundle
//! artifacts.
//!
//! Engine-neutral: a `Session` watches one long-running rendering process
//! (`quarto preview` or `calepin watch`) and exposes the same shape --
//! status, page, log tail, stop -- regardless of which adapter drives it.
//! `preview::quarto` and `preview::calepin` are the only two modules that
//! know how to build the command line and recognize the process's own log
//! lines; everything else here (pumps, the stable-file wait, the log
//! ring-buffer, expiry) is shared.

pub(crate) mod calepin;
pub(crate) mod quarto;

use crate::local::protocol::PreviewRequest;
use crate::local::quarto::BindingStore;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    sync::Mutex as AsyncMutex,
};

/// Bound on the combined stdout/stderr log kept per preview. Truncated from
/// the front so the tail -- the part a reader actually wants to see -- is
/// always what survives.
pub(crate) const PREVIEW_LOG_CAP_BYTES: usize = 16 * 1024;

/// Which shape the session's rendering process produces. Determines both
/// the completeness check applied to a fresh render and the `content-type`/
/// `x-librepaper-kind` a reader sees at `GET previews/{id}/page`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArtifactKind {
    Html,
    Pdf,
}

impl ArtifactKind {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            ArtifactKind::Html => "html",
            ArtifactKind::Pdf => "pdf",
        }
    }

    pub(crate) fn content_type(&self) -> &'static str {
        match self {
            ArtifactKind::Html => "text/html; charset=utf-8",
            ArtifactKind::Pdf => "application/pdf",
        }
    }
}

/// The last render a session's process finished writing in full -- accepted
/// only once `looks_complete` passes on a size/mtime-stable read of the
/// output file. `GET previews/{id}/page` serves this and only this: never
/// the file on disk directly, which the rendering process may rewrite in
/// place between an intermediate pass and its final one.
#[derive(Clone)]
pub(crate) struct Artifact {
    pub kind: ArtifactKind,
    pub bytes: Vec<u8>,
    pub sha256: String,
}

/// What one preview adapter (Quarto, Calepin, ...) knows about its own
/// process: how to recognize a render starting and finishing in its log
/// output, and where the finished render lands on disk.
pub(crate) trait Watch: Send + Sync {
    /// Whether `line` (already ANSI-stripped) marks the start of a render.
    fn rendering_started(&self, line: &str) -> bool;
    /// Whether `line` (already ANSI-stripped) marks a render's completion.
    /// A completed render is not necessarily an accepted one: the output
    /// file still has to pass a stable read and `looks_complete`.
    fn render_finished(&self, line: &str) -> bool;
    /// The file a finished render is expected to have written, if the
    /// adapter can currently name one. `None` defers acceptance without
    /// treating the render as failed outright.
    fn output_path(&self, root: &Path) -> Option<PathBuf>;
    /// The artifact shape this adapter's process produces.
    fn kind(&self) -> ArtifactKind;
}

/// A checked, adapter-built command ready to spawn, plus what the generic
/// session bookkeeping needs to track it. Built entirely by
/// `preview::quarto::plan` or `preview::calepin::plan`; this module adds
/// only the stdio wiring and process-group isolation common to every
/// adapter.
pub(crate) struct Plan {
    pub command: Command,
    pub adapter: Box<dyn Watch>,
    pub entrypoint: String,
    pub root: PathBuf,
    pub binding_id: String,
}

pub(crate) struct Session {
    pub engine: String,
    pub kind: ArtifactKind,
    pub origin: String,
    pub project: String,
    pub binding: String,
    pub root: PathBuf,
    /// Project-relative path to the watched document. The log pump's own
    /// `SessionWatch` holds a copy it uses to locate and validate renders;
    /// this copy is kept on `Session` too so a future reader of the struct
    /// (a status field, a diagnostic) does not have to thread it back out.
    #[allow(dead_code)]
    pub entrypoint: String,
    /// No managed web server runs any more: kept only for wire
    /// compatibility with clients that still read `url` off the response.
    /// Always empty.
    pub url: String,
    pub started: Instant,
    /// The last `PREVIEW_LOG_CAP_BYTES` of combined stdout+stderr from the
    /// watching process.
    pub log: Arc<AsyncMutex<String>>,
    /// `None` until the first render completes.
    pub latest: Arc<AsyncMutex<Option<Artifact>>>,
    /// Set while a render is in flight, cleared whether or not that
    /// render's output passed `looks_complete`.
    pub rendering: Arc<AtomicBool>,
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

/// The last `max_bytes` of a session's combined stdout+stderr log.
pub(crate) async fn log_tail(session: &Session, max_bytes: usize) -> String {
    let log = session.log.lock().await;
    tail_within(&log, max_bytes)
}

/// What a log pump task needs to drive `rendering`/`latest` for one session,
/// shared between the stdout and stderr pumps.
struct SessionWatch {
    root: PathBuf,
    entrypoint: String,
    adapter: Box<dyn Watch>,
    log: Arc<AsyncMutex<String>>,
    latest: Arc<AsyncMutex<Option<Artifact>>>,
    rendering: Arc<AtomicBool>,
}

fn strip_ansi(line: &str) -> String {
    static ANSI: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = ANSI.get_or_init(|| regex::Regex::new(r"\x1b\[[0-9;]*m").expect("constant pattern"));
    re.replace_all(line, "").into_owned()
}

fn hex_sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// A rendered HTML page is only safe to serve once it is fully
/// self-contained: Quarto's Pandoc pass writes the same output file first
/// with figures referenced by a relative `<stem>_files/...` path, and its
/// post-processing pass then rewrites it in place with everything embedded.
/// A reader polling the file in between would see broken images.
fn looks_complete_html(entrypoint: &str, bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    let mut tail_start = text.len().saturating_sub(1024);
    while tail_start < text.len() && !text.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    if !text[tail_start..].to_ascii_lowercase().contains("</html>") {
        return false;
    }
    let stem = std::path::Path::new(entrypoint)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if !stem.is_empty() {
        let src_prefix = format!("src=\"{stem}_files/");
        let href_prefix = format!("href=\"{stem}_files/");
        if text.contains(&src_prefix) || text.contains(&href_prefix) {
            return false;
        }
    }
    static RELATIVE_IMAGE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RELATIVE_IMAGE.get_or_init(|| {
        regex::Regex::new(r#"src="([^"]+\.(?:png|jpe?g|gif|svg|webp))""#).expect("constant pattern")
    });
    for capture in re.captures_iter(&text) {
        let value = &capture[1];
        if !value.starts_with("http://")
            && !value.starts_with("https://")
            && !value.starts_with("data:")
        {
            return false;
        }
    }
    true
}

/// A rendered PDF is only safe to serve once its trailer is on disk: a
/// `calepin`/`typst` watch rewrites the same output path on every
/// recompile, and a reader could otherwise observe a truncated write.
fn looks_complete_pdf(bytes: &[u8]) -> bool {
    if !bytes.starts_with(b"%PDF") {
        return false;
    }
    let tail_start = bytes.len().saturating_sub(32);
    bytes[tail_start..]
        .windows(b"%%EOF".len())
        .any(|window| window == b"%%EOF")
}

/// Whether a stable read of a finished render's output is complete enough
/// to serve, dispatched on the artifact shape the adapter produces.
pub(crate) fn looks_complete(kind: ArtifactKind, entrypoint: &str, bytes: &[u8]) -> bool {
    match kind {
        ArtifactKind::Html => looks_complete_html(entrypoint, bytes),
        ArtifactKind::Pdf => looks_complete_pdf(bytes),
    }
}

/// Wait, bounded to ~3s, until `path`'s size and mtime have been identical
/// across two reads 500ms apart, then read it -- a post-processing rewrite
/// is otherwise indistinguishable mid-write from a finished file of the
/// same size. This interval also spans Calepin's 250ms HTML postprocessing
/// poll, so an intermediate Typst output is not published during that gap.
/// Falls back to whatever is on disk once the deadline passes,
/// since a reader still deserves the newest bytes available.
async fn wait_stable_then_read(path: &std::path::Path) -> Option<Vec<u8>> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let Ok(before) = std::fs::metadata(path) else {
            return None;
        };
        tokio::time::sleep(Duration::from_millis(500)).await;
        let after = std::fs::metadata(path).ok()?;
        let stable = before.len() == after.len()
            && before
                .modified()
                .ok()
                .zip(after.modified().ok())
                .is_some_and(|(a, b)| a == b);
        if stable || Instant::now() >= deadline {
            return std::fs::read(path).ok();
        }
    }
}

/// Handles a render-finished log line: resolves the adapter's output path,
/// waits for a stable read, and accepts the result into `watch.latest` only
/// if it passes `looks_complete`. Always clears `rendering` on the way out,
/// whether or not the render was accepted.
async fn finish_render(watch: &SessionWatch) {
    let note = match watch.adapter.output_path(&watch.root) {
        Some(path) => match wait_stable_then_read(&path).await {
            Some(bytes) if looks_complete(watch.adapter.kind(), &watch.entrypoint, &bytes) => {
                let sha256 = hex_sha256(&bytes);
                let mut latest = watch.latest.lock().await;
                *latest = Some(Artifact {
                    kind: watch.adapter.kind(),
                    bytes,
                    sha256,
                });
                None
            }
            Some(_) => Some(
                "the rendered page was not yet self-contained; keeping the previous render"
                    .to_string(),
            ),
            None => Some("the rendered page could not be read after rendering".to_string()),
        },
        None => Some("the preview process finished but produced no output file".to_string()),
    };
    if let Some(note) = note {
        let mut log = watch.log.lock().await;
        append_bounded(
            &mut log,
            &format!("\n[librepaper] {note}\n"),
            PREVIEW_LOG_CAP_BYTES,
        );
    }
    watch.rendering.store(false, Ordering::SeqCst);
}

async fn process_log_line(watch: &SessionWatch, raw_line: &str) {
    let line = strip_ansi(raw_line);
    if watch.adapter.rendering_started(&line) {
        watch.rendering.store(true, Ordering::SeqCst);
    }
    if watch.adapter.render_finished(&line) {
        finish_render(watch).await;
    }
}

async fn pump<R>(mut reader: R, watch: Arc<SessionWatch>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buf = [0u8; 4096];
    let mut pending = String::new();
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let chunk = String::from_utf8_lossy(&buf[..n]);
                {
                    let mut guard = watch.log.lock().await;
                    append_bounded(&mut guard, &chunk, PREVIEW_LOG_CAP_BYTES);
                }
                pending.push_str(&chunk);
                while let Some(pos) = pending.find('\n') {
                    let line: String = pending.drain(..=pos).collect();
                    process_log_line(&watch, line.trim_end_matches(['\r', '\n'])).await;
                }
            }
        }
    }
    if !pending.is_empty() {
        process_log_line(&watch, pending.trim_end_matches(['\r', '\n'])).await;
    }
}

impl Session {
    /// Whether the watching process is still alive.
    pub(crate) fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

#[derive(Default)]
pub(crate) struct Previews(pub HashMap<String, Session>);

impl Previews {
    pub async fn start(
        &mut self,
        request: &PreviewRequest,
        bindings: &BindingStore,
    ) -> Result<(String, String), String> {
        let engine = if request.engine.is_empty() {
            "quarto"
        } else {
            request.engine.as_str()
        };
        let plan = match engine {
            "quarto" => quarto::plan(&self.0, request, bindings)?,
            "calepin" => calepin::plan(&self.0, request, bindings)?,
            other => return Err(format!("unsupported preview engine: {other}")),
        };
        let mut command = plan.command;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|e| format!("Start preview process: {e}"))?;
        let log: Arc<AsyncMutex<String>> = Arc::new(AsyncMutex::new(String::new()));
        let latest: Arc<AsyncMutex<Option<Artifact>>> = Arc::new(AsyncMutex::new(None));
        let rendering = Arc::new(AtomicBool::new(false));
        let kind = plan.adapter.kind();
        let watch = Arc::new(SessionWatch {
            root: plan.root.clone(),
            entrypoint: plan.entrypoint.clone(),
            adapter: plan.adapter,
            log: log.clone(),
            latest: latest.clone(),
            rendering: rendering.clone(),
        });
        let stdout = child.stdout.take().ok_or("preview process has no stdout")?;
        let stderr = child.stderr.take().ok_or("preview process has no stderr")?;
        tokio::spawn(pump(stdout, watch.clone()));
        tokio::spawn(pump(stderr, watch));
        // Do not expose a dead session when the process rejects its
        // arguments or exits during initial startup. Longer initial renders
        // remain pending.
        tokio::time::sleep(Duration::from_millis(250)).await;
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            crate::local::quarto::terminate_process_group(&mut child).await;
            return Err(format!(
                "preview process exited during startup ({status}); check the project locally"
            ));
        }
        let id = crate::util::new_id();
        self.0.insert(
            id.clone(),
            Session {
                engine: engine.to_string(),
                kind,
                origin: request.origin.clone(),
                project: request.project.clone(),
                binding: plan.binding_id,
                root: plan.root,
                entrypoint: plan.entrypoint,
                url: String::new(),
                started: Instant::now(),
                log,
                latest,
                rendering,
                child,
            },
        );
        Ok((id, String::new()))
    }

    pub async fn stop(&mut self, id: &str) {
        if let Some(mut session) = self.0.remove(id) {
            crate::local::quarto::terminate_process_group(&mut session.child).await;
        }
    }

    pub async fn stop_scope(&mut self, origin: &str, project: &str) {
        let ids: Vec<_> = self
            .0
            .iter()
            .filter(|(_, session)| session.origin == origin && session.project == project)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.stop(&id).await;
        }
    }

    pub async fn reap(&mut self, active_scopes: &[(String, String)], bindings: &BindingStore) {
        // Deliberately does not reap a session merely because its process
        // has exited: a render failure now ends that process quickly (there
        // is no server left to keep it alive), and the reader still needs
        // `GET previews/{id}` and its `page`/`log_tail` to see why. Such a
        // session is cleared by an explicit `DELETE`, by its pairing or
        // binding going away, or by the TTL below.
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
