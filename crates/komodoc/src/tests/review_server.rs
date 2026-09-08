//! Regression tests for the findings of REVIEW-codex-crates.md (server group).
#![allow(unused_imports)]
use super::*;

use std::sync::Mutex;
use std::time::Duration;

use futures_util::FutureExt;
use serde_json::json;

use crate::blob::{BlobError, BlobInfo, BlobResult, BlobStore, BlobVersion, FsStore};
use crate::config::Configuration;
use crate::room;
use crate::session;

/// Reads the next frame, but treats the server closing the socket as an
/// answer rather than a test failure: `Socket::read` panics on a close frame,
/// so this is what every R01/R34 regression below uses to ask "did the
/// server hang up on this connection", with a couple of seconds' grace for
/// the close to actually travel.
async fn expect_close(socket: &mut Socket) {
    let outcome = tokio::time::timeout(Duration::from_secs(3), async {
        std::panic::AssertUnwindSafe(socket.read())
            .catch_unwind()
            .await
    })
    .await
    .expect("the server must close or otherwise answer within a few seconds");
    assert!(
        outcome.is_err(),
        "expected the server to close the socket, but it kept answering normally"
    );
}

/// A store that can be told to fail every write under one key prefix. What
/// `review_creation_saves_source_or_reports_failure` uses to put the
/// `history/` write a checkpoint depends on out of reach, without touching
/// the document index underneath it.
struct HookStore {
    inner: std::sync::Arc<dyn BlobStore>,
    fail: Mutex<Option<String>>,
}

impl HookStore {
    fn new(inner: std::sync::Arc<dyn BlobStore>) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            inner,
            fail: Mutex::new(None),
        })
    }
    fn refused(&self, key: &str) -> BlobResult<()> {
        if self
            .fail
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|prefix| key.starts_with(prefix.as_str()))
        {
            return Err(BlobError::Other("injected review failure".into()));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl BlobStore for HookStore {
    async fn get(&self, k: &str) -> BlobResult<Vec<u8>> {
        self.refused(k)?;
        self.inner.get(k).await
    }
    async fn get_versioned(&self, k: &str) -> BlobResult<(Vec<u8>, BlobVersion)> {
        self.refused(k)?;
        self.inner.get_versioned(k).await
    }
    async fn put(&self, k: &str, b: Vec<u8>, t: &str) -> BlobResult<()> {
        self.refused(k)?;
        self.inner.put(k, b, t).await
    }
    async fn swap(&self, k: &str, b: Vec<u8>, e: &str) -> BlobResult<BlobVersion> {
        self.refused(k)?;
        self.inner.swap(k, b, e).await
    }
    async fn list(&self, p: &str) -> BlobResult<Vec<BlobInfo>> {
        self.inner.list(p).await
    }
    async fn delete(&self, k: &[String]) -> BlobResult<()> {
        self.inner.delete(k).await
    }
    fn describe(&self) -> String {
        self.inner.describe()
    }
}

/// Posts a directory: a `main.md` main file plus whatever extra files are
/// given, each named by its path. What every directory-publish test in this
/// file drives the real HTTP handler with.
async fn directory(
    server: &TestServer,
    slug: &str,
    extra: Vec<(&str, Vec<u8>)>,
) -> (u16, serde_json::Value) {
    let mut form = reqwest::multipart::Form::new()
        .text("title", "Directory")
        .text("main", "main.md")
        .text("slug", slug.to_string())
        .part(
            "file",
            reqwest::multipart::Part::bytes(b"# Main".to_vec()).file_name("main.md"),
        );
    for (path, bytes) in extra {
        form = form.part(
            "file",
            reqwest::multipart::Part::bytes(bytes).file_name(path.to_string()),
        );
    }
    let response = client()
        .post(format!("{}/api/documents", server.url))
        .header("cookie", session_as(TEST_PUBLISHER))
        .header("x-komodoc-client", "1")
        .multipart(form)
        .send()
        .await
        .unwrap();
    (response.status().as_u16(), response.json().await.unwrap())
}

/// R10: a creation whose checkpoint cannot land must not report success. The
/// review's version of this probe published `ONLY IN RAM` while `history/`
/// writes were refused and asserted the (buggy) 201; here a failed checkpoint
/// must fail the request and leave nothing behind for a restarted server to
/// find.
#[tokio::test]
async fn review_creation_saves_source_or_reports_failure() {
    let dir = tempfile::tempdir().unwrap();
    let inner: std::sync::Arc<dyn BlobStore> = std::sync::Arc::new(FsStore::new(dir.path()));
    let hooked = HookStore::new(inner.clone());
    let (url, _server) = server_over_blobs_legacy(hooked.clone(), Configuration::default()).await;
    *hooked.fail.lock().unwrap() = Some("history/".into());
    let (status, _entry) = post(
        &url,
        "/api/documents",
        json!({"title": "Unsaved", "source": "ONLY IN RAM", "source_format": "markdown"}),
    )
    .await;
    assert!(
        (500..600).contains(&status),
        "a creation whose checkpoint could not land must not report success, got {status}"
    );
    *hooked.fail.lock().unwrap() = None;
    let (_url, restarted) = server_over_blobs_legacy(inner, Configuration::default()).await;
    assert!(
        restarted.store.list().await.is_empty(),
        "a failed creation must not leave a half-published document behind"
    );
}

/// R11: a directory whose middle file breaks a rule must be refused whole,
/// naming the offending file, and must leave the room with no partial tree.
#[tokio::test]
async fn review_directory_rejects_whole_on_oversized_figure() {
    let config = Configuration {
        max_asset: 10,
        ..Configuration::default()
    };
    let server = test_server_with(
        config,
        crate::auth::Policy::parse(TEST_PUBLISHER),
        crate::auth::Policy::parse("anyone"),
        true,
    )
    .await;
    let (status, entry) = directory(
        &server,
        "",
        vec![
            ("big.png", vec![1; 11]),
            ("chapter.txt", b"missing".to_vec()),
        ],
    )
    .await;
    assert_eq!(
        status, 413,
        "an oversized figure must refuse the whole upload: {entry}"
    );
    let message = text(&entry, "error");
    assert!(
        message.contains("big.png"),
        "the refusal should name the offending file: {message}"
    );
    // Nothing was ever published under this title, so there is no room and
    // no partial tree to inspect: the slug this upload would have taken
    // must simply not exist.
    assert!(server.instance.store.get("directory").await.is_none());
}

/// The other half of R11: a non-UTF-8 secondary text must also refuse the
/// whole upload rather than being silently dropped.
#[tokio::test]
async fn review_directory_rejects_whole_on_bad_utf8_chapter() {
    let server = new_test_server().await;
    let (status, entry) =
        directory(&server, "", vec![("chapter.txt", vec![0xff, 0xfe, 0xfd])]).await;
    assert_eq!(
        status, 400,
        "a non-UTF-8 chapter must refuse the whole upload: {entry}"
    );
    let message = text(&entry, "error");
    assert!(
        message.contains("chapter.txt"),
        "the refusal should name the offending file: {message}"
    );
    assert!(server.instance.store.get("directory").await.is_none());
}

/// R12: republishing a directory must reconcile the whole thing -- a changed
/// chapter, a new file and a removed file -- as one operation. A chapter the
/// republish resends unchanged must be edited in place rather than deleted
/// and recreated, which is what lets a concurrent editor's caret and
/// unrelated keystrokes in it survive the same way a single-file publish's
/// diff into the main text always has.
#[tokio::test]
async fn review_directory_republish_applies_whole_directory() {
    let server = new_test_server().await;
    let (status, entry) = directory(
        &server,
        "",
        vec![
            ("chapter.txt", b"old".to_vec()),
            ("untouched.txt", b"same".to_vec()),
            ("gone.txt", b"doomed".to_vec()),
        ],
    )
    .await;
    assert_eq!(status, 201);
    let slug = text(&entry, "slug");
    let room = server.instance.rooms.get(&slug).await;
    let untouched_id_before = {
        let state = room.state.lock().await;
        session::paths_of(&state.session.doc)
            .into_iter()
            .find(|(_, path)| path == "untouched.txt")
            .map(|(id, _)| id)
            .expect("untouched.txt exists after the first publish")
    };

    let (status, _) = directory(
        &server,
        &slug,
        vec![
            ("chapter.txt", b"NEW".to_vec()),
            ("untouched.txt", b"same".to_vec()),
            ("new.txt", b"ADDED".to_vec()),
        ],
    )
    .await;
    assert_eq!(status, 201);

    let state = room.state.lock().await;
    let texts = session::texts_of(&state.session.doc);
    assert_eq!(texts.get("chapter.txt").map(String::as_str), Some("NEW"));
    assert_eq!(texts.get("new.txt").map(String::as_str), Some("ADDED"));
    assert!(
        !texts.contains_key("gone.txt"),
        "a file left out of the republish must be removed"
    );
    assert_eq!(texts.get("untouched.txt").map(String::as_str), Some("same"));
    let untouched_id_after = session::paths_of(&state.session.doc)
        .into_iter()
        .find(|(_, path)| path == "untouched.txt")
        .map(|(id, _)| id)
        .expect("untouched.txt still exists after the republish");
    assert_eq!(
        untouched_id_before, untouched_id_after,
        "a chapter the republish resends unchanged must keep its identity, not be deleted and remade"
    );
}

/// R25: a figure well within the per-file allowance but over Axum's hidden 2
/// MiB multipart default must be accepted, and a request over the derived
/// ceiling must be refused with 413.
#[tokio::test]
async fn review_directory_body_limit_honours_figure_allowance() {
    let server = new_test_server().await;
    let (status, entry) =
        directory(&server, "", vec![("plot.png", vec![1; 3 * 1024 * 1024])]).await;
    assert_eq!(
        status, 201,
        "a 3 MiB figure under the 8 MiB per-file allowance must not be refused: {entry}"
    );

    let config = Configuration::default();
    let ceiling = config.max_document + config.max_assets.max(0) as usize + (1 << 20);
    let (status, _) = directory(
        &server,
        "",
        vec![("huge.png", vec![1; ceiling + (1 << 20)])],
    )
    .await;
    assert_eq!(
        status, 413,
        "a body over the derived ceiling must be refused as too large"
    );
}

/// A one-file publish over a directory document -- the JSON body
/// `komodoc publish paper.md` sends, which names no main -- is a new version
/// of the main file and nothing more. It has never emptied the directory it
/// was published over, and the whole-directory reconciliation a multipart
/// republish now does must not change that.
#[tokio::test]
async fn a_one_file_publish_over_a_directory_keeps_the_other_files() {
    let server = new_test_server().await;
    let (status, entry) = directory(&server, "", vec![("chapter.txt", b"kept".to_vec())]).await;
    assert_eq!(status, 201);
    let slug = text(&entry, "slug");

    let (status, _) = post_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        "/api/documents",
        json!({
            "title": "Directory", "slug": slug,
            "source": "# Main, revised", "source_format": "markdown",
        }),
    )
    .await;
    assert_eq!(status, 201);

    let room = server.instance.rooms.get(&slug).await;
    let state = room.state.lock().await;
    let texts = session::texts_of(&state.session.doc);
    assert_eq!(
        texts.get("main.md").map(String::as_str),
        Some("# Main, revised")
    );
    assert_eq!(texts.get("chapter.txt").map(String::as_str), Some("kept"));
}

/// R01: the inverse of the review's `review_revoked_socket_can_read_and_write`.
/// Alice publishes a document, grants Bob editor access, and Bob connects;
/// once Alice revokes him his socket must be closed rather than go on
/// answering `y-open` with the state and accepting `y-update`, and the
/// document itself must be untouched.
#[tokio::test]
async fn review_revoked_socket_is_closed_and_stops_writing() {
    let server = test_server_with(
        Configuration::default(),
        crate::auth::Policy::parse("any"),
        crate::auth::Policy::parse("anyone"),
        true,
    )
    .await;
    let (status, entry) = post_as(
        &session_as("alice"),
        &server.url,
        "/api/documents",
        json!({"title": "Private", "source": "secret", "source_format": "markdown"}),
    )
    .await;
    assert_eq!(status, 201);
    let slug = text(&entry, "slug");
    let share = format!("/api/documents/{slug}/share");
    // A legacy grant, the only way a document names anybody by hand any more:
    // the route above no longer makes one, so this test writes it straight
    // into the index the way one made before links existed would still sit.
    server
        .instance
        .store
        .modify(&slug, |entry| {
            entry.editors.push(crate::store::Grant {
                id: "github:bob".into(),
                login: "bob".into(),
                since: crate::clock::timestamp(),
                name: "bob".into(),
            });
            Ok(())
        })
        .await
        .expect("the grant is recorded");
    let mut socket = dial_websocket_with(
        &server.url,
        &slug,
        &format!("Cookie: {}\r\n", session_as("bob")),
    )
    .await
    .unwrap();
    assert_eq!(socket.read().await["type"], "hello");

    assert_eq!(
        post_as(
            &session_as("alice"),
            &server.url,
            &share,
            json!({"revoke": "bob"})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        get_json_as(
            &session_as("bob"),
            &server.url,
            &format!("/api/documents/{slug}")
        )
        .await
        .0,
        404
    );

    expect_close(&mut socket).await;

    // Confidentiality is moot once the socket is shut, but integrity is the
    // other half of R01: nothing the revoked socket could still have sent
    // must have landed.
    assert_eq!(
        server.instance.rooms.get(&slug).await.source().await,
        "secret"
    );
}

/// R01: revoking the read link a reader came in on must close their
/// already-open socket, the same way revoking a named grant does.
#[tokio::test]
async fn review_revoking_the_read_link_closes_open_sockets() {
    let server = test_server_with(
        Configuration::default(),
        crate::auth::Policy::parse("any"),
        crate::auth::Policy::parse("anyone"),
        true,
    )
    .await;
    let (status, entry) = post_as(
        &session_as("alice"),
        &server.url,
        "/api/documents",
        json!({"title": "Doc", "source": "hello", "source_format": "markdown"}),
    )
    .await;
    assert_eq!(status, 201);
    let slug = text(&entry, "slug");
    // The read link publishing minted, which is what a reader holds.
    let key = text(&entry, "share_url")
        .rsplit("#k=")
        .next()
        .unwrap_or_default()
        .to_string();
    assert!(!key.is_empty(), "{entry}");
    let mut socket = dial_websocket_with(
        &server.url,
        &slug,
        &format!(
            "Cookie: {}\r\n{}: {key}\r\n",
            session_as("bob"),
            crate::server::LINK_HEADER
        ),
    )
    .await
    .unwrap();
    assert_eq!(socket.read().await["type"], "hello");

    let share = format!("/api/documents/{slug}/share");
    assert_eq!(
        post_as(
            &session_as("alice"),
            &server.url,
            &share,
            json!({"revoke": "reader"})
        )
        .await
        .0,
        200
    );

    expect_close(&mut socket).await;
}

/// Link metadata must be retired even when the account behind the socket can
/// still read and comment through a legacy named grant. Otherwise the socket
/// would keep charging actions to a revoked link's cached budget forever.
#[tokio::test]
async fn revoking_a_link_closes_a_named_commenters_keyed_socket() {
    let server = test_server_with(
        Configuration::default(),
        crate::auth::Policy::parse("any"),
        crate::auth::Policy::parse("anyone"),
        true,
    )
    .await;
    let (status, entry) = post_as(
        &session_as("alice"),
        &server.url,
        "/api/documents",
        json!({"title": "Doc", "source": "hello", "source_format": "markdown"}),
    )
    .await;
    assert_eq!(status, 201);
    let slug = text(&entry, "slug");
    let key = text(&entry, "share_url")
        .rsplit("#k=")
        .next()
        .unwrap_or_default()
        .to_string();
    server
        .instance
        .store
        .modify(&slug, |entry| {
            entry.commenters.push(crate::store::Grant {
                id: "github:bob".into(),
                login: "bob".into(),
                since: crate::clock::timestamp(),
                name: "Bob".into(),
            });
            Ok(())
        })
        .await
        .expect("the legacy grant is recorded");

    let mut socket = dial_websocket_with(
        &server.url,
        &slug,
        &format!(
            "Cookie: {}\r\n{}: {key}\r\n",
            session_as("bob"),
            crate::server::LINK_HEADER
        ),
    )
    .await
    .unwrap();
    assert_eq!(socket.read().await["type"], "hello");

    assert_eq!(
        post_as(
            &session_as("alice"),
            &server.url,
            &format!("/api/documents/{slug}/share"),
            json!({"revoke": "reader"})
        )
        .await
        .0,
        200
    );
    // Bob remains a commenter, but only a freshly authorized unkeyed socket
    // may now act as one.
    assert_eq!(
        get_json_as(
            &session_as("bob"),
            &server.url,
            &format!("/api/documents/{slug}")
        )
        .await
        .0,
        200
    );
    expect_close(&mut socket).await;
}

/// R01: transferring a document away must close the former owner's own
/// socket -- a transfer leaves them named on the document only if they
/// happen to also hold a grant, so their rung drops from owner to whatever a
/// stranger gets.
#[tokio::test]
async fn review_transfer_closes_former_owners_socket() {
    let server = test_server_with(
        Configuration::default(),
        crate::auth::Policy::parse("any"),
        crate::auth::Policy::parse("anyone"),
        true,
    )
    .await;
    let (status, entry) = post_as(
        &session_as("alice"),
        &server.url,
        "/api/documents",
        json!({"title": "Doc", "source": "hello", "source_format": "markdown"}),
    )
    .await;
    assert_eq!(status, 201);
    let slug = text(&entry, "slug");
    let mut socket = dial_websocket_with(
        &server.url,
        &slug,
        &format!("Cookie: {}\r\n", session_as("alice")),
    )
    .await
    .unwrap();
    assert_eq!(socket.read().await["type"], "hello");

    let (status, _) = post_as(
        &session_as("alice"),
        &server.url,
        &format!("/api/documents/{slug}/transfer"),
        json!({"to": "charlie"}),
    )
    .await;
    assert_eq!(status, 200);

    expect_close(&mut socket).await;
}

/// R34: `send_to_all`/`persist` remove a peer from the room's socket list
/// when it cannot take another frame, on the assumption that it reconnects.
/// This reproduces exactly that outcome -- a socket dropped from the room
/// while its `run_socket` task is still alive -- without needing to actually
/// overrun a channel and a kernel socket buffer, and checks the fix: the
/// connection must actually be closed, not merely un-broadcast-to, and the
/// room must go on working for someone else afterwards.
#[tokio::test]
async fn review_peer_dropped_by_the_room_gets_its_socket_closed() {
    let server = new_test_server().await;
    let (status, entry) = post_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        "/api/documents",
        json!({"title": "Room", "source": "start", "source_format": "markdown"}),
    )
    .await;
    assert_eq!(status, 201);
    let slug = text(&entry, "slug");

    let mut victim = dial_websocket_with(
        &server.url,
        &slug,
        &format!("Cookie: {}\r\n", session_as(TEST_PUBLISHER)),
    )
    .await
    .unwrap();
    assert_eq!(victim.read().await["type"], "hello");

    let room = server.instance.rooms.get(&slug).await;
    let socket_id = {
        let state = room.state.lock().await;
        *state
            .sockets
            .keys()
            .next()
            .expect("the victim is attached to the room")
    };
    // What `send_to_all`/`persist` already do to a peer whose outgoing queue
    // is too full to take another frame: drop it from the room's socket
    // list, on the assumption that it will reconnect.
    room.state.lock().await.sockets.remove(&socket_id);

    expect_close(&mut victim).await;

    // The room must still work for a fresh connection: its update must be
    // applied, proving the room was not left stuck by the dropped peer.
    let mut fresh = dial_websocket_with(
        &server.url,
        &slug,
        &format!("Cookie: {}\r\n", session_as(TEST_PUBLISHER)),
    )
    .await
    .unwrap();
    assert_eq!(fresh.read().await["type"], "hello");
    fresh.write(json!({"type": "y-open"})).await;
    let state_msg = fresh.read().await;
    assert_eq!(state_msg["type"], "y-state");
    let doc = session::new_doc();
    session::apply_update(
        &doc,
        &room::decode_update(&text(&state_msg, "update")).unwrap(),
    )
    .unwrap();
    let before = session::encode_vector(&doc);
    session::replace_text(&doc, "FRESH WRITE", "main.md");
    fresh
        .write(json!({
            "type": "y-update",
            "update": room::encode_update(&session::encode_diff(&doc, &before).unwrap()),
            "seq": 1,
        }))
        .await;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if room.source().await == "FRESH WRITE" {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
