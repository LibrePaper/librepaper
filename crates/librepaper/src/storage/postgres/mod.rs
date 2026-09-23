//! PostgreSQL persistence for catalog v3.
//!
//! This module owns connection pooling and migrations. Product operations are
//! split into focused repository modules by domain.

use std::sync::atomic::AtomicI64;
use std::sync::Arc;
use std::time::Duration;

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{ConnectOptions, PgPool, Postgres};

mod access;
pub(crate) mod annotations;

mod commit;
mod document_log;
mod labels;
mod marks;
mod meter;
mod operation_outcomes;
mod ownership;
pub(crate) use operation_outcomes::OperationReceipt;
mod persistence;
pub(crate) use persistence::PersistenceConnection;
mod proposals;
mod repository;

pub use commit::Authority;
pub use document_log::{FlushRow, LogRow, NewSnapshot, PendingWorkCursor, RowCoverage};
pub use labels::{LabelRecord, NewLabel};
pub use ownership::WriterLease;
pub use proposals::{NewProposal, StoredDecision, StoredProposal};
pub use repository::{
    AccountRecord, AssetRecord, DocumentRecord, DocumentStorage, NewAccount, NewAsset, NewDocument,
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
}

impl Default for StoragePolicy {
    fn default() -> Self {
        Self {
            owner_bytes: 100 * 1024 * 1024,
            deployment_bytes: 5 * 1024 * 1024 * 1024,
            asset_uploads_per_hour: 30,
        }
    }
}

/// A document is queued for compaction once its rows since the last base exceed this count.
pub const COMPACTION_UPDATE_THRESHOLD: i64 = 100;

/// A document is queued for compaction once its bytes since the last base exceed this many.
pub const COMPACTION_BYTE_THRESHOLD: i64 = 16 * 1024 * 1024;

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
    writer_epoch: Arc<AtomicI64>,
    /// What the pool is doing, so an operator can tell a wait for a
    /// connection from a slow statement. See [`meter`].
    meter: Arc<meter::PoolMeter>,
    /// What the pool was configured with. `PgPool` reports the connections
    /// it has opened, not the ceiling it may open to, and the difference is
    /// exactly the question "was the pool the constraint".
    max_connections: u32,
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
            writer_epoch: Arc::new(AtomicI64::new(0)),
            meter: Arc::new(meter::PoolMeter::default()),
            max_connections: options.max_connections,
        })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub fn max_connections(&self) -> u32 {
        self.max_connections
    }

    /// How many document flushes this deployment runs at once.
    ///
    /// The periodic sweep used to write one document's row at a time for the
    /// whole deployment, which made durable acknowledgement latency a
    /// function of how many *other* documents were being edited: a
    /// deployment-wide serialization between documents that share nothing.
    /// It is bounded rather than unbounded because the pool is. A sweep that
    /// asked for more connections than exist would convert the serialization
    /// into `acquire_timeout` failures, which is worse.
    ///
    /// Half the pool to leave headroom for opening documents, comments and
    /// sign-in. This is a sweep limit, not a reservation: mixed load can
    /// still exhaust the shared pool. The lease connection
    /// (`claim_writer`) never returns to the pool, so it is subtracted
    /// first.
    pub fn flush_concurrency(&self) -> usize {
        flush_concurrency(self.max_connections)
    }

    /// One transaction, timed, with a note of whether the pool had anything
    /// free when it was asked. Every write path opens its transaction here.
    /// Single-statement reads go straight to the pool as an executor and are
    /// not counted, so the snapshot does not cover them.
    async fn begin_metered(&self) -> Result<sqlx::Transaction<'static, Postgres>> {
        let contended = self.pool.num_idle() == 0 && self.pool.size() >= self.max_connections;
        let attempt = meter::PoolMeter::attempt(contended);
        let begun = self.pool.begin().await;
        self.meter.record(attempt, begun.is_ok());
        Ok(begun?)
    }

    /// The pool's own numbers, for the operator cost snapshot.
    pub fn pool_snapshot(&self) -> serde_json::Value {
        self.meter
            .snapshot(self.pool.size(), self.pool.num_idle(), self.max_connections)
    }

    /// §8.4's compaction triggers -- rows, then bytes -- as this deployment
    /// has them configured.
    ///
    /// `pending_background_work` finds documents already over these lines at
    /// startup; a sequencer reads them so the flush that crosses one asks for
    /// compaction at the same moment (§8.4, §8.6). Nowhere else should decide
    /// what "over the threshold" means.
    pub fn compaction_thresholds(&self) -> (i64, i64) {
        (COMPACTION_UPDATE_THRESHOLD, COMPACTION_BYTE_THRESHOLD)
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

/// See [`PostgresCatalog::flush_concurrency`]. Separated from the catalogue
/// so the arithmetic can be pinned without a database: the two ends are what
/// matter, and both are reachable through configuration
/// (`--database-connections` takes 1..=200).
fn flush_concurrency(max_connections: u32) -> usize {
    ((max_connections.saturating_sub(1) / 2) as usize).max(1)
}

#[cfg(test)]
mod flush_concurrency_tests {
    #[test]
    fn it_is_never_zero_and_never_asks_for_the_whole_pool() {
        // Never return zero: for_each_concurrent treats zero as unbounded.
        // A one-connection pool cannot serve writes while its lease is held.
        assert_eq!(super::flush_concurrency(1), 1);
        assert_eq!(super::flush_concurrency(2), 1);
        // The lease connection is subtracted, then the rest is split with
        // the paths a person is waiting on.
        assert_eq!(super::flush_concurrency(20), 9);
        assert_eq!(super::flush_concurrency(200), 99);
        for connections in 1..=200u32 {
            let concurrency = super::flush_concurrency(connections);
            assert!(concurrency >= 1);
            assert!(
                concurrency < connections.max(2) as usize,
                "{connections} connections must not all go to one sweep"
            );
        }
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
    Ownership(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "PostgreSQL: {error}"),
            Self::Invalid(message) | Self::Conflict(message) | Self::Ownership(message) => {
                formatter.write_str(message)
            }
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
    use std::sync::Arc;

    use futures_util::future::BoxFuture;
    use serde_json::json;
    use time::{Duration, OffsetDateTime};

    use super::*;
    use crate::log::sequencer::LogCatalog;
    use crate::log::{
        Budget, Command, CommandError, Evidence, Head, PreparedSource, Role, Sequencer,
    };
    use crate::room::annotation::{CommentTarget, OriginalAnchor, PresentationContext};
    use crate::room::outgoing::Sender;
    use crate::storage::blob::{BlobStore, FsStore};
    use crate::storage::collaboration::CollaborationStorage;
    use crate::storage::maintenance::Maintenance;

    /// §8.4 step 5 end to end, for the catalogue tests that only care about
    /// where a base and its rows end up. Production splits these two -- the
    /// blob is written with nothing locked and the activation runs under
    /// the sequencer's compaction gate (`storage::worker::Worker::compact`)
    /// -- and no sequencer exists in these tests to gate anything against.
    async fn compact_base(
        storage: &CollaborationStorage,
        catalog: &PostgresCatalog,
        document_id: uuid::Uuid,
        through: i64,
        vector: &[u8],
        snapshot: &[u8],
    ) -> Option<document_log::LogBase> {
        let written = storage.write_base(document_id, snapshot, crate::config::Configuration::default().log_quota_bytes).await.unwrap();
        catalog
            .activate_log_base(
                document_id,
                through,
                vector,
                written,
                crate::storage::collaboration::superseded_base_deadline(),
                false,
            )
            .await
            .unwrap()
            .map(|activated| activated.base)
    }

    /// Every table the "server is a log" schema still has. Each test starts
    /// from one of two postures: this whole-catalogue reset, for a test that
    /// makes claims about the catalogue as a whole, or a unique tag folded
    /// into every id it creates, for a test that is scoped to its own rows
    /// and would rather not race a truncate against whatever else is running.
    async fn truncate(catalog: &PostgresCatalog) {
        crate::tests::reset(catalog).await;
    }

    async fn seed_account(catalog: &PostgresCatalog, tag: &str) -> AccountRecord {
        catalog
            .create_account(NewAccount {
                kind: "registered".into(),
                provider: Some("test".into()),
                provider_subject: Some(format!("subject-{tag}")),
                handle: format!("handle-{tag}"),
                display_name: "Test Owner".into(),
                email: None,
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn operation_outcomes_commit_atomically_and_remain_actor_scoped() {
        let catalog = crate::tests::catalog().await.expect("test database");
        let account = seed_account(&catalog, "outcome").await;
        let document = seed_document(&catalog, account.id, "outcome").await;
        let receipt = OperationReceipt {
            document_id: document.id,
            actor: "actor-one".into(),
            request_id: "request-one".into(),
            digest: "a".repeat(64),
            tool: "document_comment".into(),
            expires_at: OffsetDateTime::now_utc().unix_timestamp() + 3600,
        };
        let outcome = json!({"status":"committed","action":"delete","comment_id":"removed"});
        let mut tx = catalog.pool().begin().await.unwrap();
        PostgresCatalog::record_operation_outcome(&mut tx, &receipt, &outcome)
            .await
            .unwrap();
        tx.rollback().await.unwrap();
        assert!(catalog
            .operation_outcome(document.id, "actor-one", "request-one")
            .await
            .unwrap()
            .is_none());
        let mut tx = catalog.pool().begin().await.unwrap();
        PostgresCatalog::record_operation_outcome(&mut tx, &receipt, &outcome)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let retained = catalog
            .operation_outcome(document.id, "actor-one", "request-one")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retained.outcome, outcome);
        assert_eq!(retained.digest, receipt.digest);
        assert_eq!(retained.tool, receipt.tool);
        catalog
            .store_operation_refusal(
                &receipt,
                "permission_changed",
                "access revoked after commit",
            )
            .await
            .unwrap();
        assert_eq!(
            catalog
                .operation_outcome(document.id, "actor-one", "request-one")
                .await
                .unwrap()
                .unwrap()
                .outcome,
            outcome
        );
        assert!(catalog
            .operation_outcome(document.id, "actor-two", "request-one")
            .await
            .unwrap()
            .is_none());
        assert!(catalog
            .operation_outcome(uuid::Uuid::new_v4(), "actor-one", "request-one")
            .await
            .unwrap()
            .is_none());
        // A reused key cannot replace the original evidence.
        let mut tx = catalog.pool().begin().await.unwrap();
        assert!(PostgresCatalog::record_operation_outcome(
            &mut tx,
            &receipt,
            &json!({"status":"different"})
        )
        .await
        .is_err());
        tx.rollback().await.unwrap();
        assert_eq!(
            catalog
                .operation_outcome(document.id, "actor-one", "request-one")
                .await
                .unwrap()
                .unwrap()
                .outcome,
            outcome
        );
        sqlx::query("UPDATE operation_outcomes SET expires_at=0 WHERE document_id=$1")
            .bind(document.id)
            .execute(catalog.pool())
            .await
            .unwrap();
        assert!(catalog
            .operation_outcome(document.id, "actor-one", "request-one")
            .await
            .unwrap()
            .is_none());
    }

    async fn seed_document(
        catalog: &PostgresCatalog,
        owner_id: uuid::Uuid,
        tag: &str,
    ) -> DocumentRecord {
        catalog
            .create_document(NewDocument {
                slug: format!("slug-{tag}"),
                owner_id,
                ownership_mode: "owned".into(),
                title: "A Paper".into(),
                source_format: "markdown".into(),
                main_path: "paper.md".into(),
                settings: json!({"version":1}),
            })
            .await
            .unwrap()
    }

    fn document_anchor(source_sequence: i64, frontier: Vec<u8>) -> OriginalAnchor {
        OriginalAnchor {
            source_sequence,
            frontier,
            target: CommentTarget::Document,
        }
    }

    fn annotation_input(
        document_id: uuid::Uuid,
        author: uuid::Uuid,
        anchor: OriginalAnchor,
    ) -> NewAnnotation {
        NewAnnotation {
            document_id,
            kind: "comment".into(),
            body: "Review this".into(),
            author_account_id: Some(author),
            author_key: format!("account:{author}"),
            author_label: "Reviewer".into(),
            color: None,
            proposal_id: None,
            original_anchor: anchor,
            presentation: PresentationContext::default(),
            render_digest: None,
            attachment: None,
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
        let _writer = catalog.claim_writer().await.unwrap();
        truncate(&catalog).await;

        let account = seed_account(&catalog, "v3-owner").await;
        let first = seed_document(&catalog, account.id, "v3-first").await;
        let second = catalog
            .create_document(NewDocument {
                slug: "v3-second".into(),
                owner_id: account.id,
                ownership_mode: "owned".into(),
                title: "Same title".into(),
                source_format: "quarto".into(),
                main_path: "paper.qmd".into(),
                settings: json!({"version":1}),
            })
            .await
            .unwrap();

        let collaborator = seed_account(&catalog, "v3-collaborator").await;
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
        // that assembling a listing entry needs. Each batch must return
        // exactly what the per-document call returns, grouped to the right
        // document: getting that wrong would show one document's guests on
        // another.
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

        // The log: a flush writes a row, and the row identity moves onto the
        // document.
        let sequence = catalog
            .flush_log_row(
                first.id,
                FlushRow {
                    expected_update_sequence: 0,
                    update_bytes: b"framed-batch-one",
                    vector: b"vector-one",
                    source_format: Some("quarto"),
                    main_path: Some("paper.qmd"),
                },
            )
            .await
            .unwrap();
        assert_eq!(sequence, 1);
        assert_eq!(catalog.log_sequence(first.id).await.unwrap(), 1);
        assert_eq!(
            catalog.document(first.id).await.unwrap().unwrap().main_path,
            "paper.qmd",
            "a flush that names an identity stamps it on the document row"
        );

        // Annotations and replies. Authorization is checked at the write, and
        // a retry with the same id returns the row a previous attempt wrote.
        let anchor = document_anchor(sequence, b"frontier-bytes".to_vec());
        let unauthorized = MutationAuthorization {
            principal_key: collaborator.id.to_string(),
            account_id: Some(collaborator.id),
            session_generation: Some(collaborator.session_generation),
            token_hash: None,
            policy_editor: false,
        };
        {
            let mut tx = catalog.pool().begin().await.unwrap();
            assert!(catalog
                .put_annotation_authorized(
                    &mut tx,
                    new_id(),
                    annotation_input(first.id, collaborator.id, anchor.clone()),
                    &unauthorized,
                    false,
                )
                .await
                .is_err());
        }
        catalog
            .set_grant(first.id, collaborator.id, AccessRole::Commenter)
            .await
            .unwrap();
        let annotation_id = new_id();
        let input = annotation_input(first.id, collaborator.id, anchor.clone());
        let first_write = {
            let mut tx = catalog.pool().begin().await.unwrap();
            let row = catalog
                .put_annotation_authorized(
                    &mut tx,
                    annotation_id,
                    input.clone(),
                    &unauthorized,
                    false,
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();
            row
        };
        let retried = {
            let mut tx = catalog.pool().begin().await.unwrap();
            let row = catalog
                .put_annotation_authorized(&mut tx, annotation_id, input, &unauthorized, false)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            row
        };
        assert_eq!(
            first_write.id, retried.id,
            "a retried create returns the row it already wrote"
        );
        catalog
            .remove_grant(first.id, collaborator.id)
            .await
            .unwrap();

        let reply_id = new_id();
        let reply_input = NewReply {
            id: reply_id,
            annotation_id: first_write.id,
            author_account_id: Some(account.id),
            author_key: format!("account:{}", account.id),
            author_label: "Owner".into(),
            body: "Done".into(),
        };
        let owner_actor = MutationAuthorization {
            principal_key: account.id.to_string(),
            account_id: Some(account.id),
            session_generation: Some(account.session_generation),
            token_hash: None,
            policy_editor: false,
        };
        let reply = {
            let mut tx = catalog.pool().begin().await.unwrap();
            let row = catalog
                .create_reply_authorized(&mut tx, first.id, reply_input.clone(), &owner_actor)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            row
        };
        let reply_retried = {
            let mut tx = catalog.pool().begin().await.unwrap();
            let row = catalog
                .create_reply_authorized(&mut tx, first.id, reply_input, &owner_actor)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            row
        };
        assert_eq!(reply.id, reply_retried.id);
        assert_eq!(
            catalog
                .annotations(first.id, None, 500, true)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            catalog.replies(&[first_write.id], None, 100).await.unwrap()[0].id,
            reply.id
        );

        // Assets: content-addressed, and a duplicate upload is reclaimed
        // rather than stored twice.
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

        // Orphan sweep: an object under `documents/` that no row names is
        // reclaimed once it is old enough; a referenced one never is.
        let object_root = tempfile::tempdir().unwrap();
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(object_root.path(), false));
        let referenced_key = format!("documents/{}/assets/one", first.id);
        blobs
            .put_new(
                &referenced_key,
                b"kept".to_vec(),
                "application/octet-stream",
            )
            .await
            .unwrap();
        let orphan_key = format!("documents/{}/assets/orphan-object", first.id);
        blobs
            .put_new(
                &orphan_key,
                b"unreferenced".to_vec(),
                "application/octet-stream",
            )
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
        assert!(blobs.exists(&referenced_key).await.unwrap());

        // Trash and restore.
        assert!(catalog.mark_document_deleting(second.id).await.unwrap());
        assert!(catalog
            .trashed_documents(account.id, 50)
            .await
            .unwrap()
            .iter()
            .any(|row| row.id == second.id));
        assert!(catalog.deletion_due(second.id).await.unwrap().is_some());
        assert!(catalog.restore_document(second.id).await.unwrap());
        assert!(
            !catalog.restore_document(second.id).await.unwrap(),
            "already restored"
        );
        assert!(catalog.mark_document_deleting(second.id).await.unwrap());
        assert!(catalog.hasten_deletion(second.id).await.unwrap());
        assert!(!catalog.finish_document_deletion(second.id).await.unwrap());
        assert!(catalog.claim_document_purge(second.id).await.unwrap());
        assert!(catalog.finish_document_deletion(second.id).await.unwrap());
        assert!(catalog.document(second.id).await.unwrap().is_none());

        // Account erasure.
        let erased = seed_account(&catalog, "v3-erased").await;
        let erased_document = catalog
            .create_document(NewDocument {
                slug: "v3-erase-document".into(),
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
        assert!(catalog.hasten_deletion(erased_document.id).await.unwrap());
        assert!(catalog
            .claim_document_purge(erased_document.id)
            .await
            .unwrap());
        assert!(catalog
            .finish_document_deletion(erased_document.id)
            .await
            .unwrap());
        assert!(catalog.finish_account_erasure(erased.id).await.unwrap());
        assert!(catalog.account(erased.id).await.unwrap().is_none());

        catalog.close().await;
    }

    /// A label names a state by its vector, its frontier and its projection
    /// digest (§8.2), and a retry with the same `request_id` returns the row
    /// a previous attempt already wrote rather than a second one (§7.2).
    ///
    /// `document_versions` is gone, and with it the claim the old test made
    /// about a version's tree digest and the paths it moved: nothing in the
    /// new schema records which paths a commit touched, because nothing
    /// computes a diff of trees any more (§8.2, §8.3). What is left to claim
    /// about a named moment is what the row actually holds.
    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn a_label_records_the_vector_frontier_and_digest_of_the_moment_it_names() {
        let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
            .expect("set LIBREPAPER_TEST_POSTGRES_URL to run the PostgreSQL contract");
        let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .unwrap();
        catalog.migrate().await.unwrap();
        let _writer = catalog.claim_writer().await.unwrap();
        let tag = new_id().simple().to_string();
        let account = seed_account(&catalog, &format!("label-{tag}")).await;
        let document = seed_document(&catalog, account.id, &format!("label-{tag}")).await;

        let request_id = uuid::Uuid::new_v4();
        let first = NewLabel {
            id: new_id(),
            document_id: document.id,
            source_sequence: 1,
            vector: b"vector-a".to_vec(),
            frontier: b"frontier-a".to_vec(),
            tree_digest: Some([7; 32]),
            label: Some("v1".into()),
            reason: "restore".into(),
            request_id: Some(request_id),
            author_account_id: Some(account.id),
            author_label: "Owner".into(),
        };
        let written = {
            let mut tx = catalog.pool().begin().await.unwrap();
            let row = catalog.insert_label(&mut tx, &first).await.unwrap();
            tx.commit().await.unwrap();
            row
        };
        assert_eq!(written.sequence, 1);
        assert_eq!(written.vector, b"vector-a");
        assert_eq!(written.frontier, b"frontier-a");
        assert_eq!(written.tree_digest.as_deref(), Some(&[7u8; 32][..]));
        assert_eq!(written.label.as_deref(), Some("v1"));

        let read_back = catalog
            .label(document.id, written.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(read_back.vector, written.vector);
        assert_eq!(read_back.frontier, written.frontier);

        // A second, unrelated label bumps the sequence.
        let second = NewLabel {
            id: new_id(),
            document_id: document.id,
            source_sequence: 2,
            vector: b"vector-b".to_vec(),
            frontier: b"frontier-b".to_vec(),
            tree_digest: None,
            label: None,
            reason: "restore".into(),
            request_id: None,
            author_account_id: Some(account.id),
            author_label: "Owner".into(),
        };
        let second_written = {
            let mut tx = catalog.pool().begin().await.unwrap();
            let row = catalog.insert_label(&mut tx, &second).await.unwrap();
            tx.commit().await.unwrap();
            row
        };
        assert_eq!(second_written.sequence, 2);
        let page = catalog.label_page(document.id, None, 10).await.unwrap();
        assert_eq!(
            page.iter().map(|row| row.sequence).collect::<Vec<_>>(),
            vec![2, 1],
            "the timeline is newest first"
        );

        assert!(catalog
            .rename_label(document.id, written.id, Some("renamed"))
            .await
            .unwrap());
        assert_eq!(
            catalog
                .label(document.id, written.id)
                .await
                .unwrap()
                .unwrap()
                .label
                .as_deref(),
            Some("renamed")
        );

        // A retry with the same request_id, even carrying different
        // evidence, returns the first row rather than writing (or
        // overwriting) anything: `insert_label` finds it by `request_id`
        // before it looks at anything else the caller sent.
        let retry = NewLabel {
            id: new_id(),
            document_id: document.id,
            source_sequence: 99,
            vector: b"vector-different".to_vec(),
            frontier: b"frontier-different".to_vec(),
            tree_digest: None,
            label: Some("should not stick".into()),
            reason: "restore".into(),
            request_id: Some(request_id),
            author_account_id: Some(account.id),
            author_label: "Owner".into(),
        };
        let retried = {
            let mut tx = catalog.pool().begin().await.unwrap();
            let row = catalog.insert_label(&mut tx, &retry).await.unwrap();
            tx.commit().await.unwrap();
            row
        };
        assert_eq!(
            retried.id, written.id,
            "a retry by request_id returns the original row"
        );
        assert_eq!(
            retried.vector, b"vector-a",
            "not the evidence the retry carried"
        );
        assert_eq!(
            catalog
                .label_page(document.id, None, 10)
                .await
                .unwrap()
                .len(),
            2,
            "a retry writes no row"
        );

        catalog.close().await;
    }

    /// Two labels naming the same projection name the same archive object,
    /// and the storage counter that watches `document_labels.archive_key`
    /// counts that object once no matter how many labels point at it (§8.2,
    /// §8.5).
    ///
    /// The old test asserted this of a maintenance pass,
    /// `share_duplicate_archives`, that repointed versions written under
    /// distinct private names at one object after the fact. That pass is
    /// gone and nothing replaces it: an archive is now named by the digest
    /// of what it holds from the moment it is written (§8.5), so there is
    /// nothing left to reclaim after the fact. What is worth testing instead
    /// is the trigger that makes sharing free: `usage_bytes` must not double
    /// count a key two labels hold, and must give the bytes back only when
    /// the last label naming it is gone.
    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn two_labels_naming_the_same_archive_are_charged_once() {
        let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
            .expect("set LIBREPAPER_TEST_POSTGRES_URL to run the PostgreSQL contract");
        let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .unwrap();
        catalog.migrate().await.unwrap();
        let _writer = catalog.claim_writer().await.unwrap();
        let tag = new_id().simple().to_string();
        let account = seed_account(&catalog, &format!("archive-{tag}")).await;
        let document = seed_document(&catalog, account.id, &format!("archive-{tag}")).await;

        let label = |sequence: i64| NewLabel {
            id: new_id(),
            document_id: document.id,
            source_sequence: sequence,
            vector: format!("vector-{sequence}").into_bytes(),
            frontier: format!("frontier-{sequence}").into_bytes(),
            tree_digest: None,
            label: None,
            reason: "restore".into(),
            request_id: None,
            author_account_id: Some(account.id),
            author_label: "Owner".into(),
        };
        let (one, two) = {
            let mut tx = catalog.pool().begin().await.unwrap();
            let one = catalog.insert_label(&mut tx, &label(1)).await.unwrap();
            let two = catalog.insert_label(&mut tx, &label(2)).await.unwrap();
            tx.commit().await.unwrap();
            (one, two)
        };

        let before = catalog.usage_bytes(Some(account.id)).await.unwrap();
        let key = format!("documents/{}/labels/shared.tar.zst", document.id);
        assert!(catalog
            .request_label_archive(document.id, one.id)
            .await
            .unwrap());
        assert!(catalog
            .attach_label_archive(one.id, &key, 500)
            .await
            .unwrap());
        assert_eq!(
            catalog.usage_bytes(Some(account.id)).await.unwrap() - before,
            500,
            "the first label to name an object is charged for it"
        );
        assert!(catalog
            .request_label_archive(document.id, two.id)
            .await
            .unwrap());
        assert!(catalog
            .attach_label_archive(two.id, &key, 500)
            .await
            .unwrap());
        assert_eq!(
            catalog.usage_bytes(Some(account.id)).await.unwrap() - before,
            500,
            "a second label naming the same key adds no bytes"
        );
        assert_eq!(
            catalog
                .label_archive_keys(None, 100)
                .await
                .unwrap()
                .iter()
                .filter(|k| **k == key)
                .count(),
            1,
            "the archive key listing names a shared key once"
        );

        // Removing the first label leaves the object charged, because the
        // second still names it.
        sqlx::query!("DELETE FROM document_labels WHERE id=$1", one.id)
            .execute(catalog.pool())
            .await
            .unwrap();
        assert_eq!(
            catalog.usage_bytes(Some(account.id)).await.unwrap() - before,
            500,
            "the object is still named by the surviving label"
        );

        // And removing the last label naming it gives the bytes back.
        sqlx::query!("DELETE FROM document_labels WHERE id=$1", two.id)
            .execute(catalog.pool())
            .await
            .unwrap();
        assert_eq!(
            catalog.usage_bytes(Some(account.id)).await.unwrap(),
            before,
            "nothing names the object any more"
        );

        catalog.close().await;
    }

    /// Saving a document's links again must not collide with the rows it is
    /// replacing.
    ///
    /// `replace_share_links` revokes the live rows rather than deleting them
    /// -- a revoked link has to stay on record so a guest admitted through it
    /// can be recognised and pruned -- and then writes the wanted set. Every
    /// save therefore rewrites links that did not change, with the tokens
    /// they already have. Minting a second role's key is exactly that: the
    /// new role carries a fresh token and every other role carries the one
    /// it had. Unrelated to the "server is a log" cutover; kept working.
    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn a_link_kept_across_a_save_does_not_collide_with_its_own_revoked_row() {
        let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
            .expect("set LIBREPAPER_TEST_POSTGRES_URL to run the PostgreSQL contract");
        let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .unwrap();
        catalog.migrate().await.unwrap();
        let _writer = catalog.claim_writer().await.unwrap();
        truncate(&catalog).await;
        let account = seed_account(&catalog, "link-owner").await;
        let document = seed_document(&catalog, account.id, "link-shared").await;

        let reader = [1u8; 32];
        let commenter = [2u8; 32];
        let link = |role: &str, hash: [u8; 32]| {
            (
                role.to_string(),
                hash,
                hash.to_vec(),
                String::new(),
                None,
                None,
            )
        };

        catalog
            .replace_share_links(document.id, &[link("reader", reader)])
            .await
            .expect("the first save writes the reader's link");

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

        catalog
            .replace_share_links(
                document.id,
                &[link("reader", reader), link("commenter", commenter)],
            )
            .await
            .expect("saving an unchanged set of links must not collide either");
        let unchanged = catalog.share_links(document.id).await.unwrap();
        assert_eq!(unchanged.len(), 2);
        let kept = unchanged
            .iter()
            .find(|row| row.token_hash == reader.to_vec())
            .expect("the reader's link");
        assert_eq!(kept.generation, 1, "an untouched link has not been rotated");

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

    /// Marks are one person's, and the trash is a week long.
    ///
    /// The two are tested together because they meet: deleting a starred
    /// project and putting it back has to return it starred, which is the
    /// whole reason the mark survives the `deleting` status and dies only
    /// with the row. There is no queue row to cancel any more: the trash is
    /// derived from `documents.deleted_at` plus `storage::maintenance::
    /// DELETION_GRACE` (§8.6), so putting a project back is just clearing
    /// the status.
    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn marks_are_private_and_the_trash_is_recoverable() {
        let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
            .expect("set LIBREPAPER_TEST_POSTGRES_URL to run the PostgreSQL contract");
        let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .unwrap();
        catalog.migrate().await.unwrap();
        let _writer = catalog.claim_writer().await.unwrap();
        truncate(&catalog).await;

        let owner = seed_account(&catalog, "marks-owner").await;
        let other = seed_account(&catalog, "marks-other").await;
        let paper = seed_document(&catalog, owner.id, "marks-paper").await;

        assert!(catalog
            .set_favorite(owner.id, paper.id, true)
            .await
            .unwrap());
        assert!(catalog
            .set_favorite(owner.id, paper.id, true)
            .await
            .unwrap());
        let mine = catalog
            .marks_for_documents(owner.id, &[paper.id])
            .await
            .unwrap();
        assert_eq!(mine.len(), 1, "one row, however many times it was starred");
        assert!(mine[0].favorited_at.is_some());
        assert!(
            mine[0].opened_at.is_none(),
            "starring is not opening: Recent must not fill up with things nobody read",
        );
        assert!(
            catalog
                .marks_for_documents(other.id, &[paper.id])
                .await
                .unwrap()
                .is_empty(),
            "one person's favourites are invisible to everybody else",
        );

        catalog.mark_opened(owner.id, paper.id).await.unwrap();
        let both = catalog
            .marks_for_documents(owner.id, &[paper.id])
            .await
            .unwrap();
        assert_eq!(both.len(), 1);
        assert!(both[0].favorited_at.is_some() && both[0].opened_at.is_some());

        assert!(!catalog
            .set_favorite(owner.id, paper.id, false)
            .await
            .unwrap());
        let after = catalog
            .marks_for_documents(owner.id, &[paper.id])
            .await
            .unwrap();
        assert_eq!(after.len(), 1, "the visit outlives the star");
        assert!(after[0].favorited_at.is_none() && after[0].opened_at.is_some());

        catalog
            .set_favorite(owner.id, paper.id, true)
            .await
            .unwrap();

        let counts = catalog.listing_counts(&[paper.id]).await.unwrap();
        assert_eq!(counts.len(), 1, "a document with no label still has a row");
        assert_eq!(counts[0].comments, 0);
        assert_eq!(
            counts[0].file_count, None,
            "a project with no label does not claim to hold zero files",
        );
        assert!(
            catalog.listing_counts(&[]).await.unwrap().is_empty(),
            "and an empty page asks the database nothing",
        );

        // Into the trash. The row is still here, and so is the mark.
        assert!(catalog.mark_document_deleting(paper.id).await.unwrap());
        let trash = catalog.trashed_documents(owner.id, 50).await.unwrap();
        assert_eq!(trash.len(), 1, "a deleted project is in its owner's trash");
        assert_eq!(trash[0].id, paper.id);
        assert!(
            catalog.deletion_due(paper.id).await.unwrap().is_some(),
            "and the listing can say when it goes",
        );
        assert!(
            catalog
                .trashed_documents(other.id, 50)
                .await
                .unwrap()
                .is_empty(),
            "and it is not in anybody else's",
        );

        // Back out again, still starred.
        assert!(catalog.restore_document(paper.id).await.unwrap());
        assert!(catalog
            .trashed_documents(owner.id, 50)
            .await
            .unwrap()
            .is_empty());
        let restored = catalog
            .marks_for_documents(owner.id, &[paper.id])
            .await
            .unwrap();
        assert!(
            restored
                .first()
                .is_some_and(|mark| mark.favorited_at.is_some()),
            "a project comes back from the trash exactly as it went in",
        );

        assert!(
            !catalog.restore_document(paper.id).await.unwrap(),
            "there is nothing to restore once it is no longer in the trash",
        );

        catalog.close().await;
    }

    /// §5.1: a flush writes exactly one row, and the ambiguous-outcome check
    /// it relies on -- re-reading `documents.update_sequence` and comparing
    /// it with the sequence the flush attempted -- refuses a second write of
    /// the same batch.
    ///
    /// This is the fence `Sequencer::resolve_ambiguous_flush` reads when the
    /// database's response to a flush is lost: if the sequence has already
    /// advanced past what this flush attempted, the flush committed and must
    /// not be retried against the same `expected_update_sequence`. What this
    /// test pins is that refusal, directly, at the row it guards.
    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn a_flush_writes_one_row_and_a_duplicate_attempt_writes_no_second_one() {
        let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
            .expect("set LIBREPAPER_TEST_POSTGRES_URL to run the PostgreSQL contract");
        let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .unwrap();
        catalog.migrate().await.unwrap();
        let _writer = catalog.claim_writer().await.unwrap();
        let tag = new_id().simple().to_string();
        let account = seed_account(&catalog, &format!("flush-{tag}")).await;
        let document = seed_document(&catalog, account.id, &format!("flush-{tag}")).await;

        let sequence = catalog
            .flush_log_row(
                document.id,
                FlushRow {
                    expected_update_sequence: 0,
                    update_bytes: b"batch-one",
                    vector: b"vector-one",
                    source_format: Some("markdown"),
                    main_path: Some("paper.md"),
                },
            )
            .await
            .unwrap();
        assert_eq!(sequence, 1);
        assert_eq!(
            catalog.log_sequence(document.id).await.unwrap(),
            1,
            "this is exactly what the ambiguous-outcome branch re-reads",
        );

        // A second attempt at the same flush -- the shape of a retry after a
        // lost response, before the sequencer has re-read the sequence -- is
        // refused rather than appended again.
        let duplicate = catalog
            .flush_log_row(
                document.id,
                FlushRow {
                    expected_update_sequence: 0,
                    update_bytes: b"batch-one",
                    vector: b"vector-one",
                    source_format: None,
                    main_path: None,
                },
            )
            .await;
        assert!(
            matches!(duplicate, Err(Error::Conflict(_))),
            "a flush against a sequence that has already advanced must be refused",
        );
        assert_eq!(
            catalog.log_coverage(document.id).await.unwrap().len(),
            1,
            "exactly one row was written"
        );

        // And a flush against the sequence the log is actually at proceeds,
        // appending a second, distinct row.
        let second = catalog
            .flush_log_row(
                document.id,
                FlushRow {
                    expected_update_sequence: 1,
                    update_bytes: b"batch-two",
                    vector: b"vector-two",
                    source_format: None,
                    main_path: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(second, 2);
        assert_eq!(catalog.log_coverage(document.id).await.unwrap().len(), 2);

        catalog.close().await;
    }

    /// §8.4 steps 2 and 4: compaction only ever acts on a cache entry whose
    /// content actually equals the log vector it claims to cover, and when
    /// no such entry can be produced, not one row is touched.
    ///
    /// `worker.rs::tests::coverage_rejects_*` already pins the pure half of
    /// this -- a fabricated or truncated snapshot fails the header and
    /// scratch-import checks. What was missing is the storage-side half:
    /// that the production entry point for those checks,
    /// `Sequencer::snapshot_at_log_vector`, refuses to use an entry that
    /// does not match, and that refusal leaves the rows exactly where they
    /// were. A genuine mismatch cannot be produced through the ordinary
    /// ingest path -- gaps are refused at ingest, which is the whole point
    /// -- so this test constructs a sequencer directly with a log vector
    /// that does not match what the rows it was handed actually decode to,
    /// the same way `Sequencer::from_parts` lets the ingest and flush tests
    /// exercise the sequencer without a real admission.
    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn a_compaction_entry_that_does_not_cover_the_log_leaves_every_row_in_place() {
        let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
            .expect("set LIBREPAPER_TEST_POSTGRES_URL to run the PostgreSQL contract");
        let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .unwrap();
        catalog.migrate().await.unwrap();
        let _writer = catalog.claim_writer().await.unwrap();
        let tag = new_id().simple().to_string();
        let account = seed_account(&catalog, &format!("coverage-{tag}")).await;
        let document = seed_document(&catalog, account.id, &format!("coverage-{tag}")).await;

        let doc = crate::document::session::new_doc();
        doc.set_peer_id(1).unwrap();
        doc.get_text("body").insert(0, "hello").unwrap();
        doc.commit();
        let real_vector = doc.oplog_vv().encode();
        let update = doc.export(loro::ExportMode::Snapshot).unwrap();
        let framed = crate::log::frame::encode(&[crate::log::Batch {
            peer_key: "test-peer".into(),
            client_seq: 1,
            bytes: update,
        }]);
        catalog
            .flush_log_row(
                document.id,
                FlushRow {
                    expected_update_sequence: 0,
                    update_bytes: &framed,
                    vector: &real_vector,
                    source_format: Some("markdown"),
                    main_path: Some("paper.md"),
                },
            )
            .await
            .unwrap();

        let object_root = tempfile::tempdir().unwrap();
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(object_root.path(), false));
        let config = Arc::new(crate::config::Configuration::default());
        let budget = Budget::new(config.memory_budget_bytes, config.cache_expansion);
        let pending =
            crate::log::PendingBudget::new(config.pending_bytes, config.pending_scratch_bytes);
        let catalog_arc: Arc<dyn LogCatalog> = Arc::new(catalog.clone());
        // A log vector that does not match the row the sequencer was just
        // handed: `admit` would never produce this, because it reads the
        // vector off the row itself, but a corrupted or mismatched entry is
        // exactly what step 2 has to refuse to use.
        let sequencer = Sequencer::from_parts(
            document.id,
            document.slug.clone(),
            uuid::Uuid::nil(),
            catalog_arc,
            blobs,
            config,
            budget,
            pending,
            "deployment-test-peer".into(),
            crate::log::ledger::StorageLedger::new(),
            2,
            loro::VersionVector::default(),
            0,
            0,
            0,
            loro::VersionVector::default(),
            false,
            Arc::new(std::sync::OnceLock::new()),
        );

        let outcome = sequencer.snapshot_at_log_vector(crate::log::sequencer::SnapshotMode::Full).await.unwrap();
        assert!(
            outcome.is_none(),
            "an entry built from the rows does not match the fabricated log vector, so it is not used",
        );
        assert_eq!(
            catalog.log_rows(document.id, 0, None).await.unwrap().len(),
            1,
            "coverage could not be proved, so the row was never deleted",
        );
        assert!(
            catalog.log_base(document.id).await.unwrap().is_none(),
            "and no base was ever activated over it",
        );

        catalog.close().await;
    }

    /// §8.4 step 5 keeps the base a compaction replaced for seven days, so
    /// a reader already downloading it is not cut off. Nothing looks at that
    /// row again afterwards: no route reads it, the backup enumerator only
    /// copies it, and the orphan sweeper treats it as referenced. So if the
    /// worker never comes back for it, a deployment leaks one stale snapshot
    /// per compaction and the object store grows without bound.
    ///
    /// Driven here the way production drives it (§8.6): the durable state is
    /// the row, the startup scan finds it, and the worker does the rest. A
    /// test that called the sweep directly would prove the sweep works and
    /// leave the missing caller -- which was the actual defect -- untested.
    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn a_superseded_compaction_base_is_deleted_once_its_grace_has_passed() {
        let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
            .expect("set LIBREPAPER_TEST_POSTGRES_URL to run the PostgreSQL contract");
        let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .unwrap();
        catalog.migrate().await.unwrap();
        let _writer = catalog.claim_writer().await.unwrap();
        let tag = new_id().simple().to_string();
        let account = seed_account(&catalog, &format!("sweep-{tag}")).await;
        let document = seed_document(&catalog, account.id, &format!("sweep-{tag}")).await;

        let doc = crate::document::session::new_doc();
        doc.set_peer_id(1).unwrap();
        doc.get_text("body").insert(0, "alpha").unwrap();
        doc.commit();
        let v1 = doc.oplog_vv();
        let batch1 = doc.export(loro::ExportMode::Snapshot).unwrap();
        let snapshot_through_one = batch1.clone();

        doc.get_text("body").insert(5, " beta").unwrap();
        doc.commit();
        let v2 = doc.oplog_vv();
        let batch2 = doc
            .export(loro::ExportMode::Updates {
                from: std::borrow::Cow::Owned(v1.clone()),
            })
            .unwrap();
        let snapshot_through_two = doc.export(loro::ExportMode::Snapshot).unwrap();

        doc.get_text("body").insert(10, " gamma").unwrap();
        doc.commit();
        let v3 = doc.oplog_vv();
        let batch3 = doc
            .export(loro::ExportMode::Updates {
                from: std::borrow::Cow::Owned(v2.clone()),
            })
            .unwrap();
        let snapshot_through_three = doc.export(loro::ExportMode::Snapshot).unwrap();

        let frame_of = |batch: &[u8], seq: i64| {
            crate::log::frame::encode(&[crate::log::Batch {
                peer_key: "test-peer".into(),
                client_seq: seq,
                bytes: batch.to_vec(),
            }])
        };
        for (expected, bytes, vector) in [
            (0, frame_of(&batch1, 1), v1.encode()),
            (1, frame_of(&batch2, 2), v2.encode()),
            (2, frame_of(&batch3, 3), v3.encode()),
        ] {
            catalog
                .flush_log_row(
                    document.id,
                    FlushRow {
                        expected_update_sequence: expected,
                        update_bytes: &bytes,
                        vector: &vector,
                        source_format: Some("markdown"),
                        main_path: Some("paper.md"),
                    },
                )
                .await
                .unwrap();
        }

        let object_root = tempfile::tempdir().unwrap();
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(object_root.path(), false));
        let catalog = Arc::new(catalog);
        let storage = CollaborationStorage::new(catalog.clone(), blobs.clone());
        // THREE compactions inside one grace window, which is what an
        // actively edited document does in a day: every base but the last is
        // superseded, and each one has to be remembered. A single
        // predecessor column held only the newest of them and silently
        // forgot the rest, which is the leak this test exists to catch.
        compact_base(
            &storage,
            &catalog,
            document.id,
            1,
            &v1.encode(),
            &snapshot_through_one,
        )
        .await;
        let first_key = catalog
            .log_base(document.id)
            .await
            .unwrap()
            .expect("the first base")
            .snapshot_key;
        compact_base(
            &storage,
            &catalog,
            document.id,
            2,
            &v2.encode(),
            &snapshot_through_two,
        )
        .await;
        let second_key = catalog
            .log_base(document.id)
            .await
            .unwrap()
            .expect("the second base")
            .snapshot_key;
        compact_base(
            &storage,
            &catalog,
            document.id,
            3,
            &v3.encode(),
            &snapshot_through_three,
        )
        .await;
        let base = catalog
            .log_base(document.id)
            .await
            .unwrap()
            .expect("the third base");

        let mut scheduled = catalog
            .superseded_base_keys(document.id)
            .await
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>();
        scheduled.sort();
        let mut expected = vec![first_key.clone(), second_key.clone()];
        expected.sort();
        assert_eq!(
            scheduled, expected,
            "both replaced bases are scheduled, not just the newest",
        );
        assert!(blobs.exists(&first_key).await.unwrap());
        assert!(blobs.exists(&second_key).await.unwrap());

        // The grace is seven days and a test is not going to wait for it.
        sqlx::query!(
            "UPDATE document_snapshots SET delete_after=now()-interval '1 hour'
             WHERE document_id=$1 AND delete_after IS NOT NULL",
            document.id,
        )
        .execute(catalog.pool())
        .await
        .unwrap();

        let config = Arc::new(crate::config::Configuration::default());
        let registry = crate::log::Registry::new(
            catalog.clone(),
            blobs.clone(),
            config.clone(),
            "sweep-test-peer".to_string(),
        );
        let (worker, _handle) = crate::storage::worker::Worker::new(
            catalog.clone(),
            blobs.clone(),
            registry,
            config.clone(),
        );
        let running = tokio::spawn(worker.run());

        let mut swept = false;
        for _ in 0..100 {
            if !blobs.exists(&first_key).await.unwrap() && !blobs.exists(&second_key).await.unwrap()
            {
                swept = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        running.abort();
        assert!(
            swept,
            "a superseded base is still in the object store: first={} second={}",
            blobs.exists(&first_key).await.unwrap(),
            blobs.exists(&second_key).await.unwrap(),
        );
        assert!(
            catalog
                .superseded_base_keys(document.id)
                .await
                .unwrap()
                .is_empty(),
            "no row still names bytes that are gone",
        );
        assert!(
            blobs.exists(&base.snapshot_key).await.unwrap(),
            "and the live base was not swept with it",
        );

        catalog.close().await;
    }

    /// §6.2: a join from a vector the log has entirely outrun, from one that
    /// covers the compaction base and nothing after it, and from one that
    /// covers everything but the newest row, each receive what step 3
    /// through 5 says -- and none of the three builds a cache, because
    /// `Sequencer::join` never touches one.
    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn a_join_receives_what_its_vector_does_not_cover_and_builds_no_cache() {
        let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
            .expect("set LIBREPAPER_TEST_POSTGRES_URL to run the PostgreSQL contract");
        let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .unwrap();
        catalog.migrate().await.unwrap();
        let _writer = catalog.claim_writer().await.unwrap();
        let tag = new_id().simple().to_string();
        let account = seed_account(&catalog, &format!("join-{tag}")).await;
        let document = seed_document(&catalog, account.id, &format!("join-{tag}")).await;

        let doc = crate::document::session::new_doc();
        doc.set_peer_id(1).unwrap();
        doc.get_text("body").insert(0, "alpha").unwrap();
        doc.commit();
        let v1 = doc.oplog_vv();
        let batch1 = doc.export(loro::ExportMode::Snapshot).unwrap();

        doc.get_text("body").insert(5, " beta").unwrap();
        doc.commit();
        let v2 = doc.oplog_vv();
        let batch2 = doc
            .export(loro::ExportMode::Updates {
                from: std::borrow::Cow::Owned(v1.clone()),
            })
            .unwrap();
        // Captured now, before the third edit: this is the full state
        // through row 2, which is what the compaction base below claims to
        // cover.
        let snapshot_through_two = doc.export(loro::ExportMode::Snapshot).unwrap();

        doc.get_text("body").insert(10, " gamma").unwrap();
        doc.commit();
        let v3 = doc.oplog_vv();
        let batch3 = doc
            .export(loro::ExportMode::Updates {
                from: std::borrow::Cow::Owned(v2.clone()),
            })
            .unwrap();

        let frame_of = |batch: &[u8], seq: i64| {
            crate::log::frame::encode(&[crate::log::Batch {
                peer_key: "test-peer".into(),
                client_seq: seq,
                bytes: batch.to_vec(),
            }])
        };
        let row1 = frame_of(&batch1, 1);
        let row2 = frame_of(&batch2, 2);
        let row3 = frame_of(&batch3, 3);
        catalog
            .flush_log_row(
                document.id,
                FlushRow {
                    expected_update_sequence: 0,
                    update_bytes: &row1,
                    vector: &v1.encode(),
                    source_format: Some("markdown"),
                    main_path: Some("paper.md"),
                },
            )
            .await
            .unwrap();
        catalog
            .flush_log_row(
                document.id,
                FlushRow {
                    expected_update_sequence: 1,
                    update_bytes: &row2,
                    vector: &v2.encode(),
                    source_format: None,
                    main_path: None,
                },
            )
            .await
            .unwrap();
        catalog
            .flush_log_row(
                document.id,
                FlushRow {
                    expected_update_sequence: 2,
                    update_bytes: &row3,
                    vector: &v3.encode(),
                    source_format: None,
                    main_path: None,
                },
            )
            .await
            .unwrap();

        let object_root = tempfile::tempdir().unwrap();
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(object_root.path(), false));
        compact_base(
            &CollaborationStorage::new(Arc::new(catalog.clone()), blobs.clone()),
            &catalog,
            document.id,
            2,
            &v2.encode(),
            &snapshot_through_two,
        )
        .await;
        // Only row 3 remains; rows 1 and 2 are folded into the base.
        assert_eq!(
            catalog.log_rows(document.id, 0, None).await.unwrap().len(),
            1
        );

        let config = Arc::new(crate::config::Configuration::default());
        let budget = Budget::new(config.memory_budget_bytes, config.cache_expansion);
        let pending =
            crate::log::PendingBudget::new(config.pending_bytes, config.pending_scratch_bytes);
        let sequencer = Sequencer::admit(
            document.id,
            document.slug.clone(),
            Arc::new(catalog.clone()),
            blobs,
            config,
            budget,
            pending,
            "deployment-test-peer".into(),
            crate::log::ledger::StorageLedger::new(),
            Arc::new(std::sync::OnceLock::new()),
        )
        .await
        .unwrap();

        let (sender, _receiver) = Sender::channel(16, 1 << 20, None, None);

        // An empty vector: the client covers neither the base nor the row
        // after it, so the reply carries the base by reference and every row
        // behind it -- here, the single remaining row.
        let from_empty = sequencer
            .join(1, Role::Editor, "peer-empty", sender.clone(), None)
            .await
            .unwrap();
        assert_eq!(from_empty.vector, v3.encode());
        assert_eq!(
            from_empty.base.as_deref(),
            Some(snapshot_through_two.as_slice())
        );
        assert_eq!(from_empty.batches, vec![batch3.clone()]);

        // A vector covering exactly the base: the base is covered, so no
        // base is sent, and the reply is the row after it -- `doc-rows`.
        let base_vector = v2.encode();
        let from_base = sequencer
            .join(
                2,
                Role::Editor,
                "peer-base",
                sender.clone(),
                Some(&base_vector),
            )
            .await
            .unwrap();
        assert!(from_base.base.is_none());
        assert_eq!(from_base.batches, vec![batch3.clone()]);
        assert!(
            from_base.from_rows,
            "some rows are uncovered while the base is: doc-rows"
        );

        // A vector covering everything: nothing is sent but the head vector,
        // and the reply carries neither a base nor rows.
        let head_vector = v3.encode();
        let from_head = sequencer
            .join(3, Role::Editor, "peer-head", sender, Some(&head_vector))
            .await
            .unwrap();
        assert!(from_head.base.is_none());
        assert!(from_head.batches.is_empty());
        assert!(!from_head.from_rows);

        // None of the three joins built a cache: `Sequencer::join` answers
        // entirely from `log_coverage` and `log_rows`, never from a decoded
        // document (§6.2's own text: "neither step needs a document").
        assert!(!sequencer.is_warm().await, "a join must not build a cache");

        catalog.close().await;
    }

    /// A command that implements a "restore to this text" kind of source
    /// edit for the purpose of the test below: setting the document's body
    /// to a target string, idempotently. Reapplying it once the body already
    /// reads as `target` is a genuine no-op edit -- `Head::prepare` exports
    /// nothing from an unchanged fork -- which is what makes the retry in
    /// the test below a real retry rather than a second, different write.
    struct SetBodyLabel {
        document_id: uuid::Uuid,
        catalog: Arc<PostgresCatalog>,
        request_id: uuid::Uuid,
        target: String,
    }

    impl Command for SetBodyLabel {
        type Output = LabelRecord;

        fn name(&self) -> &'static str {
            "test-set-body-label"
        }

        fn replay(
            &mut self,
        ) -> BoxFuture<'_, std::result::Result<Option<Self::Output>, CommandError>> {
            Box::pin(async move {
                self.catalog
                    .label_by_request(self.document_id, self.request_id)
                    .await
                    .map_err(CommandError::from)
            })
        }

        fn evaluate(
            &mut self,
            head: &Head<'_>,
        ) -> std::result::Result<Option<PreparedSource>, CommandError> {
            let target = self.target.clone();
            head.prepare(1, move |doc| {
                let text = doc.get_text("body");
                if text.to_string() != target {
                    let existing_len = text.to_string().chars().count();
                    if existing_len > 0 {
                        text.delete(0, existing_len)
                            .map_err(|error| error.to_string())?;
                    }
                    text.insert(0, &target).map_err(|error| error.to_string())?;
                }
                Ok(())
            })
            .map_err(CommandError::Conflict)
        }

        fn transact<'a>(
            &'a mut self,
            tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
            evidence: &'a Evidence,
        ) -> BoxFuture<'a, std::result::Result<Self::Output, CommandError>> {
            Box::pin(async move {
                let frontier = evidence
                    .after_frontier
                    .clone()
                    .unwrap_or_else(|| evidence.before_frontier.clone());
                let new_label = NewLabel {
                    id: new_id(),
                    document_id: self.document_id,
                    source_sequence: evidence.source_sequence,
                    vector: evidence.vector.clone(),
                    frontier,
                    tree_digest: None,
                    label: None,
                    reason: "test-set-body-label".into(),
                    request_id: Some(self.request_id),
                    author_account_id: None,
                    author_label: "Test".into(),
                };
                self.catalog
                    .insert_label(tx, &new_label)
                    .await
                    .map_err(CommandError::from)
            })
        }
    }

    /// §7.2, §7.3: a source-producing command records before and after
    /// evidence that match the bytes of the row its flush wrote, and a retry
    /// carrying the same `request_id` returns the same label rather than
    /// writing a second row.
    ///
    /// This exercises the real path -- `Sequencer::command`, the same
    /// method a comment, a restore or an agent patch goes through -- rather
    /// than calling `insert_label` on its own, because the claim is about
    /// what the sequencer assembles around a command, not about the label
    /// table in isolation.
    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn a_source_producing_command_records_matching_evidence_and_a_retry_returns_the_same_label(
    ) {
        let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
            .expect("set LIBREPAPER_TEST_POSTGRES_URL to run the PostgreSQL contract");
        let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .unwrap();
        catalog.migrate().await.unwrap();
        let _writer = catalog.claim_writer().await.unwrap();
        let tag = new_id().simple().to_string();
        let account = seed_account(&catalog, &format!("command-{tag}")).await;
        let document = seed_document(&catalog, account.id, &format!("command-{tag}")).await;

        let object_root = tempfile::tempdir().unwrap();
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(object_root.path(), false));
        let config = Arc::new(crate::config::Configuration::default());
        let budget = Budget::new(config.memory_budget_bytes, config.cache_expansion);
        let pending =
            crate::log::PendingBudget::new(config.pending_bytes, config.pending_scratch_bytes);
        let sequencer = Sequencer::admit(
            document.id,
            document.slug.clone(),
            Arc::new(catalog.clone()),
            blobs,
            config,
            budget,
            pending,
            "deployment-test-peer".into(),
            crate::log::ledger::StorageLedger::new(),
            Arc::new(std::sync::OnceLock::new()),
        )
        .await
        .unwrap();

        let authority = Authority {
            principal_key: account.id.to_string(),
            account_id: Some(account.id),
            link_hash: None,
        };
        let request_id = uuid::Uuid::new_v4();
        let mut command = SetBodyLabel {
            document_id: document.id,
            catalog: Arc::new(catalog.clone()),
            request_id,
            target: "hello world".into(),
        };

        let first = sequencer.command(&authority, &mut command).await.unwrap();
        assert_eq!(first.request_id, Some(request_id));
        assert_eq!(first.reason, "test-set-body-label");

        let rows = catalog.log_coverage(document.id).await.unwrap();
        assert_eq!(rows.len(), 1, "the command's edit flushed exactly one row");
        assert_eq!(
            rows[0].vector, first.vector,
            "the label's evidence names the same state the row it was written beside covers",
        );
        let written = catalog.log_rows(document.id, 0, None).await.unwrap();
        assert_eq!(written.len(), 1);
        let check = loro::LoroDoc::new();
        for batch in crate::log::frame::decode(&written[0].update_bytes).unwrap() {
            check.import(&batch.bytes).unwrap();
        }
        assert_eq!(
            check.get_text("body").to_string(),
            "hello world",
            "the row's bytes hold exactly the edit the command made",
        );

        // A retry with the same request_id and the same target text is a
        // genuine no-op against the now-current head: the edit produces no
        // delta, so no second row is written, and the label lookup by
        // request_id returns the first answer.
        let second = sequencer.command(&authority, &mut command).await.unwrap();
        assert_eq!(
            second.id, first.id,
            "a retry by request_id returns the same label"
        );
        assert_eq!(second.sequence, first.sequence);
        assert_eq!(
            catalog.log_coverage(document.id).await.unwrap().len(),
            1,
            "an idempotent retry writes no second row",
        );

        catalog.close().await;
    }
}

#[cfg(test)]
mod benchmarks;
pub use access::AccessRole;
pub use annotations::{
    original_anchor_from_record, presentation_from_record, AnnotationRecord, AnnotationState,
    MutationAuthorization, NewAnnotation, NewReply, ReplyRecord, SizedReply,
};
