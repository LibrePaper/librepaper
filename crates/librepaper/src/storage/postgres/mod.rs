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
mod marks;
mod proposals;
mod publications;
mod repository;

pub use collaboration::{ActivityBucket, CollaborationBase, CollaborationState, PersistedUpdate};
pub use jobs::{Job, JobClaim, JobStatus, NewJob};
pub use proposals::{NewProposal, StoredDecision, StoredProposal};
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
        sqlx::query!("SELECT pg_advisory_lock($1)", MIGRATION_LOCK)
            .fetch_one(&mut *connection)
            .await?;
        let result = MIGRATOR.run(&mut *connection).await;
        let unlock = sqlx::query!("SELECT pg_advisory_unlock($1)", MIGRATION_LOCK)
            .fetch_one(&mut *connection)
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
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn postgres_v3_contract() {
        let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
            .expect("set LIBREPAPER_TEST_POSTGRES_URL to run the PostgreSQL contract");
        let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .unwrap();
        catalog.migrate().await.unwrap();
        sqlx::query!(
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
                Some(5),
            )
            .await
            .unwrap();
        assert_eq!(link.comment_budget, Some(5));
        assert_eq!(
            catalog
                .access_role(first.id, None, Some(link_hash), OffsetDateTime::now_utc())
                .await
                .unwrap(),
            Some(AccessRole::Commenter)
        );
        catalog
            .pin_link_guest(first.id, collaborator.id, AccessRole::Commenter, link_hash)
            .await
            .unwrap();
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
            Some(AccessRole::Commenter)
        );
        // Reading a page of documents batches the three per-document lookups
        // that assembling a listing entry needs. Each batch must return exactly
        // what the per-document call returns, grouped to the right document:
        // getting that wrong would show one document's guests on another.
        let pages = [first.id, second.id];
        let batched_grants = catalog.grants_for_documents(&pages).await.unwrap();
        let batched_links = catalog.share_links_for_documents(&pages).await.unwrap();
        for document_id in pages {
            let grants: Vec<_> = batched_grants
                .iter()
                .filter(|grant| grant.document_id == document_id)
                .map(|grant| {
                    (
                        grant.account_id,
                        grant.role.clone(),
                        grant.source_link_hash.clone(),
                    )
                })
                .collect();
            let expected: Vec<_> = catalog
                .grants(document_id)
                .await
                .unwrap()
                .into_iter()
                .map(|grant| (grant.account_id, grant.role, grant.source_link_hash))
                .collect();
            assert_eq!(grants, expected, "grants for {document_id}");

            let links: Vec<_> = batched_links
                .iter()
                .filter(|link| link.document_id == document_id)
                .map(|link| link.id)
                .collect();
            let expected: Vec<_> = catalog
                .share_links(document_id)
                .await
                .unwrap()
                .into_iter()
                .map(|link| link.id)
                .collect();
            assert_eq!(links, expected, "share links for {document_id}");
        }
        // Only `first` was shared, so the grouping is load-bearing here.
        assert!(batched_grants.iter().any(|g| g.document_id == first.id));
        assert!(!batched_grants.iter().any(|g| g.document_id == second.id));

        let mut accounts: Vec<_> = catalog
            .accounts_by_ids(&[account.id, collaborator.id])
            .await
            .unwrap()
            .into_iter()
            .map(|found| found.id)
            .collect();
        accounts.sort();
        let mut expected = vec![account.id, collaborator.id];
        expected.sort();
        assert_eq!(accounts, expected);
        // An id that names nothing is skipped, not an error: a listing entry
        // whose guest was erased still renders.
        assert!(catalog
            .accounts_by_ids(&[uuid::Uuid::from_u128(0)])
            .await
            .unwrap()
            .is_empty());
        assert!(catalog.accounts_by_ids(&[]).await.unwrap().is_empty());
        assert!(catalog.grants_for_documents(&[]).await.unwrap().is_empty());
        assert!(catalog
            .share_links_for_documents(&[])
            .await
            .unwrap()
            .is_empty());

        let visible = catalog
            .visible_documents(Some(collaborator.id), None, 200, false)
            .await
            .unwrap();
        assert_eq!(
            visible.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![first.id]
        );
        assert!(catalog.revoke_share_link(first.id, link.id).await.unwrap());
        assert_eq!(
            catalog
                .access_role(first.id, None, Some(link_hash), OffsetDateTime::now_utc())
                .await
                .unwrap(),
            None
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
            original_anchor: crate::room::OriginalAnchor {
                checkpoint_id: crate::room::annotation::CheckpointId("checkpoint".into()),
                target: crate::room::CommentTarget::Document,
            },
            publication_id: None,
            color: None,
            presentation: Default::default(),
            attachment: None,
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
                    original_anchor: crate::room::OriginalAnchor {
                        checkpoint_id: crate::room::annotation::CheckpointId("checkpoint".into()),
                        target: crate::room::CommentTarget::Document,
                    },
                    publication_id: None,
                    color: None,
                    presentation: Default::default(),
                    attachment: None,
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
        // One update on the log, so the cap below is already reached and the
        // next append has to refuse. This used to arrive as a side effect of
        // accepting a suggestion here; suggestions are proposals now and do not
        // write through this path, so the precondition is set up plainly.
        assert_eq!(
            catalog
                .append_update(first.id, b"first", b"")
                .await
                .unwrap(),
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
            limited.append_update(first.id, b"over-cap", b"").await,
            Err(Error::Conflict(message)) if message.contains("requires compaction")
        ));
        let durable = sqlx::query!(
            "SELECT update_sequence,uncompacted_update_count,uncompacted_update_bytes
             FROM documents WHERE id=$1",
            first.id,
        )
        .fetch_one(catalog.pool())
        .await
        .unwrap();
        assert_eq!(
            durable.update_sequence, 1,
            "a refused update must not advance the stream"
        );
        assert_eq!(durable.uncompacted_update_count, 1);
        assert_eq!(durable.uncompacted_update_bytes, b"first".len() as i64);
        let byte_limited = PostgresCatalog {
            pool: catalog.pool.clone(),
            policy: StoragePolicy {
                max_uncompacted_updates: i64::MAX / 4,
                max_uncompacted_bytes: durable.uncompacted_update_bytes,
                ..catalog.policy
            },
        };
        assert!(matches!(
            byte_limited.append_update(first.id, b"over-byte-cap", b"").await,
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
                    source: crate::storage::publication::PublicationSource::Owned(b"<h1>One</h1>".to_vec()),
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
                    source: crate::storage::publication::PublicationSource::Owned(b"<h1>One</h1>".to_vec()),
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
                    source: crate::storage::publication::PublicationSource::Owned(b"<h1>Two</h1>".to_vec()),
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
        let sequence = collaboration
            .append(first.id, b"update-one", b"at-one")
            .await
            .unwrap();
        assert_eq!(sequence, 2);
        let before = collaboration.recover(first.id).await.unwrap();
        assert!(before.base.is_none());
        assert_eq!(before.updates[1].update_bytes, b"update-one");
        let (base, second_sequence) = tokio::join!(
            collaboration.compact(first.id, sequence, 0, b"base-state"),
            collaboration.append(first.id, b"update-two", b"at-two")
        );
        assert!(base.unwrap().is_some());
        assert_eq!(second_sequence.unwrap(), 3);
        let recovered = collaboration.recover(first.id).await.unwrap();
        assert_eq!(recovered.base.as_deref(), Some(b"base-state".as_slice()));
        assert_eq!(recovered.updates.len(), 1);
        assert_eq!(recovered.updates[0].update_bytes, b"update-two");
        let backlog = sqlx::query!(
            "SELECT uncompacted_update_count,uncompacted_update_bytes FROM documents WHERE id=$1",
            first.id,
        )
        .fetch_one(catalog.pool())
        .await
        .unwrap();
        assert_eq!(
            (
                backlog.uncompacted_update_count,
                backlog.uncompacted_update_bytes
            ),
            (1, b"update-two".len() as i64)
        );
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
        sqlx::query!(
            "UPDATE jobs SET locked_at=now()-interval '10 minutes' WHERE id=$1",
            stale.job.id,
        )
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

    /// A version carries what it holds and what it moved, or says it cannot.
    ///
    /// Both answers come back from the catalogue rather than from the memory
    /// of the process that wrote them, which is the whole point of the
    /// columns: a server that has just loaded a timeline must be able to tell
    /// that the live document is the one the newest version already holds,
    /// and a reader must be able to scope a file's history without opening
    /// two archives per row.
    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn a_version_records_its_tree_and_the_paths_it_moved() {
        let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
            .expect("set LIBREPAPER_TEST_POSTGRES_URL to run the PostgreSQL contract");
        let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .unwrap();
        catalog.migrate().await.unwrap();
        sqlx::query!(
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
        let document = catalog
            .create_document(NewDocument {
                slug: "paper".into(),
                owner_id: account.id,
                ownership_mode: "owned".into(),
                title: "A Paper".into(),
                source_format: "latex".into(),
                main_path: "paper.tex".into(),
                settings: json!({"version":1}),
            })
            .await
            .unwrap();

        let object_root = tempfile::tempdir().unwrap();
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(object_root.path(), false));
        let sources = SourceStorage::new(
            Arc::new(catalog.clone()),
            blobs.clone(),
            ArchiveLimits::default(),
        );
        let source = |body: &str| crate::storage::source_archive::SourceArchive {
            source_format: "latex".into(),
            main_path: "paper.tex".into(),
            files: vec![crate::storage::source_archive::SourceFile::Inline {
                path: "paper.tex".into(),
                bytes: body.as_bytes().to_vec(),
            }],
        };

        // A commit that was given files rather than a tree cannot answer, and
        // says so rather than claiming that nothing moved.
        sources
            .commit_project(CommitProject {
                document_id: document.id,
                files: vec![ProjectFile {
                    path: "paper.tex".into(),
                    bytes: b"\\documentclass{article}\n".to_vec(),
                    media_type: "text/x-tex".into(),
                }],
                through_update_sequence: 0,
                project_generation: 0,
                reason: "created".into(),
                label: None,
                author_account_id: Some(account.id),
                author_label: "Owner".into(),
                make_current: true,
            })
            .await
            .unwrap();

        let tree_digest = [7_u8; 32];
        let committed = sources
            .commit_archive(crate::storage::source::CommitArchive {
                document_id: document.id,
                archive: source("\\documentclass{article}\n\\begin{document}\n"),
                through_update_sequence: 0,
                project_generation: 0,
                tree_digest: Some(tree_digest),
                changed_paths: Some(vec!["paper.tex".into()]),
                reason: "quiet".into(),
                label: None,
                author_account_id: Some(account.id),
                author_label: "Owner".into(),
                make_current: true,
            })
            .await
            .unwrap();
        assert_eq!(
            committed.version.tree_digest.as_deref(),
            Some(&tree_digest[..])
        );

        let versions = catalog.versions(document.id, 10).await.unwrap();
        assert_eq!(versions.len(), 2);
        let (newest, first) = (&versions[0], &versions[1]);
        assert_eq!(newest.tree_digest.as_deref(), Some(&tree_digest[..]));
        assert_eq!(
            newest.changed_paths.as_deref(),
            Some(&["paper.tex".to_string()][..]),
            "the paths a version moved survive the catalogue"
        );
        // A publish hands storage files rather than a tree, and storage names
        // the tree itself: the version is still recognisable as the document
        // it holds. What it moved stays unanswered, and must read back as
        // unknown rather than as "nothing".
        let published = crate::storage::source_archive::tree_of(
            &crate::storage::source_archive::SourceArchive {
                source_format: "latex".into(),
                main_path: "paper.tex".into(),
                files: vec![crate::storage::source_archive::SourceFile::Inline {
                    path: "paper.tex".into(),
                    bytes: b"\\documentclass{article}\n".to_vec(),
                }],
            },
        )
        .0;
        assert_eq!(
            first.tree_digest.as_deref(),
            Some(&published.digest_bytes()[..]),
            "a version is named by what it holds, however it was written"
        );
        assert!(first.changed_paths.is_none());

        catalog.close().await;
    }

    /// The copies already on disk are reclaimed without losing an event.
    ///
    /// Sharing only helps versions written after it existed; every version
    /// written before holds its own copy of its bytes, which is where the
    /// duplication actually accumulated. The maintenance pass repoints those
    /// at one object. What it must not do -- and what this pins -- is shorten
    /// the history: the rows, their order and their reasons all survive, and
    /// only the storage behind them is shared.
    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn duplicate_archives_already_written_are_reclaimed() {
        let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
            .expect("set LIBREPAPER_TEST_POSTGRES_URL to run the PostgreSQL contract");
        let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .unwrap();
        catalog.migrate().await.unwrap();
        let unique = new_id().simple().to_string();
        let account = catalog
            .create_account(NewAccount {
                kind: "registered".into(),
                provider: Some("test".into()),
                provider_subject: Some(format!("legacy-{unique}")),
                handle: format!("owner-{unique}"),
                display_name: "Owner".into(),
                email: None,
            })
            .await
            .unwrap();
        let document = catalog
            .create_document(NewDocument {
                slug: format!("paper-{unique}"),
                owner_id: account.id,
                ownership_mode: "owned".into(),
                title: "A Paper".into(),
                source_format: "latex".into(),
                main_path: "paper.tex".into(),
                settings: json!({"version":1}),
            })
            .await
            .unwrap();

        // Two versions holding the same bytes under private names: what every
        // version looked like before archives were shared.
        let digest = [11_u8; 32];
        let legacy = |name: &str| NewVersion {
            document_id: document.id,
            parent_id: None,
            through_update_sequence: 0,
            project_generation: 0,
            archive_key: format!("documents/{}/versions/{name}.tar.zst", document.id),
            archive_encoding_version: 1,
            archive_digest: digest,
            archive_bytes: 500,
            logical_bytes: 400,
            tree_digest: None,
            changed_paths: None,
            reason: "quiet".into(),
            label: None,
            author_account_id: Some(account.id),
            author_label: "Owner".into(),
            make_current: true,
        };
        let first = catalog.create_version(legacy("one")).await.unwrap();
        let second = catalog.create_version(legacy("two")).await.unwrap();
        assert_ne!(first.archive_key, second.archive_key);
        assert_eq!(
            catalog.usage_bytes(Some(account.id)).await.unwrap(),
            1000,
            "two private copies are two objects, and are charged as two"
        );

        let object_root = tempfile::tempdir().unwrap();
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(object_root.path(), false));
        let maintenance = Maintenance::new(Arc::new(catalog.clone()), blobs);
        assert_eq!(
            maintenance.share_duplicate_archives(100).await.unwrap(),
            1,
            "the later copy is repointed at the earlier one"
        );

        let versions = catalog.versions(document.id, 10).await.unwrap();
        assert_eq!(versions.len(), 2, "reclaiming bytes does not remove events");
        assert!(
            versions.iter().all(|v| v.archive_key == first.archive_key),
            "both events now name the object that was there first"
        );
        assert_eq!(
            catalog.usage_bytes(Some(account.id)).await.unwrap(),
            500,
            "one object is charged once"
        );
        assert_eq!(
            maintenance.share_duplicate_archives(100).await.unwrap(),
            0,
            "a second pass finds nothing left to share"
        );

        catalog.close().await;
    }

    /// Two versions that hold the same document hold one archive.
    ///
    /// The timeline still gets a row for each -- returning to what a document
    /// said an hour ago is an event, and the history is the record of events
    /// -- but the bytes behind those rows are one object, named by their own
    /// digest. The account is charged for it once, which is the half that
    /// used to be wrong in the expensive direction: a document saved fifty
    /// times between two edits was fifty archives and fifty charges for one
    /// document.
    ///
    /// Deliberately account-scoped and given its own object store, so it
    /// makes no claim about a counter other tests are also moving.
    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn versions_holding_the_same_document_share_one_archive() {
        let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
            .expect("set LIBREPAPER_TEST_POSTGRES_URL to run the PostgreSQL contract");
        let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .unwrap();
        catalog.migrate().await.unwrap();
        let unique = new_id().simple().to_string();
        let account = catalog
            .create_account(NewAccount {
                kind: "registered".into(),
                provider: Some("test".into()),
                provider_subject: Some(format!("shared-{unique}")),
                handle: format!("owner-{unique}"),
                display_name: "Owner".into(),
                email: None,
            })
            .await
            .unwrap();
        let document = catalog
            .create_document(NewDocument {
                slug: format!("paper-{unique}"),
                owner_id: account.id,
                ownership_mode: "owned".into(),
                title: "A Paper".into(),
                source_format: "latex".into(),
                main_path: "paper.tex".into(),
                settings: json!({"version":1}),
            })
            .await
            .unwrap();

        let object_root = tempfile::tempdir().unwrap();
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(object_root.path(), false));
        let sources = SourceStorage::new(
            Arc::new(catalog.clone()),
            blobs.clone(),
            ArchiveLimits::default(),
        );
        let commit = |body: &'static str, reason: &'static str| {
            let sources = &sources;
            let id = document.id;
            async move {
                sources
                    .commit_archive(crate::storage::source::CommitArchive {
                        document_id: id,
                        archive: crate::storage::source_archive::SourceArchive {
                            source_format: "latex".into(),
                            main_path: "paper.tex".into(),
                            files: vec![crate::storage::source_archive::SourceFile::Inline {
                                path: "paper.tex".into(),
                                bytes: body.as_bytes().to_vec(),
                            }],
                        },
                        through_update_sequence: 0,
                        project_generation: 0,
                        tree_digest: None,
                        changed_paths: None,
                        reason: reason.into(),
                        label: None,
                        author_account_id: Some(account.id),
                        author_label: "Owner".into(),
                        make_current: true,
                    })
                    .await
                    .unwrap()
            }
        };

        let first = commit("\\documentclass{article}\n", "created").await;
        let charged = catalog.usage_bytes(Some(account.id)).await.unwrap();
        assert_eq!(
            charged, first.version.archive_bytes,
            "the first version is charged for the archive it wrote"
        );

        // The same document again: a new event, no new bytes.
        let same = commit("\\documentclass{article}\n", "quiet").await;
        assert_eq!(
            same.version.archive_key, first.version.archive_key,
            "an identical document is stored under the name its bytes already have"
        );
        assert_ne!(
            same.version.id, first.version.id,
            "sharing the archive must not collapse two events into one row"
        );
        assert_eq!(
            catalog.usage_bytes(Some(account.id)).await.unwrap(),
            charged,
            "holding the same bytes again costs nothing"
        );
        let archives = blobs
            .list(&format!("documents/{}/archives/", document.id))
            .await
            .unwrap();
        assert_eq!(archives.len(), 1, "one object stands behind both versions");

        // A document that says something else is a different object, and is
        // charged for, so the saving is in duplication alone.
        let changed = commit("\\documentclass{book}\n", "quiet").await;
        assert_ne!(changed.version.archive_key, first.version.archive_key);
        assert_eq!(
            catalog.usage_bytes(Some(account.id)).await.unwrap(),
            charged + changed.version.archive_bytes,
            "new bytes are still new bytes"
        );

        catalog.close().await;
    }

    /// Compaction deletes the update rows; the activity they measured has to
    /// outlive them, and the two sources must never disagree about a row.
    ///
    /// Account-scoped like the test above, so it makes no claim about rows
    /// another test is moving.
    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn activity_outlives_the_updates_it_was_measured_from() {
        let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
            .expect("set LIBREPAPER_TEST_POSTGRES_URL to run the PostgreSQL contract");
        let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .unwrap();
        catalog.migrate().await.unwrap();
        let unique = new_id().simple().to_string();
        let account = catalog
            .create_account(NewAccount {
                kind: "registered".into(),
                provider: Some("test".into()),
                provider_subject: Some(format!("activity-{unique}")),
                handle: format!("writer-{unique}"),
                display_name: "Writer".into(),
                email: None,
            })
            .await
            .unwrap();
        let document = catalog
            .create_document(NewDocument {
                slug: format!("activity-{unique}"),
                owner_id: account.id,
                ownership_mode: "owned".into(),
                title: "A Paper".into(),
                source_format: "latex".into(),
                main_path: "paper.tex".into(),
                settings: json!({"version":1}),
            })
            .await
            .unwrap();

        // Three writes in the same minute. A persisted row is the document's
        // whole encoded history, so these stand for a document growing rather
        // than for three separate edits -- which is why the bucket keeps the
        // largest of them and not their sum.
        let writes: [(&[u8], &[u8]); 3] = [
            (b"one", b"frontier-1"),
            (b"two-two", b"frontier-2"),
            (b"three-three", b"frontier-3"),
        ];
        let largest = writes
            .iter()
            .map(|(body, _)| body.len() as i64)
            .max()
            .unwrap();
        let last_anchor = b"frontier-3".to_vec();
        for (body, frontier) in writes {
            catalog
                .append_update(document.id, body, frontier)
                .await
                .unwrap();
        }
        // What the minute says about itself: how much of it was worked in, how
        // large the document got, and where to go to see it as it was left.
        let shape = |buckets: &[ActivityBucket]| {
            (
                buckets.iter().map(|row| row.changes).sum::<i64>(),
                buckets.iter().map(|row| row.state_bytes).max().unwrap_or(0),
                buckets
                    .last()
                    .map(|row| row.frontier.clone())
                    .unwrap_or_default(),
            )
        };

        // Before any compaction the answer comes entirely from the live rows.
        let live = catalog.document_activity(document.id, None).await.unwrap();
        assert_eq!(shape(&live), (3, largest, last_anchor.clone()));

        let compact = |through: i64, key: &'static str| {
            let catalog = catalog.clone();
            let id = document.id;
            async move {
                catalog
                    .activate_collaboration_base(
                        id,
                        through,
                        0,
                        format!("documents/{id}/collaboration/{key}.loro.zst"),
                        [7; 32],
                        128,
                        OffsetDateTime::now_utc() + Duration::days(1),
                    )
                    .await
                    .unwrap()
                    .expect("the base must be activated")
            }
        };

        // Two of the three are now in the base. The rows are gone and the
        // count is unchanged: one source handed those updates to the other.
        compact(2, "first").await;
        let after_first = catalog.document_activity(document.id, None).await.unwrap();
        assert_eq!(
            shape(&after_first),
            (3, largest, last_anchor.clone()),
            "compaction lost the updates it absorbed, or the anchor of the write still live"
        );
        let remaining = sqlx::query_scalar!(
            "SELECT count(*) FROM document_updates WHERE document_id=$1",
            document.id,
        )
        .fetch_one(catalog.pool())
        .await
        .unwrap();
        assert_eq!(remaining, Some(1), "compaction kept an absorbed update row");

        // The third lands in the same minute as the first two, so the bucket
        // it already wrote must accumulate rather than be replaced.
        compact(3, "second").await;
        let after_second = catalog.document_activity(document.id, None).await.unwrap();
        assert_eq!(
            shape(&after_second),
            (3, largest, last_anchor.clone()),
            "a second compaction into a live bucket replaced it instead of adding to it"
        );
        let stored = sqlx::query_scalar!(
            "SELECT COALESCE(sum(changes),0)::bigint FROM document_activity WHERE document_id=$1",
            document.id,
        )
        .fetch_one(catalog.pool())
        .await
        .unwrap();
        assert_eq!(stored, Some(3), "the whole history is in the buckets now");

        // A base that lost the race writes nothing: it deletes no rows, so
        // recording activity for them would count the same work twice.
        let stale = catalog
            .activate_collaboration_base(
                document.id,
                1,
                0,
                format!("documents/{}/collaboration/stale.loro.zst", document.id),
                [9; 32],
                128,
                OffsetDateTime::now_utc() + Duration::days(1),
            )
            .await
            .unwrap();
        assert!(stale.is_none(), "a stale base must not be activated");
        assert_eq!(
            shape(&catalog.document_activity(document.id, None).await.unwrap()),
            (3, largest, last_anchor),
            "a base that deleted nothing still recorded activity"
        );

        // `since` bounds the scan, and a window after the work is empty
        // rather than a total.
        let later = catalog
            .document_activity(
                document.id,
                Some(OffsetDateTime::now_utc() + Duration::days(1)),
            )
            .await
            .unwrap();
        assert!(later.is_empty(), "a window after the work reported some");

        catalog.close().await;
    }

    /// Saving a document's links again must not collide with the rows it is
    /// replacing.
    ///
    /// `replace_share_links` revokes the live rows rather than deleting them
    /// -- a revoked link has to stay on record so a guest admitted through it
    /// can be recognised and pruned -- and then writes the wanted set. Every
    /// save therefore rewrites links that did not change, with the tokens they
    /// already have. Minting a second role's key is exactly that: the new role
    /// carries a fresh token and every other role carries the one it had.
    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn a_link_kept_across_a_save_does_not_collide_with_its_own_revoked_row() {
        let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
            .expect("set LIBREPAPER_TEST_POSTGRES_URL to run the PostgreSQL contract");
        let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .unwrap();
        catalog.migrate().await.unwrap();
        sqlx::query!(
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
        let document = catalog
            .create_document(NewDocument {
                slug: "shared".into(),
                owner_id: account.id,
                ownership_mode: "owned".into(),
                title: "A Paper".into(),
                source_format: "latex".into(),
                main_path: "paper.tex".into(),
                settings: json!({"version":1}),
            })
            .await
            .unwrap();

        let reader = [1u8; 32];
        let commenter = [2u8; 32];
        let link = |role: &str, hash: [u8; 32]| (role.to_string(), hash, String::new(), None, None);

        catalog
            .replace_share_links(document.id, &[link("reader", reader)])
            .await
            .expect("the first save writes the reader's link");

        // The owner shares with a commenter. The reader's link is untouched,
        // so it is written again with the token it already has.
        catalog
            .replace_share_links(
                document.id,
                &[link("reader", reader), link("commenter", commenter)],
            )
            .await
            .expect("keeping a link while minting another must not collide with itself");

        let live = catalog.share_links(document.id).await.unwrap();
        let mut roles: Vec<&str> = live.iter().map(|row| row.role.as_str()).collect();
        roles.sort_unstable();
        assert_eq!(roles, ["commenter", "reader"], "both links are live");
        assert!(
            live.iter()
                .any(|row| row.role == "reader" && row.token_hash == reader.to_vec()),
            "the kept link still answers to the token its holder has",
        );

        // And the same set saved again -- a label edit, a revocation
        // elsewhere, any owner change at all -- is still not a collision.
        catalog
            .replace_share_links(
                document.id,
                &[link("reader", reader), link("commenter", commenter)],
            )
            .await
            .expect("saving an unchanged set of links must not collide either");
        let unchanged = catalog.share_links(document.id).await.unwrap();
        assert_eq!(unchanged.len(), 2);
        // The rows are the same rows: a link nobody touched was not remade,
        // so it still says when it was shared and how often it has rotated.
        let kept = unchanged
            .iter()
            .find(|row| row.token_hash == reader.to_vec())
            .expect("the reader's link");
        assert_eq!(kept.generation, 1, "an untouched link has not been rotated");

        // Dropping a role still revokes it, and only it.
        catalog
            .replace_share_links(document.id, &[link("commenter", commenter)])
            .await
            .unwrap();
        let after = catalog.share_links(document.id).await.unwrap();
        assert_eq!(
            after
                .iter()
                .map(|row| row.role.as_str())
                .collect::<Vec<_>>(),
            ["commenter"],
            "the role that was dropped is no longer live",
        );

        // And rotating one is a new token in place of the old, which stops
        // answering. The commenter beside it is untouched.
        let rotated = [3u8; 32];
        catalog
            .replace_share_links(
                document.id,
                &[link("reader", rotated), link("commenter", commenter)],
            )
            .await
            .unwrap();
        let live = catalog.share_links(document.id).await.unwrap();
        assert!(
            live.iter().any(|row| row.token_hash == rotated.to_vec()),
            "the rotated link answers to its new token",
        );
        assert!(
            !live.iter().any(|row| row.token_hash == reader.to_vec()),
            "and no longer to the old one",
        );

        catalog.close().await;
    }
    /// A published page points at the figures the document already holds, and
    /// cleaning the page up afterwards must leave them exactly where they are.
    ///
    /// This is the one way this design can lose somebody's work. A figure is
    /// stored once, content-addressed, and shared between the live document
    /// and every publication that shows it. The cleanup that runs an hour
    /// after a publication is superseded deletes the blobs that publication
    /// named -- so it has to delete only the ones nothing else names, or it
    /// takes the figure out of the paper its author is still writing.
    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn a_superseded_page_takes_its_rendering_and_leaves_the_figures() {
        let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
            .expect("set LIBREPAPER_TEST_POSTGRES_URL to run the PostgreSQL contract");
        let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .unwrap();
        catalog.migrate().await.unwrap();
        sqlx::query!(
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
                provider_subject: Some("shared-files".into()),
                handle: "owner".into(),
                display_name: "Owner".into(),
                email: None,
            })
            .await
            .unwrap();
        let document = catalog
            .create_document(NewDocument {
                slug: "paper".into(),
                owner_id: account.id,
                ownership_mode: "owned".into(),
                title: "Paper".into(),
                source_format: "markdown".into(),
                main_path: "paper.md".into(),
                settings: serde_json::json!({}),
            })
            .await
            .unwrap();

        let object_root = tempfile::tempdir().unwrap();
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(object_root.path(), false));
        let figure = b"PNG-BYTES".to_vec();
        let figure_digest: [u8; 32] = <sha2::Sha256 as sha2::Digest>::digest(&figure).into();
        let figure_key = format!("documents/{}/assets/plot", document.id);
        blobs
            .put_new(&figure_key, figure.clone(), "application/octet-stream")
            .await
            .unwrap();
        catalog
            .complete_asset(NewAsset {
                document_id: document.id,
                storage_key: figure_key.clone(),
                digest: figure_digest,
                byte_length: figure.len() as i64,
                media_type: "application/octet-stream".into(),
            original_name: Some("plot.png".into()),
            })
            .await
            .unwrap();
        let held_after_upload = catalog.usage_bytes(None).await.unwrap();

        let publications = PublicationStorage::new(Arc::new(catalog.clone()), blobs.clone());
        let page = |body: &'static [u8], key: &str| Publish {
            document_id: document.id,
            source_version_id: None,
            request_key: key.to_string(),
            expected_current_id: None,
            publisher_account_id: Some(account.id),
            publisher_label: "Owner".into(),
            files: vec![
                PublicationFile {
                    path: "index.html".into(),
                    media_type: "text/html".into(),
                    source: crate::storage::publication::PublicationSource::Owned(body.to_vec()),
                },
                PublicationFile {
                    path: "figures/plot.png".into(),
                    media_type: "image/png".into(),
                    source: crate::storage::publication::PublicationSource::Shared {
                        storage_key: figure_key.clone(),
                        digest: figure_digest,
                        byte_length: figure.len() as i64,
                    },
                },
                // Not a figure anybody uploaded: the renderer emits it, it is
                // the same every time, and no publication before this one
                // holds it. It is owned -- and named by its own digest, so
                // the next publication of the same stylesheet finds it
                // already written instead of rewriting it.
                PublicationFile {
                    path: "style.css".into(),
                    media_type: "text/css".into(),
                    source: crate::storage::publication::PublicationSource::Owned(
                        b"body{font-family:serif}".to_vec(),
                    ),
                },
            ],
        };
        let first = publications
            .publish(page(b"<h1>One</h1>", "one"))
            .await
            .unwrap();

        // The figure was not copied: what went into the blob store for this
        // publication is the rendering and the manifest, and nothing else.
        let written = blobs
            .list(&format!("documents/{}/publications/", document.id))
            .await
            .unwrap();
        assert_eq!(
            written.len(),
            3,
            "a publication writes its rendering, its stylesheet and its manifest -- not the figures in it"
        );

        // And it was not charged for a second time. The quota counts
        // `publication_files`, so what this publication added to it is its
        // rendering alone -- not the figure, which is counted once where it
        // lives, in `document_assets`.
        assert_eq!(
            catalog.usage_bytes(None).await.unwrap() - held_after_upload,
            ("<h1>One</h1>".len() + "body{font-family:serif}".len()) as i64,
            "a shared figure is counted where it lives and not again here"
        );
        // The reader still gets it: the row points at the document's own blob.
        let files = catalog.publication_files(first.id).await.unwrap();
        let shown = files
            .iter()
            .find(|file| file.path == "figures/plot.png")
            .unwrap();
        assert_eq!(shown.storage_key, figure_key);
        assert_eq!(shown.media_type, "image/png", "the page knows it is a PNG");
        assert_eq!(blobs.get(&shown.storage_key).await.unwrap(), figure);

        let mut second = page(b"<h1>Two</h1>", "two");
        second.expected_current_id = Some(first.id);
        let second = publications.publish(second).await.unwrap();
        assert_ne!(second.id, first.id);

        // The second page differs only in its rendering, so that is all it
        // wrote: one more HTML object and one more manifest. The stylesheet
        // it names is the object the first page already wrote.
        let after_second = blobs
            .list(&format!("documents/{}/publications/", document.id))
            .await
            .unwrap();
        assert_eq!(
            after_second.len(),
            written.len() + 2,
            "republishing rewrites the rendering, not the files that did not change"
        );
        let mut stylesheets = Vec::new();
        for publication in [first.id, second.id] {
            stylesheets.push(
                catalog
                    .publication_files(publication)
                    .await
                    .unwrap()
                    .into_iter()
                    .find(|file| file.path == "style.css")
                    .unwrap()
                    .storage_key,
            );
        }
        assert_eq!(
            stylesheets[0], stylesheets[1],
            "two publications of the same stylesheet name one object"
        );


        // Superseding the first page schedules its cleanup. It must take the
        // rendering it owned and leave the figure it only pointed at.
        let doomed = catalog.publication_cleanup_keys(first.id).await.unwrap();
        assert!(
            !doomed.contains(&figure_key),
            "cleaning up a page must not delete the figure the document still holds",
        );
        assert!(
            !doomed.contains(&stylesheets[0]),
            "nor the stylesheet the page that replaced it still names",
        );
        blobs.delete(&doomed).await.unwrap();
        assert!(catalog
            .finish_publication_cleanup(first.id)
            .await
            .unwrap());
        assert!(
            blobs.exists(&figure_key).await.unwrap(),
            "the figure survived the cleanup of a page that showed it",
        );
        assert_eq!(
            blobs.get(&figure_key).await.unwrap(),
            figure,
            "and is byte-for-byte what was uploaded"
        );
        // The page that is still current can still serve both of them.
        let still = catalog.publication_files(second.id).await.unwrap();
        assert!(still
            .iter()
            .any(|file| file.storage_key == figure_key && file.path == "figures/plot.png"));
        assert!(
            blobs.exists(&stylesheets[1]).await.unwrap(),
            "the stylesheet the current page names survived its predecessor's cleanup",
        );

        catalog.close().await;
    }
}

#[cfg(test)]
mod benchmarks;
pub use access::{AccessRole, GrantRecord, ShareLinkRecord};
pub use marks::MarkRecord;
pub use annotations::{
    attachment_from_record, original_anchor_from_record, presentation_from_record,
    AnnotationRecord, MutationAuthorization, NewAnnotation, NewReply, ReplyRecord,
};
