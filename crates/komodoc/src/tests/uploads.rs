//! Publishing a directory: an upload is taken whole or refused whole, and a
//! later publish of one file keeps the rest.

use std::sync::Mutex;

use serde_json::json;

use super::*;
use crate::config::Configuration;
use crate::document::session;
use crate::storage::blob::{BlobError, BlobInfo, BlobResult, BlobStore, BlobVersion, FsStore};

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
async fn creation_saves_source_or_reports_failure() {
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
