//! Short-lived credentials the signed-in browser delegates to its local
//! companion. The companion receives a document-and-link grant, never the
//! browser's cookie.

use super::*;
use axum::extract::{Extension, Path, State};

/// `POST /api/documents/{slug}/agent-token` issues a five-minute grant for
/// the document and link this browser is currently viewing. The route is
/// cookie-only and same-origin so an unrelated site cannot mint authority
/// from a visitor's active session.
pub(super) async fn issue(
    State(server): State<Arc<Server>>,
    Extension(context): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    if request.method() != Method::POST {
        return plain(405, "method not allowed");
    }
    let headers = request.headers();
    if cross_site_refused(headers, &context.arrival)
        || headers.get(header::ORIGIN).is_none()
        || headers.contains_key(header::AUTHORIZATION)
    {
        return write_json(403, &json!({"error": "agent authorization must be requested by the signed-in document page"}));
    }

    let identity = context.identity();
    if !identity.is_signed_in() || !server.provider_configured(&identity) {
        return write_json(401, &json!({"error": "sign in again to authorize the companion"}));
    }

    let (entry, viewer) = match server
        .entry_viewer_in(&slug, &context, headers, request.uri().query())
        .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = server.check_readable(&entry, &viewer) {
        return response;
    }

    // A grant must name one live link capability. A bare owner page may read
    // the document, but cannot delegate that wider account authority to an
    // automation process.
    let link_hash = server.link_hash(headers, request.uri().query());
    if link_hash.is_empty() || entry.link_role(&link_hash, crate::util::now_unix()).is_none() {
        return write_json(403, &json!({"error": "open the document through a live share link to authorize its companion"}));
    }

    let delegated_role = automation_role(
        &entry,
        &link_hash,
        server.ceiling_for(&identity),
        crate::util::now_unix(),
    );
    let role = match delegated_role {
        Role::Reader => 1,
        Role::Commenter => 2,
        Role::Editor | Role::Owner => 3,
    };
    let expires_at = crate::auth::now_unix()
        + crate::auth::AGENT_GRANT_MAX_AGE.as_secs() as i64;
    let grant = crate::auth::AgentGrant {
        identity,
        slug,
        link_hash,
        role,
        expires_at,
    };
    let Some(token) = crate::auth::sign_agent_grant(&server.key, &grant) else {
        return write_json(400, &json!({"error": "could not create a scoped companion authorization"}));
    };
    write_json(
        200,
        &json!({"token": token, "expires_at": expires_at, "role": delegated_role.as_str()}),
    )
}
