//! LibrePaper: host HTML, markdown and typst documents that readers can
//! annotate. One binary: the server, and the command line that talks to it.
//!
//! A library only so that `tools/fuzz/` can reach the three modules that read what
//! a peer sends -- the shared document, the path rules, and the configuration
//! that holds those rules -- and the integration tests the automation peer.
//! Those are `pub`; everything else stays private, so the dead-code lint
//! still covers it. `main.rs` is one line, and `main` itself lives with the
//! command line it parses.
//!
//! One package per feature: `cli` is what a terminal runs, `server` what
//! answers HTTP, `room` one live document, `document` what a document is
//! whoever serves it, `storage` where the bytes go and what makes them
//! durable, `auth` who somebody is, `seed` the examples, and `local` the
//! loopback service that compiles TeX on the author's machine.

mod agent_query;
mod auth;
mod cli;
pub mod config;
mod document;
mod http;
mod local;
pub mod quarto;
pub mod results;
mod room;
mod seed;
mod server;
mod storage;
mod util;

pub use cli::main;
// The headless automation peer, for the integration tests in `tests/`.
pub use cli::peer;
// What the fuzz targets read: the shared document and the path rules.
pub use document::{paths, session};
// The durable layer is reachable from outside so that what it exposes and
// nothing yet calls -- restore, conversations, journal readers -- is API in
// progress rather than dead code to the lint. Drop this line to see the list.
pub use storage::{backup, catalog, journal, maintenance};

#[cfg(test)]
mod tests;

/// The release version, stamped in at build time. Unreleased builds keep the
/// placeholder.
pub const VERSION: &str = match option_env!("LIBREPAPER_VERSION") {
    Some(version) => version,
    None => "dev",
};
