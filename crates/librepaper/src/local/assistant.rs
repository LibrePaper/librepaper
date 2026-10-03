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

impl AsLink for protocol::AssistantRenewal {
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
    if !body
        .agent_token
        .starts_with(librepaper_base::auth::AGENT_GRANT_PREFIX)
    {
        return write_json(
            401,
            &json!({"error": "sign in to authorize the local assistant"}),
        );
    }
    // An agent that cannot be driven is reported as such rather than silently
    // replaced by a different one. Which model runs is the user's choice, and
    // substituting it quietly would be the opposite of bringing your own.
    let Some(command) = super::acp_agents::acp_command(&inner.state_home, &body.agent) else {
        return write_json(
            409,
            &json!({"error": "that agent cannot be driven from the sidebar on this computer"}),
        );
    };
    let environment = super::acp_agents::acp_environment(&inner.state_home, &body.agent);
    let store = super::connections::ConnectionStore::new(&inner.state_home);
    if let Err(error) = store.put_runner(
        &parsed_link.credential_url(),
        origin,
        &body.conversation,
        &body.chat_token,
    ) {
        return write_json(500, &json!({"error": error}));
    }
    match inner
        .assistant_sessions
        .start(
            &parsed_link,
            &body.conversation,
            &body.chat_token,
            &body.agent_token,
            &command,
            &environment,
        )
        .await
    {
        Ok(()) => write_json(200, &json!({"running": true})),
        Err(error) => write_json(409, &json!({"error": error})),
    }
}

/// Replace the scoped server credential for a live runner. The browser's
/// signed-in session mints the replacement and transfers only this bearer
/// over the existing paired loopback channel.
pub(super) async fn handle_assistant_renew(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    let (body, link, _origin) =
        match accept::<protocol::AssistantRenewal>(inner, headers, origin, request).await {
            Ok(accepted) => accepted,
            Err(response) => return *response,
        };
    if !body
        .agent_token
        .starts_with(librepaper_base::auth::AGENT_GRANT_PREFIX)
    {
        return write_json(
            401,
            &json!({"error": "sign in again to renew assistant access"}),
        );
    }
    match inner
        .assistant_sessions
        .renew_agent_token(&link, &body.conversation, &body.agent_token)
        .await
    {
        Ok(()) => write_json(200, &json!({"renewed": true})),
        Err(error) => write_json(409, &json!({"renewed": false, "error": error})),
    }
}

/// Whether the sidebar assistant is attached to this conversation.
pub(super) async fn handle_assistant_status(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    let (body, link, _origin) =
        match accept::<protocol::AssistantQuery>(inner, headers, origin, request).await {
            Ok(accepted) => accepted,
            Err(response) => return *response,
        };
    match inner
        .assistant_sessions
        .status(&link, &body.conversation)
        .await
    {
        Ok(status) => {
            let value = match serde_json::to_value(&status) {
                Ok(value) => value,
                Err(error) => {
                    return write_json(500, &json!({"running": false, "error": error.to_string()}))
                }
            };
            let state = value["state"].as_str().unwrap_or_default().to_string();
            let running = !matches!(state.as_str(), "stopped" | "failed" | "");
            write_json(200, &json!({"running": running, "state": value["state"]}))
        }
        Err(crate::assistant::lifecycle::Error::NotRunning) => {
            write_json(200, &json!({"running": false, "state": null}))
        }
        Err(error) => write_json(500, &json!({"running": false, "error": error.to_string()})),
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
    let (body, link, _origin) =
        match accept::<protocol::AssistantQuery>(inner, headers, origin, request).await {
            Ok(accepted) => accepted,
            Err(response) => return *response,
        };
    match inner
        .assistant_sessions
        .stop(&link, &body.conversation)
        .await
    {
        Ok(()) => write_json(200, &json!({"stopped": true})),
        Err(error) => write_json(409, &json!({"error": error.to_string()})),
    }
}
