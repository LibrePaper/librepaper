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
// `config` and `log` are the two modules whose names a narrowing facade
// cannot take: a crate root holds one item per name, and these two live
// directly in it rather than nested, so `pub mod config { ... }` beside
// `mod config;` is simply a redefinition. Renaming the real ones would move
// about two hundred `crate::log::` paths for no behavioural gain, so they
// stay whole -- and the dead re-exports a narrowing pass exposed inside them
// have been removed by hand instead.
pub mod config;
mod document;
mod http;
mod local;
pub mod log;
mod private_files;
mod quarto;
mod results;
mod room;
mod seed;
mod server;
mod storage;
mod util;

pub use cli::main;
// The headless automation peer, for the integration tests in `tests/`.
pub use automation::peer;

// -- what is public, and why ------------------------------------------------
//
// Every module above is private. What follows is the whole of this crate's
// public surface, written out item by item rather than by making a module
// public, because a public module is invisible to the dead-code lint in
// both directions: nothing in it is ever reported as unused, and nothing
// outside it can tell which parts are actually load-bearing. A module that
// was public "for the fuzz targets" once hid an object-store leak for as
// long as it took somebody to drop the line, and this list is the answer to
// that: an item here is an item something outside the crate names, and
// anything that stops being named stops compiling as reachable and shows up.
//
// The facades keep the paths the way the consumers already spell them
// (`librepaper::postgres::PostgresCatalog`, `librepaper::session::put_text`)
// so nothing outside has to move.

/// The path rules, which decide what an uploaded name may be. A fuzz target
/// (`tools/fuzz/fuzz_targets/paths.rs`) runs every one of these on whatever
/// libFuzzer produces.
pub mod paths {
    pub use crate::document::paths::{
        check, collision_key, kind_of, normalise, placeholder, suffixed, Rules,
    };
}

/// The shared document. Its bytes come from a browser, so what reads them
/// is fuzzed (`tools/fuzz/fuzz_targets/document.rs`,
/// `tools/fuzz/fuzz_targets/update.rs`).
pub mod session {
    pub use crate::document::session::{
        apply_edits_at, apply_update, decode_update, encode_state, main_path, new_doc, paths_of,
        put_asset, put_text, replace_text, set_main, text_of, ASSETS, FILES, MAIN, META, PATHS,
    };
}

/// Anchoring: `locate` turns a reader's selection into a range in a source
/// file the author uploaded, so both of its inputs come from outside and its
/// index arithmetic runs between a UTF-8 file and UTF-16 offsets.
pub mod locate {
    pub use crate::room::locate::{flatten, is_html, locate, Candidate, Quote};
}

/// The range `locate` returns, as a comment stores it.
pub mod annotation {
    pub use crate::room::annotation::{CommentTarget, OriginalAnchor};
}

/// The loopback service takes requests from any page in the browser, and a
/// request there becomes files on the author's own machine.
pub mod protocol {
    pub use crate::local::protocol::{
        decode_preview, safe_relative_path, PreviewInputs, WorkspaceRequest,
    };
}

/// What a version's bytes travel in, encoded and decoded by
/// `tools/fuzz/fuzz_targets/archive.rs`.
pub mod source_archive {
    pub use crate::storage::source_archive::{
        decode, encode, ArchiveLimits, SourceArchive, SourceFile,
    };
}

/// The catalogue the deployment-shaped tests stand a server on.
pub mod postgres {
    pub use crate::storage::postgres::{
        AccountRecord, Error, MutationAuthorization, NewAccount, NewAnnotation, NewDocument,
        NewReply, PostgresCatalog, PostgresOptions,
    };
}

/// The background worker, which those tests run so deletion and compaction
/// behave as they do in a deployment.
pub mod worker {
    pub use crate::storage::worker::{Handle, Worker};
}
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
pub use server::shell::ShellFile;
pub use server::Server;
pub use storage::blob::{BlobStore, FsStore};

#[cfg(test)]
mod tests;

/// The release version, stamped in at build time. Unreleased builds keep the
/// placeholder.
pub const VERSION: &str = match option_env!("LIBREPAPER_VERSION") {
    Some(version) => version,
    None => "dev",
};
