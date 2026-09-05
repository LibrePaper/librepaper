//! The test suite, in the shape the Go one had: a server in this process, a
//! client that speaks to it the way the shell and the command line do, and
//! one file per concern.

mod harness;
pub use harness::*;

mod assets;
mod auth;
mod blob;
pub mod edit;
mod export;
mod hardening;
mod history;
mod ownership;
mod quota;
mod retention;
mod s3;
mod seed;
mod serve;
mod visitor;
pub mod yjs;
