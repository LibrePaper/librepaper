//! What renders on this side of the network.
//!
//! Each renderer is a crate of its own now, pinned by tag in `Cargo.toml`, and
//! this one gathers them under the names the rest of the binary already used.
//! The pin is the point: the browser fetches modules built from those same
//! tags, so what the editor previews and what a save stores come out of one
//! version of one implementation, and neither can drift from the other.
//!
//! What is genuinely left here is `html` -- authored HTML, which never had a
//! browser module of its own because a browser already has an HTML renderer.

pub mod html;

/// The shape every compile answers in, and the standalone page a document is
/// stored as.
pub use wasm_helpers::{diagnostic, page};

#[cfg(feature = "markdown")]
pub use wasm_markdown::markdown;

#[cfg(feature = "typst")]
pub use wasm_typst::typst;

#[cfg(feature = "bibliography")]
pub use wasm_bibliography::bib;

#[cfg(feature = "citations")]
pub use wasm_bibliography::citations;
