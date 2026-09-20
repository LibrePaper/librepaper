//! What the agent tools need of a comment beyond what `comments::load` gives
//! them: a version token for optimistic concurrency, and a read of one by id.
//!
//! Everything that used to live here beyond that -- `AnnotationBatch`,
//! `AgentAnnotationAuthority`, `apply_agent_annotations` -- is gone. It was a
//! bespoke bulk upsert/delete/reply path built against the pre-cutover
//! `command_owner` lock and the receipt-keyed `apply_annotation_batch`
//! catalogue call (§12: both are deleted). What replaced it is not a second
//! copy of that machinery rebuilt against the sequencer: `comments.rs` now
//! exposes the semantic commands themselves -- `AddComment`, `AddReply`,
//! `ResolveComment`, `DeleteComment`, `AcceptSuggestion`, `RejectSuggestion`,
//! `AgentSuggestionBatch` -- and an agent-facing caller runs one of those
//! through `room.command` exactly as a browser's does (§7). There is nothing
//! left for a batch adapter in between to do.
use serde_json::json;

use super::catalog::request_digest;
use super::*;

pub(crate) fn comment_version(comment: &Comment) -> String {
    request_digest(&json!(comment))
}
