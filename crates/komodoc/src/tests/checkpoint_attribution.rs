//! Stable checkpoint attribution as the room writes it: which paths carry an
//! authenticated account, and what an erasure that starts mid-write reaches.

use std::sync::Arc;

use super::room::HookStore;
use crate::config::Configuration;
use crate::document::store;
use crate::room::{self, Attribution};
use crate::storage::blob::{self, BlobStore};
use crate::storage::catalog::{Account, Catalog};

fn contributor(id: &str, handle: &str) -> Account {
    Account {
        id: id.into(),
        provider: "github".into(),
        handle: handle.into(),
        name: handle.into(),
        email: format!("{handle}@example.test"),
        first_seen: "2026-01-01T00:00:00.000Z".into(),
        last_seen: "2026-01-01T00:00:00.000Z".into(),
        plan: "free".into(),
        status: "active".into(),
        session_generation: "generation-1".into(),
        erasure_cursor: None,
    }
}

/// A catalogue-backed room with one published document, and the blob store it
/// writes through, so a test can pause an object write mid-checkpoint.
async fn catalog_room(
    slug: &str,
) -> (
    tempfile::TempDir,
    Arc<Catalog>,
    Arc<HookStore>,
    room::RoomSet,
) {
    let dir = tempfile::tempdir().unwrap();
    let inner: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(dir.path().join("objects")));
    let blobs = HookStore::new(inner);
    let catalog = Arc::new(Catalog::open(dir.path().join("catalog.db")).unwrap());
    let config = Arc::new(Configuration::default());
    let store = Arc::new(
        store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
            .await
            .unwrap(),
    );
    let rooms = room::RoomSet::new(blobs.clone(), config);
    rooms.attach_store(store.clone());
    store
        .put(store::Publication {
            slug: slug.into(),
            source: "start".into(),
            source_format: "markdown".into(),
            owner: "owner".into(),
            peak_bytes: Some(1 << 20),
            ..Default::default()
        })
        .await
        .unwrap();
    // Finish the production publication receipt, so the document is active
    // and the room's ordinary checkpoint path is admitted.
    store
        .prepare_publication(slug, &store::digest_of("start"), "publish", None)
        .await
        .unwrap();
    let room = rooms.get(slug).await;
    let mut token = room.reserve_publication_checkpoint().unwrap();
    room.set_main_file("start", "markdown", "main.md").await;
    let sha = room
        .checkpoint_publication_now("cli", Attribution::system(), &mut token)
        .await
        .unwrap()
        .unwrap();
    store.commit_publication(slug, &sha).await.unwrap();
    token.commit();
    (dir, catalog, blobs, rooms)
}

fn attribution_of(catalog: &Catalog, slug: &str, sha: &str) -> (String, Option<String>) {
    let row = catalog.checkpoint(slug, sha).unwrap().unwrap();
    (row.by, row.by_account)
}

/// The signed-in writer paths record the account; a display-only caller
/// records nothing but its name, and the room's own automatic checkpoints
/// record neither.
#[tokio::test]
async fn writer_paths_record_the_account_they_authenticated() {
    let (_dir, catalog, _blobs, rooms) = catalog_room("attributed").await;
    catalog
        .upsert_account(&contributor("acct-1", "alice"))
        .unwrap();
    let room = rooms.get("attributed").await;

    room.set_source("signed in", "markdown").await;
    let signed = room
        .checkpoint_now("cli", Attribution::account("acct-1", "alice"))
        .await
        .unwrap()
        .unwrap();
    room.set_source("anonymous", "markdown").await;
    let anonymous = room
        .checkpoint_now("comment", Attribution::unattributed("Reviewer two"))
        .await
        .unwrap()
        .unwrap();
    room.set_source("system", "markdown").await;
    let system = room
        .checkpoint_now("automatic", Attribution::system())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        attribution_of(&catalog, "attributed", &signed),
        ("alice".to_string(), Some("acct-1".to_string()))
    );
    assert_eq!(
        attribution_of(&catalog, "attributed", &anonymous),
        ("Reviewer two".to_string(), None)
    );
    assert_eq!(
        attribution_of(&catalog, "attributed", &system),
        (String::new(), None)
    );
    // The timeline the browser is served still says only what it always did.
    let manifest = room.manifest().await;
    let served = serde_json::to_value(&manifest).unwrap();
    let entry = served["checkpoints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|point| point["sha"] == serde_json::json!(signed))
        .expect("the signed-in checkpoint is on the timeline")
        .clone();
    assert_eq!(entry["by"], "alice");
    assert!(
        entry.get("by_account").is_none(),
        "the stable id is catalogue state, not wire state"
    );
}

/// An empty account id is not an identity. A caller who is not signed in must
/// never be recorded as an account, however its display string reads.
#[test]
fn a_display_string_never_becomes_an_account() {
    assert_eq!(Attribution::unattributed("acct-1").account_id(), None);
    assert_eq!(Attribution::account("", "alice").account_id(), None);
    assert_eq!(Attribution::system().account_id(), None);
    assert_eq!(
        Attribution::account("acct-1", "alice").account_id(),
        Some("acct-1")
    );
    // A bare string is display-only wherever a writer path takes one.
    assert_eq!(Attribution::from("alice").account_id(), None);
    assert_eq!(Attribution::from("alice").display(), "alice");
}

/// The checkpoint's own durable boundary is the last word. An erasure that
/// begins while the objects are being written -- held here at the tree write
/// by an injected barrier, not by a timer -- must leave the committed row
/// with its content and without its identity.
#[tokio::test]
async fn an_erasure_during_the_object_writes_wins_at_the_durable_boundary() {
    let (_dir, catalog, blobs, rooms) = catalog_room("interleaved").await;
    catalog
        .upsert_account(&contributor("acct-1", "alice"))
        .unwrap();
    let room = rooms.get("interleaved").await;
    room.set_source("first revision", "markdown").await;
    let tree = room.tree().await;
    let storage_id = catalog.document("interleaved").unwrap().unwrap().storage_id;
    *blobs.pause.lock().unwrap() = Some((
        "put".into(),
        blob::checkpoint_key(&storage_id, &tree.digest()),
    ));
    let writing = tokio::spawn({
        let room = room.clone();
        async move {
            room.checkpoint_now("cli", Attribution::account("acct-1", "alice"))
                .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), blobs.reached.notified())
        .await
        .expect("the checkpoint reaches its tree write");
    // The account is erased while this checkpoint is queued behind its own
    // object writes, and the worker has already drained every existing row.
    catalog.begin_erasure("acct-1", "generation-2").unwrap();
    catalog
        .erase_account_batch("acct-1", "checkpoints", None, 0, 1000)
        .unwrap();
    blobs.resume.notify_one();
    let sha = writing.await.unwrap().unwrap().unwrap();

    let row = catalog.checkpoint("interleaved", &sha).unwrap().unwrap();
    assert_eq!(row.by_account, None, "the identity is not reintroduced");
    assert_eq!(row.by, "Deleted user");
    assert_eq!(row.tree_sha, tree.digest(), "the content is retained");
    assert_eq!(room.source().await, "first revision");
}

/// A room can stay resident for the whole erasure. Its cached manifest and its
/// pending update author are what a later staged write and a timeline read are
/// built from, so both must be scrubbed when the erasure starts.
#[tokio::test]
async fn resident_room_caches_drop_the_erased_account() {
    let (_dir, catalog, _blobs, rooms) = catalog_room("resident").await;
    catalog
        .upsert_account(&contributor("acct-1", "alice"))
        .unwrap();
    catalog
        .upsert_account(&contributor("acct-2", "bob"))
        .unwrap();
    let room = rooms.get("resident").await;
    room.set_source("alice wrote this", "markdown").await;
    let mine = room
        .checkpoint_now("cli", Attribution::account("acct-1", "alice"))
        .await
        .unwrap()
        .unwrap();
    room.set_source("bob wrote this", "markdown").await;
    let theirs = room
        .checkpoint_now("cli", Attribution::account("acct-2", "bob"))
        .await
        .unwrap()
        .unwrap();
    {
        let mut state = room.state.lock().await;
        state.session.by = Attribution::account("acct-1", "alice");
        state.session.asked = Some(("cli".into(), Attribution::account("acct-1", "alice")));
    }

    catalog.begin_erasure("acct-1", "generation-2").unwrap();
    rooms.erase_author_from_caches("acct-1").await;

    // Read the room s own cached tail, not the catalogue: this is the copy a
    // staged write and a resident timeline read are built from, and the
    // erasure worker has not reached SQLite yet.
    let manifest = room.state.lock().await.manifest.clone();
    let scrubbed = manifest
        .checkpoints
        .iter()
        .find(|point| point.sha == mine)
        .expect("the resident tail still holds the checkpoint");
    assert_eq!(scrubbed.by, "Deleted user");
    assert_eq!(scrubbed.by_account, None);
    let kept = manifest
        .checkpoints
        .iter()
        .find(|point| point.sha == theirs)
        .expect("another account's checkpoint stays resident");
    assert_eq!(kept.by, "bob");
    assert_eq!(kept.by_account.as_deref(), Some("acct-2"));
    let state = room.state.lock().await;
    assert_eq!(state.session.by.account_id(), None);
    assert_eq!(state.session.by.display(), "Deleted user");
    assert_eq!(state.session.asked.as_ref().unwrap().1.account_id(), None);
}

/// Erasing one account changes nothing about another's attribution and
/// nothing about the document itself.
#[tokio::test]
async fn erasing_one_account_leaves_the_document_and_other_authors_alone() {
    let (_dir, catalog, _blobs, rooms) = catalog_room("shared").await;
    catalog
        .upsert_account(&contributor("acct-1", "alice"))
        .unwrap();
    catalog
        .upsert_account(&contributor("acct-2", "alice"))
        .unwrap();
    let room = rooms.get("shared").await;
    room.set_source("one", "markdown").await;
    let mine = room
        .checkpoint_now("cli", Attribution::account("acct-1", "alice"))
        .await
        .unwrap()
        .unwrap();
    room.set_source("two", "markdown").await;
    let theirs = room
        .checkpoint_now("cli", Attribution::account("acct-2", "alice"))
        .await
        .unwrap()
        .unwrap();
    let before = catalog.checkpoint("shared", &mine).unwrap().unwrap();

    catalog.begin_erasure("acct-1", "generation-2").unwrap();
    crate::storage::maintenance::run_erasure_pass(&catalog, 0, 10, 1000).unwrap();
    while catalog.account("acct-1").unwrap().is_some() {
        crate::storage::maintenance::run_erasure_pass(&catalog, 0, 10, 1000).unwrap();
    }

    let after = catalog.checkpoint("shared", &mine).unwrap().unwrap();
    assert_eq!(after.by, "Deleted user");
    assert_eq!(after.by_account, None);
    assert_eq!(after.sha, before.sha);
    assert_eq!(after.tree_sha, before.tree_sha);
    assert_eq!(after.parent, before.parent);
    assert_eq!(after.at, before.at);
    assert_eq!(after.seq, before.seq);
    assert_eq!(after.size, before.size);
    assert_eq!(
        attribution_of(&catalog, "shared", &theirs),
        ("alice".to_string(), Some("acct-2".to_string())),
        "the same display name on another account is untouched"
    );
    assert_eq!(room.source().await, "two");
}
