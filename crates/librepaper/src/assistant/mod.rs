//! Local assistant execution and durable task state.
//!
//! The companion (`local start`) is the only thing that ever runs a session:
//! `registry::SessionRegistry` spawns one supervised tokio task per
//! (document, conversation) pair and drives `runtime::run` inside it,
//! directly in the companion's own process. There is no separate runner
//! process any more; see `crate::local::assistant` for the loopback routes
//! that call into the registry.

pub(crate) mod acp;
pub(crate) mod context;
pub(crate) mod guidance;
pub(crate) mod journal;
pub(crate) mod lifecycle;
pub(crate) mod protocol;
pub(crate) mod registry;
pub(crate) mod runtime;
pub(crate) mod task;
pub(crate) mod transport;
