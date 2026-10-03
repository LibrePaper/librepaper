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

use std::borrow::Cow;
use std::sync::Arc;

use loro::{ExportMode, LoroDoc, VersionVector};

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
pub async fn reset(catalog: &PostgresCatalog) {
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
pub async fn catalog() -> Option<Arc<PostgresCatalog>> {
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

/// A causally-linked source of update batches, standing in for one editor's
/// outbox: each call exports exactly the ops since the last call, which is
/// what one `doc-update` looks like on the wire.
pub struct Outbox {
    pub doc: LoroDoc,
    pub at: VersionVector,
}

impl Outbox {
    pub fn new() -> Self {
        let doc = LoroDoc::new();
        let _ = doc.get_text("t");
        Self {
            doc,
            at: VersionVector::default(),
        }
    }

    pub fn export_since_last(&mut self) -> Vec<u8> {
        self.doc.commit();
        let batch = self
            .doc
            .export(ExportMode::Updates {
                from: Cow::Borrowed(&self.at),
            })
            .expect("exporting from a covered vector never fails");
        self.at = self.doc.oplog_vv();
        batch
    }

    /// One keystroke's worth of edit, batched.
    pub fn edit(&mut self) -> Vec<u8> {
        let text = self.doc.get_text("t");
        text.insert_utf16(text.len_utf16(), "x").unwrap();
        self.export_since_last()
    }

    /// An edit padded to at least `bytes` wide. The filler is random, not
    /// one repeated character: Loro's run-length encoding would otherwise
    /// collapse a repeated character back under the trigger it is meant to
    /// cross.
    pub fn edit_at_least(&mut self, bytes: usize) -> Vec<u8> {
        let filler = hex::encode(librepaper_base::util::random_bytes(bytes / 2 + 1));
        let text = self.doc.get_text("t");
        text.insert_utf16(text.len_utf16(), &filler).unwrap();
        self.export_since_last()
    }

    /// What this outbox's own document currently reads, for a test to
    /// compare against what a server it sent batches to ends up holding.
    pub fn text(&self) -> String {
        self.doc.get_text("t").to_string()
    }

    /// This outbox's own vector -- what it has sent and can vouch for --
    /// distinct from `export_since_last`'s bookkeeping of what it has
    /// already exported once.
    pub fn vector(&self) -> VersionVector {
        self.doc.oplog_vv()
    }

    /// Every op after `vector`, without touching this outbox's own
    /// bookkeeping of what it has already exported once -- what a client
    /// resending its whole unsent history after a `doc-gap` does.
    pub fn export_from(&self, vector: &VersionVector) -> Vec<u8> {
        self.doc
            .export(ExportMode::Updates {
                from: Cow::Borrowed(vector),
            })
            .expect("exporting from a covered vector never fails")
    }
}
