//! Platform-neutral document domain boundary.
//!
//! This crate deliberately has no I/O, clock, database, HTTP, filesystem or
//! async-runtime dependency. It holds the three things both sides of the
//! wire have to agree on exactly: the schema constants that name the four
//! root maps, the path rules, and the projection algorithm that turns a
//! `LoroDoc` into a directory.
//!
//! What it deliberately no longer holds is validation. Under
//! SPEC-server-is-a-log the server never refuses an update for its content
//! and never repairs one: a shape the schema does not use is simply absent
//! from the projection, with a diagnostic (§2.2, §4.4). The resource bounds
//! that remain are on inputs -- update size, log quota, memory budget -- and
//! live where those inputs arrive.

pub mod paths;
pub mod projection;

pub use paths::{Kind, Rules};
pub use projection::{project, Diagnostic, Entry, Projected, Projection};

pub const SCHEMA_VERSION: u32 = 1;
pub const ROOTS: [&str; 4] = [FILES, PATHS, ASSETS, META];

pub const FILES: &str = "files";
pub const PATHS: &str = "paths";
pub const ASSETS: &str = "assets";
pub const META: &str = "meta";

/// The key in `meta` that names the main file, by id.
pub const MAIN: &str = "main";
