//! Every new account starts with four private, normally owned documents.
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
const STARTERS: [Starter; 4] = [
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
];

impl Server {
    pub(crate) async fn initialize_account_examples(&self, who: &Identity) -> Result<(), String> {
        let Some(catalog) = &self.store.catalog else {
            return Ok(());
        };
        if catalog
            .pending_account_examples(&who.id)
            .map_err(|e| e.to_string())?
            .is_empty()
        {
            return Ok(());
        }
        let _guard = self.onboarding.lock().await;
        for (position, slug) in catalog
            .pending_account_examples(&who.id)
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
                room.set_main_file(starter.source, starter.format, starter.file)
                    .await;
            }
            let sha = room
                .checkpoint("onboarding", &who.handle)
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
            catalog
                .complete_account_example(&who.id, position)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
