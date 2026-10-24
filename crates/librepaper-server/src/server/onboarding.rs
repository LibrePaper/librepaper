//! Every new account starts with one private, normally owned starter project:
//! the tutorial in every format LibrePaper edits, opening on the Markdown
//! copy. The catalogue records the completed copy independently of
//! its lifetime.

use super::*;
use librepaper_engine::storage::store::DocumentInput;

struct Starter {
    /// Where the starter's own source file lives in the project.
    path: &'static str,
    title: &'static str,
    format: &'static str,
    source: &'static str,
    files: &'static [(&'static str, &'static [u8])],
}

const STARTER: Starter = Starter {
    path: "librepaper.md",
    title: "Explore LibrePaper",
    format: "markdown",
    source: include_str!(concat!(
        env!("LIBREPAPER_DOCS"),
        "/examples/tutorial-markdown/librepaper.md"
    )),
    files: &[
        // Markdown
        (
            "sections/rendering.md",
            include_bytes!(concat!(
                env!("LIBREPAPER_DOCS"),
                "/examples/tutorial-markdown/sections/rendering.md"
            )),
        ),
        // Typst
        (
            "librepaper.typ",
            include_bytes!(concat!(
                env!("LIBREPAPER_DOCS"),
                "/examples/tutorial-typst/librepaper.typ"
            )),
        ),
        (
            "sections/rendering.typ",
            include_bytes!(concat!(
                env!("LIBREPAPER_DOCS"),
                "/examples/tutorial-typst/sections/rendering.typ"
            )),
        ),
        // LaTeX
        (
            "librepaper.tex",
            include_bytes!(concat!(
                env!("LIBREPAPER_DOCS"),
                "/examples/tutorial-latex/librepaper.tex"
            )),
        ),
        (
            "sections/rendering.tex",
            include_bytes!(concat!(
                env!("LIBREPAPER_DOCS"),
                "/examples/tutorial-latex/sections/rendering.tex"
            )),
        ),
        // HTML
        (
            "librepaper.html",
            include_bytes!(concat!(
                env!("LIBREPAPER_DOCS"),
                "/examples/tutorial-html/librepaper.html"
            )),
        ),
        (
            "sections/rendering.html",
            include_bytes!(concat!(
                env!("LIBREPAPER_DOCS"),
                "/examples/tutorial-html/sections/rendering.html"
            )),
        ),
        // Quarto
        (
            "librepaper.qmd",
            include_bytes!(concat!(
                env!("LIBREPAPER_DOCS"),
                "/examples/tutorial-quarto/librepaper.qmd"
            )),
        ),
        (
            "sections/rendering.qmd",
            include_bytes!(concat!(
                env!("LIBREPAPER_DOCS"),
                "/examples/tutorial-quarto/sections/rendering.qmd"
            )),
        ),
        // Shared by every format. The copies in the other tutorial directories are identical.
        (
            "librepaper-icon.png",
            include_bytes!(concat!(
                env!("LIBREPAPER_DOCS"),
                "/examples/tutorial-markdown/librepaper-icon.png"
            )),
        ),
        (
            "references.bib",
            include_bytes!(concat!(
                env!("LIBREPAPER_DOCS"),
                "/examples/tutorial-markdown/references.bib"
            )),
        ),
    ],
};

impl Server {
    pub(crate) async fn initialize_account_examples(&self, who: &Identity) -> Result<(), String> {
        let catalog = &self.store.catalog;
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
        if !catalog
            .account_examples_pending(account_id)
            .await
            .map_err(|e| e.to_string())?
        {
            return Ok(());
        }
        // Account-example provisioning is an authenticated first-party write.
        // `Store::put` deliberately refuses catalogue-backed writes because it
        // has no request actor, so carry the identity that sign-in already
        // authenticated through every catalogue admission and checkpoint.
        let actor = librepaper_engine::storage::postgres::Authority {
            principal_key: account.handle.clone(),
            account_id: Some(account.id),
            session_generation: Some(account.session_generation),
            link_hash: None,
            // Provisioning is an authenticated first-party write for the
            // account itself.  It must not depend on the deployment's public
            // publisher policy: a newly signed-in reader still receives the
            // private starter project, while ordinary document creation remains
            // governed by that policy at its request boundary.
            policy_edit: true,
            policy_comment: true,
            automation: false,
        };
        let starter = &STARTER;
        let slug = format!("starter-{}", account.id.simple());
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
            // cosmetic, so the account keeps its project and the
            // operator hears about it.
            if let Err(error) = self
                .simulate_starter(catalog, &slug, starter, &actor, account_id, days)
                .await
            {
                tracing::warn!("{slug} activity not simulated: {error}");
            }
        } else {
            self.store
                .put_directory_as_actor(
                    DocumentInput {
                        slug: slug.clone(),
                        title: starter.title.into(),
                        source: starter.source.into(),
                        source_format: starter.format.into(),
                    },
                    std::iter::once((starter.path.into(), starter.source.as_bytes().to_vec()))
                        .chain(
                            starter
                                .files
                                .iter()
                                .map(|(path, bytes)| ((*path).into(), bytes.to_vec())),
                        )
                        .collect(),
                    actor.clone(),
                )
                .await
                .map_err(|e| e.to_string())?;
        }
        catalog
            .mark_account_examples(account_id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Creates the starter's catalogue row bare -- no content, no log rows --
    /// and hands it to `seed::activity::simulate`, which is the only thing
    /// allowed to write its first row: `simulate` insists on an empty log so
    /// that every label it takes names a state the document's history
    /// genuinely passed through, which a document already published by
    /// `put_directory_as_actor` could no longer promise.
    async fn simulate_starter(
        &self,
        catalog: &std::sync::Arc<librepaper_engine::storage::postgres::PostgresCatalog>,
        slug: &str,
        starter: &Starter,
        actor: &librepaper_engine::storage::postgres::Authority,
        account_id: uuid::Uuid,
        days: u32,
    ) -> Result<(), String> {
        let owner_session_generation = actor
            .session_generation
            .ok_or_else(|| "invalid onboarding session".to_string())?;
        let document = catalog
            .create_document(librepaper_engine::storage::postgres::NewDocument {
                slug: slug.to_string(),
                owner_id: account_id,
                owner_session_generation: Some(owner_session_generation),
                ownership_mode: "owned".into(),
                title: starter.title.into(),
                source_format: starter.format.into(),
            })
            .await
            .map_err(|e| e.to_string())?;
        let files: Vec<(String, Vec<u8>)> = starter
            .files
            .iter()
            .map(|(path, bytes)| ((*path).into(), bytes.to_vec()))
            .collect();
        let (texts, assets, staged_assets) = self
            .store
            .sort_and_stage_assets(document.id, files)
            .await
            .map_err(|e| e.to_string())?;
        let texts: std::collections::BTreeMap<String, String> = texts.into_iter().collect();
        let author_label = match catalog.account_display_name(account_id).await {
            name if name.is_empty() => actor.principal_key.clone(),
            name => name,
        };
        librepaper_engine::storage::seed::activity::simulate(
            catalog.clone(),
            document.id,
            slug,
            starter.path,
            starter.source,
            &texts,
            &assets,
            staged_assets,
            account_id,
            owner_session_generation,
            &author_label,
            days,
        )
        .await
    }
}
