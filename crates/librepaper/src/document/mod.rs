//! What a document is, whoever is serving it: the index entry that says it
//! exists and who may do what to it, the shared Yjs document its editors
//! type into, the manifest of what it used to say, the rules for the paths
//! in its directory, and shared source interpretation. The server and
//! companion build on this; nothing here
//! answers HTTP or runs a command.

pub mod history;
pub mod html;
pub mod hunks;
#[cfg(test)]
pub mod needs;
pub mod paths;
pub mod quarto;
pub mod quota;
pub mod render;
pub mod retention;
pub mod session;
pub mod store;
