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
