//! Internal to librepaper: no stable API, versioned in lockstep with it.
//! See docs/dev/specs/SPEC-split-crates.md, "Workspace layout".
//! The write path and where the bytes go: the log a document's edits ride,
//! the PostgreSQL catalogue, the blob store and the worker that compacts them.

pub mod log;
pub mod storage;
/// Test support: the opt-in PostgreSQL harness and the update-batch source.
/// Compiled always, not under `cfg(test)`, because the tests of sibling crates
/// use it too; it pulls in nothing this crate does not already depend on.
pub mod testing;
