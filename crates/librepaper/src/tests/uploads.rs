//! Publishing a directory: an upload is taken whole or refused whole, and a
//! later publish of one file keeps the rest.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use serde_json::json;

use super::*;
use crate::config::Configuration;
use crate::document::session;
use crate::storage::blob::{BlobError, BlobInfo, BlobResult, BlobStore, BlobVersion, FsStore};

/// A store that can be told to fail every write under one key prefix. What
/// `creation_saves_source_or_reports_failure` uses to put immutable
/// checkpoint object writes out of reach, without touching
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
    let response = directory_response(server, slug, extra).await;
    (response.status().as_u16(), response.json().await.unwrap())
}

async fn directory_response(
    server: &TestServer,
    slug: &str,
    extra: Vec<(&str, Vec<u8>)>,
) -> reqwest::Response {
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
    client()
        .post(format!("{}/api/documents", server.url))
        .header("cookie", session_as(TEST_PUBLISHER))
        .header("x-librepaper-client", "1")
        .multipart(form)
        .send()
        .await
        .unwrap()
}

/// R10: a creation whose checkpoint cannot land must not report success. The
/// probe publishes `ONLY IN RAM` while immutable object writes are refused;
/// a failed checkpoint
/// must fail the request and leave nothing behind for a restarted server to
/// find.
#[tokio::test]
async fn creation_saves_source_or_reports_failure() {
    let dir = tempfile::tempdir().unwrap();
    let inner: std::sync::Arc<dyn BlobStore> =
        std::sync::Arc::new(FsStore::new(dir.path().join("objects"), true));
    let hooked = HookStore::new(inner.clone());
    let catalog = Arc::new(
        crate::storage::catalog::Catalog::open(dir.path().join("catalog.sqlite")).unwrap(),
    );
    let (url, server) =
        server_over_blobs(hooked.clone(), Configuration::default(), catalog.clone()).await;
    *hooked.fail.lock().unwrap() = Some("v2/documents/".into());
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
    drop(server);
    crate::storage::maintenance_v2::recover_v2_startup(catalog.as_ref(), inner.as_ref())
        .await
        .unwrap();
    let (_url, restarted) = server_over_blobs(inner, Configuration::default(), catalog).await;
    assert!(
        restarted.store.list().await.is_empty(),
        "a failed creation must not leave a half-published document behind"
    );
}

/// R11: a directory whose middle file breaks a rule must be refused whole,
/// naming the offending file, and must leave the room with no partial tree.
#[tokio::test]
async fn directory_rejects_whole_on_oversized_figure() {
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
async fn directory_rejects_whole_on_bad_utf8_chapter() {
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
async fn directory_republish_applies_whole_directory() {
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
async fn directory_body_limit_honours_figure_allowance() {
    let server = new_test_server().await;
    let (status, entry) =
        directory(&server, "", vec![("plot.png", vec![1; 3 * 1024 * 1024])]).await;
    assert_eq!(
        status, 201,
        "a 3 MiB figure under the 8 MiB per-file allowance must not be refused: {entry}"
    );

    let config = Configuration::default();
    let ceiling = config.max_document + config.max_assets.max(0) as usize + (1 << 20);
    // An early rejection may close the stream while the oversized request
    // is still being sent. This assertion needs the received status only.
    let response = directory_response(
        &server,
        "",
        vec![("huge.png", vec![1; ceiling + (1 << 20)])],
    )
    .await;
    assert_eq!(
        response.status().as_u16(),
        413,
        "a body over the derived ceiling must be refused as too large"
    );
}

/// A one-file publish over a directory document -- the JSON body `librepaper
/// publish paper.md` sends, which names no main -- is a new version of the
/// main file and nothing more. It has never emptied the directory it was
/// published over, and the whole-directory reconciliation a multipart
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

/// A replacement's quota preflight measures the resulting tree.  An old
/// sibling omitted by a directory upload is deleted, so it must not make the
/// replacement fail the document ceiling.
#[tokio::test]
async fn directory_replacement_does_not_count_deleted_siblings() {
    let config = Configuration {
        max_document: 100,
        ..Configuration::default()
    };
    let server = test_server_with(
        config,
        crate::auth::Policy::parse(TEST_PUBLISHER),
        crate::auth::Policy::parse("anyone"),
        true,
    )
    .await;
    let (status, first) = directory(&server, "", vec![("old.txt", vec![b'x'; 60])]).await;
    assert_eq!(status, 201, "{first}");
    let slug = text(&first, "slug");
    let (status, replacement) = directory(&server, &slug, vec![("new.txt", vec![b'y'; 60])]).await;
    assert_eq!(status, 201, "a deleted sibling was counted: {replacement}");
}

/// Checkpoint rate refusal must preserve the current head and accounting,
/// leaving no prepared publication that prevents a later retry.
#[tokio::test]
async fn failed_replacement_checkpoint_admission_clears_receipt() {
    let mut config = Configuration::default();
    config.session.checkpoint_owner_per_hour = 1;
    let server = test_server_with(
        config,
        crate::auth::Policy::parse(TEST_PUBLISHER),
        crate::auth::Policy::parse("anyone"),
        true,
    )
    .await;
    let first = publish_test_document(&server.url).await;
    let slug = text(&first, "slug");
    let catalog = server.instance.store.catalog.as_ref().unwrap();
    let current = catalog.document(&slug).unwrap().unwrap();
    let room = server.instance.rooms.get(&slug).await;
    let before_source = room.source().await;
    let rate = catalog
        .reserve_process_rate(
            &format!("account:{}", current.owner_id.as_deref().unwrap()),
            "checkpoint_owner",
            1,
        )
        .unwrap();
    let (status, body) = post(
        &server.url,
        "/api/documents",
        json!({"slug": slug, "title": "Paper", "html": "<p>replacement</p>"}),
    )
    .await;
    assert_eq!(status, 429, "{body}");
    let pending: i64 = catalog.with_connection(|connection| {
        connection.query_row("SELECT count(*) FROM operations WHERE document_id=?1 AND state='prepared' AND kind IN ('source_publish','checkpoint')", [&current.storage_id], |row| row.get(0)).map_err(crate::storage::catalog::CatalogError::from)
    }).unwrap();
    assert_eq!(
        pending, 0,
        "failed admission stranded a prepared publication"
    );
    let after = catalog.document(&slug).unwrap().unwrap();
    assert_eq!(after.sha, current.sha);
    assert_eq!(after.maintenance_reserved, current.maintenance_reserved);
    assert_eq!(room.source().await, before_source);
    assert!(catalog.audit_v2_counters().unwrap());
    drop(rate);
    let (status, retry) = post(
        &server.url,
        "/api/documents",
        json!({"slug": slug, "title": "Paper", "html": "<p>replacement</p>"}),
    )
    .await;
    assert_eq!(status, 201, "released admission must allow retry: {retry}");
}

/// Chat channels are purged only after deletion admission succeeds.  A stale
/// or missing document must not make an unrelated live channel disappear.
#[tokio::test]
async fn failed_delete_does_not_purge_chat_channel() {
    let server = new_test_server().await;
    let channel = server
        .instance
        .chat
        .create("missing-document")
        .await
        .expect("channel");
    let id = text(&channel, "id").to_string();
    let token = text(&channel, "token").to_string();
    assert!(server
        .instance
        .delete_document("missing-document")
        .await
        .is_err());
    let (sender, _receiver) = tokio::sync::mpsc::channel(1);
    assert!(server
        .instance
        .chat
        .attach("missing-document", &id, &token, "user", 1, sender)
        .await
        .is_ok());
}

struct PublicationGate {
    inner: Arc<dyn BlobStore>,
    armed: AtomicBool,
    started: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl BlobStore for PublicationGate {
    async fn get(&self, key: &str) -> BlobResult<Vec<u8>> {
        self.inner.get(key).await
    }
    async fn get_versioned(&self, key: &str) -> BlobResult<(Vec<u8>, BlobVersion)> {
        self.inner.get_versioned(key).await
    }
    async fn put(&self, key: &str, body: Vec<u8>, content_type: &str) -> BlobResult<()> {
        // Pause the first immutable checkpoint object write after admission.
        if key.starts_with("v2/documents/")
            && key.contains("/objects/")
            && self.armed.swap(false, Ordering::SeqCst)
        {
            self.started.notify_one();
            self.resume.notified().await;
            return Err(BlobError::Other("injected publication failure".into()));
        }
        self.inner.put(key, body, content_type).await
    }
    async fn swap(&self, key: &str, body: Vec<u8>, expected: &str) -> BlobResult<BlobVersion> {
        self.inner.swap(key, body, expected).await
    }
    async fn list(&self, prefix: &str) -> BlobResult<Vec<BlobInfo>> {
        self.inner.list(prefix).await
    }
    async fn delete(&self, keys: &[String]) -> BlobResult<()> {
        self.inner.delete(keys).await
    }
    fn describe(&self) -> String {
        self.inner.describe()
    }
}

/// A failed checkpoint rollback must not erase an editor update that was
/// concurrent with publication.  The publication gate makes the editor wait
/// until rollback completes, then its update is applied to the restored tree.
#[tokio::test]
async fn failed_replacement_preserves_concurrent_editor_update() {
    let dir = tempfile::tempdir().expect("temporary store");
    let gate = Arc::new(PublicationGate {
        inner: Arc::new(FsStore::new(dir.path().join("objects"), true)),
        armed: AtomicBool::new(false),
        started: tokio::sync::Notify::new(),
        resume: tokio::sync::Notify::new(),
    });
    let catalog = Arc::new(
        crate::storage::catalog::Catalog::open(dir.path().join("catalog.sqlite")).unwrap(),
    );
    let (url, server) = server_over_blobs(gate.clone(), Configuration::default(), catalog).await;
    let first = publish_test_document(&url).await;
    let slug = text(&first, "slug").to_string();
    let room = server.rooms.get(&slug).await;
    let (sender, _receiver) = tokio::sync::mpsc::channel(32);
    room.attach(99, sender, true).await;
    gate.armed.store(true, Ordering::SeqCst);

    let request = tokio::spawn({
        let url = url.clone();
        let slug = slug.clone();
        async move {
            post(
                &url,
                "/api/documents",
                json!({"slug": slug, "title": "Paper", "html": "<p>published</p>"}),
            )
            .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), gate.started.notified())
        .await
        .expect("publication reached injected failure");

    let update = {
        let state = room.state.lock().await;
        let peer = crate::document::session::new_doc();
        crate::document::session::apply_update(
            &peer,
            &crate::document::session::encode_state(&state.session.doc),
        )
        .expect("copy live state");
        let vector = crate::document::session::encode_vector(&peer);
        crate::document::session::put_text(&peer, "notes.txt", "concurrent editor work");
        crate::document::session::encode_diff(&peer, &vector).expect("encode editor update")
    };
    let editor = tokio::spawn({
        let room = room.clone();
        async move { room.receive_update(99, &update, 1, "editor").await }
    });
    // The publication owns the mutation gate while the injected checkpoint
    // is paused.  Let it finish and roll back before the queued editor edit
    // runs against the restored state.
    gate.resume.notify_one();
    let (status, failure) = request.await.expect("request task");
    assert_eq!(status, 503, "{failure}");
    assert_eq!(failure["retryable"], true);
    assert!(matches!(
        editor.await.expect("editor task"),
        crate::room::Applied::Relay
    ));

    let state = room.state.lock().await;
    let texts = crate::document::session::texts_of(&state.session.doc);
    assert_eq!(
        texts.get("notes.txt").map(String::as_str),
        Some("concurrent editor work")
    );
}

#[tokio::test]
async fn duplicate_project_names_are_refused_before_replacing_content() {
    let server = new_test_server().await;
    let first = publish_test_document(&server.url).await;
    let (status, error) = post(
        &server.url,
        "/api/documents",
        json!({"title": "  MY PAPER  ", "html": "<p>duplicate</p>"}),
    )
    .await;
    assert_eq!(status, 409, "{error}");
    assert!(text(&error, "error").contains("project with this name"));
    let (status, second) = post(
        &server.url,
        "/api/documents",
        json!({"title": "Second", "html": "<p>original</p>"}),
    )
    .await;
    assert_eq!(status, 201, "{second}");
    let second_slug = text(&second, "slug");
    let original = server.instance.store.get(&second_slug).await.unwrap();
    let (status, error) = post(
        &server.url,
        "/api/documents",
        json!({"slug": second_slug, "title": "My Paper", "html": "<p>replacement</p>"}),
    )
    .await;
    assert_eq!(status, 409, "{error}");
    let unchanged = server.instance.store.get(&second_slug).await.unwrap();
    assert_eq!(unchanged.title, "Second");
    assert_eq!(unchanged.sha, original.sha);
    let (status, result) = post(
        &server.url,
        "/api/documents",
        json!({"slug": text(&first, "slug"), "title": "My Paper", "html": "<p>updated</p>"}),
    )
    .await;
    assert_eq!(status, 201, "{result}");
}
