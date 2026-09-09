//! The local LibrePaper app: a loopback service that runs native TeX tools on
//! the author's machine when the browser compiler cannot. See
//! `docs/specs/latex-compiler.md` and `docs/specs/latex-interfaces.md`.
//!
//! `protocol` is the wire contract; `service` and `pairing` are the HTTP
//! surface and its authorization (package R1a); `discovery`, `native` and
//! `confine` find and run the tools (package R1b); `cli` is the command line.

pub mod cli;
pub mod confine;
pub mod discovery;
pub mod engine_adapter;
pub mod native;
pub mod pairing;
pub mod protocol;
pub mod quarto;
pub mod quarto_capture;
pub mod service;
pub mod texlog;
