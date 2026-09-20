//! Typed PostgreSQL background jobs.

use serde::Deserialize;
use std::sync::Arc;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::blob::BlobStore;
use super::collaboration::CollaborationStorage;
use super::maintenance::Maintenance;
use super::postgres::{JobClaim, PostgresCatalog};
use super::source::{CommitArchive, SourceStorage};
use super::source_archive::{SourceArchive, SourceFile};

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
struct BundleCleanupPayload {
    bundle_id: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceCompactionPayload {
    through_update_sequence: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckpointArchivePayload {
    checkpoint_id: Uuid,
    source_revision: i64,
    tree_digest: String,
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
            "checkpoint_archive" => self.checkpoint_archive(claim).await,
            "document_deletion" => self.delete_document(claim).await,
            "account_deletion" => self.delete_account(claim).await,
            "bundle_cleanup" => self.cleanup_bundle(claim).await,
            "maintenance" => self.maintenance(claim).await,
            other => Err(format!("unsupported job kind {other}")),
        }
    }

    async fn checkpoint_archive(&self, claim: &JobClaim) -> Result<(), String> {
        let payload: CheckpointArchivePayload =
            serde_json::from_value(claim.job.payload.clone())
                .map_err(|error| format!("invalid checkpoint archive payload: {error}"))?;
        let result = self.materialize_checkpoint_archive(claim, &payload).await;
        if let Err(error) = &result {
            let terminal = claim.job.attempts >= claim.job.max_attempts;
            let _ = self
                .catalog
                .note_checkpoint_archive_error(payload.checkpoint_id, error, terminal)
                .await;
        }
        result
    }

    async fn materialize_checkpoint_archive(
        &self,
        claim: &JobClaim,
        payload: &CheckpointArchivePayload,
    ) -> Result<(), String> {
        let document_id = claim
            .job
            .document_id
            .ok_or("checkpoint archive job has no document")?;
        let checkpoint = self
            .catalog
            .checkpoint(document_id, payload.checkpoint_id)
            .await
            .map_err(|error| format!("read checkpoint: {error:?}"))?
            .ok_or("checkpoint is missing")?;
        if checkpoint.archive_status == "ready" {
            return Ok(());
        }
        if checkpoint.source_revision != Some(payload.source_revision) {
            return Err("checkpoint archive revision does not match its job".into());
        }
        let expected_digest: [u8; 32] = hex::decode(&payload.tree_digest)
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or("checkpoint archive tree digest is invalid")?;
        if checkpoint.tree_digest.as_deref() != Some(expected_digest.as_slice()) {
            return Err("checkpoint archive tree digest does not match its job".into());
        }
        let revision = self
            .catalog
            .source_revision_record(document_id, payload.source_revision)
            .await
            .map_err(|error| format!("read checkpoint revision: {error:?}"))?
            .ok_or("checkpoint source revision is missing")?;
        let recovered = CollaborationStorage::new(self.catalog.clone(), self.blobs.clone())
            .recover(document_id)
            .await
            .map_err(|error| format!("recover checkpoint history: {error:?}"))?;
        if recovered.update_sequence < revision.update_sequence {
            return Err("checkpoint source revision is newer than recoverable history".into());
        }
        let live = crate::document::session::new_doc();
        if let Some(base) = recovered.base {
            crate::document::session::apply_update(&live, &base).map_err(|e| e.to_string())?;
        }
        for update in recovered.updates {
            crate::document::session::apply_update(&live, &update.update_bytes)
                .map_err(|e| e.to_string())?;
        }
        let frontier = loro::Frontiers::decode(&revision.frontier)
            .map_err(|error| format!("checkpoint frontier is invalid: {error}"))?;
        let past = live
            .fork_at(&frontier)
            .map_err(|error| format!("checkpoint frontier is unreachable: {error}"))?;
        let asset_digests: Vec<_> = crate::document::session::assets_of(&past)
            .into_values()
            .collect();
        let assets = self
            .catalog
            .assets_by_digests(document_id, &asset_digests)
            .await
            .map_err(|error| format!("read checkpoint assets: {error:?}"))?;
        let sizes = assets
            .iter()
            .map(|asset| (hex::encode(&asset.digest), asset.byte_length))
            .collect();
        let (tree, bodies) = crate::room::tree_of(&past, &sizes);
        if tree.digest_bytes() != expected_digest {
            return Err("checkpoint archive reconstructed a different tree".into());
        }
        let by_digest: std::collections::HashMap<_, _> = assets
            .into_iter()
            .map(|asset| (hex::encode(&asset.digest), asset))
            .collect();
        let mut files = Vec::with_capacity(tree.files.len());
        for (path, entry) in &tree.files {
            if entry.kind == "text" {
                let body = bodies
                    .get(&entry.sha)
                    .ok_or("checkpoint archive text is missing")?;
                files.push(SourceFile::Inline {
                    path: path.clone(),
                    bytes: body.as_bytes().to_vec(),
                });
            } else {
                let asset = by_digest
                    .get(&entry.sha)
                    .ok_or("checkpoint archive asset is missing")?;
                let digest = asset
                    .digest
                    .as_slice()
                    .try_into()
                    .map_err(|_| "checkpoint archive asset digest is invalid")?;
                files.push(SourceFile::Asset {
                    path: path.clone(),
                    asset_id: asset.id,
                    digest,
                    bytes: asset.byte_length as u64,
                    media_type: asset.media_type.clone(),
                });
            }
        }
        let document = self
            .catalog
            .document(document_id)
            .await
            .map_err(|error| format!("read checkpoint document: {error:?}"))?
            .ok_or("checkpoint document is missing")?;
        let stored =
            SourceStorage::new(self.catalog.clone(), self.blobs.clone(), Default::default())
                .commit_archive(CommitArchive {
                    document_id,
                    archive: SourceArchive {
                        source_format: document.source_format,
                        main_path: tree.main.clone(),
                        files,
                    },
                    through_update_sequence: revision.update_sequence,
                    project_generation: recovered.project_generation,
                    tree_digest: Some(expected_digest),
                    changed_paths: checkpoint.changed_paths.clone(),
                    reason: "checkpoint-archive".into(),
                    label: None,
                    author_account_id: checkpoint.author_account_id,
                    author_label: checkpoint.author_label.clone(),
                    make_current: false,
                })
                .await
                .map_err(|error| format!("commit checkpoint archive: {error:?}"))?;
        if !self
            .catalog
            .attach_checkpoint_archive(payload.checkpoint_id, stored.version.id, &expected_digest)
            .await
            .map_err(|error| format!("attach checkpoint archive: {error:?}"))?
        {
            return Err("checkpoint archive could not be attached".into());
        }
        Ok(())
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
        if self
            .catalog
            .compacted_after(id, payload.through_update_sequence - 1)
            .await
            .map_err(|e| e.to_string())?
        {
            return Ok(());
        }
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

    /// Forgets a superseded bundle. It deletes rows and no blobs.
    ///
    /// This is what version pruning already does with shared archives, and
    /// for the same reason. A published file is named by the digest of its
    /// contents, so two bundles that hold the same stylesheet hold one
    /// object, and a figure is the document's own asset. Deciding what is
    /// unreferenced here would mean deciding it before the delete and acting
    /// on it after: a publish landing in between would commit a row naming an
    /// object this job was already about to remove, and its page would come
    /// back empty.
    ///
    /// The orphan sweeper is where that decision belongs. It re-reads every
    /// reference at the moment it deletes, and only touches objects that have
    /// also gone untouched for its grace period, so a bundle written a
    /// second ago is never a candidate. Quota is freed here regardless: it
    /// counts rows, not bytes on disk.
    async fn cleanup_bundle(&self, claim: &JobClaim) -> Result<(), String> {
        let payload: BundleCleanupPayload = serde_json::from_value(claim.job.payload.clone())
            .map_err(|error| format!("invalid bundle cleanup payload: {error}"))?;
        self.catalog
            .finish_bundle_cleanup(payload.bundle_id)
            .await
            .map_err(|error| error.to_string())?;
        // And forget the pages retired long enough ago that nobody can still
        // be reading one. Done here because publishing is the only thing that
        // makes another one, so it is the only moment the answer changes.
        if let Some(document_id) = claim.job.document_id {
            self.catalog
                .forget_retired_bundles(document_id)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::blob::FsStore;
    use crate::storage::postgres::{
        Authority, NewCheckpoint, PostgresOptions, PreparedSource, SemanticReceipt,
    };

    #[tokio::test]
    #[ignore = "requires a disposable LIBREPAPER_TEST_POSTGRES_URL"]
    async fn checkpoint_metadata_commits_before_its_archive_and_replays_once() {
        let Ok(url) = std::env::var("LIBREPAPER_TEST_POSTGRES_URL") else {
            return;
        };
        let catalog = Arc::new(
            PostgresCatalog::connect(PostgresOptions::new(url))
                .await
                .unwrap(),
        );
        catalog.migrate().await.unwrap();
        sqlx::query("TRUNCATE maintenance_cursors,jobs,document_updates,document_bases,document_source_revisions,document_command_receipts,document_checkpoints,document_versions,document_assets,documents,accounts CASCADE")
            .execute(catalog.pool()).await.unwrap();
        let owner = Uuid::now_v7();
        let document = Uuid::now_v7();
        sqlx::query("INSERT INTO accounts(id,kind,provider,provider_subject,handle,display_name,status) VALUES($1,'registered','test',$2,'owner','Owner','active')")
            .bind(owner).bind(owner.to_string()).execute(catalog.pool()).await.unwrap();
        sqlx::query("INSERT INTO documents(id,slug,owner_id,ownership_mode,title,status,source_format,main_path) VALUES($1,$2,$3,'owned','Test','active','markdown','main.md')")
            .bind(document).bind(format!("checkpoint-{document}")).bind(owner).execute(catalog.pool()).await.unwrap();
        let _lease = catalog.claim_writer().await.unwrap();
        let authority = Authority {
            principal_key: owner.to_string(),
            account_id: Some(owner),
            link_hash: None,
        };
        let doc = crate::document::session::new_doc();
        let body = "# Durable\n";
        let digest = crate::document::store::digest_of(body);
        let tree = crate::document::history::Tree {
            main: "main.md".into(),
            files: std::iter::once((
                "main.md".into(),
                crate::document::history::TreeEntry {
                    kind: "text".into(),
                    id: String::new(),
                    sha: digest.clone(),
                    size: body.len() as i64,
                },
            ))
            .collect(),
            settings: None,
        };
        crate::document::session::restore(
            &doc,
            &tree,
            &std::iter::once((digest, body.into())).collect(),
        );
        let update = crate::document::session::encode_state(&doc);
        let frontier = doc.state_frontiers().encode();
        let committed = catalog
            .commit_source(
                document,
                &authority,
                PreparedSource {
                    expected_update_sequence: 0,
                    update: &update,
                    frontier: &frontier,
                    encoded_history_bytes: update.len() as i64,
                    actor_key: "test",
                    source_format: None,
                    main_path: None,
                    resolve_annotation_id: None,
                    supersede_proposals: false,
                },
                None,
            )
            .await
            .unwrap();
        let (tree, _) = crate::room::tree_of(&doc, &Default::default());
        let checkpoint_id = Uuid::now_v7();
        let request_id = Uuid::now_v7();
        let receipt = SemanticReceipt {
            request_id,
            canonical_command: serde_json::json!({"checkpoint": checkpoint_id}),
            stable_result: serde_json::json!({"checkpoint_id": checkpoint_id}),
            status: "committed".into(),
        };
        let input = NewCheckpoint {
            id: checkpoint_id,
            document_id: document,
            source_revision: committed.source_revision.parse().unwrap(),
            tree_digest: tree.digest_bytes(),
            changed_paths: Some(vec!["main.md".into()]),
            file_count: 1,
            reason: "test".into(),
            label: None,
            author_account_id: Some(owner),
            author_label: "Owner".into(),
        };
        let created = catalog
            .create_checkpoint(input.clone(), &authority, Some(&receipt))
            .await
            .unwrap();
        assert_eq!(created.archive_status, "pending");
        assert!(created.archive_version_id.is_none());
        let replay = catalog
            .create_checkpoint(input, &authority, Some(&receipt))
            .await
            .unwrap();
        assert_eq!(replay.id, checkpoint_id);
        let checkpoint_jobs: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM jobs WHERE kind='checkpoint_archive' AND document_id=$1",
        )
        .bind(document)
        .fetch_one(catalog.pool())
        .await
        .unwrap();
        assert_eq!(checkpoint_jobs, 1);

        let objects = tempfile::tempdir().unwrap();
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(objects.path(), false));
        let worker = Worker::new(catalog.clone(), blobs.clone());
        let claims = catalog.claim_jobs("checkpoint-test", 8).await.unwrap();
        let claim = claims
            .iter()
            .find(|claim| claim.job.kind == "checkpoint_archive")
            .unwrap();
        worker.execute(claim).await.unwrap();
        catalog
            .complete_job(claim, serde_json::json!({"ok": true}))
            .await
            .unwrap();
        let ready = catalog
            .checkpoint(document, checkpoint_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ready.archive_status, "ready");
        let version = catalog
            .version(document, ready.archive_version_id.unwrap())
            .await
            .unwrap()
            .unwrap();
        let stored = SourceStorage::new(catalog.clone(), blobs, Default::default())
            .read_version(version)
            .await
            .unwrap();
        assert_eq!(
            crate::storage::source_archive::tree_of(&stored.archive)
                .0
                .digest(),
            tree.digest()
        );
    }
}
