//! What a room may still hold while it waits on storage or on the catalogue.
//!
//! Every assertion here is a barrier proof rather than a measurement: a writer
//! is parked at an exact point inside its durable write, and another caller is
//! then asked to do something that needs the lock the writer used to hold. On
//! the code these tests were written against, each of those callers blocked
//! until the writer was released.

use super::room::{fixture, HookStore};
use crate::config::Configuration;
use crate::document::store;
use crate::room::{self, Applied};
use crate::storage::blob::{self, BlobStore};
use std::sync::Arc;
use std::time::Duration;

/// Two seconds is far longer than any of these operations needs; it is the
/// bound that turns "did not deadlock" into a test result.
const PATIENCE: Duration = Duration::from_secs(2);

fn comment(body: &str, temp_id: &str) -> room::Command {
    room::Command::Comment {
        motivation: "commenting".into(),
        body: body.into(),
        creator: "Reviewer".into(),
        exact: "A".into(),
        prefix: String::new(),
        suffix: String::new(),
        position: None,
        region: None,
        source: None,
        proposed: None,
        revision: String::new(),
        temp_id: temp_id.into(),
        request_id: String::new(),
    }
}

/// A legacy room over a store whose comment blob writes can be paused and
/// failed. `fixture` has no catalogue, so this is the whole-blob comment path.
async fn legacy_room() -> (
    tempfile::TempDir,
    Arc<store::Store>,
    Arc<HookStore>,
    room::RoomSet,
    Arc<room::Room>,
) {
    let (dir, store, _rooms) = fixture(Configuration::default()).await;
    store
        .blobs
        .delete(&[blob::room_lock_key("probe")])
        .await
        .unwrap();
    let hooked = HookStore::new(store.blobs.clone());
    let rooms = room::RoomSet::new(hooked.clone(), Arc::new(Configuration::default()));
    rooms.attach_store(store.clone());
    let room = rooms.get("probe").await;
    (dir, store, hooked, rooms, room)
}

/// The room's comment blob write no longer holds room state, so a person
/// typing and a socket attaching both get through while it is in storage.
#[tokio::test]
async fn paused_comment_persistence_lets_an_edit_and_a_socket_through() {
    let (_dir, _store, hooked, _rooms, room) = legacy_room().await;
    *hooked.pause.lock().unwrap() = Some(("swap".into(), blob::room_key("probe")));
    let writing = tokio::spawn({
        let room = room.clone();
        async move {
            room.apply_command(comment("first", ""), "1.1.1.1", "", "", None, true)
                .await
        }
    });
    tokio::time::timeout(PATIENCE, hooked.reached.notified())
        .await
        .expect("the comment write reached storage");

    // Room state, with the comment blob write parked mid-flight.
    tokio::time::timeout(PATIENCE, room.set_source("EDITED WHILE SAVING", "markdown"))
        .await
        .expect("a source edit must not wait on a comment write")
        .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    tokio::time::timeout(PATIENCE, room.attach(7, tx, true))
        .await
        .expect("a socket must not wait on a comment write");
    let seen = tokio::time::timeout(PATIENCE, room.snapshot_for("", true))
        .await
        .expect("a snapshot must not wait on a comment write");
    assert!(
        seen.is_empty(),
        "the comment is not visible until it is durable"
    );

    hooked.resume.notify_one();
    let (event, ok) = writing.await.unwrap();
    assert!(ok, "the comment landed: {event}");
    assert_eq!(room.snapshot().await.len(), 1);
    assert_eq!(room.source().await, "EDITED WHILE SAVING");
    println!("legacy comment write: source edit, socket attach and snapshot all proceed");
}

/// Two comment writers overlapping on the whole-blob path both land. The
/// second prepares from the list the first installed rather than from the one
/// it started with, so neither change is written away.
#[tokio::test]
async fn two_concurrent_comment_writers_both_land() {
    let (_dir, store, hooked, _rooms, room) = legacy_room().await;
    *hooked.pause.lock().unwrap() = Some(("swap".into(), blob::room_key("probe")));
    let first = tokio::spawn({
        let room = room.clone();
        async move {
            room.apply_command(comment("first", ""), "1.1.1.1", "", "", None, true)
                .await
        }
    });
    tokio::time::timeout(PATIENCE, hooked.reached.notified())
        .await
        .expect("the first comment write reached storage");
    let second = tokio::spawn({
        let room = room.clone();
        async move {
            room.apply_command(comment("second", ""), "1.1.1.2", "", "", None, true)
                .await
        }
    });
    hooked.resume.notify_one();
    assert!(first.await.unwrap().1);
    assert!(
        tokio::time::timeout(PATIENCE, second)
            .await
            .expect("the second writer completes")
            .unwrap()
            .1
    );

    let bodies: Vec<String> = room
        .snapshot()
        .await
        .into_iter()
        .map(|item| item.body)
        .collect();
    assert_eq!(bodies, vec!["first".to_string(), "second".to_string()]);
    // And the durable object holds both, not just the last writer's copy.
    let raw = store.blobs.get(&blob::room_key("probe")).await.unwrap();
    let stored: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let saved = stored["comments"].as_array().unwrap();
    assert_eq!(saved.len(), 2, "both comments are in the stored list");
    println!("two overlapping comment writers: both changes survive");
}

/// A failed comment write installs nothing. The edit that arrived while it was
/// in storage survives, no comment appears, and the room still works -- the
/// old path put the pre-write snapshot back and would have dropped it.
#[tokio::test]
async fn a_failed_comment_write_preserves_later_state() {
    let (_dir, store, hooked, _rooms, room) = legacy_room().await;
    room.apply_command(comment("kept", ""), "1.1.1.1", "", "", None, true)
        .await;
    *hooked.pause.lock().unwrap() = Some(("swap".into(), blob::room_key("probe")));
    let writing = tokio::spawn({
        let room = room.clone();
        async move {
            room.apply_command(comment("doomed", ""), "1.1.1.2", "", "", None, true)
                .await
        }
    });
    tokio::time::timeout(PATIENCE, hooked.reached.notified())
        .await
        .expect("the comment write reached storage");
    room.set_source("EDITED WHILE SAVING", "markdown")
        .await
        .unwrap();
    *hooked.fail.lock().unwrap() = Some(blob::room_key("probe"));
    hooked.resume.notify_one();
    let (event, ok) = writing.await.unwrap();
    assert!(!ok, "a failed write is refused: {event}");

    *hooked.fail.lock().unwrap() = None;
    let kept: Vec<String> = room
        .snapshot()
        .await
        .into_iter()
        .map(|item| item.body)
        .collect();
    assert_eq!(kept, vec!["kept".to_string()]);
    assert_eq!(room.source().await, "EDITED WHILE SAVING");
    // The room is not fenced or wedged: the next comment lands, and every
    // reader converges on the same list.
    assert!(
        room.apply_command(comment("after", ""), "1.1.1.3", "", "", None, true)
            .await
            .1
    );
    let raw = store.blobs.get(&blob::room_key("probe")).await.unwrap();
    let stored: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let saved: Vec<String> = stored["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["body"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(saved, vec!["kept".to_string(), "after".to_string()]);
    println!("failed comment write: nothing installed, later edits and peers intact");
}

/// Cancelling a comment write mid-storage leaves the room exactly as it was.
#[tokio::test]
async fn a_cancelled_comment_write_leaves_the_room_untouched() {
    let (_dir, _store, hooked, _rooms, room) = legacy_room().await;
    room.apply_command(comment("kept", ""), "1.1.1.1", "", "", None, true)
        .await;
    *hooked.pause.lock().unwrap() = Some(("swap".into(), blob::room_key("probe")));
    let writing = tokio::spawn({
        let room = room.clone();
        async move {
            room.apply_command(comment("abandoned", ""), "1.1.1.2", "", "", None, true)
                .await
        }
    });
    tokio::time::timeout(PATIENCE, hooked.reached.notified())
        .await
        .expect("the comment write reached storage");
    writing.abort();
    let _ = writing.await;
    hooked.resume.notify_one();

    // The gate the cancelled writer held is released, so the next one runs.
    let (_, ok) = tokio::time::timeout(
        PATIENCE,
        room.apply_command(comment("after", ""), "1.1.1.3", "", "", None, true),
    )
    .await
    .expect("the comment gate is not left held by a cancelled writer");
    assert!(ok);
    let bodies: Vec<String> = room
        .snapshot()
        .await
        .into_iter()
        .map(|item| item.body)
        .collect();
    assert_eq!(bodies, vec!["kept".to_string(), "after".to_string()]);
    println!("cancelled comment write: no half-applied comment, gate released");
}

/// The pending-edit quota reservation is awaited without room state. A caller
/// parked in that window must not stop a reader, a socket attachment or a
/// second look at the document.
#[tokio::test]
async fn the_edit_reservation_wait_does_not_hold_room_state() {
    let dir = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(dir.path().join("objects"), true));
    let catalog =
        Arc::new(crate::storage::catalog::Catalog::open(dir.path().join("catalog.db")).unwrap());
    let config = Arc::new(Configuration::default());
    let store = Arc::new(
        store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
            .await
            .unwrap(),
    );
    let rooms = room::RoomSet::new(blobs, config);
    rooms.attach_store(store.clone());
    store
        .put(store::Publication {
            slug: "reserved".into(),
            source: "A".into(),
            source_format: "markdown".into(),
            owner: "alice".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let room = rooms.get("reserved").await;
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    room.attach(1, tx, true).await;
    let update = {
        let state = room.state.lock().await;
        crate::document::session::encode_state(&state.session.doc)
    };
    let scratch = crate::document::session::new_doc();
    crate::document::session::apply_update(&scratch, &update).unwrap();
    crate::document::session::put_text(&scratch, "main.md", "AB");
    let edit = crate::document::session::encode_diff(&scratch, &{
        let state = room.state.lock().await;
        crate::document::session::encode_vector(&state.session.doc)
    })
    .unwrap();

    let gate = room::ReservationGate::new("reserved");
    *room::after_edit_reservation_gate().lock().unwrap() = Some(gate.clone());
    let editing = tokio::spawn({
        let room = room.clone();
        async move { room.receive_update(1, &edit, 1, "alice").await }
    });
    tokio::time::timeout(PATIENCE, gate.reached.notified())
        .await
        .expect("the edit reached the reservation window");

    // Room state, with the reservation parked. This is the assertion the
    // whole change exists for: it deadlocked before.
    let seen = tokio::time::timeout(PATIENCE, room.snapshot_for("", true))
        .await
        .expect("a reader must not wait on the edit reservation");
    assert!(seen.is_empty());
    let (other, _other_rx) = tokio::sync::mpsc::channel(8);
    tokio::time::timeout(PATIENCE, room.attach(2, other, true))
        .await
        .expect("a socket must not wait on the edit reservation");

    gate.resume.add_permits(1);
    let applied = editing.await.unwrap();
    assert!(matches!(applied, Applied::Relay));
    assert_eq!(room.source().await, "AB");
    *room::after_edit_reservation_gate().lock().unwrap() = None;
    println!("edit reservation: room state free while the quota decision runs");
}
