//! Typed PostgreSQL background jobs.

use serde::Deserialize;
use std::sync::Arc;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::blob::BlobStore;
use super::collaboration::CollaborationStorage;
use super::maintenance::Maintenance;
use super::postgres::{JobClaim, PostgresCatalog};

pub struct Worker {
    catalog: Arc<PostgresCatalog>,
    blobs: Arc<dyn BlobStore>,
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentDeletionPayload {
    document_id: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountDeletionPayload {
    account_id: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicationCleanupPayload {
    publication_id: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceCompactionPayload {
    through_update_sequence: i64,
}

impl Worker {
    pub fn new(catalog: Arc<PostgresCatalog>, blobs: Arc<dyn BlobStore>) -> Self {
        Self {
            catalog,
            blobs,
            id: format!("worker-{}", uuid::Uuid::now_v7()),
        }
    }
    pub async fn run(self) {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            ticker.tick().await;
            let _ = self
                .catalog
                .recover_expired_jobs(OffsetDateTime::now_utc() - Duration::minutes(5), 100)
                .await;
            let jobs = match self.catalog.claim_jobs(&self.id, 8).await {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("warning: job claim failed: {e}");
                    continue;
                }
            };
            for claim in jobs {
                if let Err(error) = self.execute(&claim).await {
                    let retry_after = if claim.job.kind == "account_deletion" {
                        Duration::hours(1)
                    } else {
                        Duration::seconds(5)
                    };
                    let _ = self.catalog.fail_job(&claim, &error, retry_after).await;
                } else {
                    let _ = self
                        .catalog
                        .complete_job(&claim, serde_json::json!({"ok":true}))
                        .await;
                }
            }
        }
    }
    pub(crate) async fn execute(&self, claim: &JobClaim) -> Result<(), String> {
        match claim.job.kind.as_str() {
            "source_compaction" => self.compact(claim).await,
            "document_deletion" => self.delete_document(claim).await,
            "account_deletion" => self.delete_account(claim).await,
            "publication_cleanup" => self.cleanup_publication(claim).await,
            "maintenance" => self.maintenance(claim).await,
            other => Err(format!("unsupported job kind {other}")),
        }
    }
    async fn compact(&self, claim: &JobClaim) -> Result<(), String> {
        let payload: SourceCompactionPayload = serde_json::from_value(claim.job.payload.clone())
            .map_err(|error| format!("invalid source compaction payload: {error}"))?;
        if payload.through_update_sequence < 1 {
            return Err("source compaction sequence must be positive".into());
        }
        let id = claim
            .job
            .document_id
            .ok_or("compaction job has no document")?;
        let storage = CollaborationStorage::new(self.catalog.clone(), self.blobs.clone());
        let recovered = storage.recover(id).await.map_err(|e| e.to_string())?;
        if recovered.update_sequence < payload.through_update_sequence {
            return Err(
                "source compaction target is newer than durable collaboration state".into(),
            );
        }
        let doc = crate::document::session::new_doc();
        if let Some(base) = recovered.base {
            crate::document::session::apply_update(&doc, &base).map_err(|e| e.to_string())?;
        }
        for update in recovered.updates {
            crate::document::session::apply_update(&doc, &update.update_bytes)
                .map_err(|e| e.to_string())?;
        }
        let snapshot = crate::document::session::encode_state(&doc);
        storage
            .compact(
                id,
                recovered.update_sequence,
                recovered.project_generation,
                &snapshot,
            )
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn delete_document(&self, claim: &JobClaim) -> Result<(), String> {
        let payload: DocumentDeletionPayload = serde_json::from_value(claim.job.payload.clone())
            .map_err(|error| format!("invalid document deletion payload: {error}"))?;
        if claim.job.document_id != Some(payload.document_id) {
            return Err("document deletion payload does not match job scope".into());
        }
        let document = self
            .catalog
            .document(payload.document_id)
            .await
            .map_err(|error| error.to_string())?;
        let Some(document) = document else {
            return Ok(());
        };
        if document.status != "deleting" {
            return Err("document deletion job targets an active document".into());
        }
        let prefix = format!("documents/{}/", payload.document_id);
        loop {
            let page = self
                .blobs
                .list_page(&prefix, None, 500)
                .await
                .map_err(|error| error.to_string())?;
            if page.is_empty() {
                break;
            }
            let keys: Vec<_> = page.into_iter().map(|item| item.key).collect();
            self.blobs
                .delete(&keys)
                .await
                .map_err(|error| error.to_string())?;
        }
        self.catalog
            .finish_document_deletion(payload.document_id)
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    async fn delete_account(&self, claim: &JobClaim) -> Result<(), String> {
        let payload: AccountDeletionPayload = serde_json::from_value(claim.job.payload.clone())
            .map_err(|error| format!("invalid account deletion payload: {error}"))?;
        if claim.job.account_id != Some(payload.account_id) {
            return Err("account deletion payload does not match job scope".into());
        }
        self.catalog
            .finish_account_erasure(payload.account_id)
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Forgets a superseded publication. It deletes rows and no blobs.
    ///
    /// This is what version pruning already does with shared archives, and
    /// for the same reason. A published file is named by the digest of its
    /// contents, so two publications that hold the same stylesheet hold one
    /// object, and a figure is the document's own asset. Deciding what is
    /// unreferenced here would mean deciding it before the delete and acting
    /// on it after: a publish landing in between would commit a row naming an
    /// object this job was already about to remove, and its page would come
    /// back empty.
    ///
    /// The orphan sweeper is where that decision belongs. It re-reads every
    /// reference at the moment it deletes, and only touches objects that have
    /// also gone untouched for its grace period, so a publication written a
    /// second ago is never a candidate. Quota is freed here regardless: it
    /// counts rows, not bytes on disk.
    async fn cleanup_publication(&self, claim: &JobClaim) -> Result<(), String> {
        let payload: PublicationCleanupPayload = serde_json::from_value(claim.job.payload.clone())
            .map_err(|error| format!("invalid publication cleanup payload: {error}"))?;
        self.catalog
            .finish_publication_cleanup(payload.publication_id)
            .await
            .map_err(|error| error.to_string())?;
        // And forget the pages retired long enough ago that nobody can still
        // be reading one. Done here because publishing is the only thing that
        // makes another one, so it is the only moment the answer changes.
        if let Some(document_id) = claim.job.document_id {
            self.catalog
                .forget_retired_publications(document_id)
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    async fn maintenance(&self, claim: &JobClaim) -> Result<(), String> {
        let _ = &claim.job.payload;
        let maintenance = Maintenance::new(self.catalog.clone(), self.blobs.clone());
        let base_cleanup_batch = 100;
        let orphan_scan_batch = 500;
        let orphan_grace_hours = 24 * 7;
        let completed_job_retention_days = 7;
        let completed_job_batch = 1000;
        // Before the sweep, not after: repointing duplicates is what makes the
        // copies they released collectable, and the sweep's grace period starts
        // from when an object stops being referenced.
        let duplicate_archive_batch = 1000;
        let mut failures = Vec::new();
        if let Err(error) = maintenance
            .delete_superseded_bases(base_cleanup_batch)
            .await
        {
            failures.push(error);
        }
        if let Err(error) = maintenance
            .share_duplicate_archives(duplicate_archive_batch)
            .await
        {
            failures.push(error);
        }
        if let Err(error) = maintenance
            .delete_orphans(orphan_scan_batch, Duration::hours(orphan_grace_hours))
            .await
        {
            failures.push(error);
        }
        if let Err(error) = self
            .catalog
            .prune_jobs(
                OffsetDateTime::now_utc() - Duration::days(completed_job_retention_days),
                completed_job_batch,
            )
            .await
        {
            failures.push(error.to_string());
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }
}
