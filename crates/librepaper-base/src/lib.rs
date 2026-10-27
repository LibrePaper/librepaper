//! Internal to librepaper: no stable API, versioned in lockstep with it.
//! The base: configuration, identity and the small helpers every other part
//! of librepaper builds on. It depends on nothing else in the workspace.

pub mod assistant_protocol;
pub mod auth;
pub mod canonical_json;
pub mod config;
pub mod digest;
pub mod http;
pub mod ids;
pub mod lines;
pub mod pages;
pub mod paths;
pub mod private_files;
pub mod process;
pub mod role;
pub mod secret;
pub mod shell;
pub mod site;
pub mod text;
pub mod tls;
pub mod util;
