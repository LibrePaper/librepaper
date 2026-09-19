//! Loopback assistant route handlers.

use super::protocol;
use super::service::*;
use axum::{
    body::Body,
    http::{HeaderMap, Request},
};
use serde_json::json;

/// Start the sidebar assistant, driving whichever installed agent the user
/// picked.
pub(super) async fn handle_assistant_start(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let body = match read_json_body::<protocol::AssistantRequest>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let connection = match owned_connection(inner, origin, &body.connection) {
        Ok(connection) => connection,
        Err(response) => return *response,
    };
    // An agent that cannot be driven is reported as such rather than silently
    // replaced by a different one. Which model runs is the user's choice, and
    // substituting it quietly would be the opposite of bringing your own.
    let Some(command) = super::acp_agents::acp_command(&inner.state_home, &body.agent) else {
        return write_json(
            409,
            &json!({"error": "that agent cannot be driven from the sidebar on this computer"}),
        );
    };
    let link = connection.link;
    let conversation = body.conversation;
    let chat_token = body.chat_token;
    let result = tokio::task::spawn_blocking(move || {
        crate::assistant::start(&link, &conversation, &chat_token, &command)
    })
    .await;
    match result {
        Err(error) => write_json(
            500,
            &json!({"error": format!("assistant startup task failed: {error}")}),
        ),
        Ok(result) => match result {
            Ok(()) => write_json(200, &json!({"running": true, "agent": body.agent})),
            Err(error) => write_json(409, &json!({"error": error})),
        },
    }
}

/// Whether the sidebar assistant is attached to this conversation.
pub(super) async fn handle_assistant_status(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let body = match read_json_body::<protocol::AssistantQuery>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let connection = match owned_connection(inner, origin, &body.connection) {
        Ok(connection) => connection,
        Err(response) => return *response,
    };
    let link = connection.link;
    let conversation = body.conversation;
    match tokio::task::spawn_blocking(move || crate::assistant::status(&link, &conversation)).await
    {
        Err(error) => write_json(
            500,
            &json!({"running": false, "detail": format!("assistant status task failed: {error}")}),
        ),
        Ok(result) => match result {
            Ok(status) => {
                let running = !matches!(status["state"].as_str(), Some("stopped" | "failed"));
                write_json(200, &json!({"running": running, "status": status}))
            }
            Err(crate::assistant::lifecycle::Error::NotRunning) => {
                write_json(200, &json!({"running": false}))
            }
            Err(error) => write_json(500, &json!({"running": false, "error": error.to_string()})),
        },
    }
}

/// Detach the sidebar assistant. Completed document changes are not undone by
/// stopping; only the attachment ends.
pub(super) async fn handle_assistant_stop(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let body = match read_json_body::<protocol::AssistantQuery>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let connection = match owned_connection(inner, origin, &body.connection) {
        Ok(connection) => connection,
        Err(response) => return *response,
    };
    let link = connection.link;
    let conversation = body.conversation;
    match tokio::task::spawn_blocking(move || crate::assistant::stop(&link, &conversation)).await {
        Err(error) => write_json(
            500,
            &json!({"error": format!("assistant stop task failed: {error}")}),
        ),
        Ok(result) => match result {
            Ok(()) => write_json(200, &json!({"stopped": true})),
            Err(error) => write_json(409, &json!({"error": error.to_string()})),
        },
    }
}
