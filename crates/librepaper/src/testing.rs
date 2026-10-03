//! Catalog v3 integration coverage lives with the PostgreSQL repositories.
//!
//! Database tests are marked `#[ignore]`, so ordinary test runs do not need a
//! database. Explicitly selecting one requires `LIBREPAPER_TEST_POSTGRES_URL`
//! to point to a disposable PostgreSQL database; run these tests serially
//! (`--test-threads=1`) because each test resets the catalogue.
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
    "moderation_audit,moderated_projects,pending_log_reservations,operation_outcomes,document_template_operations,document_templates,document_proposal_outcomes,document_proposal_hunks,document_proposals,document_labels,\
     document_updates,document_snapshots,document_assets,replies,annotations,\
     document_marks,share_links,grants,upload_admissions,account_upload_admissions,documents,accounts";

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
    sqlx::query("UPDATE storage_usage SET bytes=0 WHERE singleton")
        .execute(catalog.pool())
        .await
        .expect("reset deployment storage accounting");
}

/// A migrated, empty catalogue.
///
/// The `Option` return is retained because many fixtures already propagate
/// it with `?`; database-dependent tests are explicitly `#[ignore]` instead
/// of using `None` as an implicit skip. Selecting one requires a configured
/// disposable database and a serial test run.
pub(crate) async fn catalog() -> Option<Arc<PostgresCatalog>> {
    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL").unwrap_or_else(|_| {
        panic!(
            "LIBREPAPER_TEST_POSTGRES_URL is required for this ignored PostgreSQL test; set it to a disposable PostgreSQL database URL and rerun with --test-threads=1"
        )
    });
    assert!(
        !url.trim().is_empty(),
        "LIBREPAPER_TEST_POSTGRES_URL is empty; set it to a disposable PostgreSQL database URL and rerun with --test-threads=1"
    );
    let catalog = Arc::new(
        PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .expect("could not connect to LIBREPAPER_TEST_POSTGRES_URL; check that it is a valid PostgreSQL URL and points to a running disposable database"),
    );
    catalog.migrate().await.expect("apply the current schema");
    reset(&catalog).await;
    Some(catalog)
}
