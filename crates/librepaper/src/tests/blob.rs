//! The seam every store goes through. What is tested here is the contract
//! itself -- an absent object, a conditional write, a listing -- because two
//! implementations have to agree on it.

use std::sync::Arc;
use std::time::Duration;

use crate::storage::blob::{
    clear_storage_checked, v2_object_key, version_of, BlobError, BlobStore, FsStore, ObjectId,
};

#[tokio::test]
async fn blob_store_contract() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = FsStore::new(dir.path(), true);

    assert!(matches!(
        blobs.get("items/absent/x.html").await,
        Err(BlobError::NotFound)
    ));

    blobs
        .put(
            "fixtures/body/a-paper/abc",
            b"<p>hello</p>".to_vec(),
            "text/html",
        )
        .await
        .unwrap();
    let (body, at) = blobs
        .get_versioned("fixtures/body/a-paper/abc")
        .await
        .unwrap();
    assert_eq!(body, b"<p>hello</p>");
    assert!(!at.is_empty(), "a stored object has no version");

    // A version has the property conditional writes depend on: it changes when the
    // content does, and only then.
    blobs
        .put(
            "fixtures/body/a-paper/abc",
            b"<p>hello</p>".to_vec(),
            "text/html",
        )
        .await
        .unwrap();
    let (_, again) = blobs
        .get_versioned("fixtures/body/a-paper/abc")
        .await
        .unwrap();
    assert_eq!(again, at, "rewriting the same bytes changed the version");

    // Listing is by prefix, and says nothing about what is outside it.
    blobs
        .put(
            "fixtures/source/a-paper/sha1",
            b"# hello".to_vec(),
            "text/plain",
        )
        .await
        .unwrap();
    let found = blobs.list("fixtures/body/a-paper/").await.unwrap();
    assert!(
        found
            .iter()
            .any(|item| item.key == "fixtures/body/a-paper/abc"),
        "{found:?}"
    );

    // Prefixes keep their ordinary string semantics, including a partial last
    // path component. The optimized walk may start below the store root, but
    // it must not turn `items/a-paper` into an exact-directory match.
    blobs
        .put("items/a-paper-copy/one", b"copy".to_vec(), "")
        .await
        .unwrap();
    blobs
        .put("items/a-pap/one", b"other".to_vec(), "")
        .await
        .unwrap();
    let partial = blobs.list("items/a-paper").await.unwrap();
    assert_eq!(
        partial
            .iter()
            .map(|item| item.key.as_str())
            .collect::<Vec<_>>(),
        vec!["items/a-paper-copy/one"]
    );
    let first = blobs.list_page("items/", None, 2).await.unwrap();
    assert_eq!(first.len(), 2);
    assert!(first[0].key < first[1].key);
    let second = blobs
        .list_page("items/", Some(&first[1].key), 2)
        .await
        .unwrap();
    assert!(second.iter().all(|item| item.key > first[1].key));

    // A trailing separator scopes the walk to that subtree. Empty and absent
    // prefixes remain useful for maintenance and are ordinary empty listings.
    let subtree = blobs.list("items/a-paper/").await.unwrap();
    assert!(subtree.is_empty());
    assert!(blobs.list("missing/").await.unwrap().is_empty());
    assert!(blobs.list("/").await.unwrap().is_empty());
    assert!(blobs.list("../").await.unwrap().is_empty());
    assert!(blobs.list("").await.unwrap().len() >= 4);

    // An object whose name is also a prefix is a file, not a directory;
    // probing its slash-qualified descendants is an empty prefix query.
    blobs.put("standalone", b"old".to_vec(), "").await.unwrap();
    assert!(blobs.list("standalone/").await.unwrap().is_empty());

    // Deleting something that is not there is the outcome asked for, not an
    // error: callers delete a source that may never have existed.
    blobs
        .delete(&["fixtures/absent/sha1".to_string()])
        .await
        .unwrap();
}

// A conditional write preserves the existing object when its version mismatches.
#[tokio::test]
async fn swap_is_conditional() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = FsStore::new(dir.path(), true);

    // The empty version means "only if it does not exist".
    let first = blobs
        .swap("conditional/object", br#"{"a":1}"#.to_vec(), "")
        .await
        .unwrap();
    assert!(matches!(
        blobs
            .swap("conditional/object", br#"{"b":2}"#.to_vec(), "")
            .await,
        Err(BlobError::Conflict)
    ));

    // The wrong version is refused, and leaves the object alone.
    assert!(matches!(
        blobs
            .swap("conditional/object", br#"{"c":3}"#.to_vec(), "\"nonsense\"")
            .await,
        Err(BlobError::Conflict)
    ));
    let (body, _) = blobs.get_versioned("conditional/object").await.unwrap();
    assert_eq!(body, br#"{"a":1}"#, "a refused write changed the object");

    // And the right one goes through.
    blobs
        .swap("conditional/object", br#"{"d":4}"#.to_vec(), &first)
        .await
        .expect("a write against the current version was refused");
}

// Two writers racing for one object must not both believe they won, whichever
// of them the runtime happens to schedule first.
#[tokio::test]
async fn swap_under_contention() {
    let dir = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(dir.path(), true));
    // The starting bytes are distinct from every racer's, so a racer that
    // happened to write the same content -- and so leave the version
    // unchanged -- cannot make a second writer look like a winner.
    let at = blobs
        .swap("conditional/object", b"start".to_vec(), "")
        .await
        .unwrap();

    let mut racers = Vec::new();
    for n in 0..8 {
        let blobs = blobs.clone();
        let at = at.clone();
        racers.push(tokio::spawn(async move {
            blobs
                .swap("conditional/object", format!("racer {n}").into_bytes(), &at)
                .await
                .is_ok()
        }));
    }
    let mut wins = 0;
    for racer in racers {
        if racer.await.unwrap() {
            wins += 1;
        }
    }
    assert_eq!(wins, 1, "{wins} writers thought they won; want exactly one");
}

// Plain puts are allowed to overlap. Atomic replacement must therefore use a
// private temporary pathname per operation, or one writer can rename another
// writer's temporary file and leave the loser with a missing-temp error.
#[tokio::test]
async fn puts_under_contention_leave_one_complete_value() {
    let dir = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(dir.path(), true));
    let mut writers = Vec::new();
    for n in 0..32 {
        let blobs = blobs.clone();
        writers.push(tokio::spawn(async move {
            let body = vec![b'a' + (n % 2) as u8; 128 * 1024];
            blobs.put("same/key", body, "").await
        }));
    }
    for writer in writers {
        writer.await.unwrap().unwrap();
    }
    let body = blobs.get("same/key").await.unwrap();
    assert!(
        body == vec![b'a'; 128 * 1024] || body == vec![b'b'; 128 * 1024],
        "a concurrent put left a mixed or partial value"
    );
}

// Seeding starts from nothing, and on a bucket somebody else supplied,
// "nothing" means librepaper's own keys. Whatever else is in there is not ours
// to remove -- that is the whole difference between a bucket we made and a
// bucket we were lent.
#[tokio::test]
async fn clearing_leaves_what_is_not_ours() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = FsStore::new(dir.path(), true);
    let keys = [
        "0123456789abcdef0123456789abcdef",
        "fedcba9876543210fedcba9876543210",
    ]
    .map(|id| v2_object_key("native-document", &ObjectId::parse(id).unwrap()).unwrap());
    for key in &keys {
        blobs.put(key, b"ours".to_vec(), "").await.unwrap();
    }
    blobs
        .put("someone-elses/backup.tar", b"theirs".to_vec(), "")
        .await
        .unwrap();

    clear_storage_checked(&blobs).await.unwrap();

    for key in &keys {
        assert!(
            matches!(blobs.get(key).await, Err(BlobError::NotFound)),
            "native object survived a clear: {key}"
        );
    }
    assert_eq!(
        blobs.get("someone-elses/backup.tar").await.unwrap(),
        b"theirs",
        "clearing removed something that was not ours"
    );
}

// A key that escaped the directory would be a serious thing to get wrong.
#[tokio::test]
async fn a_key_cannot_escape_the_directory() {
    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().parent().unwrap().join("librepaper-escaped");
    let _ = std::fs::remove_file(&outside);
    let blobs = FsStore::new(dir.path().join("data"), true);
    let _ = blobs
        .put("../../librepaper-escaped", b"escaped".to_vec(), "")
        .await;
    assert!(
        !outside.exists(),
        "a key wrote outside the storage directory"
    );
}

// A FIFO gives this test a deterministic filesystem operation that cannot
// complete until another thread opens it. On a current-thread Tokio runtime,
// synchronous `std::fs::read` would prevent the ticker from running at all.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn filesystem_reads_do_not_block_tokio() {
    use std::ffi::CString;
    use std::os::raw::c_char;
    use std::thread;
    use std::time::Instant;

    unsafe extern "C" {
        fn mkfifo(path: *const c_char, mode: u32) -> i32;
    }

    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("pending");
    let fifo_name = CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { mkfifo(fifo_name.as_ptr(), 0o600) }, 0);

    let blobs = Arc::new(FsStore::new(dir.path(), true));
    let reader = tokio::spawn({
        let blobs = blobs.clone();
        async move { blobs.get("pending").await }
    });
    // Delay the writer so the read is definitely pending while the runtime
    // gets a chance to schedule the ticker.
    let writer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(150));
        std::fs::write(fifo, b"ready").unwrap();
    });

    let started = Instant::now();
    let mut ticker = tokio::time::interval(Duration::from_millis(10));
    ticker.tick().await;
    tokio::pin!(reader);
    tokio::select! {
        result = &mut reader => {
            panic!("FIFO read completed before the writer delay: {result:?}");
        }
        _ = ticker.tick() => {}
    }
    let body = tokio::time::timeout(Duration::from_secs(2), reader)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    writer.join().unwrap();
    assert_eq!(body, b"ready");
    assert!(started.elapsed() < Duration::from_secs(2));
}
#[test]
fn a_version_is_the_digest_of_the_bytes() {
    assert_eq!(version_of(b"a"), version_of(b"a"));
    assert_ne!(version_of(b"a"), version_of(b"b"));
}
