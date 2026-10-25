//! Internal to librepaper: no stable API, versioned in lockstep with it.
//!
//! Sync between a working directory and a web project. This is the logic
//! behind `librepaper pull` and `librepaper push`, and what the desktop app
//! runs in process.

mod diff;
mod error;
mod files;
mod git;
mod link;
mod rules;
mod state;
mod walk;

pub use error::Error;
pub use files::{FileRecord, Kind};
pub use link::Link;
pub use rules::Rules;
pub use state::{store_for, Fetched, FileStore, GitRefStore, Pushed, State, StateStore};
