//! Internal to librepaper: no stable API, versioned in lockstep with it.
//! See docs/dev/specs/SPEC-split-crates.md, "Workspace layout".
//! The companion: the local app that compiles on the author's machine, the
//! assistant sessions it hosts, and the headless automation peer.

pub mod assistant;
pub mod automation;
pub mod local;

/// The release version, stamped in at build time for release artifacts, or
/// the package version otherwise.
pub const VERSION: &str = match option_env!("LIBREPAPER_VERSION") {
    Some(version) => version,
    None => concat!("v", env!("CARGO_PKG_VERSION")),
};
