//! Every new account starts with private, normally owned starter documents.
//! The catalogue records each completed copy independently of its lifetime.

use super::*;
use crate::document::store::Publication;

struct Starter {
    main: &'static str,
    title: &'static str,
    format: &'static str,
    source: &'static str,
    extra: &'static str,
    extra_source: &'static str,
}

const STARTERS: [Starter; crate::seed::ACCOUNT_EXAMPLE_COUNT] = [
    Starter {
        main: "librepaper.md",
        title: "Learn LibrePaper with Markdown",
        format: "markdown",
        source: include_str!("../../../../docs/examples/tutorial-markdown/librepaper.md"),
        extra: "sections/rendering.md",
        extra_source: include_str!(
            "../../../../docs/examples/tutorial-markdown/sections/rendering.md"
        ),
    },
    Starter {
        main: "librepaper.typ",
        title: "Learn LibrePaper with Typst",
        format: "typst",
        source: include_str!("../../../../docs/examples/tutorial-typst/librepaper.typ"),
        extra: "sections/rendering.typ",
        extra_source: include_str!(
            "../../../../docs/examples/tutorial-typst/sections/rendering.typ"
        ),
    },
    Starter {
        main: "librepaper.html",
        title: "Learn LibrePaper with HTML",
        format: "html",
        source: include_str!("../../../../docs/examples/tutorial-html/librepaper.html"),
        extra: "sections/rendering.html",
        extra_source: include_str!(
            "../../../../docs/examples/tutorial-html/sections/rendering.html"
        ),
    },
    Starter {
        main: "librepaper.tex",
        title: "Learn LibrePaper with LaTeX",
        format: "latex",
        source: include_str!("../../../../docs/examples/tutorial-latex/librepaper.tex"),
        extra: "sections/rendering.tex",
        extra_source: include_str!(
            "../../../../docs/examples/tutorial-latex/sections/rendering.tex"
        ),
    },
    Starter {
        main: "librepaper.qmd",
        title: "Learn LibrePaper with Quarto",
        format: "quarto",
        source: include_str!("../../../../docs/examples/tutorial-quarto/librepaper.qmd"),
        extra: "sections/rendering.qmd",
        extra_source: include_str!(
            "../../../../docs/examples/tutorial-quarto/sections/rendering.qmd"
        ),
    },
];

impl Server {
    pub(crate) async fn initialize_account_examples(&self, who: &Identity) -> Result<(), String> {
        let Some(catalog) = &self.store.catalog else {
            return Ok(());
        };
        if pending_account_examples(catalog, &who.id)
            .await
            .map_err(|e| e.to_string())?
            .is_empty()
        {
            return Ok(());
        }
        let _guard = self.onboarding.lock().await;
        // The caller can be an identity captured before the account row was
        // refreshed. Read the live generation once and use it for every write
        // in this provisioning pass; accepting an empty or stale generation
        // would turn a valid authenticated retry into a 401.
        let account_id = who.id.clone();
        let account = catalog
            .execute_catalog(
                crate::server::SERVER_JOB_BYTES + account_id.len(),
                move |catalog| catalog.account(&account_id),
            )
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "account disappeared during onboarding".to_string())?;
        // Account-example provisioning is an authenticated first-party write.
        // `Store::put` deliberately refuses catalogue-backed writes because it
        // has no request actor, so carry the identity that sign-in already
        // authenticated through every catalogue admission and checkpoint.
        let actor = crate::document::store::MutationActor {
            account_id: account.id.clone(),
            owner_key: account.handle.clone(),
            session_generation: account.session_generation.clone(),
            link_hash: String::new(),
            policy_editor: self.publishers.allows(&account.handle),
            automation: false,
            unowned_publisher: false,
        };
        for (position, slug) in pending_account_examples(catalog, &who.id)
            .await
            .map_err(|e| e.to_string())?
        {
            let starter = STARTERS
                .get(position)
                .ok_or_else(|| format!("invalid starter position: {position}"))?;
            if let Some(entry) = self
                .store
                .get_result(&slug)
                .await
                .map_err(|e| e.to_string())?
            {
                if entry.publisher_id != who.id {
                    return Err("starter document belongs to another account".into());
                }
            } else {
                self.store
                    .put_as_actor(
                        Publication {
                            slug: slug.clone(),
                            title: starter.title.into(),
                            source: starter.source.into(),
                            source_format: starter.format.into(),
                            main: starter.main.into(),
                            owner: who.handle.clone(),
                            owner_id: who.id.clone(),
                            owner_name: who.name.clone(),
                            ..Publication::default()
                        },
                        actor.clone(),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
            }
            let room = self.rooms.try_get(&slug).await.map_err(|e| e.to_string())?;
            // A retry after a crash finishes this same copy. Never replace a
            // source already recovered from its durable session.
            if room.source().await.is_empty() {
                // A refused starter write stops the provisioning: the
                // checkpoint below would otherwise record an example
                // document that has no source in it.
                room.set_main_file(starter.source, starter.format, starter.main)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            if !room.tree().await.files.contains_key("librepaper-icon.png") {
                let icon = include_bytes!(
                    "../../../../docs/examples/tutorial-markdown/librepaper-icon.png"
                )
                .to_vec();
                let (sha, _) = room
                    .put_asset_authorized(
                        icon,
                        (self.config.max_asset, self.config.max_assets),
                        &actor,
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                room.name_asset("librepaper-icon.png", &sha)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            if !room.tree().await.files.contains_key(starter.extra) {
                room.add_text(starter.extra, starter.extra_source)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            // First sign-in provisioning is the new account's own write. The
            // final catalogue insertion rechecks the live account/session in
            // the same transaction, so a revoked sign-in cannot finish a
            // partially provisioned example.
            room.checkpoint_now_with_authority(
                "onboarding",
                crate::room::Attribution::account(&who.id, &who.handle),
                crate::storage::catalog::MutationAuthority {
                    account_id: &actor.account_id,
                    owner_key: &actor.owner_key,
                    generation: &actor.session_generation,
                    link_hash: &actor.link_hash,
                    policy_editor: actor.policy_editor,
                    automation: actor.automation,
                    unowned_publisher: actor.unowned_publisher,
                    execution_epoch: "",
                    agent_checkpoint: None,
                },
            )
                .await
                .map_err(|e| e.to_string())?;
            // The starter document and its checkpoint are durable before this
            // runs. Rendering is an on-demand browser/companion concern and
            // creates no deployment artifact during provisioning.
            let account_id = who.id.clone();
            catalog
                .execute_catalog(
                    crate::server::SERVER_JOB_BYTES + account_id.len(),
                    move |catalog| catalog.complete_account_example(&account_id, position),
                )
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

async fn pending_account_examples(
    catalog: &std::sync::Arc<crate::storage::catalog::Catalog>,
    account_id: &str,
) -> Result<Vec<(usize, String)>, crate::storage::catalog::CatalogError> {
    let account_id = account_id.to_string();
    catalog
        .execute_catalog(
            crate::server::SERVER_JOB_BYTES + account_id.len(),
            move |catalog| catalog.pending_account_examples(&account_id),
        )
        .await
        .map_err(crate::storage::catalog::CatalogError::from)
}
