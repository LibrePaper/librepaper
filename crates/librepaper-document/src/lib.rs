//! Internal to librepaper: no stable API, versioned in lockstep with it.
//! What a document is whoever serves it: the shared text, the path rules, its
//! projection as a directory, and the results a computed document carries.

pub mod document;
pub mod quarto;
pub mod results;
