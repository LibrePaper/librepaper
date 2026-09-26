//! Every new account starts with private, normally owned starter documents.
//! The catalogue records each completed copy independently of its lifetime.

use super::*;
use crate::document::store::DocumentInput;

struct Starter {
    main: &'static str,
    title: &'static str,
    format: &'static str,
    source: &'static str,
    files: &'static [(&'static str, &'static [u8])],
}

const STARTERS: [Starter; 5] = [
    Starter {
        main: "librepaper.md",
        title: "Learn LibrePaper with Markdown",
        format: "markdown",
        source: include_str!("../../../../docs/examples/tutorial-markdown/librepaper.md"),
        files: &[
            (
                "sections/rendering.md",
                include_bytes!("../../../../docs/examples/tutorial-markdown/sections/rendering.md"),
            ),
            (
                "librepaper-icon.png",
                include_bytes!("../../../../docs/examples/tutorial-markdown/librepaper-icon.png"),
            ),
            (
                "references.bib",
                include_bytes!("../../../../docs/examples/tutorial-markdown/references.bib"),
            ),
        ],
    },
    Starter {
        main: "librepaper.typ",
        title: "Learn LibrePaper with Typst",
        format: "typst",
        source: include_str!("../../../../docs/examples/tutorial-typst/librepaper.typ"),
        files: &[
            (
                "sections/rendering.typ",
                include_bytes!("../../../../docs/examples/tutorial-typst/sections/rendering.typ"),
            ),
            (
                "librepaper-icon.png",
                include_bytes!("../../../../docs/examples/tutorial-markdown/librepaper-icon.png"),
            ),
            (
                "references.bib",
                include_bytes!("../../../../docs/examples/tutorial-typst/references.bib"),
            ),
        ],
    },
    Starter {
        main: "librepaper.html",
        title: "Learn LibrePaper with HTML",
        format: "html",
        source: include_str!("../../../../docs/examples/tutorial-html/librepaper.html"),
        files: &[
            (
                "sections/rendering.html",
                include_bytes!("../../../../docs/examples/tutorial-html/sections/rendering.html"),
            ),
            (
                "librepaper-icon.png",
                include_bytes!("../../../../docs/examples/tutorial-markdown/librepaper-icon.png"),
            ),
            (
                "references.bib",
                include_bytes!("../../../../docs/examples/tutorial-html/references.bib"),
            ),
        ],
    },
    Starter {
        main: "librepaper.tex",
        title: "Learn LibrePaper with LaTeX",
        format: "latex",
        source: include_str!("../../../../docs/examples/tutorial-latex/librepaper.tex"),
        files: &[
            (
                "sections/rendering.tex",
                include_bytes!("../../../../docs/examples/tutorial-latex/sections/rendering.tex"),
            ),
            (
                "references.bib",
                include_bytes!("../../../../docs/examples/tutorial-latex/references.bib"),
            ),
            (
                "librepaper-icon.png",
                include_bytes!("../../../../docs/examples/tutorial-latex/librepaper-icon.png"),
            ),
        ],
    },
    Starter {
        main: "librepaper.qmd",
        title: "Learn LibrePaper with Quarto",
        format: "quarto",
        source: include_str!("../../../../docs/examples/tutorial-quarto/librepaper.qmd"),
        files: &[
            (
                "sections/rendering.qmd",
                include_bytes!("../../../../docs/examples/tutorial-quarto/sections/rendering.qmd"),
            ),
            (
                "librepaper-icon.png",
                include_bytes!("../../../../docs/examples/tutorial-markdown/librepaper-icon.png"),
            ),
            (
                "references.bib",
                include_bytes!("../../../../docs/examples/tutorial-quarto/references.bib"),
            ),
        ],
    },
];

impl Server {
    pub(crate) async fn initialize_account_examples(&self, who: &Identity) -> Result<(), String> {
        let catalog = &self.store.catalog;
        let _guard = self.onboarding.lock().await;
        // The caller can be an identity captured before the account row was
        // refreshed. Read the live generation once and use it for every write
        // in this provisioning pass; accepting an empty or stale generation
        // would turn a valid authenticated retry into a 401.
        let account_id = uuid::Uuid::parse_str(&who.id).map_err(|_| "invalid account id")?;
        let account = catalog
            .account(account_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "account disappeared during onboarding".to_string())?;
        if who.session_generation.is_empty()
            || who.session_generation != account.session_generation.to_string()
        {
            return Err("account session changed during onboarding".into());
        }
        // Account-example provisioning is an authenticated first-party write.
        // `Store::put` deliberately refuses catalogue-backed writes because it
        // has no request actor, so carry the identity that sign-in already
        // authenticated through every catalogue admission and checkpoint.
        let actor = crate::document::store::MutationActor {
            account_id: account.id.to_string(),
            owner_key: account.handle.clone(),
            session_generation: who.session_generation.clone(),
            link_hash: String::new(),
            // Provisioning is an authenticated first-party write for the
            // account itself.  It must not depend on the deployment's public
            // publisher policy: a newly signed-in reader still receives the
            // private starter set, while ordinary document creation remains
            // governed by that policy at its request boundary.
            policy_editor: true,
            unowned_publisher: false,
        };
        for (position, starter) in STARTERS.iter().enumerate() {
            let slug = format!("starter-{}-{}", position + 1, &account.id.to_string()[..8]);
            if let Some(entry) = self
                .store
                .get_result(&slug)
                .await
                .map_err(|e| e.to_string())?
            {
                if entry.publisher_id != who.id {
                    return Err("starter document belongs to another account".into());
                }
            } else if let Some(days) = self.simulate_activity {
                // A demonstration deployment asks for a history it can look
                // at: the starter is written as though it had been typed
                // over the requested days rather than published whole.
                // `seed::activity::simulate` insists its document holds no
                // log rows yet, so this creates the bare catalogue row
                // itself instead of going through `put_directory_as_actor`,
                // which would give it one straight away. A failure here is
                // cosmetic, so the account keeps its documents and the
                // operator hears about it.
                if let Err(error) = self
                    .simulate_starter(catalog, &slug, starter, &actor, account_id, days)
                    .await
                {
                    eprintln!("warning: {slug} activity not simulated: {error}");
                }
            } else {
                self.store
                    .put_directory_as_actor(
                        DocumentInput {
                            slug: slug.clone(),
                            title: starter.title.into(),
                            source: starter.source.into(),
                            source_format: starter.format.into(),
                            main: starter.main.into(),
                        },
                        starter
                            .files
                            .iter()
                            .map(|(path, bytes)| ((*path).into(), bytes.to_vec()))
                            .collect(),
                        actor.clone(),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    /// Creates one starter's catalogue row bare -- no content, no log rows --
    /// and hands it to `seed::activity::simulate`, which is the only thing
    /// allowed to write its first row: `simulate` insists on an empty log so
    /// that every label it takes names a state the document's history
    /// genuinely passed through, which a document already published by
    /// `put_directory_as_actor` could no longer promise.
    async fn simulate_starter(
        &self,
        catalog: &std::sync::Arc<crate::storage::postgres::PostgresCatalog>,
        slug: &str,
        starter: &Starter,
        actor: &crate::document::store::MutationActor,
        account_id: uuid::Uuid,
        days: u32,
    ) -> Result<(), String> {
        let document = catalog
            .create_document(crate::storage::postgres::NewDocument {
                slug: slug.to_string(),
                owner_id: account_id,
                ownership_mode: if actor.unowned_publisher {
                    "open".into()
                } else {
                    "owned".into()
                },
                title: starter.title.into(),
                source_format: starter.format.into(),
                main_path: starter.main.into(),
                settings: serde_json::json!({"version":1}),
            })
            .await
            .map_err(|e| e.to_string())?;
        let files: Vec<(String, Vec<u8>)> = starter
            .files
            .iter()
            .map(|(path, bytes)| ((*path).into(), bytes.to_vec()))
            .collect();
        let (texts, assets) = self
            .store
            .sort_and_write_assets(document.id, files)
            .await
            .map_err(|e| e.to_string())?;
        let texts: std::collections::BTreeMap<String, String> = texts.into_iter().collect();
        let author_label = match catalog.account_display_name(account_id).await {
            name if name.is_empty() => actor.owner_key.clone(),
            name => name,
        };
        crate::seed::activity::simulate(
            catalog.clone(),
            document.id,
            slug,
            starter.main,
            starter.source,
            &texts,
            &assets,
            account_id,
            &author_label,
            days,
        )
        .await
    }
}
