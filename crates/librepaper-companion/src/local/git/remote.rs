//! The operations that talk to a remote, and the ones that decide which
//! remote that is: listing branches, connecting, fetching, pushing and
//! granting a local folder.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{json, Value};

use super::storage::{Conn, ConnectionRecord, RemoteSpec};
use super::validate::{label_for_url, redact, require_id, valid_branch, validate_url};
use super::{parse, At, GitEnv, GitError, GitManager};

/// Where a connection fetches from, as Git is told on the command line.
struct Target {
    location: String,
    label: String,
    local: bool,
}

/// The name shown for a chosen folder: its last path component.
fn folder_label(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("local repository")
        .to_string()
}

/// A path without the `\\?\` prefix Windows canonicalization adds, which Git
/// does not understand.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(rest) = path.to_str().and_then(|text| text.strip_prefix(r"\\?\")) {
            if !rest.starts_with("UNC\\") {
                return PathBuf::from(rest);
            }
        }
    }
    path
}

/// Turns what Git said on a failed network command into a 502 with a kind.
fn remote_error(stderr: &str) -> GitError {
    let text = redact(stderr);
    let lower = text.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|needle| lower.contains(needle));
    let kind = if has(&[
        "could not resolve host",
        "could not resolve hostname",
        "connection refused",
        "timed out",
        "network is unreachable",
        "no route to host",
        "unable to connect",
        "failed to connect",
        "connection reset",
        "ssl certificate",
    ]) {
        "network"
    } else if has(&[
        "authentication failed",
        "permission denied",
        "could not read username",
        "could not read password",
        "terminal prompts disabled",
        "host key verification failed",
        "access denied",
        "invalid credentials",
        "returned error: 401",
        "returned error: 403",
    ]) {
        "auth"
    } else if has(&[
        "repository not found",
        "does not appear to be a git repository",
        "returned error: 404",
        "no such file or directory",
        "not found",
    ]) {
        "not-found"
    } else {
        "network"
    };
    GitError::remote(kind, format!("Git could not use the remote: {text}"))
}

impl GitManager {
    /// What a remote the page named stands for on this computer.
    fn resolve(&self, conn: &Conn, spec: &RemoteSpec) -> Result<Target, GitError> {
        match spec {
            RemoteSpec::Url { url } => {
                let url = validate_url(url)?;
                Ok(Target {
                    label: label_for_url(&url),
                    location: url,
                    local: false,
                })
            }
            RemoteSpec::Grant { grant } => {
                let found = self.find_grant(conn, grant).ok_or_else(|| {
                    GitError::new(
                        404,
                        Some("grant"),
                        "That folder was not chosen on this computer. Choose it again.",
                    )
                })?;
                let path = Path::new(&found.path);
                if !path.is_dir() {
                    return Err(GitError::remote(
                        "not-found",
                        "The chosen folder no longer exists.",
                    ));
                }
                Ok(Target {
                    label: folder_label(path),
                    location: found.path.clone(),
                    local: true,
                })
            }
        }
    }

    /// Refuses a SHA-256 repository at `path`. A Git too old to say is taken
    /// to be SHA-1.
    async fn require_sha1_folder(&self, path: &Path) -> Result<(), GitError> {
        let output = self
            .runtime
            .run(
                GitEnv::Local,
                At::Dir(path),
                &["rev-parse", "--show-object-format"],
                None,
                &[],
            )
            .await?;
        if output.code == 0 && output.text() == "sha256" {
            return Err(GitError::refused(
                "sha256",
                "That repository uses SHA-256 object ids. Git sync supports SHA-1 repositories only.",
            ));
        }
        Ok(())
    }

    /// Stores a folder the person chose as a local remote for this page and
    /// project. The folder must be a Git repository, bare or not.
    pub(crate) async fn add_grant(
        &self,
        origin: &str,
        project: &str,
        chosen: PathBuf,
    ) -> Result<Value, GitError> {
        let conn = super::storage::conn_for(&self.root, origin, project);
        let dir = tokio::task::spawn_blocking(move || chosen.canonicalize())
            .await
            .map_err(|error| GitError::internal(format!("could not resolve the folder: {error}")))?
            .map_err(|error| {
                GitError::bad(format!("could not resolve the selected folder: {error}"))
            })?;
        let dir = strip_verbatim(dir);
        if !dir.is_dir() {
            return Err(GitError::bad("Select an existing folder."));
        }
        let output = self
            .runtime
            .run(
                GitEnv::Local,
                At::Dir(&dir),
                &["rev-parse", "--absolute-git-dir"],
                None,
                &[],
            )
            .await?;
        let not_repository = || {
            GitError::bad("That folder is not a Git repository. Choose the folder that holds the repository, or its .git folder.")
        };
        if output.code != 0 {
            return Err(not_repository());
        }
        let git_dir = Path::new(&output.text())
            .canonicalize()
            .ok()
            .map(strip_verbatim);
        // A bare repository or a `.git` folder chosen directly is its own git
        // dir. A working tree, linked worktrees included, is the folder Git
        // reports as its top level. Any other folder inside a working tree is
        // refused.
        let mut accepted = git_dir.as_deref() == Some(dir.as_path());
        if !accepted {
            let top = self
                .runtime
                .run(
                    GitEnv::Local,
                    At::Dir(&dir),
                    &["rev-parse", "--show-toplevel"],
                    None,
                    &[],
                )
                .await?;
            accepted = top.code == 0
                && Path::new(&top.text())
                    .canonicalize()
                    .ok()
                    .map(strip_verbatim)
                    .is_some_and(|top| top == dir);
        }
        if !accepted {
            return Err(not_repository());
        }
        self.require_sha1_folder(&dir).await?;
        let grant = self.add_grant_record(&conn, &dir)?;
        Ok(json!({"grant": grant.grant, "label": folder_label(&dir)}))
    }

    pub(super) async fn branches(&self, conn: &Conn, body: &[u8]) -> Result<Value, GitError> {
        #[derive(Deserialize)]
        struct Request {
            remote: RemoteSpec,
        }
        let request: Request = parse(body)?;
        let target = self.resolve(conn, &request.remote)?;
        if target.local {
            self.require_sha1_folder(Path::new(&target.location))
                .await?;
        }
        let ceiling = self
            .root
            .parent()
            .unwrap_or(&self.root)
            .as_os_str()
            .to_owned();
        let output = self
            .runtime
            .run(
                GitEnv::Transport,
                At::Dir(&self.root),
                &[
                    "ls-remote",
                    "--symref",
                    "--",
                    &target.location,
                    "HEAD",
                    "refs/heads/*",
                ],
                None,
                &[("GIT_CEILING_DIRECTORIES", ceiling.as_os_str())],
            )
            .await?;
        if output.code != 0 {
            return Err(remote_error(&output.stderr));
        }
        let mut branches: Vec<String> = Vec::new();
        let mut default: Option<String> = None;
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if let Some(rest) = line.strip_prefix("ref: ") {
                if let Some((reference, "HEAD")) = rest.split_once('\t') {
                    default = reference.strip_prefix("refs/heads/").map(str::to_string);
                }
                continue;
            }
            let Some((id, name)) = line.split_once('\t') else {
                continue;
            };
            if id.len() == 64 {
                return Err(GitError::refused(
                    "sha256",
                    "That repository uses SHA-256 object ids. Git sync supports SHA-1 repositories only.",
                ));
            }
            if let Some(branch) = name.strip_prefix("refs/heads/") {
                branches.push(branch.to_string());
            }
        }
        branches.sort();
        branches.dedup();
        Ok(json!({"branches": branches, "default": default, "label": target.label}))
    }

    pub(super) async fn connect(&self, conn: &Conn, body: &[u8]) -> Result<Value, GitError> {
        #[derive(Deserialize)]
        struct Request {
            connection_id: String,
            remote: RemoteSpec,
            branch: String,
        }
        let request: Request = parse(body)?;
        let connection_id = uuid::Uuid::parse_str(&request.connection_id)
            .map_err(|_| GitError::bad("The connection id must be a UUID."))?
            .to_string();
        if !valid_branch(&request.branch) {
            return Err(GitError::bad("That is not a valid branch name."));
        }
        let remote = match &request.remote {
            RemoteSpec::Url { url } => RemoteSpec::Url {
                url: validate_url(url)?,
            },
            grant => grant.clone(),
        };
        let target = self.resolve(conn, &remote)?;
        if target.local {
            self.require_sha1_folder(Path::new(&target.location))
                .await?;
        }
        self.ensure_dir(conn)?;
        // Scratch directories a crashed operation left behind.
        if let Ok(entries) = std::fs::read_dir(&conn.dir) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().starts_with("tmp-") {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
        if !conn.repo.join("HEAD").exists() {
            self.runtime
                .run(
                    GitEnv::Local,
                    At::Dir(&conn.dir),
                    &[
                        "init",
                        "--bare",
                        "--quiet",
                        "--object-format=sha1",
                        "repo.git",
                    ],
                    None,
                    &[],
                )
                .await?
                .ok()?;
        } else {
            let format = self
                .local_raw(conn, &["rev-parse", "--show-object-format"], None, &[])
                .await?;
            if format.code == 0 && format.text() == "sha256" {
                return Err(GitError::refused(
                    "sha256",
                    "The private repository for this project uses SHA-256 object ids, which Git sync does not support.",
                ));
            }
        }
        let hooks = self.runtime.hooks_dir()?;
        let hooks = hooks
            .to_str()
            .ok_or_else(|| GitError::internal("The state directory path is not valid text."))?;
        self.local(conn, &["config", "core.hooksPath", hooks], None)
            .await?;
        self.local(conn, &["config", "core.autocrlf", "false"], None)
            .await?;
        let previous = self.read_record(conn);
        if let Some(previous) = &previous {
            if previous.remote != remote || previous.branch != request.branch {
                let _ = self
                    .local_raw(
                        conn,
                        &["update-ref", "-d", "refs/librepaper/remote"],
                        None,
                        &[],
                    )
                    .await;
            }
        }
        self.write_record(
            conn,
            &ConnectionRecord {
                connection_id,
                remote,
                branch: request.branch,
                label: target.label.clone(),
            },
        )?;
        Ok(json!({"ok": true, "label": target.label}))
    }

    /// Whether `id` is a commit in the private repository.
    async fn has_commit(&self, conn: &Conn, id: &str) -> Result<bool, GitError> {
        let spec = format!("{id}^{{commit}}");
        let output = self
            .local_raw(conn, &["cat-file", "-e", &spec], None, &[])
            .await?;
        Ok(output.code == 0)
    }

    pub(super) async fn fetch(
        &self,
        conn: &Conn,
        record: &ConnectionRecord,
        body: &[u8],
    ) -> Result<Value, GitError> {
        #[derive(Deserialize)]
        struct Request {
            #[serde(default)]
            also: Vec<String>,
        }
        let request: Request = parse(body)?;
        for id in &request.also {
            require_id(id, "An also commit")?;
        }
        let target = self.resolve(conn, &record.remote)?;
        let refspec = format!("+refs/heads/{}:refs/librepaper/remote", record.branch);
        let output = self
            .runtime
            .run(
                GitEnv::Transport,
                At::Repo(&conn.repo),
                &[
                    "fetch",
                    "--no-tags",
                    "--no-write-fetch-head",
                    "--",
                    &target.location,
                    &refspec,
                ],
                None,
                &[],
            )
            .await?;
        if output.code != 0 {
            if output.stderr.contains("couldn't find remote ref") {
                // The branch does not exist yet, or was deleted.
                let _ = self
                    .local_raw(
                        conn,
                        &["update-ref", "-d", "refs/librepaper/remote"],
                        None,
                        &[],
                    )
                    .await;
            } else {
                return Err(remote_error(&output.stderr));
            }
        }
        let head = self
            .local_raw(
                conn,
                &[
                    "rev-parse",
                    "--verify",
                    "-q",
                    "refs/librepaper/remote^{commit}",
                ],
                None,
                &[],
            )
            .await?;
        let head = (head.code == 0 && !head.text().is_empty()).then(|| head.text());
        let mut missing = Vec::new();
        let mut seen = HashSet::new();
        for id in &request.also {
            if !seen.insert(id.clone()) || self.has_commit(conn, id).await? {
                continue;
            }
            // Many servers serve a commit by id; a failure just leaves it missing.
            let _ = self
                .runtime
                .run(
                    GitEnv::Transport,
                    At::Repo(&conn.repo),
                    &[
                        "fetch",
                        "--no-tags",
                        "--no-write-fetch-head",
                        "--",
                        &target.location,
                        id,
                    ],
                    None,
                    &[],
                )
                .await?;
            if !self.has_commit(conn, id).await? {
                missing.push(id.clone());
            }
        }
        Ok(json!({"head": head, "missing": missing}))
    }

    pub(super) async fn push(
        &self,
        conn: &Conn,
        record: &ConnectionRecord,
        body: &[u8],
    ) -> Result<Value, GitError> {
        #[derive(Deserialize)]
        struct Request {
            commit: String,
            expected: Option<String>,
            connection_id: String,
        }
        let request: Request = parse(body)?;
        let commit = require_id(&request.commit, "commit")?;
        let expected = match &request.expected {
            Some(id) => Some(require_id(id, "expected")?),
            None => None,
        };
        if request.connection_id != record.connection_id {
            return Err(GitError::new(
                409,
                Some("stale-connection"),
                "The project is connected to another remote now. Reconnect and try again.",
            ));
        }
        if !self.has_commit(conn, commit).await? {
            return Err(GitError::refused(
                "missing",
                format!("The commit {commit} is not in the private repository."),
            ));
        }
        if let Some(expected) = expected {
            let output = self
                .local_raw(
                    conn,
                    &["merge-base", "--is-ancestor", expected, commit],
                    None,
                    &[],
                )
                .await?;
            match output.code {
                0 => {}
                1 => {
                    return Err(GitError::bad(format!(
                        "The commit {commit} does not descend from {expected}."
                    )))
                }
                _ => {
                    return Err(GitError::refused(
                        "git",
                        format!("Git failed: {}", super::validate::redact(&output.stderr)),
                    ))
                }
            }
        }
        let target = self.resolve(conn, &record.remote)?;
        // The lease names the head the review saw. An empty value means the
        // branch must not exist, so a branch that moved, was deleted or was
        // reset since the review is refused.
        let lease = format!(
            "--force-with-lease=refs/heads/{}:{}",
            record.branch,
            expected.unwrap_or("")
        );
        let refspec = format!("{commit}:refs/heads/{}", record.branch);
        // A chosen folder is usually an ordinary repository with the branch
        // checked out. With updateInstead, Git updates that folder's files when
        // the folder is clean and refuses the push when it is not. URL remotes
        // keep Git's default.
        let receive_pack =
            "--receive-pack=git -c receive.denyCurrentBranch=updateInstead receive-pack";
        let mut args: Vec<&str> = vec!["push", "--no-verify", lease.as_str()];
        if target.local {
            args.push(receive_pack);
        }
        args.extend(["--", target.location.as_str(), refspec.as_str()]);
        let output = self
            .runtime
            .run(GitEnv::Transport, At::Repo(&conn.repo), &args, None, &[])
            .await?;
        if output.code != 0 {
            let said = format!(
                "{}\n{}",
                output.stderr,
                String::from_utf8_lossy(&output.stdout)
            );
            let lower = said.to_lowercase();
            if lower.contains("non-fast-forward")
                || lower.contains("fetch first")
                || lower.contains("stale info")
            {
                return Err(GitError::new(
                    409,
                    Some("moved"),
                    "The branch on the remote has moved. Fetch it and merge before pushing.",
                ));
            }
            let dirty = [
                "working directory has unstaged changes",
                "working directory has staged changes",
                "untracked working tree file",
                "would be overwritten",
                "could not update working tree",
            ];
            if target.local && dirty.iter().any(|needle| lower.contains(needle)) {
                return Err(GitError::refused(
                    "dirty",
                    format!(
                        "The folder {} has changes that are not committed. Commit or stash them there, then push again.",
                        target.label
                    ),
                ));
            }
            if lower.contains("checked out") {
                return Err(GitError::refused(
                    "checked-out",
                    "The remote refused the push because that branch is checked out there. Push to another branch.",
                ));
            }
            if lower.contains("remote rejected") || lower.contains("hook declined") {
                return Err(GitError::refused(
                    "rejected",
                    format!("The remote rejected the push: {}", redact(&said)),
                ));
            }
            return Err(remote_error(&said));
        }
        self.local(
            conn,
            &["update-ref", "refs/librepaper/remote", commit],
            None,
        )
        .await?;
        Ok(json!({"pushed": true}))
    }
}
