//! What a document is, whoever is serving it: the index entry that says it
//! exists and who may do what to it, the shared Loro document its editors
//! type into, the rules for the paths in its directory, and shared source
//! interpretation. The server and companion build on this; nothing here
//! answers HTTP or runs a command.
//!
//! ## Two things used to live here and no longer do
//!
//! **What a document says** was `history::Tree`, with its own digest and its
//! own canonical form. It is now `librepaper_document_core::Projection`,
//! computed by `project(&doc, &config.paths())`: the one projection
//! algorithm of SPEC-server-is-a-log §4.4, implemented in Rust and in
//! JavaScript and held equal by the fixtures in
//! `web/tests/fixtures/projection.json`. Two implementations of one concept
//! is what the cutover removes, and a digest that the browser and the server
//! can disagree about is worse than no digest at all.
//!
//! One thing `Tree` carried that `Projection` does not is the LaTeX engine.
//! That is deliberate. §4.4 defines the projection as what the document
//! *says*; a compile setting is a choice about how to render it. It lives in
//! `meta` and a caller reads it with `session::latex_engine`. Folding it
//! into the identity would mean two documents with identical text but
//! different engines failing to recognise each other as the same content,
//! which is exactly the question a digest exists to answer.
//!
//! **The version timeline** was `history::{Checkpoint, Manifest}` over
//! `document_versions`. Both tables are gone; the timeline is
//! `document_labels` (§8.2) and the server serializes
//! `storage::postgres::labels::LabelRecord` directly. The fields a
//! `Checkpoint` had that a label does not -- `tree`, `dirty`, `commit`,
//! `ancestry_gap`, `original_parent`, `archive_status` -- described a stored
//! archive of a rendered bundle, and a bounded retention policy that pruned
//! old checkpoints and reparented events to close the gaps that left.
//! Nothing prunes a label: it exists because somebody asked for it.
//!
//! [`retention`] is a different thing that is still live: whole-document
//! expiry by age, which has nothing to do with the version timeline.

pub mod html;
pub mod hunks;

#[cfg(test)]
pub mod needs;
pub mod paths;

pub mod render;
pub mod retention;
pub mod session;
pub mod store;
