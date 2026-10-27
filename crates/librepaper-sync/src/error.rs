use std::path::{Path, PathBuf};

/// What can go wrong in a sync, in the terms a caller acts on.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The text is not a project URL.
    #[error("{0}")]
    Link(String),
    /// The directory has no recorded link to a project.
    #[error("this directory is not linked to a project")]
    NotLinked,
    /// The repository has a merge in progress, so its files may hold conflict
    /// markers.
    #[error("a merge is in progress; resolve the conflicts and commit, then push")]
    Merging,
    /// HEAD names no branch, so there is none to sync.
    #[error("HEAD is detached, so there is no branch to sync; check out the branch first, such as `git switch main`")]
    DetachedHead,
    /// The repository has no branch chosen for sync yet.
    #[error("this repository has no branch chosen for sync; it is on `{checked_out}`, so run again with `--branch {checked_out}` to sync that branch")]
    BranchUnbound { checked_out: String },
    /// The checkout is not on the branch sync is bound to.
    #[error("this checkout is on `{checked_out}` but sync uses `{expected}`; run `git switch {expected}` and try again")]
    BranchMismatch {
        expected: String,
        checked_out: String,
    },
    /// The branch asked for is not the one sync is bound to.
    #[error("sync is bound to `{bound}`, not `{requested}`; run `git switch {bound}` and leave out `--branch` or name `{bound}`")]
    BranchRebind { bound: String, requested: String },
    /// A branch was named for a directory that is not a repository.
    #[error("a branch applies only to a git repository, and this directory is a plain folder")]
    BranchNeedsRepository,
    /// A web push was sent and its outcome is not known yet.
    #[error("an earlier push was interrupted before its outcome was known; run `librepaper push` to settle it first")]
    PendingPush,
    /// The interrupted push was sent under another sign-in.
    #[error("the interrupted push was sent under a different sign-in; sign in as the same account and run `librepaper push` again")]
    PendingActor,
    /// The directory is linked to another project than the one named.
    #[error("this directory is linked to {linked}, not {named}; unlink it before using another project")]
    OtherProject { linked: String, named: String },
    /// The server answered with a failure status.
    #[error("{detail} ({status})")]
    Http { status: u16, detail: String },
    /// The server could not be reached, or did not finish answering.
    #[error("{0}")]
    Network(String),
    /// A git command failed, or git is not there.
    #[error("git: {0}")]
    Git(String),
    #[error("{}: {source}", .path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The directory holds more than the deployment's rules allow.
    #[error("{0}")]
    Rules(String),
    /// The version asked for is before the project's trim point.
    #[error("the project's history before its trim point is gone")]
    Trimmed,
    /// The recorded state cannot be read or written.
    #[error("sync state: {0}")]
    State(String),
    /// A caller's mistake, or a reply that makes no sense.
    #[error("{0}")]
    Invalid(String),
}

impl Error {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Error {
        Error::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}
