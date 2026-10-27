//! Running the person's own Git in a repository, for the hidden connection of
//! a project.
//!
//! This file refuses what must not run and shapes the answers. The runtime in
//! `crate::local::git` finds Git, makes it non-interactive and bounds it.
//!
//! LibrePaper never merges text, never holds a forge credential and never
//! forces anything. A pull is `git pull --no-rebase`: a conflicting one leaves
//! the merge in progress with markers in the files. A push never pulls; a
//! branch that fell behind is a 409 with the reason `behind`. A failure of the
//! network, a missing credential or a missing repository is a 502 with the
//! reason `network`, `auth` or `not-found` and the redacted words of Git as
//! `detail`. Any other refusal by Git is a 422 with its words as `error`.

use super::*;

use crate::local::git::status::{self, Status};
use crate::local::git::{self, remote, validate, Limit, Output, RunError};

/// The message `sync` commits with when the request names none.
pub(super) const SYNC_MESSAGE: &str = "Edits from LibrePaper";

/// A refusal: the status and the JSON body to answer with.
type Failure = (u16, Value);

fn refuse(status: u16, error: &str) -> Failure {
    (status, json!({"error": error}))
}

pub(super) fn refuse_because(status: u16, reason: &str, error: &str) -> Failure {
    (status, json!({"reason": reason, "error": error}))
}

/* ---------------------------------------------------------- running Git */

/// Runs Git in `root`. A Git that is missing or too old, or a command that
/// ran out of time, is already the answer to send.
pub(super) async fn run(root: &Path, limit: Limit, args: &[&str]) -> Result<Output, Failure> {
    git::run(root, limit, args)
        .await
        .map_err(|error| run_failure(error, limit))
}

fn run_failure(error: RunError, limit: Limit) -> Failure {
    match error {
        RunError::Missing => refuse_because(
            409,
            "git-missing",
            "Git is not installed on this computer. Install Git 2.38 or newer.",
        ),
        RunError::TooOld(version) => refuse_because(
            409,
            "git-old",
            &format!("Git {version} is too old. The Git panel needs Git 2.38 or newer."),
        ),
        RunError::Spawn(message) => refuse(500, &message),
        RunError::Timeout if limit == Limit::Transport => network_failure(
            remote::Kind::Network,
            "Git took too long to reach the remote.",
        ),
        RunError::Timeout => refuse(500, "Git took too long to answer."),
        RunError::TooLarge => refuse(500, "Git produced more output than allowed."),
    }
}

/// What Git said, without the credentials of any URL in it.
fn words(output: &Output) -> String {
    if output.stderr.trim().is_empty() {
        validate::redact(&output.said())
    } else {
        validate::redact(&output.stderr)
    }
}

/// Git refused, and the reason is its own words.
pub(super) fn declined(output: &Output) -> Failure {
    refuse(422, &words(output))
}

/// A failure of the remote, for the sign-in help and the retry.
fn network_failure(kind: remote::Kind, detail: &str) -> Failure {
    (
        502,
        json!({"reason": kind.as_str(), "detail": detail, "error": detail}),
    )
}

/// A failed command that talks to a remote: the remote's doing when what Git
/// said shows it, else Git's own refusal.
pub(super) fn remote_failure(output: &Output) -> Failure {
    match remote::classify(&output.said()) {
        Some(kind) => network_failure(kind, &words(output)),
        None => declined(output),
    }
}

/* ---------------------------------------------------------- the repository */

fn has_repository(root: &Path) -> bool {
    root.join(".git").exists()
}

fn require_repository(root: &Path) -> Result<(), Failure> {
    if has_repository(root) {
        Ok(())
    } else {
        Err(refuse_because(
            409,
            "no-repository",
            "The copy of the repository on this computer is missing. Disconnect, then connect again.",
        ))
    }
}

fn detached() -> Failure {
    refuse_because(
        409,
        "detached",
        "The repository is not on a branch. Switch to a branch first.",
    )
}

fn no_remote() -> Failure {
    refuse_because(
        409,
        "no-remote",
        "This repository has no remote. Set one first.",
    )
}

async fn read_status(root: &Path) -> Result<Status, Failure> {
    let output = run(
        root,
        Limit::Local,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--untracked-files=all",
        ],
    )
    .await?;
    if !output.success() {
        return Err(declined(&output));
    }
    Ok(status::parse(&output.stdout))
}

/// The remotes of the repository as `(name, url)`, with the credentials of
/// the URLs removed.
async fn read_remotes(root: &Path) -> Result<Vec<(String, String)>, Failure> {
    let output = run(
        root,
        Limit::Local,
        &["config", "-z", "--get-regexp", r"^remote\..+\.url$"],
    )
    .await?;
    // Exit 1 means nothing matched.
    if output.code > 1 {
        return Err(declined(&output));
    }
    let mut remotes: Vec<(String, String)> = Vec::new();
    for entry in String::from_utf8_lossy(&output.stdout).split('\0') {
        let Some((key, url)) = entry.split_once('\n') else {
            continue;
        };
        let Some(name) = key
            .strip_prefix("remote.")
            .and_then(|rest| rest.strip_suffix(".url"))
        else {
            continue;
        };
        // A name the config spells with anything but letters, digits, dots,
        // dashes and underscores is left out: the panel puts names into a
        // command the person copies.
        if validate::valid_remote_name(name) && !remotes.iter().any(|(seen, _)| seen == name) {
            remotes.push((name.to_string(), validate::display_url(url)));
        }
    }
    Ok(remotes)
}

/// Whether a merge was started and not finished.
pub(super) async fn merge_in_progress(root: &Path) -> Result<bool, Failure> {
    let output = run(
        root,
        Limit::Local,
        &["rev-parse", "-q", "--verify", "MERGE_HEAD"],
    )
    .await?;
    Ok(output.success())
}

/// The commit `HEAD` names, or `None` before the first commit.
async fn head_id(root: &Path) -> Result<Option<String>, Failure> {
    let output = run(root, Limit::Local, &["rev-parse", "-q", "--verify", "HEAD"]).await?;
    Ok(output.success().then(|| output.text()))
}

/// The files with unresolved conflicts.
pub(super) async fn conflicted_paths(root: &Path) -> Result<Vec<String>, Failure> {
    let output = run(
        root,
        Limit::Local,
        &["diff", "--name-only", "--diff-filter=U", "-z"],
    )
    .await?;
    if !output.success() {
        return Err(declined(&output));
    }
    let mut paths: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect();
    paths.sort();
    Ok(paths)
}

/// The remote a fetch, a pull or a push talks to: the one the branch follows,
/// else `origin`, else the first one.
async fn pick_remote(
    root: &Path,
    status: &Status,
    remotes: &[(String, String)],
) -> Result<String, Failure> {
    if remotes.is_empty() {
        return Err(no_remote());
    }
    if let (Some(branch), Some(_)) = (&status.branch, &status.upstream) {
        let key = format!("branch.{branch}.remote");
        let configured = run(root, Limit::Local, &["config", "--get", &key]).await?;
        let name = configured.text();
        if configured.success() && remotes.iter().any(|(known, _)| *known == name) {
            return Ok(name);
        }
    }
    if let Some((name, _)) = remotes.iter().find(|(name, _)| name == "origin") {
        return Ok(name.clone());
    }
    Ok(remotes[0].0.clone())
}

/// Directories and files the project never lists, besides anything hidden.
const IGNORED_NAMES: [&str; 5] = ["node_modules", "renv", "__pycache__", ".quarto", "_freeze"];

/// Why a directory entry named `name` is never listed, when it is not.
fn skip_reason(name: &str) -> Option<&'static str> {
    if name.starts_with('.') {
        Some("hidden")
    } else if IGNORED_NAMES.contains(&name) {
        Some("ignored")
    } else {
        None
    }
}

/// Whether the project holds `path`. Hidden entries and the ignored names are
/// not the editor's, so a change to `.gitignore` or to a build folder does not
/// make a pull wait.
fn held(path: &str) -> bool {
    librepaper_base::paths::RelPath::is_local(path)
        && !path.split('/').any(|part| skip_reason(part).is_some())
}

/// The tracked files of the project with changes not committed. Untracked
/// files are not among them: they cannot be overwritten silently, since Git
/// refuses a merge that would.
fn dirty_paths(status: &Status) -> Vec<String> {
    status
        .changes
        .iter()
        .filter(|change| change.tracked && held(&change.path))
        .map(|change| change.path.clone())
        .collect()
}

/* ----------------------------------------------------------------- pull */

/// `git pull --no-rebase`. The project's tracked files must be committed
/// first, and a merge in progress must be finished. A branch with no upstream
/// pulls the branch of that name from its remote, and finds nothing to pull
/// when the remote has none.
pub(super) async fn run_pull(root: &Path) -> Result<Value, Failure> {
    require_repository(root)?;
    let status = read_status(root).await?;
    if merge_in_progress(root).await? {
        return Err(refuse_because(
            409,
            "merging",
            "Finish the merge in progress first: resolve the conflicts, then commit.",
        ));
    }
    let dirty = dirty_paths(&status);
    if !dirty.is_empty() {
        return Err((
            409,
            json!({
                "reason": "dirty",
                "paths": dirty,
                "error": "Commit your changes to tracked files first.",
            }),
        ));
    }
    let named = match (&status.upstream, &status.branch) {
        (Some(_), _) => None,
        (None, Some(branch)) => {
            let remotes = read_remotes(root).await?;
            Some((pick_remote(root, &status, &remotes).await?, branch.clone()))
        }
        (None, None) => return Err(detached()),
    };
    let before = head_id(root).await?;
    let mut args = vec!["pull", "--no-rebase", "--no-edit"];
    if let Some((remote, branch)) = &named {
        args.extend([remote.as_str(), branch.as_str()]);
    }
    let output = run(root, Limit::Transport, &args).await?;
    if output.success() {
        let after = head_id(root).await?;
        let state = if after == before {
            protocol::PullOutcome::UpToDate
        } else if before.is_none() || output.said().contains("Fast-forward") {
            protocol::PullOutcome::FastForward
        } else {
            protocol::PullOutcome::Merged
        };
        return Ok(json!({"status": state, "conflicts": []}));
    }
    let conflicts = conflicted_paths(root).await?;
    if !conflicts.is_empty() {
        return Ok(json!({"status": protocol::PullOutcome::Conflicts, "conflicts": conflicts}));
    }
    if named.is_some() && output.said().contains("couldn't find remote ref") {
        return Ok(json!({"status": protocol::PullOutcome::UpToDate, "conflicts": []}));
    }
    Err(remote_failure(&output))
}

/* ----------------------------------------------------------------- push */

/// `git push` of the current branch, never forced and never after a pull. The
/// push names one refspec: the local branch, to the branch on the remote that
/// `branch.<name>.merge` names. A branch with no upstream goes to the branch of
/// its own name. The repository's `push.default` and `remote.<name>.push`
/// settings do not choose what goes. The first push of a branch sets its
/// upstream.
pub(super) async fn run_push(root: &Path) -> Result<Value, Failure> {
    require_repository(root)?;
    let status = read_status(root).await?;
    let Some(branch) = status.branch.clone() else {
        return Err(detached());
    };
    let remotes = read_remotes(root).await?;
    let remote = pick_remote(root, &status, &remotes).await?;
    let mut destination = format!("refs/heads/{branch}");
    if status.upstream.is_some() {
        let key = format!("branch.{branch}.merge");
        let configured = run(root, Limit::Local, &["config", "--get", &key]).await?;
        let merged = configured.text();
        if configured.success() && !merged.is_empty() {
            destination = merged;
        }
    }
    let refspec = format!("refs/heads/{branch}:{destination}");
    let mut args = vec!["push"];
    if status.upstream.is_none() {
        args.push("-u");
    }
    args.extend(["--", remote.as_str(), refspec.as_str()]);
    let output = run(root, Limit::Transport, &args).await?;
    if output.success() {
        let state = if output
            .said()
            .to_lowercase()
            .contains("everything up-to-date")
        {
            "up-to-date"
        } else {
            "pushed"
        };
        return Ok(json!({"status": state, "branch": branch, "remote": remote}));
    }
    if remote::behind(&output.said()) {
        return Err(refuse_because(
            409,
            "behind",
            "The remote has commits you do not have. Pull first.",
        ));
    }
    Err(remote_failure(&output))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_command_is_sorted_into_what_the_panel_acts_on() {
        let failed = |stderr: &str| Output {
            code: 128,
            stdout: Vec::new(),
            stderr: stderr.to_string(),
        };

        let (status, body) = remote_failure(&failed(
            "fatal: could not read Username for 'https://token@github.com': terminal prompts disabled\n",
        ));
        assert_eq!((status, &body["reason"]), (502, &json!("auth")));
        let detail = body["detail"].as_str().unwrap();
        assert!(detail.contains("terminal prompts disabled"), "{detail}");
        assert!(!detail.contains("token@"), "{detail}");
        assert_eq!(body["error"], body["detail"]);

        let (status, body) = remote_failure(&failed(
            "fatal: unable to access 'https://github.com/o/r.git/': Could not resolve host: github.com",
        ));
        assert_eq!((status, &body["reason"]), (502, &json!("network")));

        let (status, body) = remote_failure(&failed("remote: Repository not found."));
        assert_eq!((status, &body["reason"]), (502, &json!("not-found")));

        // Not the remote's doing: Git's own words, as a refusal.
        let (status, body) = remote_failure(&failed(
            "error: Your local changes to the following files would be overwritten by merge:\n\ta.txt",
        ));
        assert_eq!(status, 422);
        assert!(body["error"].as_str().unwrap().contains("a.txt"));
        assert!(body.get("reason").is_none());
    }

    #[test]
    fn a_missing_git_and_a_slow_remote_have_their_own_answers() {
        let (status, body) = run_failure(RunError::Missing, Limit::Local);
        assert_eq!((status, &body["reason"]), (409, &json!("git-missing")));
        let (status, body) = run_failure(RunError::TooOld("2.30.1".into()), Limit::Local);
        assert_eq!((status, &body["reason"]), (409, &json!("git-old")));
        assert!(body["error"].as_str().unwrap().contains("2.30.1"));
        let (status, body) = run_failure(RunError::Timeout, Limit::Transport);
        assert_eq!((status, &body["reason"]), (502, &json!("network")));
        assert_eq!(run_failure(RunError::Timeout, Limit::Local).0, 500);
        assert_eq!(run_failure(RunError::TooLarge, Limit::Local).0, 500);
    }

    #[test]
    fn only_the_files_the_project_holds_can_make_a_pull_wait() {
        let change = |path: &str, tracked: bool| status::Change {
            path: path.to_string(),
            tracked,
        };
        let status = Status {
            changes: vec![
                change("main.tex", true),
                change("chapters/one.tex", true),
                change("notes.txt", false),
                change(".gitignore", true),
                change(".github/workflows/librepaper.yml", true),
                change("node_modules/pkg/index.js", true),
            ],
            ..Status::default()
        };
        assert_eq!(
            dirty_paths(&status),
            vec!["main.tex".to_string(), "chapters/one.tex".to_string()]
        );
    }
}
