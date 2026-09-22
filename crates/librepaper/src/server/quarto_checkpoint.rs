//! Deliberate, execution-free input checkpoint for a local Quarto invocation.
//!
//! A local render needs the source it is about to run on to be durable and
//! named before it starts, so a slow render's result can be traced back to
//! the exact input it ran against rather than to whatever the document
//! happens to say by the time it finishes. That is exactly what a label is
//! (§7.1: no precondition, records head vector, frontier and digest), so
//! this route is `history::Label` with `reason: "render"` in place of
//! `"label"`.

use super::*;

impl Server {
    pub(super) async fn handle_quarto_checkpoint(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
        slug: &str,
    ) -> Reply {
        if cross_site_refused(request.headers(), arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return write_json(404, &json!({"error":"not found"})),
            Err(response) => return response,
        };
        let who = self
            .viewer(&entry, request.headers(), arrival, request.uri().query())
            .await;
        if !who.at_least(Role::Editor) {
            return write_json(403, &json!({"error":"edit access required"}));
        }
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            tree_sha256: String,
        }
        let bytes = match to_bytes(request.into_body(), 4096).await {
            Ok(bytes) => bytes,
            Err(_) => return write_json(413, &json!({"error":"checkpoint request is too large"})),
        };
        let input: Input = match serde_json::from_slice(&bytes) {
            Ok(input) => input,
            Err(_) => return write_json(400, &json!({"error":"tree_sha256 is required"})),
        };
        if !is_sha(&input.tree_sha256) {
            return write_json(400, &json!({"error":"invalid tree_sha256"}));
        }
        let room = match self.rooms.get(slug).await {
            Ok(room) => room,
            Err(error) => {
                return write_json(503, &json!({"error":error.to_string(),"retryable":true}))
            }
        };
        let projected = match room.projection().await {
            Ok(projected) => projected,
            Err(error) => return sequencer_reply(&error),
        };
        if !crate::document::render::is_quarto(&projected.projection.main) {
            return write_json(400, &json!({"error":"document is not Quarto source"}));
        }
        if projected.projection.digest() != input.tree_sha256 {
            return write_json(
                409,
                &json!({"error":"source changed or edits have not synchronized"}),
            );
        }
        let authority = who.document_authority();
        let row = match room
            .take_label(
                "render",
                None,
                who.attribution(),
                &authority,
                Some(uuid::Uuid::new_v4()),
            )
            .await
        {
            Ok(row) => row,
            Err(error) => return refused("checkpoint a render", &error),
        };
        // Edits can arrive while the transaction above was in flight. The
        // label names whatever head the flush actually captured, which may
        // already differ from the digest this render claimed; a checkpoint
        // of different inputs is useful history, but cannot authorize this
        // job's claimed source association.
        let captured = row
            .tree_digest
            .as_ref()
            .map(hex::encode)
            .unwrap_or_default();
        if captured != input.tree_sha256 {
            return write_json(409, &json!({"error":"source changed while checkpointing"}));
        }
        write_json(
            200,
            &json!({
                "revision": row.id,
                "tree_sha256": captured,
                "main": projected.projection.main,
            }),
        )
    }
}
