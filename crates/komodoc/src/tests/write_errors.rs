//! What a refused write is, and what a caller does with it.
//!
//! The room used to report a refusal as an empty CRDT update or as prose a
//! handler searched for "quota exceeded". These tests hold the three
//! distinctions that replaced it: a refusal is not an empty update, a storage
//! failure is not a refusal, and what a route answers with follows the error's
//! variant rather than its wording.

use std::sync::Arc;

use super::room::{fixture, HookStore};
use crate::config::Configuration;
use crate::document::store;
use crate::room::{self, WriteError};
use crate::storage::blob::{self, BlobStore};

/// A read-only room refuses every direct mutator, and the refusal says so.
/// Nothing downstream -- the checkpoint a publication would register, the
/// update a socket would relay -- may proceed on it.
#[tokio::test]
async fn a_fenced_room_refuses_the_writes_a_publication_depends_on() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    let other = room::RoomSet::new(store.blobs.clone(), Arc::new(Configuration::default()));
    other.attach_store(store);
    let room = other.get("probe").await;
    assert!(room.read_only());

    let refusal = room
        .set_main_file("a whole directory", "markdown", "main.md")
        .await
        .expect_err("a fenced room must refuse the source write");
    assert!(matches!(refusal, WriteError::ReadOnly(_)));
    assert!(refusal.refused(), "a fence is a refusal, not a failure");

    // The dependent work a publication would do next.
    assert!(room.add_text("chapters/one.md", "one").await.is_err());
    assert!(room
        .name_asset("figures/one.png", "deadbeef")
        .await
        .is_err());
    let checkpoint = room
        .checkpoint("cli", "alice")
        .await
        .expect_err("a fenced room must not record a checkpoint");
    assert!(matches!(checkpoint, WriteError::ReadOnly(_)));

    // And nothing moved.
    assert_eq!(room.source().await, "A");
    println!("a fenced room refuses each publication step instead of dropping it");
}

/// Writing the source a document already holds is a valid write that produces
/// an empty update. Before this it was indistinguishable from a refusal,
/// which is what let a refused publication carry on to registration.
#[tokio::test]
async fn an_empty_update_is_not_a_refusal() {
    let (_dir, _store, rooms) = fixture(Configuration::default()).await;
    let room = rooms.get("probe").await;
    // The point is the `Ok`, whatever its length: a caller reads the result,
    // not the size of the update, to know the write happened.
    let again = room
        .set_source("A", "markdown")
        .await
        .expect("writing the source a document already holds is a valid write");
    assert!(
        crate::document::session::decode_update(&again).is_ok(),
        "an unchanged write still answers with a valid update"
    );
    assert_eq!(room.source().await, "A");

    let changed = room
        .set_source("B", "markdown")
        .await
        .expect("an ordinary source write");
    assert!(!changed.is_empty(), "a real edit relays a real update");
    assert_eq!(room.source().await, "B");
    println!("a valid empty update stays distinct from a refused mutation");
}

/// A storage failure is not a refusal: the room tried, its cause is kept for
/// the log, and no client is shown the object that failed.
#[tokio::test]
async fn a_storage_failure_is_told_apart_from_a_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let hooked = HookStore::new(Arc::new(blob::FsStore::new(dir.path())));
    let config = Arc::new(Configuration::default());
    let store = Arc::new(
        store::Store::open(hooked.clone() as Arc<dyn BlobStore>, config.clone())
            .await
            .unwrap(),
    );
    let rooms = room::RoomSet::new(hooked.clone(), config);
    rooms.attach_store(store.clone());
    store
        .put(store::Publication {
            slug: "probe".into(),
            source: "A".into(),
            source_format: "markdown".into(),
            owner: "alice".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let room = rooms.get("probe").await;
    room.set_source("A", "markdown").await.unwrap();

    *hooked.fail.lock().unwrap() = Some("sessions/".into());
    let error = room
        .persist()
        .await
        .expect_err("a failing store must not report a durable write");
    assert!(
        matches!(error, WriteError::Storage(_)),
        "a store failure is a storage failure, not a refusal: {error:?}"
    );
    assert!(!error.refused());
    assert!(
        error
            .log_context()
            .is_some_and(|context| context.contains("injected")),
        "the cause has to survive for the log"
    );
    assert!(
        !error.client_message().contains("injected"),
        "and must not reach a client: {}",
        error.client_message()
    );
    println!("a storage failure keeps its cause for the log and hides it from clients");
}

/// The one rule the mapping exists for: a route's status, its retry advice
/// and its cleanup follow the variant. This walks every variant a handler can
/// receive rather than the two a particular route happens to produce.
#[test]
fn every_variant_maps_without_reading_its_message() {
    use crate::room::error::{DocumentLimit, QuotaKind};
    let cases: [(WriteError, u16); 8] = [
        (WriteError::Quota(QuotaKind::Owner), 507),
        (WriteError::Quota(QuotaKind::Deployment), 507),
        (WriteError::Quota(QuotaKind::UploadRate), 429),
        (WriteError::PermissionDenied, 403),
        (WriteError::NotFound, 404),
        (WriteError::Conflict("moved on".into()), 409),
        (WriteError::Document(DocumentLimit::Quota), 507),
        (WriteError::Storage("bucket reset".into()), 503),
    ];
    for (error, status) in cases {
        assert_eq!(error.status(), status, "{error:?}");
    }
}

/// No handler may go back to deciding what an error means by reading it.
/// A grep, deliberately: the rule is about the shape of the code, and the
/// only way to hold it is to look at the code.
#[test]
fn no_handler_classifies_a_write_error_by_its_text() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    // What these lines legitimately inspect: headers, paths, handles, and the
    // command line's own rendering of a reply body -- never a room write
    // error. Each is listed so that a new one has to be justified here.
    let allowed: &[&str] = &[
        "accept",
        "authorization",
        "text/html",
        "origin",
        "sec-fetch",
        "@",
        "\\\\",
        "y-",
        "'",
    ];
    let mut offenders = Vec::new();
    for area in ["server", "cli"] {
        let dir = root.join(area);
        let mut stack = vec![dir];
        while let Some(path) = stack.pop() {
            for entry in std::fs::read_dir(&path).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let body = std::fs::read_to_string(&path).unwrap();
                for (number, line) in body.lines().enumerate() {
                    let trimmed = line.trim_start();
                    if trimmed.starts_with("//") || trimmed.starts_with("assert") {
                        continue;
                    }
                    for needle in ["error.contains(", "err.contains(", "why.contains("] {
                        if line.contains(needle) {
                            offenders.push(format!("{}:{}", path.display(), number + 1));
                        }
                    }
                    // The classification this track removed, in the shape it
                    // had: a refusal message searched for a quota or a right.
                    if (line.contains(".contains(") || line.contains(".starts_with("))
                        && (line.contains("quota") || line.contains("actor rights"))
                        && !allowed.iter().any(|allowed| line.contains(allowed))
                    {
                        offenders.push(format!("{}:{}", path.display(), number + 1));
                    }
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these lines classify an error by its text: {offenders:?}"
    );
    println!("no server or CLI handler reads an error message to classify it");
}
