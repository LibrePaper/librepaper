//! Catalog v3 integration coverage lives with the PostgreSQL repositories.
//!
//! Database tests use `LIBREPAPER_TEST_POSTGRES_URL` and are skipped when the
//! variable is absent so ordinary source builds do not require a daemon.
//!
//! What lives here is the part every one of those harnesses had its own copy
//! of: the opt-in, the connection, the migration and the reset. Not the
//! harnesses themselves -- each one stands up the deployment its own
//! scenario needs, with its own writer lease, blob store, rooms and
//! accounts, and those genuinely differ.

use std::sync::Arc;

use crate::storage::postgres::{PostgresCatalog, PostgresOptions};

/// Every table the "server is a log" schema has.
///
/// One list, because six harnesses had their own and two of them had
/// diverged: a table added to the schema and to five of the lists leaves the
/// sixth test seeing the previous test's rows, which fails somewhere else
/// and days later. `document_marks` is named explicitly rather than left to
/// cascade from `documents` and `accounts`, so this does not quietly depend
/// on a foreign key's `ON DELETE` staying what it is.
const SCHEMA_TABLES: &str =
    "operation_outcomes,document_proposal_hunks,document_proposals,document_labels,\
     document_updates,document_snapshots,document_assets,replies,annotations,\
     document_marks,share_links,grants,documents,accounts";

/// Empties the catalogue this process is pointed at.
///
/// It really does truncate: these tests must run with `--test-threads=1`
/// against a database nothing else is using. Calling this from a harness
/// does not change when a reset happens, only which tables it names.
pub(crate) async fn reset(catalog: &PostgresCatalog) {
    sqlx::query(&format!("TRUNCATE {SCHEMA_TABLES} CASCADE"))
        .execute(catalog.pool())
        .await
        .expect("reset the test catalogue");
}

/// A migrated, empty catalogue, or `None` when no database was configured --
/// which is what makes a gated test skip rather than fail on a machine with
/// no PostgreSQL.
pub(crate) async fn catalog() -> Option<Arc<PostgresCatalog>> {
    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL").ok()?;
    let catalog = Arc::new(
        PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .expect("connect to the test catalogue"),
    );
    catalog.migrate().await.expect("apply the current schema");
    reset(&catalog).await;
    Some(catalog)
}
