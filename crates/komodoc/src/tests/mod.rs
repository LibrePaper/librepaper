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
mod local_native;
mod local_service;
mod ownership;
mod pseudonym;
mod quota;
mod renderings;
mod retention;
mod review_auth;
mod review_cli;
mod review_misc;
mod review_room;
mod review_server;
mod review_session;
mod review_store;
mod review_sync;
mod s3;
mod seed;
mod serve;
mod share_cli;
mod sharing;
mod source_anchor;
mod sync;
mod timeline;
mod visitor;
mod wasmtex_server;
pub mod yjs;
