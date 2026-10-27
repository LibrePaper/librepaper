//! Internal to librepaper: no stable API, versioned in lockstep with it.
//!
//! Sync between a working directory and a web project. This is the logic
//! behind `librepaper pull` and `librepaper push`, and what the desktop app
//! runs in process.
//!
//! The web project is the truth. A directory records which version of it it
//! last took in and the hash of every synced file as of then (`State`). `pull`
//! brings the project's named versions in: in a git repository as commits on
//! `refs/librepaper/main` that git merges, elsewhere as files written into the
//! folder. `push` sends what the directory changed since, as one named
//! version, and is refused when the web changed a file it touches. Nothing
//! here merges text, holds a credential for a forge, or speaks the git
//! protocol.

mod chain;
mod client;
mod diff;
mod error;
mod files;
mod git;
mod link;
mod pull;
mod push;
mod rules;
mod state;
mod walk;

pub use client::Client;
pub use error::Error;
pub use files::{FileRecord, Kind};
pub use link::Link;
pub use pull::{pull, MergeOutcome, PullOptions, PullOutcome, VersionCommit};
pub use push::{
    changes_here, commit_summary, publish, push, web_changed, CommitSummary, PushOptions,
    PushOutcome,
};
pub use rules::Rules;
pub use state::{store_for, Fetched, FileStore, GitRefStore, PendingPush, Pushed, State, StateStore};
