//! Proves §7.2's retry contract is real for `take_label`, the command
//! `crates/librepaper/src/server/mcp/operations.rs`'s `mcp_label` calls.
//!
//! `Sequencer::command_reporting_replay` and `Room::command_reporting_replay`
//! already exist and are exercised by other command tests through the plain
//! `command`/`take_label` wrappers, which discard the `bool`. This file is
//! the one place that keeps the wrapper honest: a second call with the same
//! `request_id` must answer `replay: true` with the row the first call
//! wrote, not a fresh one.
//!
//! Needs `LIBREPAPER_TEST_POSTGRES_URL`, like the rest of this module's
//! coverage; point it at a throwaway database, not `librepaper` or
//! `librepaper_sqlx`.

use super::*;
use crate::document::store::{DocumentInput, MutationActor, Store};
use crate::log::Registry;
use crate::storage::blob::FsStore;
use crate::storage::postgres::{Authority, PostgresCatalog, PostgresOptions};

const PAPER: &str = "# Interval estimates\n\nThe *interval* covers the mean of the posterior.\n";

struct Deployment {
    rooms: Rooms,
    slug: String,
    account_id: Uuid,
    _writer: crate::storage::postgres::WriterLease,
    _objects: tempfile::TempDir,
}

async fn deployment(slug: &str) -> Option<Deployment> {
    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL").ok()?;
    let catalog = Arc::new(
        PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .unwrap(),
    );
    catalog.migrate().await.unwrap();
    sqlx::query!(
        "TRUNCATE document_updates,document_bases,document_proposal_hunks,document_proposals,\
         replies,annotations,document_labels,document_assets,share_links,grants,documents,\
         accounts CASCADE",
    )
    .execute(catalog.pool())
    .await
    .unwrap();
    let writer = catalog.claim_writer().await.unwrap();
    let account = catalog
        .create_account(crate::storage::postgres::NewAccount {
            kind: "registered".into(),
            provider: Some("test".into()),
            provider_subject: Some("one".into()),
            handle: "owner".into(),
            display_name: "Owner".into(),
            email: None,
        })
        .await
        .unwrap();
    let objects = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn crate::storage::blob::BlobStore> =
        Arc::new(FsStore::new(objects.path(), false));
    let config = Arc::new(Configuration::default());
    let registry = Registry::new(
        catalog.clone(),
        blobs.clone(),
        config.clone(),
        "deployment".into(),
    );
    let store = Arc::new(
        Store::open_with_catalog(
            blobs.clone(),
            config.clone(),
            catalog.clone(),
            registry.clone(),
        )
        .await
        .unwrap(),
    );
    let actor = MutationActor {
        account_id: account.id.to_string(),
        owner_key: "owner".into(),
        session_generation: account.session_generation.to_string(),
        link_hash: String::new(),
        policy_editor: true,
        unowned_publisher: false,
    };
    store
        .put_directory_as_actor(
            DocumentInput {
                slug: slug.into(),
                title: "A Paper".into(),
                source: PAPER.into(),
                source_format: "markdown".into(),
                main: "paper.md".into(),
            },
            Vec::new(),
            actor,
        )
        .await
        .unwrap();
    Some(Deployment {
        rooms: Rooms::new(catalog.clone(), blobs, config, registry),
        slug: slug.into(),
        account_id: account.id,
        _writer: writer,
        _objects: objects,
    })
}

impl Deployment {
    fn authority(&self) -> Authority {
        Authority {
            principal_key: self.account_id.to_string(),
            account_id: Some(self.account_id),
            link_hash: None,
        }
    }
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_retried_request_id_reports_replay_and_the_first_row() {
    let Some(deployment) = deployment("label-replay-paper").await else {
        return;
    };
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    let authority = deployment.authority();
    let request_id = Uuid::new_v4();

    let (first, first_replay) = room
        .take_label_reporting_replay("agent", None, "Agent", &authority, Some(request_id))
        .await
        .unwrap();
    assert!(
        !first_replay,
        "the first call with a fresh request_id is a real commit, not a replay"
    );

    let (second, second_replay) = room
        .take_label_reporting_replay("agent", None, "Agent", &authority, Some(request_id))
        .await
        .unwrap();
    assert!(
        second_replay,
        "a retry with the same request_id must be reported as a replay (§7.2)"
    );
    assert_eq!(
        first.id, second.id,
        "a replay returns the row the first call wrote, not a new one"
    );
    assert_eq!(first.sequence, second.sequence);
    assert_eq!(first.source_sequence, second.source_sequence);

    // A different request_id is a genuinely new command, not a replay of the
    // first -- §7.2 is keyed by request_id, not by "the same command ran on
    // this document before".
    let (third, third_replay) = room
        .take_label_reporting_replay("agent", None, "Agent", &authority, Some(Uuid::new_v4()))
        .await
        .unwrap();
    assert!(!third_replay);
    assert_ne!(first.id, third.id);
}
