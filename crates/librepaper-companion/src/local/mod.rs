//! The local LibrePaper app: a loopback service that runs native TeX tools on
//! the author's machine when the browser compiler cannot.
//!
//! `protocol` is the wire contract; `service` and `store` are the HTTP
//! surface and the state files it keeps (package R1a); `discovery`, `native` and
//! discovery and the native runners find and run the tools; `cli` is the command line.

pub mod acp_agents;
pub(crate) mod approval;
pub(crate) mod assistant;
pub(crate) mod backup;
pub mod bindings;
pub mod builders;
pub mod cli;
pub mod connections;
pub mod credentials;
pub mod discovery;
pub mod engine_adapter;
pub mod environment;
pub mod folder;
pub mod integrations;
pub mod lifecycle;
pub mod native;
pub mod paths;
pub mod protocol;
pub mod quarto;
pub mod quarto_capture;
pub mod service;
pub mod store;
pub(crate) mod tools;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod tray;

pub mod zotero;

pub(crate) mod preview;
