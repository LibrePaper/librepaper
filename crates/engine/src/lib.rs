//! The rendering engine: a markdown source or a typst source in, the
//! standalone HTML page Komodoc stores out.
//!
//! Two callers that cannot share a binary render through this crate: the
//! command line, natively, and the editor, as WebAssembly in the browser. One
//! crate means one configuration -- the same extensions, the same compiler,
//! the same page template -- so what the editor previews is what the server
//! stores, and neither can drift from the other.

pub mod diagnostic;
pub mod html;
pub mod page;

#[cfg(feature = "markdown")]
pub mod markdown;

#[cfg(feature = "typst")]
pub mod typst;

#[cfg(target_arch = "wasm32")]
mod abi;
