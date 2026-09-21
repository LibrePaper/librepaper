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

/// The optimistic-concurrency token an agent echoes back as
/// `expected_version`.
///
/// Deliberately not a digest of the whole serialized `Comment`, which is
/// what it used to be: a `Comment` now carries a *page* of its thread, so
/// that digest would have depended on how many replies the caller's page
/// happened to load and a `reply` would have raced its own version check.
/// This names the row and the thread instead -- the row's own mutable
/// fields, how many replies it has, and when the newest of them last
/// changed -- so the same comment produces the same token from a page, a
/// single-row read or an agent window, and any create, edit, resolve,
/// delete or reply changes it.
pub(crate) fn comment_version(comment: &Comment) -> String {
    request_digest(&json!({
        "id": comment.id,
        "body": comment.body,
        "motivation": comment.motivation,
        "color": comment.color,
        "resolved": comment.resolved,
        "resolved_at": comment.resolved_at,
        "created": comment.created,
        "creator": comment.creator,
        "proposal": comment.proposal,
        "outcome": comment.outcome,
        "render_digest": comment.render_digest,
        "presentation": comment.presentation,
        "replies": comment.reply_total,
        "replies_changed": comment.thread_changed_micros,
    }))
}
