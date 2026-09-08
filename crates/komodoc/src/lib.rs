//! Komodoc: host HTML, markdown and typst documents that readers can
//! annotate. One binary: the server, and the command line that talks to it.
//!
//! A library only so that `fuzz/` can reach the three modules that read what
//! a peer sends -- the shared document, the path rules, and the configuration
//! that holds those rules -- and the integration tests the automation peer.
//! Those are `pub`; everything else stays private, so the dead-code lint
//! still covers it. `main.rs` is one line, and `main` itself lives with the
//! command line it parses.

mod assets;
mod auth;
pub mod backup;
mod blob;
pub mod catalog;
mod checkpoint_cache;
mod cli;
pub use cli::main;
mod clock;
pub mod config;
mod export;
mod history;
mod http;
pub mod journal;
mod latex;
mod local;
pub mod maintenance;
mod origins;
pub mod paths;
pub mod peer;
mod pseudonym;
mod render;
mod retention;
mod room;
mod s3;
mod seed;
mod seed_examples;
mod serve;
mod server;
pub mod session;
mod storage;
mod store;
mod sync;
mod util;

#[cfg(test)]
mod tests;

/// The release version, stamped in at build time. Unreleased builds keep the
/// placeholder.
pub const VERSION: &str = match option_env!("KOMODOC_VERSION") {
    Some(version) => version,
    None => "dev",
};
