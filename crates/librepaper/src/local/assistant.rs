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
    let access = connection.access;
    let connection_name = body.connection.clone();
    let selected_agent = body.agent.clone();
    let store = super::connections::ConnectionStore::new(&inner.state_home);
    let expected_config_hash = match crate::automation::peer::DocumentLink::parse(&link, "") {
        Ok(parsed) => crate::assistant::lifecycle::configuration_hash(&parsed, &command),
        Err(error) => return write_json(409, &json!({"error": error})),
    };
    let status_link = link.clone();
    let status_conversation = body.conversation.clone();
    let conversation_for_status = status_conversation.clone();
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
            Ok(()) => {
                let status = match tokio::task::spawn_blocking(move || {
                    crate::assistant::status(&status_link, &conversation_for_status)
                })
                .await
                {
                    Ok(Ok(status)) => status,
                    Ok(Err(error)) => {
                        return write_json(
                            500,
                            &json!({"error": format!("assistant started but status could not be read: {error}")}),
                        )
                    }
                    Err(error) => {
                        return write_json(
                            500,
                            &json!({"error": format!("assistant started but status task failed: {error}")}),
                        )
                    }
                };
                let Some(nonce) = status["nonce"].as_str().filter(|value| !value.is_empty()) else {
                    return write_json(
                        500,
                        &json!({"error": "assistant started but runner identity is unavailable"}),
                    );
                };
                let Some(config_hash) = status["config_hash"]
                    .as_str()
                    .filter(|value| !value.is_empty())
                else {
                    return write_json(
                        500,
                        &json!({"error": "assistant started but runner configuration identity is unavailable"}),
                    );
                };
                if config_hash != expected_config_hash {
                    return write_json(
                        409,
                        &json!({"error": "assistant configuration changed while it was starting; inspect its current status before retrying"}),
                    );
                }
                let configuration = super::connections::AssistantConfiguration {
                    agent: selected_agent.clone(),
                    access,
                    nonce: nonce.to_string(),
                    config_hash: config_hash.to_string(),
                };
                if let Err(error) = store.set_assistant_configuration(
                    &connection_name,
                    &status_conversation,
                    configuration,
                ) {
                    return write_json(
                        500,
                        &json!({"error": format!("assistant started but configuration could not be retained: {error}")}),
                    );
                }
                write_json(200, &json!({"running": true, "agent": selected_agent}))
            }
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
    let connection_name = body.connection;
    let conversation_id = body.conversation;
    let conversation = conversation_id.clone();
    let configurations = connection.assistant_sessions;
    match tokio::task::spawn_blocking(move || crate::assistant::status(&link, &conversation)).await
    {
        Err(error) => write_json(
            500,
            &json!({"running": false, "detail": format!("assistant status task failed: {error}")}),
        ),
        Ok(result) => match result {
            Ok(status) => {
                let running = !matches!(status["state"].as_str(), Some("stopped" | "failed"));
                let configuration = configurations
                    .get(&conversation_id)
                    .filter(|configuration| {
                        running
                            && status["nonce"].as_str() == Some(configuration.nonce.as_str())
                            && status["config_hash"].as_str()
                                == Some(configuration.config_hash.as_str())
                    });
                write_json(
                    200,
                    &json!({
                        "running": running,
                        "connection": connection_name,
                        "access": configuration.map(|value| value.access.as_str()),
                        "agent": configuration.map(|value| value.agent.as_str()),
                        "status": status
                    }),
                )
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
