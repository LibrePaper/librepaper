//! The local LibrePaper app: a loopback service that renders documents with
//! the tools installed on the author's machine.
//!
//! `protocol` is the wire contract; `service` and `store` are the HTTP
//! surface and the state files it keeps; `render` is the pipeline that
//! writes a project's files to a folder, runs one command there and reads
//! one result file; `native` runs subprocesses; `cli` is the command line.

pub mod acp_agents;
pub(crate) mod assistant;
pub(crate) mod backup;
pub mod bindings;
pub mod cli;
pub mod connections;
pub mod credentials;
pub mod environment;
pub mod folder;
pub mod lifecycle;
pub mod native;
pub mod paths;
pub mod protocol;
pub mod render;
pub mod service;
pub mod store;
pub(crate) mod tools;

pub mod zotero;
