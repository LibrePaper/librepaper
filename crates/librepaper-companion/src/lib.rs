//! Internal to librepaper: no stable API, versioned in lockstep with it.
//! See docs/dev/specs/SPEC-split-crates.md, "Workspace layout".
//! The companion: the local app that compiles on the author's machine, the
//! assistant sessions it hosts, and the headless automation peer.

pub mod assistant;
pub mod automation;
pub mod local;

/// The release version, stamped in at build time for release artifacts and
/// derived from the package version for crates.io installs.
// The workspace version reaches the source through the build script, which
// is the one place a sub-crate may read it.
pub const VERSION: &str = match option_env!("LIBREPAPER_VERSION") {
    Some(version) => version,
    None => env!("LIBREPAPER_BUILD_VERSION"),
};
