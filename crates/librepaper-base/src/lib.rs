//! Internal to librepaper: no stable API, versioned in lockstep with it.
//! See docs/dev/specs/SPEC-split-crates.md, "Workspace layout".
//! The base: configuration, identity and the small helpers every other part
//! of librepaper builds on. It depends on nothing else in the workspace.

pub mod assistant_protocol;
pub mod auth;
pub mod canonical_json;
pub mod config;
pub mod http;
pub mod private_files;
pub mod shell;
pub mod tls;
pub mod util;
