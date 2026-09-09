//! Every new account starts with private, normally owned starter documents.
//! The catalogue records each completed copy independently of its lifetime.

use super::*;
use crate::document::store::Publication;

struct Starter {
    file: &'static str,
    title: &'static str,
    format: &'static str,
    source: &'static str,
}

// Slot order is durable: changing a title or source must not reorder these.
const STARTERS: [Starter; crate::seed::ACCOUNT_EXAMPLE_COUNT] = [
    Starter {
        file: "regression-tables.md",
        title: "Markdown: What a Regression Table Is Hiding",
        format: "markdown",
        source: include_str!("../../../../examples/regression-tables.md"),
    },
    Starter {
        file: "intervals.typ",
        title: "Typst: What a Confidence Interval Does Not Say",
        format: "typst",
        source: include_str!("../../../../examples/intervals.typ"),
    },
    Starter {
        file: "bootstrap.html",
        title: "HTML: What the Bootstrap Actually Resamples",
        format: "html",
        source: include_str!("../../assets/bootstrap.html"),
    },
    Starter {
        file: "standard-errors.tex",
        title: "LaTeX: What a Standard Error Assumes",
        format: "latex",
        source: include_str!("../../../../examples/standard-errors.tex"),
    },
    Starter {
        file: "getting-started.qmd",
        title: "Quarto: Your First Reproducible Report",
        format: "quarto",
        source: include_str!("../../../../examples/getting-started.qmd"),
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
                    .put(Publication {
                        slug: slug.clone(),
                        title: starter.title.into(),
                        source: starter.source.into(),
                        source_format: starter.format.into(),
                        main: starter.file.into(),
                        owner: who.handle.clone(),
                        owner_id: who.id.clone(),
                        owner_name: who.name.clone(),
                        ..Publication::default()
                    })
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
                room.set_main_file(starter.source, starter.format, starter.file)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            let sha = room
                // First sign-in provisioning is the new account's own write.
                .checkpoint(
                    "onboarding",
                    crate::room::Attribution::account(&who.id, &who.handle),
                )
                .await
                .map_err(|e| e.to_string())?
                .unwrap_or(room.tree().await.digest());
            if starter.format == "typst" {
                let source = room.source().await;
                let pdf = tokio::task::spawn_blocking(move || {
                    let rendered = crate::document::render::render_typst_document(
                        std::path::Path::new("intervals.typ"),
                        &source,
                        "What a Confidence Interval Does Not Say",
                    );
                    crate::document::render::pdf_of(&rendered)
                        .ok_or_else(|| "could not render the starter Typst document".to_string())
                })
                .await
                .map_err(|e| e.to_string())??;
                room.put_rendering(&sha, false, pdf)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            // The starter document, its checkpoint and its rendering are all
            // durable before this runs, so a caller cancelled after dispatch
            // still marks the example complete and the retry above finds
            // nothing left to provision.
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
