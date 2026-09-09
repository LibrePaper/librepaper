//! The test suite, in the shape the Go one had: a server in this process, a
//! client that speaks to it the way the shell and the command line do, and
//! one file per concern.

mod harness;
pub use harness::*;

mod admission;
mod agent_cli;
mod assets;
mod assistant;
mod assistant_cli;
mod assistant_protocol;
mod auth;
mod auth_regressions;
mod automation;
mod blob;
mod catalogue_room;
mod checkpoint_attribution;
mod commands;
mod device;
mod directories;
pub mod edit;
mod export;
mod figures;
mod guests;
mod hardening;
mod history;
mod latex;
mod local;
mod mirror_server;
mod onboarding;
mod ownership;
mod pseudonym;
mod publish_cli;
mod quota;
mod renderings;
mod retention;
mod room;
mod room_checkpoint_fixes;
mod room_figure_fixes;
mod room_lifecycle_fixes;
mod room_lock_scopes;
mod room_suggestion_fixes;
mod s3;
mod s3_operations;
mod seed;
mod serve;
mod session;
mod share_cli;
mod sharing;
mod size_limits;
mod sockets;
mod source_anchor;
mod store;
mod suggest_cli;
mod suggestions;
mod sync;
mod timeline;
mod tokens;
mod typst_needs;
mod uploads;
mod visitor;
mod write_errors;
pub mod yjs;

mod bibliography;
