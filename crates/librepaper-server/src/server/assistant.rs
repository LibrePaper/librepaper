//! HTTP endpoints used by the writing assistant.

use super::*;

fn assistant_capabilities(role: Role) -> Value {
    let can_comment = role.at_least(Role::Commenter);
    let can_edit = role.at_least(Role::Editor);
    json!({
        "can_read": true,
        "can_comment": can_comment,
        "can_edit": can_edit,
        "can_delete": can_comment,
        "can_resolve": can_comment,
        "can_label": can_edit,
        "can_suggest": can_edit,
        "can_reply": can_comment,
    })
}

/// The browser asks what authority an assistant would receive on its selected
/// link. A browser cookie is enough to authenticate this probe, but it must
/// not widen the answer beyond that link. A request already using an agent
/// grant has had the same bound applied by `viewer_as`, including the grant's
/// role ceiling.
fn assistant_link_role(
    entry: &IndexEntry,
    who: &Viewer,
    ceiling: librepaper_engine::storage::store::Ceiling,
    link_hash: &str,
    now: i64,
) -> Option<Role> {
    entry.link_role(link_hash, now)?;
    Some(if who.automation {
        who.role
    } else {
        automation_role(entry, link_hash, ceiling, now)
    })
}

impl Server {
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
        // Authenticate the browser normally, then report only the authority
        // an assistant would receive through the selected live link. Forcing
        // automation mode here would also require an agent bearer, which the
        // browser must not have before it starts the companion.
        let who = self.viewer(&entry, headers, arrival, None).await;
        if let Err(response) = self.check_readable(&entry, &who) {
            return response;
        }
        let link_hash = self.link_hash(headers, None);
        let Some(role) = assistant_link_role(
            &entry,
            &who,
            self.ceiling_for(&who.id),
            &link_hash,
            librepaper_base::util::now_unix(),
        ) else {
            return no_such_document();
        };
        write_json(200, &assistant_capabilities(role))
    }
}

#[cfg(test)]
mod capability_tests {
    use super::*;

    #[test]
    fn suggesting_requires_editor_permission_while_comments_allow_commenters() {
        for (role, can_suggest, can_edit, can_comment) in [
            (Role::Reader, false, false, false),
            (Role::Commenter, false, false, true),
            (Role::Editor, true, true, true),
            (Role::Owner, true, true, true),
        ] {
            let capabilities = assistant_capabilities(role);
            assert_eq!(capabilities["can_suggest"], can_suggest, "{role:?}");
            assert_eq!(capabilities["can_edit"], can_edit, "{role:?}");
            assert_eq!(capabilities["can_label"], can_edit, "{role:?}");
            assert_eq!(capabilities["can_comment"], can_comment, "{role:?}");
        }
    }

    #[test]
    fn browser_capability_probe_uses_selected_link_without_agent_bearer() {
        let now = librepaper_base::util::now_unix();
        let entry = IndexEntry {
            slug: "paper".into(),
            publisher: "owner".into(),
            publisher_id: "github:owner".into(),
            links: vec![LinkGrant {
                hash: "commenter-hash".into(),
                role: Role::Commenter.as_str().into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let owner = Identity {
            id: "github:owner".into(),
            handle: "owner".into(),
            provider: librepaper_base::auth::PROVIDER_GITHUB.into(),
            session_generation: "generation".into(),
            ..Default::default()
        };
        let browser = Viewer {
            id: owner,
            key: String::new(),
            link: "commenter-hash".into(),
            comment_budget: None,
            role: Role::Owner,
            automation: false,
            bearer: false,
            auth_failed: false,
            suggest_only: false,
            token: None,
        };
        let ceiling = librepaper_engine::storage::store::Ceiling {
            comment: true,
            edit: true,
        };

        assert_eq!(
            assistant_link_role(&entry, &browser, ceiling, "commenter-hash", now),
            Some(Role::Commenter)
        );
        assert_eq!(
            assistant_link_role(&entry, &browser, ceiling, "missing-hash", now),
            None
        );
    }
}
