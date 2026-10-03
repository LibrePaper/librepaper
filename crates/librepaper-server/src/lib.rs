//! Internal to librepaper: no stable API, versioned in lockstep with it.
//! See docs/dev/specs/SPEC-split-crates.md, "Workspace layout".
//! The service: the routes the shell and the command line talk to, the socket
//! a room's readers hang on, and the document origin that serves the bytes.

pub mod server;
