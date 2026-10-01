//! The local LibrePaper app: a loopback service that runs native TeX tools on
//! the author's machine when the browser compiler cannot.
//!
//! `protocol` is the wire contract; `service` and `pairing` are the HTTP
//! surface and its authorization (package R1a); `discovery`, `native` and
//! discovery and the native runners find and run the tools; `cli` is the command line.

pub mod acp_agents;
pub(crate) mod approval;
pub(crate) mod assistant;
pub mod bindings;
pub(crate) mod backup;
pub mod builders;
pub mod cli;
pub mod connections;
pub(crate) mod credentials;
#[cfg(target_os = "linux")]
pub(crate) mod dialog;
pub mod discovery;
pub mod embedded;
pub mod engine_adapter;
pub mod folder;
pub mod integrations;
pub mod lifecycle;
pub mod native;
pub mod pairing;
pub(crate) mod paths;
pub mod protocol;
pub mod quarto;
pub mod quarto_capture;
pub mod service;
pub(crate) mod tools;

pub mod zotero;

pub(crate) mod preview;
