//! LibrePaper: host HTML, markdown and typst documents that readers can
//! annotate. One binary: the server, and the command line that talks to it.
//!
//! A library only so that `tools/fuzz/` can reach the modules that read what
//! somebody else wrote -- the shared document, the path rules, the
//! configuration that holds those rules, the source archives a version's
//! bytes travel in, the anchoring that places a reader's selection in the
//! source, and the loopback bridge protocol -- and the integration tests the
//! automation peer. Those are `pub`; everything else stays private, so the
//! dead-code lint still covers it. `main.rs` is one line, and `main` itself
//! lives with the command line it parses.
//!
//! One package per feature: `cli` is what a terminal runs, `server` what
//! answers HTTP, `room` one live document, `document` what a document is
//! whoever serves it, `storage` where the bytes go and what makes them
//! durable, `auth` who somebody is, `seed` the examples, and `local` the
//! loopback service that compiles TeX on the author's machine.

// Some narrow test support APIs are intentionally compiled only into the library
// test target; integration binaries do not consume them in that target.

mod agent_query;
mod assistant;
mod auth;
mod automation;
mod cli;
pub mod config;
mod document;
mod http;
mod local;
pub mod log;
mod private_files;
pub mod quarto;
pub mod results;
mod room;
mod seed;
mod server;
mod storage;
mod util;

pub use cli::main;
// The headless automation peer, for the integration tests in `tests/`.
pub use automation::peer;
// What the fuzz targets read: the shared document and the path rules, and the
// anchoring path -- `locate` turns a reader's selection into a range in a
// source file the author uploaded, so both of its inputs come from outside,
// and its index arithmetic runs between a UTF-8 file and UTF-16 offsets.
// `annotation` comes with it because it holds the range `locate` returns.
pub use document::{paths, session};
pub use room::{annotation, locate};
// The loopback service takes requests from any page in the browser, and a
// request there becomes files on the author's own machine.
pub use local::protocol;
// Two pieces of the durable layer, and only the two something outside this
// crate calls: `source_archive` is what `tools/fuzz/fuzz_targets/archive.rs`
// encodes and decodes, and `postgres` is the catalogue the deployment tests
// below stand a server on. Nothing is re-exported here to keep it out of the
// dead-code lint: a module that was public for that reason once hid an
// object-store leak for as long as it took somebody to drop the line.
pub use storage::{postgres, source_archive};
// What the deployment-shaped tests in `tests/` assemble: a whole server,
// built from the same types `server::serve` builds it from. Those tests
// (SPEC-server-is-a-log §14.2) are about the deployment rather than about a
// document -- losing the writer lease mid-buffer, an idle deployment issuing
// no queries, and authority revoked mid-buffer over a real socket -- so a
// hand-made subset of the server would prove nothing about the server.
// `postgres` and `log` are already public above and beside; these are the
// rest of what it takes to stand one up and drive it the way a browser or
// the sharing route actually does: a running `Server` behind a real socket,
// authenticated the same way a signed-in owner and a share-link guest are.
pub use auth::{sign_device, GithubApp, Identity, Policy, PROVIDER_GITHUB};
pub use document::store::Store;
pub use room::{Room, Rooms};
pub use server::Server;
pub use storage::blob::{BlobStore, FsStore};
pub use storage::worker;

#[cfg(test)]
mod tests;

/// The release version, stamped in at build time. Unreleased builds keep the
/// placeholder.
pub const VERSION: &str = match option_env!("LIBREPAPER_VERSION") {
    Some(version) => version,
    None => "dev",
};
