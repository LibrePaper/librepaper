//! Loopback assistant route handlers.
//!
//! Holding the protected document link is the authority here -- it carries
//! the document key -- so every route below takes the link directly. There is
//! no per-origin ownership check beyond the pairing itself: any paired origin
//! that holds a link may drive the assistant attached to it.

use super::protocol;
use super::service::*;
use axum::{
    body::Body,
    http::{HeaderMap, Request},
};
use serde_json::json;

/// Authenticate the request (which requires a paired origin), read and
/// decode the JSON body, and parse its `link` field. Every assistant route
/// starts here.
async fn accept<'a, T>(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&'a str>,
    request: Request<Body>,
) -> Result<(T, crate::automation::peer::DocumentLink, &'a str), Box<Reply>>
where
    T: for<'de> serde::Deserialize<'de> + AsLink,
{
    authenticate(inner, headers, origin).map_err(Box::new)?;
    // `authenticate` above already refused a missing Origin header.
    let origin = origin.expect("authenticate requires an origin");
    let body = read_json_body::<T>(request).await.map_err(Box::new)?;
    let link = match crate::automation::peer::DocumentLink::parse(body.link(), "") {
        Ok(link) => link,
        Err(error) => return Err(Box::new(write_json(400, &json!({"error": error})))),
    };
    Ok((body, link, origin))
}

/// What every request body handled here has in common: the document link.
trait AsLink {
    fn link(&self) -> &str;
}

impl AsLink for protocol::AssistantRequest {
    fn link(&self) -> &str {
        &self.link
    }
}

impl AsLink for protocol::AssistantQuery {
    fn link(&self) -> &str {
        &self.link
    }
}

/// Start the sidebar assistant, driving whichever installed agent the user
/// picked.
pub(super) async fn handle_assistant_start(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    let (body, parsed_link, origin) =
        match accept::<protocol::AssistantRequest>(inner, headers, origin, request).await {
            Ok(accepted) => accepted,
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
    let store = super::connections::ConnectionStore::new(&inner.state_home);
    if let Err(error) = store.put_runner(
        &parsed_link.credential_url(),
        origin,
        &body.conversation,
        &body.chat_token,
    ) {
        return write_json(500, &json!({"error": error}));
    }
    let link = body.link;
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
        Ok(Ok(())) => write_json(200, &json!({"running": true})),
        Ok(Err(error)) => write_json(409, &json!({"error": error})),
    }
}

/// Whether the sidebar assistant is attached to this conversation.
pub(super) async fn handle_assistant_status(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    let (body, _link, _origin) =
        match accept::<protocol::AssistantQuery>(inner, headers, origin, request).await {
            Ok(accepted) => accepted,
            Err(response) => return *response,
        };
    let link = body.link;
    let conversation = body.conversation;
    match tokio::task::spawn_blocking(move || crate::assistant::status(&link, &conversation)).await
    {
        Err(error) => write_json(
            500,
            &json!({"running": false, "detail": format!("assistant status task failed: {error}")}),
        ),
        Ok(Ok(status)) => {
            let value = match serde_json::to_value(&status) {
                Ok(value) => value,
                Err(error) => {
                    return write_json(500, &json!({"running": false, "error": error.to_string()}))
                }
            };
            let state = value["state"].as_str().unwrap_or_default().to_string();
            let running = !matches!(state.as_str(), "stopped" | "failed");
            write_json(200, &json!({"running": running, "state": state}))
        }
        Ok(Err(crate::assistant::lifecycle::Error::NotRunning)) => {
            write_json(200, &json!({"running": false, "state": null}))
        }
        Ok(Err(error)) => write_json(500, &json!({"running": false, "error": error.to_string()})),
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
    let (body, _link, _origin) =
        match accept::<protocol::AssistantQuery>(inner, headers, origin, request).await {
            Ok(accepted) => accepted,
            Err(response) => return *response,
        };
    let link = body.link;
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
