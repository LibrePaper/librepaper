//! The test suite, in the shape the Go one had: a server in this process, a
//! client that speaks to it the way the shell and the command line do, and
//! one file per concern.

mod harness;
pub use harness::*;

mod admission;
mod agent_cli;
mod assets;
mod auth;
mod automation;
mod blob;
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
mod ownership;
mod pseudonym;
mod publish_cli;
mod quota;
mod renderings;
mod retention;
mod room;
mod s3;
mod seed;
mod serve;
mod session;
mod share_cli;
mod sharing;
mod sockets;
mod source_anchor;
mod store;
mod suggest_cli;
mod suggestions;
mod sync;
mod timeline;
mod tokens;
mod uploads;
mod visitor;
mod wasmtex_server;
pub mod yjs;
