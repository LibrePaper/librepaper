use super::*;
use crate::room::agent::OperationKey;

impl Server {
    pub(super) async fn mcp_cancellation(
        &self,
        _slug: &str,
        _actor: &str,
        _who: &Viewer,
        _target: &OperationKey,
    ) -> Result<Option<Value>, Failure> {
        Ok(None)
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
        self.mcp_epoch(actor, &operation, false)?;
        self.mcp_epoch(actor, &target, true)?;
        let _who = self.mcp_recheck(slug, headers, arrival, actor).await?;
        let kind = args
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("operation");
        let target_id = args.get("id").and_then(Value::as_str).unwrap_or(&target.id);
        if kind != "render" {
            return Err(Failure::new(
                "unsupported",
                "remote document operations cannot be cancelled reliably; inspect the document for an in-flight result",
            ));
        }
        let conversation = headers
            .get("x-librepaper-conversation")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let token = headers
            .get("x-librepaper-chat-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if conversation.is_empty() || token.is_empty() {
            return Err(Failure::new(
                "renderer_unavailable",
                "there is no browser render to cancel",
            ));
        }
        self.chat
            .cancel_render(slug, conversation, token, target_id)
            .await
            .map_err(|(_, message)| Failure::new("renderer_unavailable", message))?;
        Ok(
            json!({"operation":operation,"target_operation":target,"kind":kind,"id":target_id,"status":"cancel_requested"}),
        )
    }
}
