//! What a document is, whoever is serving it: the index entry that says it
//! exists and who may do what to it, the shared Yjs document its editors
//! type into, the manifest of what it used to say, the rules for the paths
//! in its directory, and the rendering the command line does before it is
//! stored. The server and the command line both build on this; nothing here
//! answers HTTP or runs a command.

pub mod checkpoint_cache;
pub mod history;
pub mod html;
pub mod needs;
pub mod paths;
pub mod render;
pub mod retention;
pub mod session;
pub mod store;
