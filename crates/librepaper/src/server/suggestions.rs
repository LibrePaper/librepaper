//! HTTP endpoints used by the writing assistant.

use super::*;
use crate::room::{self, OriginalAnchor};
use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct BatchRequest {
    /// The `tree_digest` the caller read the document at -- the projection
    /// digest `document_read` hands out. It says the source still says what it
    /// said when the ranges below were worked out, and it is not stored:
    /// what each anchor goes on record against is the room's own frontier at
    /// the moment it verifies the range.
    pub tree_digest: String,
    pub items: Vec<BatchItem>,
}

#[derive(Deserialize)]
pub(super) struct BatchItem {
    /// A client-chosen id, when the caller sends one: the primary key a
    /// retry has to repeat verbatim for §7.2's `ON CONFLICT DO NOTHING` to
    /// land on the same row rather than a duplicate. A caller that omits it
    /// gets one derived from this item's own content instead, which is
    /// retry-safe only as long as a retry resubmits unchanged content.
    #[serde(default)]
    pub id: Option<String>,
    pub original_anchor: OriginalAnchor,
    pub proposed: String,
    #[serde(default)]
    pub body: String,
}

/// A stable id for a batch item that did not bring its own: the sha256 of
/// what makes it this suggestion and nothing else, truncated into a UUID the
/// same way `server::mcp::comments::comment_uuid` derives one.
fn content_id(slug: &str, index: usize, exact: &str, proposed: &str) -> uuid::Uuid {
    let digest = Sha256::digest(format!("{slug}\0{index}\0{exact}\0{proposed}").as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Builder::from_random_bytes(bytes).into_uuid()
}

impl Server {
    pub(super) async fn handle_assistant_batch(
        &self,
        request: Request<Body>,
        _peer: SocketAddr,
        arrival: &Arrival,
        slug: &str,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error":"bad slug"}));
        }
        if cross_site_refused(request.headers(), arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let headers = request.headers().clone();
        let query = request.uri().query().map(str::to_string);
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return write_json(404, &json!({"error":"not found"})),
            Err(response) => return response,
        };
        // Assistant mutations are always link-scoped. In particular, an
        // owner session must not widen a read/comment link merely because it
        // is present on the same request.
        let mut bounded = headers.clone();
        bounded.insert(AUTOMATION_HEADER, HeaderValue::from_static("1"));
        let who = self
            .viewer(&entry, &bounded, arrival, query.as_deref())
            .await;
        if who.auth_failed {
            return write_json(
                401,
                &json!({"error":"authentication expired or was revoked"}),
            );
        }
        if !self.may_read(&entry, &who) {
            return write_json(404, &json!({"error":"not found"}));
        }
        if !who.at_least(Role::Editor) {
            return write_json(
                403,
                &json!({"error":"editor access is required for suggestions"}),
            );
        }
        let bytes = match to_bytes(request.into_body(), 1 << 20).await {
            Ok(bytes) => bytes,
            Err(_) => return write_json(413, &json!({"error":"batch too large"})),
        };
        let parsed = match serde_json::from_slice::<BatchRequest>(&bytes) {
            Ok(parsed) => parsed,
            Err(_) => return write_json(400, &json!({"error":"invalid suggestion batch"})),
        };
        // Body reads are an await boundary. Recheck the link/session before
        // admitting any annotation, as the comments route does.
        let current_entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return write_json(404, &json!({"error":"not found"})),
            Err(response) => return response,
        };
        let current_who = self
            .viewer(&current_entry, &bounded, arrival, query.as_deref())
            .await;
        if current_who.auth_failed || !self.may_read(&current_entry, &current_who) {
            return write_json(403, &json!({"error":"comment access changed"}));
        }
        if !current_who.at_least(Role::Editor) {
            return write_json(403, &json!({"error":"editor access changed"}));
        }
        let room = match self.rooms.get(slug).await {
            Ok(room) => room,
            Err(error) => {
                return write_json(503, &json!({"error":error.to_string(),"retryable":true}))
            }
        };
        if let Some(why) = room.unreadable().await {
            return write_json(503, &json!({"error":why,"retryable":true}));
        }
        // `AgentSuggestionBatch::evaluate` checks each item's own quote
        // against head (§7.1); it has no notion of a whole-document tree
        // digest, and it would be the wrong place for one -- §7.1's
        // `StaleSelection` is about a rendered page's digest, not source the
        // caller read directly. This is a pre-flight only: a documented
        // tree digest the caller no longer has is worth refusing before the
        // work of relocating every item, but it is not the precondition
        // that decides whether any one item is admitted.
        if !parsed.tree_digest.is_empty() {
            match room.projection().await {
                Ok(projected) if projected.projection.digest() != parsed.tree_digest => {
                    return write_json(
                        409,
                        &json!({"error":"preflight: document moved since this tree digest was read","retryable":true}),
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    return write_json(503, &json!({"error":error.to_string(),"retryable":true}))
                }
            }
        }
        let author = self.comment_author(&headers, arrival, &current_who.id);
        let creator = if current_who.id.is_signed_in() {
            current_who.id.name.clone()
        } else if author.is_empty() {
            "Anonymous".to_string()
        } else {
            pseudonym_for(&author, slug)
        };
        // Nothing is recorded before the pass. Each suggestion's range is
        // relocated against head inside `AgentSuggestionBatch::evaluate`
        // (§7 step 2), which is a name for that state that costs nothing to
        // write and cannot be stale by the time the sequencer's lock is
        // taken; only the quoted words the client already captured travel
        // here, never an offset or a revision it worked out on its own.
        let submitted = parsed.items.len();
        let items: Vec<room::BatchSuggestion> = parsed
            .items
            .into_iter()
            .enumerate()
            .filter_map(|(index, item)| {
                let source = item.original_anchor.target.source()?.clone();
                let id = item
                    .id
                    .as_deref()
                    .and_then(|id| uuid::Uuid::parse_str(id).ok())
                    .unwrap_or_else(|| content_id(slug, index, &source.exact, &item.proposed));
                Some(room::BatchSuggestion {
                    id,
                    exact: source.exact,
                    prefix: source.prefix,
                    suffix: source.suffix,
                    proposed: item.proposed,
                    body: item.body,
                })
            })
            .collect();
        if items.len() != submitted {
            return write_json(
                400,
                &json!({"error":"every suggestion must be anchored to a passage of source text"}),
            );
        }
        let author_account_id = uuid::Uuid::parse_str(&current_who.id.id).ok();
        let account_id = uuid::Uuid::parse_str(&current_who.id.id).ok();
        // The same identity `authority` below carries, in the shape
        // `authorize_annotation_mutation` checks a comment command's rung
        // against (§7): the account's live session and grant, or the link's.
        let authorization = crate::storage::postgres::MutationAuthorization {
            principal_key: account_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| current_who.key.clone()),
            account_id,
            session_generation: current_who.id.session_generation.parse::<i64>().ok(),
            token_hash: (!current_who.link.is_empty())
                .then(|| hex::decode(&current_who.link).ok())
                .flatten()
                .and_then(|bytes| bytes.try_into().ok()),
            policy_editor: self.ceiling_for(&current_who.id).edit,
        };
        let mut cmd = match room::AgentSuggestionBatch::new(
            room.catalog().clone(),
            room.document_id,
            items,
            &self.config,
            creator,
            author_account_id,
            author,
            authorization,
        ) {
            Ok(cmd) => cmd,
            Err(error) => return write_json(400, &json!({"error":error})),
        };
        let authority = crate::storage::postgres::Authority {
            principal_key: account_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| current_who.key.clone()),
            account_id,
            link_hash: (!current_who.link.is_empty())
                .then(|| hex::decode(&current_who.link).ok())
                .flatten(),
        };
        match room.command(&authority, &mut cmd).await {
            Ok(outcomes) => {
                let results: Vec<Value> = outcomes
                    .into_iter()
                    .map(|outcome| match outcome {
                        room::BatchItemResult::Created(comment) => {
                            json!({"status":"created","comment":*comment})
                        }
                        room::BatchItemResult::Refused(reason) => {
                            json!({"status":"refused","reason":reason})
                        }
                    })
                    .collect();
                write_json(200, &json!({"results":results}))
            }
            Err(error) => {
                let message = error.to_string();
                let status = if message.contains("too many") {
                    429
                } else {
                    400
                };
                write_json(status, &json!({"error":message}))
            }
        }
    }

    pub(super) async fn handle_assistant_capabilities(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
    ) -> Reply {
        // This is a read endpoint, so a browser need not send the mutation
        // marker used by POST routes. Still enforce the origin and fetch-site
        // checks before revealing whether the selected key is live.
        if header_of(headers, "origin")
            .is_some_and(|origin| !origin.is_empty() && origin != arrival.reader_origin())
            || header_of(headers, "sec-fetch-site")
                .is_some_and(|site| !site.is_empty() && site != "same-origin" && site != "none")
        {
            return write_json(403, &cross_site_refusal());
        }
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error":"bad slug"}));
        }
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return write_json(404, &json!({"error":"not found"})),
            Err(response) => return response,
        };
        // Force the ordinary viewer resolver into link-bounded automation
        // mode. A browser session may establish deployment policy, but it
        // cannot turn a selected read/comment link into an owner/editor link.
        let mut bounded = headers.clone();
        bounded.insert(AUTOMATION_HEADER, HeaderValue::from_static("1"));
        let who = self.viewer(&entry, &bounded, arrival, None).await;
        if who.auth_failed {
            return write_json(
                401,
                &json!({"error":"authentication expired or was revoked"}),
            );
        }
        if !self.may_read(&entry, &who) {
            return write_json(404, &json!({"error":"not found"}));
        }
        let can_read = true;
        let can_comment = who.at_least(Role::Commenter);
        let can_edit = who.at_least(Role::Editor);
        write_json(
            200,
            &json!({
                "can_read": can_read,
                "can_comment": can_comment,
                "can_edit": can_edit,
                "can_delete": can_comment,
                "can_resolve": can_comment,
                "can_label": can_edit,
                "can_suggest": can_edit,
            }),
        )
    }
}
