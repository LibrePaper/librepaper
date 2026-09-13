//! PostgreSQL persistence for catalog v3.
//!
//! This module owns connection pooling and migrations. Product operations are
//! split into focused repository modules by domain.

use std::time::Duration;

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{ConnectOptions, PgPool};

mod access;
mod annotations;
mod collaboration;
mod jobs;
mod publications;
mod repository;

pub use collaboration::{CollaborationBase, CollaborationState, PersistedUpdate};
pub use jobs::{Job, JobClaim, JobStatus, NewJob};
pub use publications::{
    NewPublication, NewPublicationFile, PublicationFileRecord, PublicationRecord,
};
pub use repository::{
    AccountRecord, AssetRecord, DocumentRecord, NewAccount, NewAsset, NewDocument, NewVersion,
    VersionRecord,
};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations/postgres");

#[derive(Clone, Debug)]
pub struct PostgresOptions {
    pub url: String,
    pub max_connections: u32,
    pub acquire_timeout: Duration,
    pub log_statements: bool,
    pub policy: StoragePolicy,
}

#[derive(Clone, Copy, Debug)]
pub struct StoragePolicy {
    pub owner_bytes: i64,
    pub deployment_bytes: i64,
    pub asset_uploads_per_hour: i64,
    pub versions_per_hour: i64,
    pub max_uncompacted_updates: i64,
    pub max_uncompacted_bytes: i64,
}

impl Default for StoragePolicy {
    fn default() -> Self {
        Self {
            owner_bytes: 100 * 1024 * 1024,
            deployment_bytes: 5 * 1024 * 1024 * 1024,
            asset_uploads_per_hour: 30,
            versions_per_hour: 30,
            max_uncompacted_updates: 1_000,
            max_uncompacted_bytes: 512 * 1024 * 1024,
        }
    }
}

impl PostgresOptions {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            max_connections: 20,
            acquire_timeout: Duration::from_secs(5),
            log_statements: false,
            policy: StoragePolicy::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PostgresCatalog {
    pool: PgPool,
    policy: StoragePolicy,
}

impl PostgresCatalog {
    pub async fn connect(options: PostgresOptions) -> std::result::Result<Self, sqlx::Error> {
        let mut connect: PgConnectOptions = options.url.parse()?;
        connect = connect.log_statements(if options.log_statements {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Off
        });
        let pool = PgPoolOptions::new()
            .max_connections(options.max_connections)
            .acquire_timeout(options.acquire_timeout)
            .connect_with(connect)
            .await?;
        Ok(Self {
            pool,
            policy: options.policy,
        })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn migrate(&self) -> std::result::Result<(), sqlx::migrate::MigrateError> {
        // Migrations are serialized across application processes. The lock is
        // session-scoped, so keeping this one pooled connection checked out
        // also keeps ownership unambiguous until migration completes.
        let mut connection = self.pool.acquire().await?;
        const MIGRATION_LOCK: i64 = 0x4c_50_43_41_54_56_33; // "LPCATV3"
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(MIGRATION_LOCK)
            .execute(&mut *connection)
            .await?;
        let result = MIGRATOR.run(&mut *connection).await;
        let unlock = sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(MIGRATION_LOCK)
            .execute(&mut *connection)
            .await;
        match (result, unlock) {
            (Err(error), _) => Err(error),
            (Ok(()), Err(error)) => Err(error.into()),
            (Ok(()), Ok(_)) => Ok(()),
        }
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }
}

pub fn new_id() -> uuid::Uuid {
    uuid::Uuid::now_v7()
}

#[derive(Debug)]
pub enum Error {
    Database(sqlx::Error),
    Invalid(String),
    Conflict(String),
    NotFound,
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "PostgreSQL: {error}"),
            Self::Invalid(message) | Self::Conflict(message) => formatter.write_str(message),
            Self::NotFound => formatter.write_str("not found"),
        }
    }
}

impl std::error::Error for Error {}

impl From<sqlx::Error> for Error {
    fn from(value: sqlx::Error) -> Self {
        Self::Database(value)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    use serde_json::json;
    use time::{Duration, OffsetDateTime};

    use super::*;
    use crate::storage::blob::{BlobError, BlobInfo, BlobResult, BlobStore, FsStore};
    use crate::storage::collaboration::CollaborationStorage;
    use crate::storage::maintenance::Maintenance;
    use crate::storage::publication::{PublicationFile, PublicationStorage, Publish};
    use crate::storage::source::{CommitProject, ProjectFile, SourceStorage};
    use crate::storage::source_archive::ArchiveLimits;
    use crate::storage::worker::Worker;

    struct PartialDeleteStore {
        inner: FsStore,
        fail_once: AtomicBool,
    }

    #[async_trait::async_trait]
    impl BlobStore for PartialDeleteStore {
        async fn get(&self, key: &str) -> BlobResult<Vec<u8>> {
            self.inner.get(key).await
        }
        async fn put_new(&self, key: &str, body: Vec<u8>, content_type: &str) -> BlobResult<()> {
            self.inner.put_new(key, body, content_type).await
        }
        async fn delete(&self, keys: &[String]) -> BlobResult<()> {
            if self.fail_once.swap(false, Ordering::SeqCst) && !keys.is_empty() {
                self.inner.delete(&keys[..1]).await?;
                return Err(BlobError::Other("injected partial deletion".into()));
            }
            self.inner.delete(keys).await
        }
        async fn list(&self, prefix: &str) -> BlobResult<Vec<BlobInfo>> {
            self.inner.list(prefix).await
        }
        fn describe(&self) -> String {
            self.inner.describe()
        }
        fn is_local(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn postgres_v3_contract() {
        let Ok(url) = std::env::var("LIBREPAPER_TEST_POSTGRES_URL") else {
            return;
        };
        let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .unwrap();
        catalog.migrate().await.unwrap();
        sqlx::query(
            "TRUNCATE maintenance_cursors,jobs,document_updates,document_bases,publication_files,publications,
             document_versions,document_assets,replies,annotations,share_links,grants,documents,
             accounts CASCADE",
        )
        .execute(catalog.pool())
        .await
        .unwrap();

        let account = catalog
            .create_account(NewAccount {
                kind: "registered".into(),
                provider: Some("test".into()),
                provider_subject: Some("one".into()),
                handle: "owner".into(),
                display_name: "Owner".into(),
                email: None,
            })
            .await
            .unwrap();
        let first = catalog
            .create_document(NewDocument {
                slug: "first".into(),
                owner_id: account.id,
                ownership_mode: "owned".into(),
                title: "Same title".into(),
                source_format: "quarto".into(),
                main_path: "paper.qmd".into(),
                settings: json!({"version":1}),
            })
            .await
            .unwrap();
        let second = catalog
            .create_document(NewDocument {
                slug: "second".into(),
                owner_id: account.id,
                ownership_mode: "owned".into(),
                title: "Same title".into(),
                source_format: "quarto".into(),
                main_path: "paper.qmd".into(),
                settings: json!({"version":1}),
            })
            .await
            .unwrap();

        let collaborator = catalog
            .create_account(NewAccount {
                kind: "registered".into(),
                provider: Some("test".into()),
                provider_subject: Some("two".into()),
                handle: "collaborator".into(),
                display_name: "Collaborator".into(),
                email: None,
            })
            .await
            .unwrap();
        assert_eq!(
            catalog
                .set_grant(first.id, collaborator.id, AccessRole::Editor)
                .await
                .unwrap()
                .role,
            "editor"
        );
        assert_eq!(
            catalog
                .access_role(
                    first.id,
                    Some(collaborator.id),
                    None,
                    OffsetDateTime::now_utc()
                )
                .await
                .unwrap(),
            Some(AccessRole::Editor)
        );
        assert!(catalog
            .remove_grant(first.id, collaborator.id)
            .await
            .unwrap());
        assert_eq!(
            catalog
                .access_role(
                    first.id,
                    Some(collaborator.id),
                    None,
                    OffsetDateTime::now_utc()
                )
                .await
                .unwrap(),
            None
        );
        let link_hash = [8; 32];
        let link = catalog
            .create_share_link(
                first.id,
                AccessRole::Commenter,
                link_hash,
                "review".into(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            catalog
                .access_role(first.id, None, Some(link_hash), OffsetDateTime::now_utc())
                .await
                .unwrap(),
            Some(AccessRole::Commenter)
        );
        assert!(catalog.revoke_share_link(first.id, link.id).await.unwrap());
        assert_eq!(
            catalog
                .access_role(first.id, None, Some(link_hash), OffsetDateTime::now_utc())
                .await
                .unwrap(),
            None
        );
        assert!(catalog
            .update_document_identity(second.id, "Transferred", collaborator.id, "owned")
            .await
            .unwrap());
        assert_eq!(
            catalog
                .access_role(
                    second.id,
                    Some(collaborator.id),
                    None,
                    OffsetDateTime::now_utc()
                )
                .await
                .unwrap(),
            Some(AccessRole::Owner)
        );
        assert!(catalog
            .update_document_identity(second.id, "Same title", account.id, "owned")
            .await
            .unwrap());

        let annotation_input = || NewAnnotation {
            document_id: first.id,
            kind: "comment".into(),
            body: "Authorized review".into(),
            author_account_id: Some(collaborator.id),
            author_key: format!("account:{}", collaborator.id),
            author_label: "Collaborator".into(),
            selector: json!({}),
            context: json!({"version":1}),
            source_version_id: None,
            source_update_sequence: None,
            source_project_generation: None,
            source_state_vector: None,
            publication_id: None,
            proposed_text: None,
        };
        let collaborator_actor = MutationAuthorization {
            account_id: Some(collaborator.id),
            session_generation: Some(collaborator.session_generation),
            token_hash: None,
            policy_editor: false,
        };
        assert!(catalog
            .put_annotation_authorized(new_id(), annotation_input(), &collaborator_actor, false,)
            .await
            .is_err());
        catalog
            .set_grant(first.id, collaborator.id, AccessRole::Commenter)
            .await
            .unwrap();
        catalog
            .put_annotation_authorized(new_id(), annotation_input(), &collaborator_actor, false)
            .await
            .unwrap();
        assert!(catalog
            .put_annotation_authorized(new_id(), annotation_input(), &collaborator_actor, true,)
            .await
            .is_err());
        catalog
            .remove_grant(first.id, collaborator.id)
            .await
            .unwrap();

        let owner_actor = MutationAuthorization {
            account_id: Some(account.id),
            session_generation: Some(account.session_generation),
            token_hash: None,
            policy_editor: false,
        };
        let annotation = catalog
            .put_annotation_authorized(
                new_id(),
                NewAnnotation {
                    document_id: first.id,
                    kind: "comment".into(),
                    body: "Review this".into(),
                    author_account_id: Some(collaborator.id),
                    author_key: format!("account:{}", collaborator.id),
                    author_label: "Collaborator".into(),
                    selector: json!({"exact":"Paper"}),
                    context: json!({"version":1}),
                    source_version_id: None,
                    source_update_sequence: Some(0),
                    source_project_generation: Some(0),
                    source_state_vector: None,
                    publication_id: None,
                    proposed_text: None,
                },
                &owner_actor,
                false,
            )
            .await
            .unwrap();
        let reply = catalog
            .create_reply_authorized(
                NewReply {
                    id: new_id(),
                    annotation_id: annotation.id,
                    author_account_id: Some(account.id),
                    author_key: format!("account:{}", account.id),
                    author_label: "Owner".into(),
                    body: "Done".into(),
                },
                &owner_actor,
            )
            .await
            .unwrap();
        assert_eq!(
            catalog
                .annotations(first.id, None, None, 500)
                .await
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            catalog.replies(&[annotation.id]).await.unwrap()[0].id,
            reply.id
        );
        let suggestion_id = new_id();
        let mut suggestion = NewAnnotation {
            document_id: first.id,
            kind: "suggestion".into(),
            body: "Change this".into(),
            author_account_id: Some(account.id),
            author_key: format!("account:{}", account.id),
            author_label: "Owner".into(),
            selector: json!({"exact":"Paper"}),
            context: json!({"version":1,"outcome":""}),
            source_version_id: None,
            source_update_sequence: Some(0),
            source_project_generation: Some(0),
            source_state_vector: None,
            publication_id: None,
            proposed_text: Some("Article".into()),
        };
        catalog
            .put_annotation_authorized(suggestion_id, suggestion.clone(), &owner_actor, true)
            .await
            .unwrap();
        suggestion.context = json!({"version":1,"outcome":"accepted"});
        let accepted_sequence = catalog
            .append_update_and_accept_suggestion(
                suggestion_id,
                suggestion.clone(),
                b"accepted-suggestion-update",
                &owner_actor,
            )
            .await
            .unwrap();
        assert_eq!(accepted_sequence, 1);
        assert!(catalog
            .append_update_and_accept_suggestion(
                suggestion_id,
                suggestion,
                b"must-not-be-inserted",
                &owner_actor,
            )
            .await
            .is_err());
        assert_eq!(
            catalog
                .annotations(first.id, None, None, 500)
                .await
                .unwrap()
                .into_iter()
                .find(|row| row.id == suggestion_id)
                .unwrap()
                .suggestion_state
                .as_deref(),
            Some("accepted")
        );
        assert_eq!(
            catalog.updates_after(first.id, 0, 10).await.unwrap().len(),
            1
        );
        let limited = PostgresCatalog {
            pool: catalog.pool.clone(),
            policy: StoragePolicy {
                max_uncompacted_updates: 1,
                max_uncompacted_bytes: i64::MAX / 4,
                ..catalog.policy
            },
        };
        assert!(matches!(
            limited.append_update(first.id, b"over-cap").await,
            Err(Error::Conflict(message)) if message.contains("requires compaction")
        ));
        let durable: (i64, i64, i64) = sqlx::query_as(
            "SELECT update_sequence,uncompacted_update_count,uncompacted_update_bytes
             FROM documents WHERE id=$1",
        )
        .bind(first.id)
        .fetch_one(catalog.pool())
        .await
        .unwrap();
        assert_eq!(durable.0, 1, "a refused update must not advance the stream");
        assert_eq!(durable.1, 1);
        assert_eq!(durable.2, b"accepted-suggestion-update".len() as i64);
        let byte_limited = PostgresCatalog {
            pool: catalog.pool.clone(),
            policy: StoragePolicy {
                max_uncompacted_updates: i64::MAX / 4,
                max_uncompacted_bytes: durable.2,
                ..catalog.policy
            },
        };
        assert!(matches!(
            byte_limited.append_update(first.id, b"over-byte-cap").await,
            Err(Error::Conflict(message)) if message.contains("requires compaction")
        ));

        let asset = NewAsset {
            document_id: first.id,
            storage_key: format!("documents/{}/assets/one", first.id),
            digest: [3; 32],
            byte_length: 100,
            media_type: "image/png".into(),
            original_name: Some("plot.png".into()),
        };
        assert!(catalog.complete_asset(asset.clone()).await.unwrap().1);
        let mut duplicate = asset;
        duplicate.storage_key = format!("documents/{}/assets/orphan", first.id);
        assert!(!catalog.complete_asset(duplicate).await.unwrap().1);
        assert_eq!(catalog.retained_asset_bytes(first.id).await.unwrap(), 100);

        let object_root = tempfile::tempdir().unwrap();
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(object_root.path(), false));
        let sources = SourceStorage::new(
            Arc::new(catalog.clone()),
            blobs.clone(),
            ArchiveLimits::default(),
        );
        for reason in ["first image version", "unchanged image version"] {
            sources
                .commit_project(CommitProject {
                    document_id: first.id,
                    files: vec![
                        ProjectFile {
                            path: "paper.qmd".into(),
                            bytes: b"# Paper\n".to_vec(),
                            media_type: "text/markdown".into(),
                        },
                        ProjectFile {
                            path: "figures/large.png".into(),
                            bytes: vec![17; 1024 * 1024 + 1],
                            media_type: "image/png".into(),
                        },
                    ],
                    through_update_sequence: 0,
                    project_generation: 0,
                    reason: reason.into(),
                    label: None,
                    author_account_id: Some(account.id),
                    author_label: "Owner".into(),
                    make_current: true,
                })
                .await
                .unwrap();
        }
        let stored_assets = blobs
            .list(&format!("documents/{}/assets/", first.id))
            .await
            .unwrap();
        assert_eq!(
            stored_assets.len(),
            1,
            "unchanged asset must not be uploaded twice"
        );
        assert_eq!(catalog.versions(first.id, 10).await.unwrap().len(), 2);
        let concurrent_commit = |marker: &'static [u8]| {
            let storage = SourceStorage::new(
                Arc::new(catalog.clone()),
                blobs.clone(),
                ArchiveLimits::default(),
            );
            async move {
                storage
                    .commit_project(CommitProject {
                        document_id: first.id,
                        files: vec![ProjectFile {
                            path: "paper.qmd".into(),
                            bytes: marker.to_vec(),
                            media_type: "text/markdown".into(),
                        }],
                        through_update_sequence: 1,
                        project_generation: 0,
                        reason: "concurrent version".into(),
                        label: None,
                        author_account_id: Some(account.id),
                        author_label: "Owner".into(),
                        make_current: true,
                    })
                    .await
                    .unwrap()
            }
        };
        let (left, right) = tokio::join!(
            concurrent_commit(b"# Concurrent A\n"),
            concurrent_commit(b"# Concurrent B\n")
        );
        assert_ne!(left.version.sequence, right.version.sequence);
        assert_eq!(
            [left.version.sequence, right.version.sequence]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            [3, 4].into_iter().collect()
        );

        let orphan_id = new_id();
        let orphan_key = format!("documents/{}/versions/{orphan_id}.tar.zst", first.id);
        blobs
            .put_new(&orphan_key, b"uncommitted".to_vec(), "application/zstd")
            .await
            .unwrap();
        let temporary_key = format!("temporary/agent/{orphan_id}/current");
        blobs
            .put_new(&temporary_key, b"in-flight".to_vec(), "application/json")
            .await
            .unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(8 * 24 * 60 * 60);
        std::fs::File::open(object_root.path().join(&orphan_key))
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();
        let maintenance = Maintenance::new(Arc::new(catalog.clone()), blobs.clone());
        assert_eq!(
            maintenance
                .delete_orphans(500, Duration::days(7))
                .await
                .unwrap(),
            1
        );
        assert!(!blobs.exists(&orphan_key).await.unwrap());
        assert!(blobs.exists(&temporary_key).await.unwrap());

        let publications = PublicationStorage::new(Arc::new(catalog.clone()), blobs.clone());
        let first_publication = publications
            .publish(Publish {
                document_id: first.id,
                source_version_id: catalog
                    .current_version(first.id)
                    .await
                    .unwrap()
                    .map(|v| v.id),
                request_key: "publish-one".into(),
                expected_current_id: None,
                publisher_account_id: Some(account.id),
                publisher_label: "Owner".into(),
                files: vec![PublicationFile {
                    path: "index.html".into(),
                    bytes: b"<h1>One</h1>".to_vec(),
                    media_type: "text/html".into(),
                }],
            })
            .await
            .unwrap();
        let object_count = blobs
            .list(&format!("documents/{}/publications/", first.id))
            .await
            .unwrap()
            .len();
        let retry = publications
            .publish(Publish {
                document_id: first.id,
                source_version_id: catalog
                    .current_version(first.id)
                    .await
                    .unwrap()
                    .map(|v| v.id),
                request_key: "publish-one".into(),
                expected_current_id: None,
                publisher_account_id: Some(account.id),
                publisher_label: "Owner".into(),
                files: vec![PublicationFile {
                    path: "index.html".into(),
                    bytes: b"<h1>One</h1>".to_vec(),
                    media_type: "text/html".into(),
                }],
            })
            .await
            .unwrap();
        assert_eq!(retry.id, first_publication.id);
        assert_eq!(
            blobs
                .list(&format!("documents/{}/publications/", first.id))
                .await
                .unwrap()
                .len(),
            object_count,
            "idempotent publication retry must not upload objects"
        );
        let second_publication = publications
            .publish(Publish {
                document_id: first.id,
                source_version_id: catalog
                    .current_version(first.id)
                    .await
                    .unwrap()
                    .map(|v| v.id),
                request_key: "publish-two".into(),
                expected_current_id: Some(first_publication.id),
                publisher_account_id: Some(account.id),
                publisher_label: "Owner".into(),
                files: vec![PublicationFile {
                    path: "index.html".into(),
                    bytes: b"<h1>Two</h1>".to_vec(),
                    media_type: "text/html".into(),
                }],
            })
            .await
            .unwrap();
        assert_ne!(second_publication.id, first_publication.id);
        assert!(catalog
            .publication_cleanup_keys(second_publication.id)
            .await
            .is_err());
        let old_keys = catalog
            .publication_cleanup_keys(first_publication.id)
            .await
            .unwrap();
        assert_eq!(old_keys.len(), 2);
        blobs.delete(&old_keys).await.unwrap();
        assert!(catalog
            .finish_publication_cleanup(first_publication.id)
            .await
            .unwrap());

        let collaboration = CollaborationStorage::new(Arc::new(catalog.clone()), blobs.clone());
        let sequence = collaboration.append(first.id, b"update-one").await.unwrap();
        assert_eq!(sequence, 2);
        let before = collaboration.recover(first.id).await.unwrap();
        assert!(before.base.is_none());
        assert_eq!(before.updates[1].update_bytes, b"update-one");
        let (base, second_sequence) = tokio::join!(
            collaboration.compact(first.id, sequence, 0, b"base-state"),
            collaboration.append(first.id, b"update-two")
        );
        assert!(base.unwrap().is_some());
        assert_eq!(second_sequence.unwrap(), 3);
        let recovered = collaboration.recover(first.id).await.unwrap();
        assert_eq!(recovered.base.as_deref(), Some(b"base-state".as_slice()));
        assert_eq!(recovered.updates.len(), 1);
        assert_eq!(recovered.updates[0].update_bytes, b"update-two");
        let backlog: (i64, i64) = sqlx::query_as(
            "SELECT uncompacted_update_count,uncompacted_update_bytes FROM documents WHERE id=$1",
        )
        .bind(first.id)
        .fetch_one(catalog.pool())
        .await
        .unwrap();
        assert_eq!(backlog, (1, b"update-two".len() as i64));
        assert!(collaboration
            .compact(first.id, 0, 0, b"stale-base")
            .await
            .unwrap()
            .is_none());

        let input = NewJob {
            kind: "source_compaction".into(),
            document_id: Some(first.id),
            account_id: None,
            scope_key: format!("document:{}", first.id),
            dedupe_key: Some("compact:1".into()),
            payload: json!({"through_update_sequence":1}),
            priority: 0,
            max_attempts: 3,
            run_after: OffsetDateTime::now_utc(),
        };
        let job = catalog.enqueue_job(input.clone()).await.unwrap();
        assert_eq!(catalog.enqueue_job(input).await.unwrap().id, job.id);
        let claim = catalog.claim_jobs("worker-one", 1).await.unwrap().remove(0);
        catalog
            .complete_job(&claim, json!({"ok":true}))
            .await
            .unwrap();

        for generation in 0..16 {
            catalog
                .enqueue_job(NewJob {
                    kind: "maintenance".into(),
                    document_id: None,
                    account_id: None,
                    scope_key: "deployment".into(),
                    dedupe_key: Some(format!("contract:{generation}")),
                    payload: json!({
                        "base_cleanup_batch":100,
                        "completed_job_retention_days":7,
                        "completed_job_batch":1000,
                        "orphan_scan_batch":500,
                        "orphan_grace_hours":168
                    }),
                    priority: 0,
                    max_attempts: 3,
                    run_after: OffsetDateTime::now_utc(),
                })
                .await
                .unwrap();
        }
        let (left, right) = tokio::join!(
            catalog.claim_jobs("worker-left", 8),
            catalog.claim_jobs("worker-right", 8)
        );
        let mut claims = left.unwrap();
        claims.extend(right.unwrap());
        assert_eq!(claims.len(), 16);
        let unique: std::collections::HashSet<_> =
            claims.iter().map(|claim| claim.job.id).collect();
        assert_eq!(unique.len(), 16, "workers must receive disjoint claims");

        let stale = claims.pop().unwrap();
        for claim in &claims {
            catalog
                .complete_job(claim, json!({"ok":true}))
                .await
                .unwrap();
        }
        sqlx::query("UPDATE jobs SET locked_at=now()-interval '10 minutes' WHERE id=$1")
            .bind(stale.job.id)
            .execute(catalog.pool())
            .await
            .unwrap();
        assert_eq!(
            catalog
                .recover_expired_jobs(OffsetDateTime::now_utc() - Duration::minutes(5), 10)
                .await
                .unwrap(),
            1
        );
        let replacement = catalog
            .claim_jobs("replacement", 1)
            .await
            .unwrap()
            .remove(0);
        assert_eq!(replacement.job.id, stale.job.id);
        assert!(catalog
            .complete_job(&stale, json!({"stale":true}))
            .await
            .is_err());
        catalog
            .complete_job(&replacement, json!({"ok":true}))
            .await
            .unwrap();

        // A deletion job must survive the row it deletes so its fenced claim
        // can still be completed and audited.
        let disposable = catalog
            .create_document(NewDocument {
                slug: "disposable".into(),
                owner_id: account.id,
                ownership_mode: "owned".into(),
                title: "Disposable".into(),
                source_format: "markdown".into(),
                main_path: "document.md".into(),
                settings: json!({"version":1}),
            })
            .await
            .unwrap();
        let deletion_root = tempfile::tempdir().unwrap();
        let deletion_blobs: Arc<dyn BlobStore> = Arc::new(PartialDeleteStore {
            inner: FsStore::new(deletion_root.path(), false),
            fail_once: AtomicBool::new(true),
        });
        let deletion_keys = [
            format!("documents/{}/assets/{}", disposable.id, new_id()),
            format!("documents/{}/versions/{}.tar.zst", disposable.id, new_id()),
        ];
        for key in &deletion_keys {
            deletion_blobs
                .put_new(key, b"delete-me".to_vec(), "application/octet-stream")
                .await
                .unwrap();
        }
        catalog.mark_document_deleting(disposable.id).await.unwrap();
        let deletion = catalog
            .enqueue_job(NewJob {
                kind: "document_deletion".into(),
                document_id: Some(disposable.id),
                account_id: Some(account.id),
                scope_key: format!("document:{}", disposable.id),
                dedupe_key: Some("delete".into()),
                payload: json!({"document_id":disposable.id}),
                priority: 0,
                max_attempts: 3,
                run_after: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        let deletion_claim = catalog
            .claim_jobs("deletion-worker", 1)
            .await
            .unwrap()
            .into_iter()
            .find(|claim| claim.job.id == deletion.id)
            .unwrap();
        let worker = Worker::new(Arc::new(catalog.clone()), deletion_blobs.clone());
        assert!(worker.execute(&deletion_claim).await.is_err());
        assert!(catalog.document(disposable.id).await.unwrap().is_some());
        assert_eq!(
            deletion_blobs
                .list(&format!("documents/{}/", disposable.id))
                .await
                .unwrap()
                .len(),
            1
        );
        worker.execute(&deletion_claim).await.unwrap();
        assert!(catalog.document(disposable.id).await.unwrap().is_none());
        let completed = catalog
            .complete_job(&deletion_claim, json!({"deleted":true}))
            .await
            .unwrap();
        assert_eq!(completed.status, "succeeded");
        assert_eq!(completed.document_id, None);

        let erased = catalog
            .create_account(NewAccount {
                kind: "registered".into(),
                provider: Some("test".into()),
                provider_subject: Some("erase".into()),
                handle: "erase".into(),
                display_name: "Erase".into(),
                email: Some("erase@example.invalid".into()),
            })
            .await
            .unwrap();
        let erased_document = catalog
            .create_document(NewDocument {
                slug: "erase-document".into(),
                owner_id: erased.id,
                ownership_mode: "owned".into(),
                title: "Erase document".into(),
                source_format: "markdown".into(),
                main_path: "document.md".into(),
                settings: json!({"version":1}),
            })
            .await
            .unwrap();
        assert!(catalog
            .begin_account_erasure(erased.id, Duration::ZERO)
            .await
            .unwrap());
        assert!(!catalog
            .begin_account_erasure(erased.id, Duration::ZERO)
            .await
            .unwrap());
        assert_eq!(
            catalog
                .document(erased_document.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            "deleting"
        );
        assert!(catalog.finish_account_erasure(erased.id).await.is_err());
        assert!(catalog
            .finish_document_deletion(erased_document.id)
            .await
            .unwrap());
        assert!(catalog.finish_account_erasure(erased.id).await.unwrap());
        assert!(catalog.account(erased.id).await.unwrap().is_none());

        catalog.close().await;
    }
}

#[cfg(test)]
mod benchmarks;
pub use access::{AccessRole, GrantRecord, ShareLinkRecord};
pub use annotations::{
    AnnotationRecord, MutationAuthorization, NewAnnotation, NewReply, ReplyRecord,
};
