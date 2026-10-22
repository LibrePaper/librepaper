//! Small typed Git primitives for desktop Git sync.
//!
//! The page owns all sync logic. This module owns one bare repository per
//! connection under app state and answers a fixed list of operations over it:
//! hash blobs, build trees with a temporary index, make commits, merge, diff,
//! fetch and push. Nothing here checks out a working tree, and the page never
//! supplies a Git argument, only typed fields that are validated first.
//!
//! Commands that touch only the private repository run with an empty global
//! configuration so that no user setting (filters, merge drivers, attributes)
//! applies. Commands that talk to a remote keep the user's configuration,
//! because credential helpers and URL rewrites live there, but run with hooks
//! disabled, prompts off and only the three allowed transports.

use std::collections::HashMap;
use std::ffi::OsStr;
#[cfg(test)]
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::local::store;
use crate::local::tools;

mod objects;
mod remote;
mod storage;
mod validate;

use storage::Conn;

/// Local commands only read and write the private repository.
const LOCAL_TIMEOUT: Duration = Duration::from_secs(60);
/// Transport commands wait on a network.
const TRANSPORT_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// The most stdout one command may produce.
const MAX_STDOUT_BYTES: usize = 64 * 1024 * 1024;
/// The most stderr kept from one command.
const MAX_STDERR_BYTES: usize = 256 * 1024;

/// Environment variables that would redirect Git away from the repository a
/// command names, or inject identity or configuration the page did not ask for.
const AMBIENT: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_AUTHOR_DATE",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "GIT_COMMITTER_DATE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_QUARANTINE_PATH",
];

/// A refused or failed operation, ready to become an HTTP answer.
#[derive(Debug)]
pub(crate) struct GitError {
    pub(crate) status: u16,
    pub(crate) kind: Option<&'static str>,
    pub(crate) message: String,
}

impl GitError {
    fn new(status: u16, kind: Option<&'static str>, message: impl Into<String>) -> Self {
        Self {
            status,
            kind,
            message: message.into(),
        }
    }

    /// The request was malformed.
    pub(crate) fn bad(message: impl Into<String>) -> Self {
        Self::new(400, None, message)
    }

    /// Another operation holds this connection or the folder chooser.
    pub(crate) fn busy(message: impl Into<String>) -> Self {
        Self::new(409, None, message)
    }

    /// The project has no Git connection on this computer.
    fn not_connected() -> Self {
        Self::new(
            404,
            None,
            "This project has no Git connection on this computer.",
        )
    }

    /// Git declined, or the objects asked for are not there.
    fn refused(kind: &'static str, message: impl Into<String>) -> Self {
        Self::new(422, Some(kind), message)
    }

    /// Git could not reach the remote.
    fn remote(kind: &'static str, message: impl Into<String>) -> Self {
        Self::new(502, Some(kind), message)
    }

    /// Something on this computer failed.
    fn internal(message: impl Into<String>) -> Self {
        Self::new(500, None, message)
    }

    /// The JSON body of the answer.
    pub(crate) fn to_json(&self) -> Value {
        match self.kind {
            Some(kind) => json!({"error": self.message, "kind": kind}),
            None => json!({"error": self.message}),
        }
    }
}

/// Which of the two execution environments a command runs in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum GitEnv {
    /// Empty global configuration, no system configuration.
    Local,
    /// The user's configuration, with hooks off and transports limited.
    Transport,
}

/// Where a command runs.
#[derive(Clone, Copy)]
enum At<'a> {
    /// Against this bare repository.
    Repo(&'a Path),
    /// In this directory with no repository named, so Git discovers its own.
    Dir(&'a Path),
}

/// What a finished command left behind.
struct GitOutput {
    code: i32,
    stdout: Vec<u8>,
    stderr: String,
}

impl GitOutput {
    /// The output when the command succeeded, else a 422 carrying its stderr.
    fn ok(self) -> Result<GitOutput, GitError> {
        if self.code == 0 {
            Ok(self)
        } else {
            Err(GitError::refused(
                "git",
                format!("Git failed: {}", validate::redact(&self.stderr)),
            ))
        }
    }

    /// Stdout as trimmed text.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).trim().to_string()
    }
}

enum ExecError {
    Spawn(String),
    Timeout,
    TooLarge,
}

/// Finds Git and runs it under one of the two environments.
struct Runtime {
    /// `<state_home>/librepaper/local/git`.
    root: PathBuf,
    /// The Git executable whose version has already passed the check.
    verified: StdMutex<Option<PathBuf>>,
    /// Extra variables for every command, so a test can stand in for a
    /// user's configuration without touching the test process.
    #[cfg(test)]
    test_env: Vec<(String, OsString)>,
}

impl Runtime {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            verified: StdMutex::new(None),
            #[cfg(test)]
            test_env: Vec::new(),
        }
    }

    /// The empty global configuration file and the empty hooks directory,
    /// created when missing.
    fn support(&self) -> Result<(PathBuf, PathBuf), GitError> {
        let config = self.root.join("empty.gitconfig");
        let hooks = self.root.join("empty-hooks");
        std::fs::create_dir_all(&hooks).map_err(|error| {
            GitError::internal(format!("could not create {}: {error}", hooks.display()))
        })?;
        if !config.exists() {
            std::fs::write(&config, b"").map_err(|error| {
                GitError::internal(format!("could not create {}: {error}", config.display()))
            })?;
        }
        Ok((config, hooks))
    }

    fn hooks_dir(&self) -> Result<PathBuf, GitError> {
        Ok(self.support()?.1)
    }

    /// The Git executable, if any is installed.
    fn find() -> Option<PathBuf> {
        tools::find("LIBREPAPER_GIT", "git")
    }

    /// Git's version text and whether it is new enough.
    async fn probe(&self, exe: &Path) -> Option<(String, bool)> {
        let mut command = Command::new(exe);
        command.arg("version");
        for name in AMBIENT {
            command.env_remove(name);
        }
        command.env("LC_ALL", "C");
        let output = exec(command, None, Duration::from_secs(10)).await.ok()?;
        if output.code != 0 {
            return None;
        }
        validate::parse_version(&String::from_utf8_lossy(&output.stdout))
    }

    /// The Git executable, once it is known to be new enough.
    async fn exe(&self) -> Result<PathBuf, GitError> {
        let Some(exe) = Self::find() else {
            return Err(GitError::refused(
                "git-missing",
                "Git is not installed on this computer. Install Git 2.38 or newer to sync.",
            ));
        };
        let known = self
            .verified
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if known.as_ref() == Some(&exe) {
            return Ok(exe);
        }
        match self.probe(&exe).await {
            Some((_, true)) => {
                *self
                    .verified
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(exe.clone());
                Ok(exe)
            }
            Some((version, false)) => Err(GitError::refused(
                "git-old",
                format!("Git {version} is too old. Git sync needs Git 2.38 or newer."),
            )),
            None => Err(GitError::refused(
                "git-missing",
                "Git did not answer `git version`. Check the Git installation.",
            )),
        }
    }

    /// A command with the environment for `env` and the arguments every
    /// command of that kind starts with.
    fn command(&self, exe: &Path, env: GitEnv, at: At<'_>) -> Result<Command, GitError> {
        let (config, hooks) = self.support()?;
        let mut command = Command::new(exe);
        for name in AMBIENT {
            command.env_remove(name);
        }
        command.env("LC_ALL", "C").env("GIT_TERMINAL_PROMPT", "0");
        match at {
            At::Repo(repo) => {
                command.env("GIT_DIR", repo).current_dir(repo);
            }
            At::Dir(directory) => {
                command.current_dir(directory);
            }
        }
        match env {
            GitEnv::Local => {
                command
                    .env("GIT_CONFIG_GLOBAL", &config)
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .env("GIT_ATTR_NOSYSTEM", "1");
            }
            GitEnv::Transport => {
                for setting in [
                    format!("core.hooksPath={}", hooks.display()),
                    "protocol.allow=never".to_string(),
                    "protocol.https.allow=always".to_string(),
                    "protocol.ssh.allow=always".to_string(),
                    "protocol.file.allow=always".to_string(),
                ] {
                    command.arg("-c").arg(setting);
                }
            }
        }
        Ok(command)
    }

    /// Runs Git with `args`. A non-zero exit is returned, not raised, so the
    /// caller can read what Git said; only a failure to run is an error.
    async fn run(
        &self,
        env: GitEnv,
        at: At<'_>,
        args: &[&str],
        stdin: Option<Vec<u8>>,
        envs: &[(&str, &OsStr)],
    ) -> Result<GitOutput, GitError> {
        let exe = self.exe().await?;
        let mut command = self.command(&exe, env, at)?;
        if env == GitEnv::Transport
            && matches!(args.first(), Some(&"fetch" | &"push" | &"ls-remote"))
        {
            self.batch_ssh(&exe, at, &mut command).await?;
        }
        command.args(args);
        for (name, value) in envs {
            command.env(name, value);
        }
        self.apply_test_env(&mut command);
        let limit = match env {
            GitEnv::Local => LOCAL_TIMEOUT,
            GitEnv::Transport => TRANSPORT_TIMEOUT,
        };
        exec(command, stdin, limit)
            .await
            .map_err(|error| match error {
                ExecError::Spawn(message) => GitError::internal(message),
                ExecError::Timeout if env == GitEnv::Transport => {
                    GitError::remote("network", "Git took too long to reach the remote.")
                }
                ExecError::Timeout => GitError::internal("Git took too long to answer."),
                ExecError::TooLarge => GitError::internal("Git produced more output than allowed."),
            })
    }

    #[cfg(test)]
    fn apply_test_env(&self, command: &mut Command) {
        for (name, value) in &self.test_env {
            command.env(name, value);
        }
    }

    #[cfg(not(test))]
    fn apply_test_env(&self, _command: &mut Command) {}

    /// Makes SSH fail instead of prompt, unless the user chose an SSH command.
    async fn batch_ssh(
        &self,
        exe: &Path,
        at: At<'_>,
        command: &mut Command,
    ) -> Result<(), GitError> {
        if std::env::var_os("GIT_SSH_COMMAND").is_some() || std::env::var_os("GIT_SSH").is_some() {
            return Ok(());
        }
        let mut probe = self.command(exe, GitEnv::Transport, at)?;
        probe.args(["config", "--get", "core.sshCommand"]);
        self.apply_test_env(&mut probe);
        let configured = match exec(probe, None, Duration::from_secs(10)).await {
            Ok(output) => output.code == 0 && !output.text().is_empty(),
            Err(_) => false,
        };
        if !configured {
            command.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
        }
        Ok(())
    }
}

/// Spawns `command`, feeds it `stdin`, and collects its output within `limit`.
/// The child leads its own process group on Unix and is killed with that group
/// when the time runs out.
async fn exec(
    mut command: Command,
    stdin: Option<Vec<u8>>,
    limit: Duration,
) -> Result<GitOutput, ExecError> {
    command
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::local::native::hide_console(command.as_std_mut());
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| ExecError::Spawn(format!("could not start git: {error}")))?;
    let pid = child.id();
    if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
        tokio::spawn(async move {
            let _ = pipe.write_all(&bytes).await;
            let _ = pipe.shutdown().await;
        });
    }
    let stdout = child
        .stdout
        .take()
        .map(|pipe| tokio::spawn(read_bounded(pipe, MAX_STDOUT_BYTES)));
    let stderr = child
        .stderr
        .take()
        .map(|pipe| tokio::spawn(read_bounded(pipe, MAX_STDERR_BYTES)));
    let status = match tokio::time::timeout(limit, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => {
            crate::local::native::kill_tree(pid);
            return Err(ExecError::Spawn(format!("could not wait for git: {error}")));
        }
        Err(_) => {
            crate::local::native::kill_tree(pid);
            let _ = child.kill().await;
            if let Some(task) = stdout {
                task.abort();
            }
            if let Some(task) = stderr {
                task.abort();
            }
            return Err(ExecError::Timeout);
        }
    };
    // A descendant such as an SSH multiplexer can keep the pipes open after
    // Git exits, so the readers are given a moment and then dropped.
    let (stdout, overflow) = join_reader(stdout).await;
    let (stderr, _) = join_reader(stderr).await;
    if overflow {
        return Err(ExecError::TooLarge);
    }
    Ok(GitOutput {
        code: status.code().unwrap_or(-1),
        stdout,
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// Keeps the first `cap` bytes of `reader` and drains the rest, so the child
/// never blocks on a full pipe. The flag says whether anything was dropped.
async fn read_bounded<R: AsyncRead + Unpin>(mut reader: R, cap: usize) -> (Vec<u8>, bool) {
    let mut kept = Vec::new();
    let mut overflow = false;
    let mut chunk = [0u8; 16 * 1024];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => return (kept, overflow),
            Ok(count) => {
                let room = cap.saturating_sub(kept.len());
                if count > room {
                    overflow = true;
                }
                kept.extend_from_slice(&chunk[..count.min(room)]);
            }
        }
    }
}

async fn join_reader(task: Option<tokio::task::JoinHandle<(Vec<u8>, bool)>>) -> (Vec<u8>, bool) {
    let Some(mut task) = task else {
        return (Vec::new(), false);
    };
    match tokio::time::timeout(Duration::from_secs(2), &mut task).await {
        Ok(result) => result.unwrap_or_default(),
        Err(_) => {
            task.abort();
            (Vec::new(), false)
        }
    }
}

/// Parses a JSON request body into the typed request an operation expects.
fn parse<T: DeserializeOwned>(body: &[u8]) -> Result<T, GitError> {
    serde_json::from_slice(body).map_err(|error| GitError::bad(format!("bad JSON body: {error}")))
}

/// Every operation the sidecar answers, in the order the contract lists them.
const OPERATIONS: &[&str] = &[
    "status",
    "choose-local",
    "branches",
    "connect",
    "fetch",
    "tree",
    "blobs",
    "known-assets",
    "put-blobs",
    "tree-edit",
    "commit-tree",
    "merge",
    "diff",
    "ancestor",
    "push",
    "keep",
    "release",
    "journal",
    "forget",
];

/// The private repositories, local-remote grants and per-connection locks.
pub(crate) struct GitManager {
    runtime: Runtime,
    /// `<state_home>/librepaper/local/git`.
    root: PathBuf,
    /// `<state_home>/librepaper/local/git-grants.json`.
    grants_path: PathBuf,
    grants_lock: StdMutex<()>,
    locks: StdMutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl GitManager {
    pub(crate) fn new(state_home: &Path) -> Self {
        let local = state_home.join("librepaper").join("local");
        let root = local.join("git");
        Self {
            runtime: Runtime::new(root.clone()),
            root,
            grants_path: local.join("git-grants.json"),
            grants_lock: StdMutex::new(()),
            locks: StdMutex::new(HashMap::new()),
        }
    }

    /// The same manager with extra variables set on every Git command it runs.
    #[cfg(test)]
    pub(crate) fn with_test_env(mut self, env: Vec<(String, OsString)>) -> Self {
        self.runtime.test_env = env;
        self
    }

    fn lock_for(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(key.to_string())
            .or_default()
            .clone()
    }

    /// Answers one operation for the page at `origin`. `body` is the request's
    /// JSON. `choose-local` is not here: it needs the folder chooser, which
    /// the HTTP layer owns.
    pub(crate) async fn call(
        &self,
        origin: &str,
        op: &str,
        body: &[u8],
    ) -> Result<Value, GitError> {
        if !OPERATIONS.contains(&op) || op == "choose-local" {
            return Err(GitError::new(
                404,
                None,
                "That Git operation does not exist.",
            ));
        }
        #[derive(serde::Deserialize)]
        struct Project {
            project: String,
        }
        let head: Project = parse(body)?;
        let project = store::valid_project(&head.project)
            .ok_or_else(|| GitError::bad("A Git request needs a valid project."))?;
        let conn = storage::conn_for(&self.root, origin, &project);
        match op {
            "status" => return self.status(&conn).await,
            "journal" => return self.journal(&conn, body).await,
            "branches" => return self.branches(&conn, body).await,
            _ => {}
        }
        let lock = self.lock_for(&conn.key);
        let _guard = if op == "push" {
            lock.try_lock_owned().map_err(|_| {
                GitError::busy(
                    "Another Git operation is running for this project. Try again in a moment.",
                )
            })?
        } else {
            lock.lock_owned().await
        };
        if op == "connect" {
            return self.connect(&conn, body).await;
        }
        let record = self
            .read_record(&conn)
            .ok_or_else(GitError::not_connected)?;
        match op {
            "fetch" => self.fetch(&conn, &record, body).await,
            "push" => self.push(&conn, &record, body).await,
            "forget" => self.forget(&conn, &record),
            "tree" => self.tree(&conn, body).await,
            "blobs" => self.blobs(&conn, body).await,
            "known-assets" => self.known_assets(&conn, body).await,
            "put-blobs" => self.put_blobs(&conn, body).await,
            "tree-edit" => self.tree_edit(&conn, body).await,
            "commit-tree" => self.commit_tree(&conn, body).await,
            "merge" => self.merge(&conn, body).await,
            "diff" => self.diff(&conn, body).await,
            "ancestor" => self.ancestor(&conn, body).await,
            "keep" => self.keep(&conn, body).await,
            "release" => self.release(&conn, body).await,
            _ => Err(GitError::new(
                404,
                None,
                "That Git operation does not exist.",
            )),
        }
    }

    /// Git's version and whether this project is connected.
    async fn status(&self, conn: &Conn) -> Result<Value, GitError> {
        let version = match Runtime::find() {
            Some(exe) => self.runtime.probe(&exe).await,
            None => None,
        };
        let mut answer = json!({
            "git": version.as_ref().map(|(text, _)| text.clone()),
            "supported": version.as_ref().map(|(_, ok)| *ok).unwrap_or(false),
            "connected": false,
        });
        if let Some(record) = self.read_record(conn) {
            answer["connected"] = json!(true);
            answer["connection_id"] = json!(record.connection_id);
            answer["remote_label"] = json!(record.label);
            answer["branch"] = json!(record.branch);
        }
        Ok(answer)
    }

    /// Runs a local command against the connection's repository and requires
    /// it to succeed.
    async fn local(
        &self,
        conn: &Conn,
        args: &[&str],
        stdin: Option<Vec<u8>>,
    ) -> Result<GitOutput, GitError> {
        self.local_raw(conn, args, stdin, &[]).await?.ok()
    }

    /// Runs a local command and hands back its output whatever the exit code.
    async fn local_raw(
        &self,
        conn: &Conn,
        args: &[&str],
        stdin: Option<Vec<u8>>,
        envs: &[(&str, &OsStr)],
    ) -> Result<GitOutput, GitError> {
        self.runtime
            .run(GitEnv::Local, At::Repo(&conn.repo), args, stdin, envs)
            .await
    }
}

#[cfg(test)]
mod testing;
