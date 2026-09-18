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

const STARTERS: [Starter; crate::seed::ACCOUNT_EXAMPLE_COUNT] = [
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
            } else {
                let entry = self
                    .store
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
                // A demonstration deployment asks for a history it can look
                // at: the starter is rewritten as though it had been typed
                // over the requested days. The operations are real and only
                // the clock is invented; see `crate::seed::activity`. A
                // failure here is cosmetic, so the account keeps its
                // documents and the operator hears about it.
                if let Some(days) = self.simulate_activity {
                    if let Ok(id) = uuid::Uuid::parse_str(&entry.storage_id) {
                        if let Err(error) = crate::seed::activity::simulate(
                            catalog.clone(),
                            self.store.blobs.clone(),
                            id,
                            &slug,
                            days,
                        )
                        .await
                        {
                            eprintln!("warning: {slug} activity not simulated: {error}");
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
