//! HTTP endpoints used by the writing assistant.

use super::*;
use crate::room::{BatchCaller, BatchSuggestion, SourceAnchor};
use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct BatchRequest {
    pub revision: String,
    pub items: Vec<BatchItem>,
}

#[derive(Deserialize)]
pub(super) struct BatchItem {
    pub anchor: SourceAnchor,
    pub proposed: String,
    #[serde(default)]
    pub body: String,
}

impl Server {
    pub(super) async fn handle_assistant_batch(
        &self,
        request: Request<Body>,
        peer: SocketAddr,
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
        if !who.at_least(Role::Commenter) {
            return write_json(403, &json!({"error":"comment access is required"}));
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
        if !current_who.at_least(Role::Commenter) {
            return write_json(403, &json!({"error":"comment access changed"}));
        }
        let room = match self.rooms.try_get(slug).await {
            Ok(room) => room,
            Err(error) => {
                return write_json(503, &json!({"error":error.to_string(),"retryable":true}))
            }
        };
        let author = self.comment_author(&headers, arrival, &current_who.id);
        let creator = if current_who.id.is_signed_in() {
            current_who.id.name.clone()
        } else if author.is_empty() {
            "Anonymous".to_string()
        } else {
            pseudonym_for(&author, slug)
        };
        let items: Vec<BatchSuggestion> = parsed
            .items
            .into_iter()
            .map(|item| BatchSuggestion {
                source: item.anchor,
                proposed: item.proposed,
                body: item.body,
            })
            .collect();
        // Keep the same durability guarantee as an individual comment: the
        // current text is recorded once for the whole pass before the room's
        // batch lock is acquired. A current checkpoint is a no-op; failures
        // remain bookkeeping warnings, as in apply_from.
        if let Err(error) = room.checkpoint("comment", &creator).await {
            eprintln!(
                "warning: could not checkpoint {} for an assistant pass: {error}",
                room.slug
            );
        }
        let address = client_address(peer, &headers, &self.config.cost.trusted_proxies);
        match room
            .apply_suggestion_batch(
                &parsed.revision,
                &items,
                BatchCaller {
                    address: &address,
                    author: &author,
                    via: &current_who.link,
                    budget: current_who.comment_budget,
                    creator: &creator,
                },
            )
            .await
        {
            Ok((pass, results)) => write_json(200, &json!({"pass":pass,"results":results})),
            Err(refusal) => {
                let status = if refusal.0.contains("too many") {
                    429
                } else {
                    400
                };
                write_json(status, &json!({"error":refusal.0}))
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
                "can_checkpoint": can_edit,
                "can_suggest": can_comment,
            }),
        )
    }
}
