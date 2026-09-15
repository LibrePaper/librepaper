//! The shared document, as the server holds it.
//!
//! One `LoroDoc` per open document, held natively so the server and the command
//! line can hold it without a JavaScript runtime beside them. The browser holds
//! the same document through the same Rust compiled to WebAssembly, so there is
//! one implementation of the format rather than two agreeing by convention.
//!
//! ## A document is a directory
//!
//! What the document holds is four maps rather than one text, because a paper
//! is `main.tex`, a `chapters/`, a `refs.bib` and a `fig/`, and a document that
//! is one text renders on one laptop and nowhere else.
//!
//! | map | keys | values |
//! | --- | --- | --- |
//! | `files` | an id | the text at it |
//! | `paths` | an id | the path that text is known by |
//! | `assets` | a path | the digest of the bytes at it |
//! | `meta` | `main` | the id of the main file |
//!
//! A text is keyed by an id and named separately because that is what makes a
//! rename free: renaming moves a string in `paths` and leaves the text where it
//! is, so a keystroke somebody makes into the file at the moment it is renamed
//! lands in the text it was always going to land in. Keying texts by path
//! instead would make a rename a delete and an insert, and would lose that
//! keystroke into a text no key reaches.
//!
//! Assets are keyed by path, because their bytes are not here: an asset is kept
//! in the store under its digest, and what the document holds is the name
//! somebody gave it.
//!
//! ## Offsets are UTF-16
//!
//! Every offset that crosses this module counts UTF-16 code units, because that
//! is what a browser counts in. The `*_utf16` family is the only text API used
//! here. Two things underneath are indexed differently and are converted at
//! their boundary rather than leaking: cursors are Unicode code points (see
//! [`anchors`]), and diff deltas are whichever basis the crate that computed
//! them was built for (see [`crate::document::hunks`]).

mod anchors;
mod edits;
mod presence;
mod repair;
mod shape;
mod sync;

pub use anchors::*;
pub use edits::*;
pub use presence::*;
pub use repair::*;
pub use shape::*;
pub use sync::*;
