//! Local assistant execution and durable task state.

pub(crate) mod acp;
pub(crate) mod context;
pub(crate) mod guidance;
pub(crate) mod journal;
pub(crate) mod lifecycle;
pub(crate) mod protocol;
pub(crate) mod runtime;
pub(crate) mod task;
pub(crate) mod transport;

use crate::automation::peer::{validate_conversation, DocumentLink};

/// Start the requested assistant configuration. Repeated starts with the same
/// configuration are idempotent; a changed configuration is an explicit
/// replacement handled in one place for both CLI and companion callers.
pub(crate) fn start(
    link: &str,
    conversation: &str,
    chat_token: &str,
    agent: &[String],
) -> Result<(), String> {
    let link = DocumentLink::parse(link, "")?;
    validate_conversation(conversation, chat_token)?;
    lifecycle::start_or_replace(&link, conversation, chat_token, None, agent)
        .map_err(|error| error.to_string())
}

pub(crate) fn status(
    link: &str,
    conversation: &str,
) -> Result<serde_json::Value, lifecycle::Error> {
    let link = DocumentLink::parse(link, "").map_err(lifecycle::Error::InvalidConfiguration)?;
    serde_json::to_value(lifecycle::status(&link, conversation, None)?)
        .map_err(|error| lifecycle::Error::Startup(error.to_string()))
}

pub(crate) fn stop(link: &str, conversation: &str) -> Result<(), lifecycle::Error> {
    let link = DocumentLink::parse(link, "").map_err(lifecycle::Error::InvalidConfiguration)?;
    lifecycle::stop(&link, conversation, None)
}
