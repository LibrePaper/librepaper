//! Internal to librepaper: no stable API, versioned in lockstep with it.
//! What a document is whoever serves it: the shared text, the path rules, its
//! projection as a directory, and the metadata that says how it is executed.

pub mod document;
pub mod results;
