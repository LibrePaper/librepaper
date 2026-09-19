//! The local LibrePaper app: a loopback service that runs native TeX tools on
//! the author's machine when the browser compiler cannot.
//!
//! `protocol` is the wire contract; `service` and `pairing` are the HTTP
//! surface and its authorization (package R1a); `discovery`, `native` and
//! discovery and the native runners find and run the tools; `cli` is the command line.

pub mod agents;
pub mod builders;
pub mod cli;
pub mod connections;
pub mod discovery;
pub mod embedded;
pub mod engine_adapter;
pub mod folder;
pub mod lifecycle;
mod management;
pub mod native;
pub mod pairing;
pub mod presets;
pub mod protocol;
pub mod quarto;
pub mod quarto_capture;
pub mod service;
pub mod texlog;
pub mod zotero;

pub(crate) mod preview;
