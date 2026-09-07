//! The seam every store goes through. What is tested here is the contract
//! itself -- an absent object, a conditional write, a listing -- because two
//! implementations have to agree on it.

use std::sync::Arc;
use std::time::Duration;

use crate::blob::{
    clear_storage, document_key, document_prefix, legacy_source_key, room_key, room_lock_key,
    source_key, take_room_lease, version_of, BlobError, BlobStore, FsStore, RoomLock, INDEX_KEY,
    LEASE_GUARD_SECONDS, LOCK_STALE_SECONDS,
};
use crate::clock::{format_unix, now_unix};
use crate::storage::migrate_legacy_source;

#[tokio::test]
async fn blob_store_contract() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = FsStore::new(dir.path());

    assert!(matches!(
        blobs.get("documents/absent/x.html").await,
        Err(BlobError::NotFound)
    ));

    blobs
        .put(
            &document_key("a-paper", "abc"),
            b"<p>hello</p>".to_vec(),
            "text/html",
        )
        .await
        .unwrap();
    let (body, at) = blobs
        .get_versioned(&document_key("a-paper", "abc"))
        .await
        .unwrap();
    assert_eq!(body, b"<p>hello</p>");
    assert!(!at.is_empty(), "a stored object has no version");

    // A version has the property the index depends on: it changes when the
    // content does, and only then.
    blobs
        .put(
            &document_key("a-paper", "abc"),
            b"<p>hello</p>".to_vec(),
            "text/html",
        )
        .await
        .unwrap();
    let (_, again) = blobs
        .get_versioned(&document_key("a-paper", "abc"))
        .await
        .unwrap();
    assert_eq!(again, at, "rewriting the same bytes changed the version");

    // Listing is by prefix, and says nothing about what is outside it.
    blobs
        .put(
            &source_key("a-paper", "sha1"),
            b"# hello".to_vec(),
            "text/plain",
        )
        .await
        .unwrap();
    let found = blobs.list(&document_prefix("a-paper")).await.unwrap();
    assert!(
        found.len() == 1 && found[0].key == document_key("a-paper", "abc"),
        "{found:?}"
    );

    // Prefixes keep their ordinary string semantics, including a partial last
    // path component. The optimized walk may start below the store root, but
    // it must not turn `documents/a-paper` into an exact-directory match.
    blobs
        .put("documents/a-paper-copy/one", b"copy".to_vec(), "")
        .await
        .unwrap();
    blobs
        .put("documents/a-pap/one", b"other".to_vec(), "")
        .await
        .unwrap();
    let partial = blobs.list("documents/a-paper").await.unwrap();
    assert_eq!(
        partial
            .iter()
            .map(|item| item.key.as_str())
            .collect::<Vec<_>>(),
        vec!["documents/a-paper-copy/one", "documents/a-paper/abc.html",]
    );

    // A trailing separator scopes the walk to that subtree. Empty and absent
    // prefixes remain useful for maintenance and are ordinary empty listings.
    let subtree = blobs.list("documents/a-paper/").await.unwrap();
    assert_eq!(subtree.len(), 1);
    assert!(blobs.list("missing/").await.unwrap().is_empty());
    assert!(blobs.list("/").await.unwrap().is_empty());
    assert!(blobs.list("../").await.unwrap().is_empty());
    assert!(blobs.list("").await.unwrap().len() >= 4);

    // A legacy object whose name is also a prefix is a file, not a directory;
    // probing its slash-qualified descendants is an empty prefix query.
    blobs.put("legacy", b"old".to_vec(), "").await.unwrap();
    assert!(blobs.list("legacy/").await.unwrap().is_empty());

    // Deleting something that is not there is the outcome asked for, not an
    // error: callers delete a source that may never have existed.
    blobs
        .delete(&[source_key("never-published", "sha1")])
        .await
        .unwrap();
}

// The compare-and-swap the index rides on. Everything else in the store is a
// plain write; this is the one operation whose failure loses a document.
#[tokio::test]
async fn swap_is_conditional() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = FsStore::new(dir.path());

    // The empty version means "only if it does not exist".
    let first = blobs
        .swap(INDEX_KEY, br#"{"a":1}"#.to_vec(), "")
        .await
        .unwrap();
    assert!(matches!(
        blobs.swap(INDEX_KEY, br#"{"b":2}"#.to_vec(), "").await,
        Err(BlobError::Conflict)
    ));

    // The wrong version is refused, and leaves the object alone.
    assert!(matches!(
        blobs
            .swap(INDEX_KEY, br#"{"c":3}"#.to_vec(), "\"nonsense\"")
            .await,
        Err(BlobError::Conflict)
    ));
    let (body, _) = blobs.get_versioned(INDEX_KEY).await.unwrap();
    assert_eq!(body, br#"{"a":1}"#, "a refused write changed the object");

    // And the right one goes through.
    blobs
        .swap(INDEX_KEY, br#"{"d":4}"#.to_vec(), &first)
        .await
        .expect("a write against the current version was refused");
}

// Two writers racing for the index must not both believe they won, whichever
// of them the runtime happens to schedule first.
#[tokio::test]
async fn swap_under_contention() {
    let dir = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(dir.path()));
    // The starting bytes are distinct from every racer's, so a racer that
    // happened to write the same content -- and so leave the version
    // unchanged -- cannot make a second writer look like a winner.
    let at = blobs.swap(INDEX_KEY, b"start".to_vec(), "").await.unwrap();

    let mut racers = Vec::new();
    for n in 0..8 {
        let blobs = blobs.clone();
        let at = at.clone();
        racers.push(tokio::spawn(async move {
            blobs
                .swap(INDEX_KEY, format!("racer {n}").into_bytes(), &at)
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
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(dir.path()));
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
// "nothing" means komodoc's own keys. Whatever else is in there is not ours
// to remove -- that is the whole difference between a bucket we made and a
// bucket we were lent.
#[tokio::test]
async fn clearing_leaves_what_is_not_ours() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = FsStore::new(dir.path());
    for key in [
        INDEX_KEY.to_string(),
        document_key("a", "1"),
        source_key("a", "1"),
        room_key("a"),
    ] {
        blobs.put(&key, b"ours".to_vec(), "").await.unwrap();
    }
    blobs
        .put("someone-elses/backup.tar", b"theirs".to_vec(), "")
        .await
        .unwrap();

    clear_storage(&blobs).await;

    assert!(
        matches!(blobs.get(INDEX_KEY).await, Err(BlobError::NotFound)),
        "the index survived a clear"
    );
    assert_eq!(
        blobs.get("someone-elses/backup.tar").await.unwrap(),
        b"theirs",
        "clearing removed something that was not ours"
    );
}

// The source used to sit inside the document's directory and now has a key of
// its own. A store written by the old layout keeps working, and is moved once.
#[tokio::test]
async fn legacy_source_is_migrated() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = FsStore::new(dir.path());
    blobs
        .put("documents/a-paper/source.txt", b"# was here".to_vec(), "")
        .await
        .unwrap();

    assert_eq!(migrate_legacy_source(&blobs).await, 1);
    assert_eq!(
        blobs.get(&legacy_source_key("a-paper")).await.unwrap(),
        b"# was here"
    );
    assert!(
        matches!(
            blobs.get("documents/a-paper/source.txt").await,
            Err(BlobError::NotFound)
        ),
        "copied but not moved"
    );
    // And running again does nothing, which is what makes it safe at startup.
    assert_eq!(migrate_legacy_source(&blobs).await, 0);
}

// A key that escaped the directory would be a serious thing to get wrong.
#[tokio::test]
async fn a_key_cannot_escape_the_directory() {
    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().parent().unwrap().join("komodoc-escaped");
    let _ = std::fs::remove_file(&outside);
    let blobs = FsStore::new(dir.path().join("data"));
    let _ = blobs
        .put("../../komodoc-escaped", b"escaped".to_vec(), "")
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

    let blobs = Arc::new(FsStore::new(dir.path()));
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
/* ----------------------------------------------------------- room leases */

// Two servers on one bucket must not both write the same room: the in-memory
// copy is authoritative while anyone is connected, so the second would save
// over the first's document without either noticing.
#[tokio::test]
async fn a_room_is_held_by_one_server() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = FsStore::new(dir.path());
    let first = take_room_lease(&blobs, "a-paper", "server-one", None).await;
    assert!(first.held, "the first server could not take the lease");
    let second = take_room_lease(&blobs, "a-paper", "server-two", None).await;
    assert!(!second.held, "a second server took a lease the first holds");
    assert_eq!(
        second.holder, "server-one",
        "the refusal named the wrong holder"
    );
    // The holder may say so again: renewing is not contention, and keeps the
    // epoch, because the lease has not changed hands.
    let renewed = take_room_lease(&blobs, "a-paper", "server-one", Some(first.epoch)).await;
    assert!(renewed.held, "the holder could not renew its own lease");
    assert_eq!(renewed.epoch, first.epoch, "renewing raised the epoch");
}

// A lease whose holder is gone is taken over -- and taking it over raises the
// epoch, which is what fences the old holder out.
#[tokio::test]
async fn a_stale_lease_is_taken_over_and_raises_the_epoch() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = FsStore::new(dir.path());
    let old = RoomLock {
        holder: "server-that-died".into(),
        taken: format_unix(now_unix() - 2 * LOCK_STALE_SECONDS),
        epoch: 7,
    };
    blobs
        .put(
            &room_lock_key("a-paper"),
            serde_json::to_vec(&old).unwrap(),
            "",
        )
        .await
        .unwrap();
    let taken = take_room_lease(&blobs, "a-paper", "server-two", None).await;
    assert!(
        taken.held,
        "a lease whose holder is long gone was not taken over"
    );
    assert_eq!(taken.epoch, 8, "taking over did not raise the epoch");

    // The server that died coming back is the case the epoch exists for: its
    // renewal asserts the epoch it remembers, and is refused.
    let stale = take_room_lease(&blobs, "a-paper", "server-that-died", Some(7)).await;
    assert!(
        !stale.held,
        "a former holder renewed a lease that had moved on without it"
    );
    assert_eq!(stale.epoch, 8);
}

// A lease is not a promise about the future: it is only good until a guard's
// width before it could be taken, so a holder stops writing strictly before
// anybody else could start.
#[test]
fn a_lease_stops_being_safe_before_it_can_be_taken() {
    let lease = crate::blob::Lease {
        held: true,
        holder: "server-one".into(),
        epoch: 1,
        taken_at: 1_000,
        verified: true,
    };
    assert_eq!(
        lease.safe_until(),
        1_000 + LOCK_STALE_SECONDS - LEASE_GUARD_SECONDS
    );
    assert!(
        lease.safe_until() < 1_000 + LOCK_STALE_SECONDS,
        "a holder trusts its lease right up to the moment it can be taken"
    );
}

#[test]
fn a_version_is_the_digest_of_the_bytes() {
    assert_eq!(version_of(b"a"), version_of(b"a"));
    assert_ne!(version_of(b"a"), version_of(b"b"));
}
