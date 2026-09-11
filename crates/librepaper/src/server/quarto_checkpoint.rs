//! Deliberate, execution-free input checkpoint for a local Quarto invocation.

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
        let room = match self.rooms.try_get(slug).await {
            Ok(room) => room,
            Err(error) => {
                return write_json(503, &json!({"error":error.to_string(),"retryable":true}))
            }
        };
        let tree = room.tree().await;
        if !crate::document::render::is_quarto(&tree.main) {
            return write_json(400, &json!({"error":"document is not Quarto source"}));
        }
        if tree.digest() != input.tree_sha256 {
            return write_json(
                409,
                &json!({"error":"source changed or edits have not synchronized"}),
            );
        }
        let actor = crate::storage::catalog::MutationAuthority {
            account_id: who.id.id.as_str(),
            owner_key: who.key.as_str(),
            generation: who.id.session_generation.as_str(),
            link_hash: who.link.as_str(),
            policy_editor: self.publishers.allows(&who.id.handle),
            automation: who.automation,
            unowned_publisher: false,
            execution_epoch: "",
            agent_checkpoint: None,
        };
        let revision = match room
            .checkpoint_now_with_authority("render", who.attribution(), actor)
            .await
        {
            Ok(Some(revision)) => revision,
            Ok(None) => {
                return write_json(
                    503,
                    &json!({"error":"source checkpoint is not ready","retryable":true}),
                )
            }
            Err(error) => return refused("could not checkpoint Quarto source", &error),
        };
        let point = match room.checkpoint_by_sha(&revision).await {
            Ok(Some(point)) => point,
            Ok(None) => {
                return write_json(
                    503,
                    &json!({"error":"source checkpoint is unavailable","retryable":true}),
                )
            }
            Err(error) => return write_json(503, &json!({"error":error,"retryable":true})),
        };
        // Edits can arrive while durable storage is awaited. A checkpoint of
        // different inputs is useful history, but cannot authorize this job's
        // claimed source association.
        if point.content_sha() != input.tree_sha256 {
            return write_json(409, &json!({"error":"source changed while checkpointing"}));
        }
        write_json(
            200,
            &json!({"revision":revision,"tree_sha256":point.content_sha(),"main":tree.main}),
        )
    }
}
