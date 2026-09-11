//! Durable cancellation for pending MCP operations.
use super::operations::Candidate;
use super::*;
use crate::room::agent::OperationKey;

fn render_request_id(candidate_id: &str, source_revision: &str) -> String {
    format!(
        "render_{}",
        hex::encode(Sha256::digest(
            format!("{candidate_id}\0{source_revision}").as_bytes()
        ))
    )
}

impl Server {
    pub(super) async fn mcp_cancellation(
        &self,
        slug: &str,
        actor: &str,
        target: &OperationKey,
    ) -> Result<Option<Value>, Failure> {
        let Some(catalog) = &self.store.catalog else {
            return Ok(None);
        };
        let target_request_id = target.scoped_request_id(actor);
        let slug_owned = slug.to_owned();
        let target_owned = target_request_id.clone();
        let cancellation = catalog
            .execute_catalog(256, move |catalog| {
                catalog.agent_cancellation(&slug_owned, &target_owned)
            })
            .await
            .map_err(|error| Failure::new("unavailable", error.to_string()))?;
        match cancellation {
            Some(value) => {
                let result = serde_json::from_str(&value.result)
                    .map_err(|_| Failure::new("internal", "invalid cancellation receipt"))?;
                self.mcp_cancelled_children(slug, actor, target, result)
                    .await
                    .map(Some)
            }
            None => Ok(None),
        }
    }

    pub(super) async fn mcp_cancel(
        &self,
        slug: &str,
        actor: &str,
        headers: &HeaderMap,
        arrival: &Arrival,
        args: &Value,
    ) -> Result<Value, Failure> {
        let operation: OperationKey = serde_json::from_value(
            args.get("operation").cloned().unwrap_or(Value::Null),
        )
        .map_err(|_| Failure::new("invalid_params", "cancellation operation is required"))?;
        let target: OperationKey =
            serde_json::from_value(args.get("target_operation").cloned().unwrap_or(Value::Null))
                .map_err(|_| Failure::new("invalid_params", "target operation is required"))?;
        if operation == target {
            return Err(Failure::new(
                "invalid_params",
                "a cancellation operation cannot target itself",
            ));
        }
        let kind = args
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("operation");
        if !matches!(kind, "operation" | "candidate" | "render") {
            return Err(Failure::new(
                "invalid_params",
                "unknown cancellation target kind",
            ));
        }
        let target_id = args.get("id").and_then(Value::as_str).unwrap_or(&target.id);
        if target_id.is_empty() || target_id.len() > 128 {
            return Err(Failure::new(
                "invalid_params",
                "cancellation target id is required",
            ));
        }
        let target_request_id = target.scoped_request_id(actor);
        let cancel_request_id = operation.scoped_request_id(actor);
        let request_digest = hex::encode(Sha256::digest(
            json!({"tool":"document_result","arguments":args}).to_string(),
        ));
        // All mutation tools share one retry namespace, including cancellation.
        self.mcp_receipt(slug, actor, &operation, Some(&request_digest))
            .await?;

        let Some(catalog) = &self.store.catalog else {
            return Err(Failure::new(
                "unavailable",
                "durable catalog required for cancellation",
            ));
        };
        // A retry must remain readable after the cancellation epoch expires.
        // It is still bound to this actor through the scoped request id and
        // the outer MCP handler has already rechecked current authority.
        let existing_slug = slug.to_owned();
        let existing_id = cancel_request_id.clone();
        let existing = catalog
            .execute_catalog(256, move |catalog| {
                catalog.agent_cancellation_by_request(&existing_slug, &existing_id)
            })
            .await
            .map_err(|error| Failure::new("unavailable", error.to_string()))?;
        if let Some(existing) = existing {
            if existing.request_digest != request_digest {
                return Err(Failure::new(
                    "operation_key_reused",
                    "cancellation operation already identifies different arguments",
                ));
            }
            let result = serde_json::from_str(&existing.result)
                .map_err(|_| Failure::new("internal", "invalid cancellation receipt"))?;
            return self
                .mcp_cancelled_children(slug, actor, &target, result)
                .await;
        }

        self.mcp_epoch(actor, &operation, false)?;
        self.mcp_epoch(actor, &target, true)?;
        self.mcp_admit(slug, actor, &operation, &request_digest)
            .await?;
        let who = self.mcp_recheck(slug, headers, arrival, actor).await?;

        // A completed receipt is an observable outcome, never something a
        // cancellation can roll back. The catalog still records this answer
        // so retries remain durable and idempotent.
        let committed = self
            .mcp_receipt(slug, actor, &target, None)
            .await?
            .filter(|value| value.get("status").and_then(Value::as_str) == Some("committed"))
            .map(|value| value.to_string());
        let cancel_slug = slug.to_owned();
        let cancel_target = target_request_id.clone();
        let cancel_id = cancel_request_id.clone();
        let cancel_digest = request_digest.clone();
        let cancel_kind = kind.to_owned();
        let cancel_target_id = target_id.to_owned();
        let cancel_committed = committed.clone();
        let account_id = who.id.id.clone();
        let generation = who.id.session_generation.clone();
        let link_hash = who.link.clone();
        let cancellation = catalog
            .execute_catalog(512, move |catalog| {
                catalog.cancel_agent_operation(
                    &cancel_slug,
                    &cancel_target,
                    &cancel_id,
                    &cancel_digest,
                    &cancel_kind,
                    &cancel_target_id,
                    cancel_committed.as_deref(),
                    &account_id,
                    &generation,
                    &link_hash,
                    now_unix(),
                )
            })
            .await
            .map_err(|error| Failure::new("unavailable", error.to_string()))?;

        // A browser render is only a pending waiter. Removing it is best
        // effort; the durable flag remains authoritative if the result races
        // this request or arrives after the waiter has timed out.
        if kind == "render" && cancellation.status == "cancel_requested" {
            if let Ok(candidate) = self
                .mcp_load::<Candidate>(slug, actor, target_id, "candidate")
                .await
            {
                let id = render_request_id(target_id, &candidate.source_revision);
                let conversation = headers
                    .get("x-librepaper-conversation")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("");
                let chat_token = headers
                    .get("x-librepaper-chat-token")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("");
                if !conversation.is_empty() && !chat_token.is_empty() {
                    let _ = self
                        .chat
                        .cancel_render(slug, conversation, chat_token, &id)
                        .await;
                }
            }
        }
        let result = serde_json::from_str(&cancellation.result)
            .map_err(|_| Failure::new("internal", "invalid cancellation receipt"))?;
        self.mcp_cancelled_children(slug, actor, &target, result)
            .await
    }
}
