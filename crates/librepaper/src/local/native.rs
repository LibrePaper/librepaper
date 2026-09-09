//! The bounded native controller: runs a requested TeX engine (and, when
//! the sources need it, BibTeX/Biber/makeindex) to convergence on a staged
//! project, or a standalone Biber job, inside the confinement `confine.rs`
//! provides. See `docs/specs/wasmtex.md`, "Complete native fallback" and "Native
//! execution boundary".
//!
//! This module never requires `latexmk`: convergence is decided by reading
//! the same log text and the same aux/bcf/idx files a human watching the
//! terminal would, in `texlog.rs`'s `rerun`/`needs_bibtex` and this file's
//! own bibliography/index checks -- the rules `latex/benchmark/native.mjs`
//! delegates to `latexmk` for, done here by hand because a bounded
//! controller cannot spawn an unbounded, unaudited coordinator to compile
//! code collaborators wrote.
//!
//! Every process this module starts is an explicit argument array against
//! an app-resolved absolute path, with `-no-shell-escape`, a cleared
//! environment and, where `confine::detect` found one available, the
//! platform sandbox from `confine::wrap`. Cancellation and the deadline
//! both kill the whole process group, never just the immediate child.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::process::Command;
use tokio::sync::{mpsc, watch, Semaphore};

use super::confine::{self, Plan};
use super::discovery::{self, ToolPaths};
use super::protocol::{
    safe_relative_path, JobOutcome, JobRequest, JobStatus, OutputEntry, Provenance, ToolVersions,
    Workspace, MAX_LOG_BYTES, MAX_PDF_BYTES,
};
use super::texlog;

/// At most one native compile runs at a time. The service is expected to
/// serialize jobs itself; this is the second, load-bearing guard.
fn job_lock() -> &'static Semaphore {
    static LOCK: std::sync::OnceLock<Semaphore> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| Semaphore::new(1))
}

/// Runs one job (`kind` `"tex"` or `"biber"`) to completion, cancellation,
/// or its deadline, and returns the status plus the output bytes.
pub async fn run_job(
    request: JobRequest,
    workspace: Workspace,
    mut cancel: watch::Receiver<bool>,
    progress: mpsc::UnboundedSender<JobStatus>,
) -> JobOutcome {
    let _permit = job_lock()
        .acquire()
        .await
        .expect("job semaphore is never closed");

    let job_id = workspace
        .root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    if let Err(message) = prepare_workspace(&workspace).await {
        return failed(&request, &job_id, &message);
    }
    if let Err(message) = verify_no_symlinks(&workspace.project()) {
        return failed(&request, &job_id, &message);
    }

    let tool_paths = discovery::tool_paths().await;
    // Versions come from the same discovery pass as the paths, and only the
    // versions ever leave this process: provenance names what ran, never
    // where it lives on this machine.
    let versions = discovery::discover(false).await.tools;
    let deadline = Instant::now() + Duration::from_secs(request.options.deadline_seconds.max(1));

    let mut ctx = Ctx {
        request: &request,
        workspace: &workspace,
        job_id,
        tool_paths: &tool_paths,
        versions: &versions,
        cancel: &mut cancel,
        progress: &progress,
        deadline,
    };

    if request.kind == "biber" {
        ctx.run_biber_job().await
    } else {
        ctx.run_tex_job().await
    }
}

async fn prepare_workspace(workspace: &Workspace) -> Result<(), String> {
    for dir in [
        workspace.project(),
        workspace.out(),
        home_dir(workspace),
        texmfvar_dir(workspace),
        texmfconfig_dir(workspace),
        texmfhome_dir(workspace),
    ] {
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| format!("could not prepare workspace: {e}"))?;
    }
    Ok(())
}

fn home_dir(workspace: &Workspace) -> PathBuf {
    workspace.root.join("home")
}
fn texmfvar_dir(workspace: &Workspace) -> PathBuf {
    workspace.root.join("texmfvar")
}
fn texmfconfig_dir(workspace: &Workspace) -> PathBuf {
    workspace.root.join("texmfconfig")
}
fn texmfhome_dir(workspace: &Workspace) -> PathBuf {
    workspace.root.join("texmfhome")
}

/// Refuses a job whose staged project contains a symlink anywhere: a
/// collaborator-controlled tree must not be able to point a path outside
/// `project/`, `out/` or the TeX root this module otherwise limits itself
/// to.
fn verify_no_symlinks(dir: &Path) -> Result<(), String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        if meta.file_type().is_symlink() {
            return Err(format!(
                "refusing staged project: symlink at {}",
                path.display()
            ));
        }
        if meta.is_dir() {
            verify_no_symlinks(&path)?;
        }
    }
    Ok(())
}

fn failed(request: &JobRequest, job_id: &str, message: &str) -> JobOutcome {
    JobOutcome {
        status: JobStatus {
            id: job_id.to_string(),
            kind: request.kind.clone(),
            status: "failed".to_string(),
            stage: "finished".to_string(),
            error: Some(message.to_string()),
            snapshot: request.snapshot.clone(),
            generation: request.generation,
            ..Default::default()
        },
        files: BTreeMap::new(),
    }
}

struct Ctx<'a> {
    request: &'a JobRequest,
    workspace: &'a Workspace,
    job_id: String,
    tool_paths: &'a ToolPaths,
    versions: &'a crate::local::protocol::Tools,
    cancel: &'a mut watch::Receiver<bool>,
    progress: &'a mpsc::UnboundedSender<JobStatus>,
    deadline: Instant,
}

/// What running one subprocess produced.
enum RunOutcome {
    Exited(i32),
    Canceled,
    TimedOut,
    SpawnFailed(String),
}

impl Ctx<'_> {
    fn base_status(&self, stage: &str, passes: u32) -> JobStatus {
        JobStatus {
            id: self.job_id.clone(),
            kind: self.request.kind.clone(),
            status: "running".to_string(),
            stage: stage.to_string(),
            passes,
            snapshot: self.request.snapshot.clone(),
            generation: self.request.generation,
            ..Default::default()
        }
    }

    fn send_progress(&self, status: &JobStatus) {
        let _ = self.progress.send(status.clone());
    }

    fn provenance(&self, engine: &str, confinement: &str) -> Provenance {
        let tools = ToolVersions {
            tex: self.tool_info(engine),
            bibtex: self.tool_info("bibtex"),
            biber: self.tool_info("biber"),
            makeindex: self.tool_info("makeindex"),
            distribution: None,
        };
        Provenance {
            backend: "local".to_string(),
            engine: engine.to_string(),
            tools,
            confinement: confinement.to_string(),
        }
    }

    /// The version discovery recorded for a tool, and nothing else: a path
    /// is this machine's business and never crosses the bridge.
    fn tool_info(&self, tool: &str) -> Option<String> {
        let tools = self.versions;
        let found = match tool {
            "pdflatex" => &tools.pdflatex,
            "xelatex" => &tools.xelatex,
            "lualatex" => &tools.lualatex,
            "bibtex" => &tools.bibtex,
            "bibtex8" => &tools.bibtex8,
            "biber" => &tools.biber,
            "makeindex" => &tools.makeindex,
            _ => return None,
        };
        found.available.then(|| found.version.clone()).flatten()
    }

    /// Builds the environment every native TeX/BibTeX/Biber/makeindex
    /// invocation runs under: cleared, then exactly the variables
    /// `docs/specs/wasmtex.md` names.
    fn base_env(&self, extra_texinputs_dir: &Path) -> Vec<(String, String)> {
        let project = self.workspace.project();
        let path_dirs = format!("{}//:{}:", project.display(), extra_texinputs_dir.display());
        let mut env = vec![
            (
                "HOME".to_string(),
                home_dir(self.workspace).display().to_string(),
            ),
            (
                "TEXMFVAR".to_string(),
                texmfvar_dir(self.workspace).display().to_string(),
            ),
            (
                "TEXMFCONFIG".to_string(),
                texmfconfig_dir(self.workspace).display().to_string(),
            ),
            (
                "TEXMFHOME".to_string(),
                texmfhome_dir(self.workspace).display().to_string(),
            ),
            ("openout_any".to_string(), "p".to_string()),
            ("openin_any".to_string(), "p".to_string()),
            ("LC_ALL".to_string(), "C.UTF-8".to_string()),
            ("SOURCE_DATE_EPOCH".to_string(), "0".to_string()),
            ("TEXINPUTS".to_string(), path_dirs.clone()),
            ("BIBINPUTS".to_string(), path_dirs.clone()),
            ("BSTINPUTS".to_string(), path_dirs),
        ];
        let path = match self.tool_paths.bin_dir() {
            Some(dir) => dir.display().to_string(),
            None => String::new(),
        };
        env.push(("PATH".to_string(), path));
        env
    }

    /// Builds, confines (when available) and runs one command, waiting for
    /// exit, the deadline, or cancellation, whichever comes first.
    async fn spawn(
        &mut self,
        tool: &Path,
        args: &[&str],
        cwd: &Path,
        env: &[(String, String)],
    ) -> RunOutcome {
        let mut command = Command::new(tool);
        command.args(args).current_dir(cwd).env_clear();
        for (key, value) in env {
            command.env(key, value);
        }
        command.stdin(std::process::Stdio::null());
        command.stdout(std::process::Stdio::null());
        command.stderr(std::process::Stdio::null());
        #[cfg(unix)]
        {
            command.process_group(0);
        }

        let confinement = confine::detect();
        if confinement.available {
            let plan = Plan {
                workspace: self.workspace.root.clone(),
                // Never `/` as a fallback: a read-only bind of the root,
                // applied after the workspace bind, would sit on top of it
                // and turn every output write into a refusal. Without a
                // known TeX root the tool's own prefix binds in `confine`
                // are what the engine reads from.
                read_only: std::iter::once(self.workspace.project())
                    .chain(self.tool_paths.texmf_root().map(Path::to_path_buf))
                    .collect(),
                network: false,
            };
            if let Err(message) = confine::wrap(&mut command, &plan) {
                return RunOutcome::SpawnFailed(format!(
                    "confinement required but unavailable: {message}"
                ));
            }
        }

        run_confined(command, self.cancel, self.deadline).await
    }
}

async fn run_confined(
    mut command: Command,
    cancel: &mut watch::Receiver<bool>,
    deadline: Instant,
) -> RunOutcome {
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => return RunOutcome::SpawnFailed(err.to_string()),
    };
    let pid = child.id();
    let remaining = deadline.saturating_duration_since(Instant::now());
    let sleep = tokio::time::sleep(remaining);
    tokio::pin!(sleep);

    loop {
        tokio::select! {
            status = child.wait() => {
                return match status {
                    Ok(status) => RunOutcome::Exited(status.code().unwrap_or(-1)),
                    Err(err) => RunOutcome::SpawnFailed(err.to_string()),
                };
            }
            _ = &mut sleep => {
                kill_tree(pid);
                let _ = child.wait().await;
                return RunOutcome::TimedOut;
            }
            changed = cancel.changed() => {
                if changed.is_err() {
                    continue;
                }
                if *cancel.borrow() {
                    kill_tree(pid);
                    let _ = child.wait().await;
                    return RunOutcome::Canceled;
                }
            }
        }
    }
}

#[cfg(unix)]
fn kill_tree(pid: Option<u32>) {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    const SIGKILL: i32 = 9;
    if let Some(pid) = pid {
        let pid = pid as i32;
        unsafe {
            // The whole process group first (the child was spawned as its
            // own group leader), then the child directly in case it never
            // made it into its own group (a spawn error between fork and
            // setpgid, in effect).
            let _ = kill(-pid, SIGKILL);
            let _ = kill(pid, SIGKILL);
        }
    }
}

#[cfg(windows)]
fn kill_tree(pid: Option<u32>) {
    if let Some(pid) = pid {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
}

fn read_to_string_lossy(path: &Path) -> String {
    std::fs::read(path)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

/// The tail of a log/byte stream, bounded to `limit` bytes, on a UTF-8
/// boundary where the input is text.
fn truncate_tail(bytes: Vec<u8>, limit: usize) -> Vec<u8> {
    if bytes.len() <= limit {
        return bytes;
    }
    let start = bytes.len() - limit;
    bytes[start..].to_vec()
}

/// Creates every subdirectory of `project` (recursively) under `out`, so a
/// nested source's own output files (its aux, its log, its index) have a
/// directory to be written into -- `-output-directory` never creates one
/// itself.
fn mirror_directories(project: &Path, out: &Path) {
    let Ok(entries) = std::fs::read_dir(project) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Ok(rel) = path.strip_prefix(project) {
                let _ = std::fs::create_dir_all(out.join(rel));
            }
            mirror_directories(&path, out);
        }
    }
}

fn project_relative_paths(project_root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    walk_paths(project_root, project_root, &mut out);
    out
}

fn walk_paths(dir: &Path, root: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_paths(&path, root, out);
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// Every `\@input{...aux}` line's target, recursively, so a multi-`\include`
/// document's citations/labels are read from all of its aux files, not just
/// the main one.
fn aux_files(out_dir: &Path, stem: &str) -> Vec<PathBuf> {
    let mut seen = Vec::new();
    let mut stack = vec![out_dir.join(format!("{stem}.aux"))];
    while let Some(path) = stack.pop() {
        if seen.contains(&path) || !path.is_file() {
            continue;
        }
        let text = read_to_string_lossy(&path);
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("\\@input{") {
                if let Some(name) = rest.strip_suffix('}') {
                    stack.push(out_dir.join(name));
                }
            }
        }
        seen.push(path);
    }
    seen
}

fn aux_bundle_signature(out_dir: &Path, stem: &str) -> Vec<u8> {
    let mut combined = Vec::new();
    for path in aux_files(out_dir, stem) {
        combined.extend(std::fs::read(&path).unwrap_or_default());
        combined.push(0);
    }
    combined
}

fn aux_mentions_bibliography(out_dir: &Path, stem: &str) -> bool {
    aux_files(out_dir, stem).iter().any(|path| {
        let text = read_to_string_lossy(path);
        text.contains("\\bibdata") || text.contains("\\citation")
    })
}

fn file_entry(path: &Path) -> Option<(OutputEntry, Vec<u8>)> {
    let bytes = std::fs::read(path).ok()?;
    let sha256 = sha256_hex(&bytes);
    Some((
        OutputEntry {
            size: bytes.len() as u64,
            sha256,
        },
        bytes,
    ))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

impl Ctx<'_> {
    /// The `kind: "tex"` job: the requested engine, then whichever of
    /// Biber/BibTeX/makeindex the outputs call for, until the rerun
    /// heuristics are quiet and the aux bundle stops changing, bounded by
    /// `max_passes` and the whole-job deadline.
    async fn run_tex_job(&mut self) -> JobOutcome {
        if !safe_relative_path(&self.request.main) {
            return failed(self.request, &self.job_id, "invalid main path");
        }
        let engine = self.request.engine.clone();
        if !matches!(engine.as_str(), "pdflatex" | "xelatex" | "lualatex") {
            return failed(
                self.request,
                &self.job_id,
                &format!("unknown engine: {engine}"),
            );
        }
        let Some(engine_tool) = self.tool_paths.get(&engine).map(Path::to_path_buf) else {
            return failed(
                self.request,
                &self.job_id,
                &format!("tool-missing: {engine}"),
            );
        };

        let main_rel = PathBuf::from(&self.request.main);
        let main_abs = self.workspace.project().join(&main_rel);
        if !main_abs.is_file() {
            return failed(self.request, &self.job_id, "main file not staged");
        }
        let main_dir = main_abs
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.workspace.project());
        let stem = main_rel
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "main".to_string());
        let main_file_name = main_abs.file_name().unwrap().to_string_lossy().into_owned();

        let out_dir = self.workspace.out();
        // `-output-directory` never creates a directory for a nested
        // `\include`/`\input`'s own aux file (`chapters/01.aux` for a
        // `\include{chapters/01}`) -- kpathsea's paranoid `openout_any=p`
        // then refuses to write it and TeX stops dead. Mirror the
        // project's own subdirectory tree into `out/` up front so every
        // nested aux/log/idx file has somewhere to land.
        mirror_directories(&self.workspace.project(), &out_dir);
        let mut passes = 0u32;
        let mut incompatible = false;
        // Every loop iteration below sets this before the loop can end, so
        // the initial value is never itself read -- it exists only so the
        // variable is initialized for the borrow checker across `continue`.
        #[allow(unused_assignments)]
        let mut last_status: Option<i32> = None;
        let mut previous_aux = Vec::new();

        loop {
            passes += 1;
            self.send_progress(&self.base_status("tex", passes));

            let out_dir_string = out_dir.display().to_string();
            let output_directory_arg = format!("-output-directory={out_dir_string}");
            // `-fmt=<engine>` rather than relying on argv[0]: the resolved,
            // canonicalized tool path from `discovery.rs` is very often
            // `pdftex` (or `xetex`/`luahbtex`) once a distribution's
            // `pdflatex`/`xelatex`/`lualatex` symlink is followed, and these
            // engines otherwise pick their preloaded format by the name they
            // were invoked as -- an absolute, symlink-resolved path would
            // silently run as plain TeX with none of LaTeX's own commands
            // defined.
            let fmt_arg = format!("-fmt={engine}");
            let args = [
                fmt_arg.as_str(),
                "-no-shell-escape",
                "-interaction=nonstopmode",
                "-halt-on-error",
                "-recorder",
                "-synctex=1",
                output_directory_arg.as_str(),
                main_file_name.as_str(),
            ];
            let env = self.base_env(&main_dir);
            let outcome = self.spawn(&engine_tool, &args, &main_dir, &env).await;

            match outcome {
                RunOutcome::Canceled => {
                    return self.terminal(&engine, "canceled", None, "", false, passes)
                }
                RunOutcome::TimedOut => {
                    return self.terminal(&engine, "failed", Some("timeout"), "", false, passes)
                }
                RunOutcome::SpawnFailed(message) => {
                    return failed(self.request, &self.job_id, &message);
                }
                RunOutcome::Exited(code) => {
                    last_status = Some(code);
                }
            }

            let log_path = out_dir.join(format!("{stem}.log"));
            let log = read_to_string_lossy(&log_path);

            if passes >= self.request.options.max_passes {
                break;
            }

            let mut ran_bibliography = false;
            let bcf_path = out_dir.join(format!("{stem}.bcf"));
            let idx_path = out_dir.join(format!("{stem}.idx"));

            if bcf_path.is_file()
                && (texlog::rerun(&log) || !out_dir.join(format!("{stem}.bbl")).is_file())
            {
                self.send_progress(&self.base_status("biber", passes));
                match self.run_biber_for_tex(&out_dir, &main_dir, &stem).await {
                    Ok(flag) => {
                        incompatible = incompatible || flag;
                        ran_bibliography = true;
                    }
                    Err(outcome) => return outcome,
                }
            } else if aux_mentions_bibliography(&out_dir, &stem) && texlog::needs_bibtex(&log) {
                self.send_progress(&self.base_status("bibtex", passes));
                match self.run_bibtex(&out_dir, &stem).await {
                    Ok(()) => ran_bibliography = true,
                    Err(outcome) => return outcome,
                }
            }

            if idx_path.is_file() {
                self.send_progress(&self.base_status("makeindex", passes));
                if let Err(outcome) = self.run_makeindex(&out_dir, &stem).await {
                    return outcome;
                }
                ran_bibliography = true;
            }

            let current_aux = aux_bundle_signature(&out_dir, &stem);
            let aux_changed = current_aux != previous_aux;
            previous_aux = current_aux;

            if !ran_bibliography && !texlog::rerun(&log) && !aux_changed {
                break;
            }
        }

        let log_path = out_dir.join(format!("{stem}.log"));
        let final_log = read_to_string_lossy(&log_path);
        let ok = matches!(last_status, Some(0) | Some(1));
        self.terminal(
            &engine,
            if ok { "done" } else { "failed" },
            None,
            &final_log,
            incompatible,
            passes,
        )
    }

    #[allow(clippy::result_large_err)]
    async fn run_biber_for_tex(
        &mut self,
        out_dir: &Path,
        main_dir: &Path,
        stem: &str,
    ) -> Result<bool, JobOutcome> {
        let Some(biber) = self.tool_paths.get("biber").map(Path::to_path_buf) else {
            return Err(failed(self.request, &self.job_id, "tool-missing: biber"));
        };
        let args = [
            "--output-directory",
            out_dir.to_str().unwrap_or("."),
            "--input-directory",
            main_dir.to_str().unwrap_or("."),
            stem,
        ];
        let env = self.base_env(main_dir);
        let outcome = self.spawn(&biber, &args, out_dir, &env).await;
        match outcome {
            RunOutcome::Canceled => Err(self.terminal("", "canceled", None, "", false, 0)),
            RunOutcome::TimedOut => Err(self.terminal("", "failed", Some("timeout"), "", false, 0)),
            RunOutcome::SpawnFailed(message) => Err(failed(self.request, &self.job_id, &message)),
            RunOutcome::Exited(_) => {
                let blg = read_to_string_lossy(&out_dir.join(format!("{stem}.blg")));
                Ok(blg_is_incompatible(&blg))
            }
        }
    }

    #[allow(clippy::result_large_err)]
    async fn run_bibtex(&mut self, out_dir: &Path, stem: &str) -> Result<(), JobOutcome> {
        let Some(bibtex) = self.tool_paths.get("bibtex").map(Path::to_path_buf) else {
            return Err(failed(self.request, &self.job_id, "tool-missing: bibtex"));
        };
        let env = self.base_env(&self.workspace.project());
        let outcome = self.spawn(&bibtex, &[stem], out_dir, &env).await;
        match outcome {
            RunOutcome::Canceled => Err(self.terminal("", "canceled", None, "", false, 0)),
            RunOutcome::TimedOut => Err(self.terminal("", "failed", Some("timeout"), "", false, 0)),
            RunOutcome::SpawnFailed(message) => Err(failed(self.request, &self.job_id, &message)),
            RunOutcome::Exited(_) => Ok(()),
        }
    }

    #[allow(clippy::result_large_err)]
    async fn run_makeindex(&mut self, out_dir: &Path, stem: &str) -> Result<(), JobOutcome> {
        let Some(makeindex) = self.tool_paths.get("makeindex").map(Path::to_path_buf) else {
            return Err(failed(
                self.request,
                &self.job_id,
                "tool-missing: makeindex",
            ));
        };
        let idx = format!("{stem}.idx");
        let env = self.base_env(&self.workspace.project());
        let outcome = self.spawn(&makeindex, &[idx.as_str()], out_dir, &env).await;
        match outcome {
            RunOutcome::Canceled => Err(self.terminal("", "canceled", None, "", false, 0)),
            RunOutcome::TimedOut => Err(self.terminal("", "failed", Some("timeout"), "", false, 0)),
            RunOutcome::SpawnFailed(message) => Err(failed(self.request, &self.job_id, &message)),
            RunOutcome::Exited(_) => Ok(()),
        }
    }

    /// The `kind: "biber"` job: the browser already staged `<stem>.bcf` and
    /// the bibliography files in `project/`; run Biber alone and return the
    /// BBL/BLG.
    async fn run_biber_job(&mut self) -> JobOutcome {
        let Some(biber) = self.tool_paths.get("biber").map(Path::to_path_buf) else {
            return failed(self.request, &self.job_id, "tool-missing: biber");
        };
        let stem = self.request.stem.clone();
        if stem.is_empty() || !safe_relative_path(&format!("{stem}.bcf")) {
            return failed(self.request, &self.job_id, "invalid stem");
        }
        let project = self.workspace.project();
        let bcf = project.join(format!("{stem}.bcf"));
        if !bcf.is_file() {
            return failed(self.request, &self.job_id, "no .bcf staged for this stem");
        }

        self.send_progress(&self.base_status("biber", 1));
        let bcf_name = format!("{stem}.bcf");
        let args = ["--output-format=bbl", bcf_name.as_str()];
        let env = self.base_env(&project);
        let outcome = self.spawn(&biber, &args, &project, &env).await;

        match outcome {
            RunOutcome::Canceled => self.terminal("", "canceled", None, "", false, 1),
            RunOutcome::TimedOut => self.terminal("", "failed", Some("timeout"), "", false, 1),
            RunOutcome::SpawnFailed(message) => failed(self.request, &self.job_id, &message),
            RunOutcome::Exited(code) => {
                let blg = read_to_string_lossy(&project.join(format!("{stem}.blg")));
                let incompatible = blg_is_incompatible(&blg);
                let ok = code == 0 && project.join(format!("{stem}.bbl")).is_file();
                let status_word = if ok { "done" } else { "failed" };
                let mut outcome = self.terminal("", status_word, None, &blg, incompatible, 1);
                outcome.status.exit = code;
                if let Some((entry, bytes)) = file_entry(&project.join(format!("{stem}.bbl"))) {
                    outcome.status.outputs.insert("bbl".to_string(), entry);
                    outcome.files.insert("bbl".to_string(), bytes);
                }
                if !blg.is_empty() {
                    let bytes = blg.clone().into_bytes();
                    outcome.status.outputs.insert(
                        "blg".to_string(),
                        OutputEntry {
                            size: bytes.len() as u64,
                            sha256: sha256_hex(&bytes),
                        },
                    );
                    outcome.files.insert("blg".to_string(), bytes);
                }
                if !ok && outcome.status.error.is_none() {
                    outcome.status.error = Some(if incompatible {
                        "incompatible: biber control file version mismatch".to_string()
                    } else {
                        "biber failed".to_string()
                    });
                }
                outcome
            }
        }
    }

    /// Assembles the final `JobOutcome` for a tex job: outputs, diagnostics,
    /// provenance. `status_word` is `"done"`, `"failed"` or `"canceled"`;
    /// `error` overrides the message when the status is not `"done"` and no
    /// more specific one was already produced (a timeout, in practice).
    fn terminal(
        &self,
        engine: &str,
        status_word: &str,
        error: Option<&str>,
        log: &str,
        incompatible: bool,
        passes: u32,
    ) -> JobOutcome {
        let out_dir = self.workspace.out();
        let stem = PathBuf::from(&self.request.main)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "main".to_string());

        let confinement = confine::detect();
        let confinement_word = if confinement.available {
            confinement.kind.clone()
        } else {
            "none".to_string()
        };

        let mut status = JobStatus {
            id: self.job_id.clone(),
            kind: self.request.kind.clone(),
            status: status_word.to_string(),
            stage: "finished".to_string(),
            passes,
            error: error.map(str::to_string),
            snapshot: self.request.snapshot.clone(),
            generation: self.request.generation,
            provenance: self.provenance(engine, &confinement_word),
            incompatible,
            ..Default::default()
        };

        let mut files = BTreeMap::new();

        // A `kind: "biber"` job has no PDF/SyncTeX/log of its own in this
        // shape -- `run_biber_job` attaches its BBL/BLG bytes itself, after
        // this call -- and Biber's log has no `!`-error syntax to read
        // diagnostics from in the first place.
        if (status_word == "done" || status_word == "failed") && self.request.kind == "tex" {
            let project = self.workspace.project();
            // TeX prints the path it opened as kpathsea resolved it, which,
            // with `TEXINPUTS` rooted at the project, is often the
            // project's own absolute workspace path rather than `./x.tex`.
            // Strip it (and the output directory's) back to the
            // project-relative and `<out>/`-relative forms `texlog::parse`
            // already knows how to place, before it reads a single line.
            let project_prefix = format!("{}/", project.display());
            let out_prefix = format!("{}/", out_dir.display());
            let normalized_log = log
                .replace(&project_prefix, "")
                .replace(&out_prefix, "<out>/");

            let paths = project_relative_paths(&project);
            status.diagnostics = texlog::parse(&normalized_log, &self.request.main, &paths);

            let log_bytes = truncate_tail(log.as_bytes().to_vec(), MAX_LOG_BYTES);
            if !log_bytes.is_empty() {
                status.outputs.insert(
                    "log".to_string(),
                    OutputEntry {
                        size: log_bytes.len() as u64,
                        sha256: sha256_hex(&log_bytes),
                    },
                );
                files.insert("log".to_string(), log_bytes);
            }

            let pdf_path = out_dir.join(format!("{stem}.pdf"));
            match std::fs::metadata(&pdf_path) {
                Ok(meta) if meta.len() as usize > MAX_PDF_BYTES => {
                    status.status = "failed".to_string();
                    status.error = Some(format!("pdf exceeds {MAX_PDF_BYTES} bytes"));
                }
                Ok(_) => {
                    if let Some((entry, bytes)) = file_entry(&pdf_path) {
                        status.outputs.insert("pdf".to_string(), entry);
                        files.insert("pdf".to_string(), bytes);
                    }
                }
                Err(_) => {}
            }

            let synctex_path = out_dir.join(format!("{stem}.synctex.gz"));
            if let Some((entry, bytes)) = file_entry(&synctex_path) {
                status.outputs.insert("synctex".to_string(), entry);
                files.insert("synctex".to_string(), bytes);
            }

            if status.status == "done" && !status.outputs.contains_key("pdf") {
                status.status = "failed".to_string();
                status.error = Some("no PDF produced".to_string());
            }
        }

        JobOutcome { status, files }
    }
}

/// Whether the BLG says Biber refused a control file it does not speak --
/// never patched, only reported, with Biber's own message quoted.
fn blg_is_incompatible(blg: &str) -> bool {
    let lower = blg.to_lowercase();
    lower.contains("control file version")
        && (lower.contains("expected") || lower.contains("mismatch"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blg_incompatibility_needs_both_the_phrase_and_a_mismatch_word() {
        assert!(blg_is_incompatible(
            "ERROR - Error: Found biblatex control file version 3.6, expected version 3.9"
        ));
        assert!(!blg_is_incompatible("INFO - This is Biber 2.21"));
    }

    #[test]
    fn truncate_tail_keeps_the_end_and_leaves_a_short_log_untouched() {
        let short = b"hello".to_vec();
        assert_eq!(truncate_tail(short.clone(), 100), short);
        let long = vec![b'x'; 10];
        let tail = truncate_tail(long, 4);
        assert_eq!(tail, vec![b'x'; 4]);
    }
}
