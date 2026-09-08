//! What a bucket does when it is not working: throttling, gateway errors,
//! connections that go away mid-request, and a batch delete that succeeds as
//! a request while refusing some of the keys in it.
//!
//! The mock here speaks HTTP over a raw socket rather than through axum,
//! because the case that matters most cannot be expressed as a response: a
//! write that the provider *applied* and then failed to acknowledge. Only
//! closing the socket after the mutation produces it.
//!
//! Every test drives a `TestClock`, so "within its bounds" is an assertion
//! about the policy rather than about how loaded the machine was, and no test
//! waits for anything.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::storage::blob::{version_of, BlobError, BlobStore, DeleteOutcome};
use crate::storage::retry::{RetryLimits, TestClock};
use crate::storage::s3::S3Store;
use crate::storage::StorageOptions;

/// What the bucket does instead of answering the next request.
#[derive(Clone, Debug)]
enum Fault {
    /// Answer with this status, and this `Retry-After` in seconds.
    Status(u16, Option<u64>),
    /// Apply the request, then drop the connection without answering: the
    /// ambiguous write.
    ApplyThenDrop,
    /// Drop the connection before applying anything.
    Drop,
}

#[derive(Default)]
struct Bucket {
    objects: HashMap<String, Vec<u8>>,
    /// Every request that arrived, as "METHOD path".
    log: Vec<String>,
    /// Consumed one per request, in order. An empty queue means "behave".
    faults: VecDeque<Fault>,
    /// How many writes actually changed an object. The assertion that a
    /// retried write is not a second mutation.
    applied: usize,
    /// Scripted `DeleteResult` bodies, one per multi-object delete request.
    batch_replies: VecDeque<String>,
    /// A gateway that does not implement multi-object deletion.
    refuse_batch: bool,
}

impl Bucket {
    fn requests(&self) -> usize {
        self.log.len()
    }

    fn methods(&self) -> Vec<String> {
        self.log
            .iter()
            .map(|line| {
                line.split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }
}

type Shared = Arc<Mutex<Bucket>>;

/// A bucket on a socket. Every connection serves one request and closes, so
/// the client sees a definite transport failure when a fault drops it.
async fn mock_bucket() -> (String, Shared) {
    let state: Shared = Arc::new(Mutex::new(Bucket::default()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    let held = state.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let held = held.clone();
            tokio::spawn(async move { serve_one(stream, held).await });
        }
    });
    (format!("http://{address}"), state)
}

async fn serve_one(mut stream: TcpStream, state: Shared) {
    let mut buffer = Vec::new();
    let head_end = loop {
        let mut chunk = [0u8; 4096];
        let read = match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(at) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).to_string();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default().to_string();
    let mut fields = request_line.split_whitespace();
    let method = fields.next().unwrap_or_default().to_string();
    let target = fields.next().unwrap_or_default().to_string();
    let (mut length, mut if_match, mut if_none_match) = (0usize, String::new(), String::new());
    let mut signed = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = value.trim().parse().unwrap_or(0),
            "if-match" => if_match = value.trim().to_string(),
            "if-none-match" => if_none_match = value.trim().to_string(),
            "authorization" => signed = value.contains("AWS4-HMAC-SHA256"),
            _ => {}
        }
    }
    while buffer.len() < head_end + length {
        let mut chunk = [0u8; 4096];
        let read = match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        buffer.extend_from_slice(&chunk[..read]);
    }
    let body = buffer[head_end..head_end + length].to_vec();
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path.to_string(), query.to_string()),
        None => (target.clone(), String::new()),
    };

    let fault = {
        let mut held = state.lock().expect("bucket");
        held.log.push(format!("{method} {target}"));
        held.faults.pop_front()
    };
    if !signed {
        let _ = stream
            .write_all(
                b"HTTP/1.1 403 Forbidden\r\nContent-Length: 8\r\nConnection: close\r\n\r\nunsigned",
            )
            .await;
        return;
    }

    let response = match fault {
        Some(Fault::Drop) => return,
        Some(Fault::ApplyThenDrop) => {
            let _ = apply(
                &state,
                &method,
                &path,
                &query,
                &body,
                &if_match,
                &if_none_match,
            );
            return;
        }
        Some(Fault::Status(status, after)) => {
            let mut head =
                format!("HTTP/1.1 {status} Whatever\r\nContent-Length: 9\r\nConnection: close\r\n");
            if let Some(seconds) = after {
                head.push_str(&format!("Retry-After: {seconds}\r\n"));
            }
            head.push_str("\r\nnot now!\n");
            head.into_bytes()
        }
        None => apply(
            &state,
            &method,
            &path,
            &query,
            &body,
            &if_match,
            &if_none_match,
        ),
    };
    let _ = stream.write_all(&response).await;
    let _ = stream.flush().await;
}

fn reply(status: u16, headers: &str, body: &[u8], with_body: bool) -> Vec<u8> {
    let mut head = format!(
        "HTTP/1.1 {status} Whatever\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
        body.len()
    );
    if !with_body {
        return head.into_bytes();
    }
    head.push_str(&String::from_utf8_lossy(body));
    head.into_bytes()
}

fn apply(
    state: &Shared,
    method: &str,
    path: &str,
    query: &str,
    body: &[u8],
    if_match: &str,
    if_none_match: &str,
) -> Vec<u8> {
    // Keys travel percent-encoded in the path and XML-escaped in a batch
    // body; the bucket stores the decoded name, as a real one does.
    let key = percent_encoding::percent_decode_str(path.trim_start_matches("/bucket/"))
        .decode_utf8_lossy()
        .to_string();
    let mut held = state.lock().expect("bucket");
    match method {
        "PUT" => {
            let current = held.objects.get(&key).map(|held| version_of(held));
            if if_none_match == "*" && current.is_some() {
                return reply(412, "", b"precondition failed", true);
            }
            if !if_match.is_empty() && current.as_deref() != Some(if_match) {
                return reply(412, "", b"precondition failed", true);
            }
            let tag = version_of(body);
            held.objects.insert(key, body.to_vec());
            held.applied += 1;
            reply(200, &format!("ETag: {tag}\r\n"), b"", true)
        }
        "GET" | "HEAD" => match held.objects.get(&key) {
            None => reply(404, "", b"no such key", method == "GET"),
            Some(content) => {
                let tag = version_of(content);
                let content = content.clone();
                reply(200, &format!("ETag: {tag}\r\n"), &content, method == "GET")
            }
        },
        "DELETE" => {
            held.objects.remove(&key);
            reply(204, "", b"", true)
        }
        "POST" if query.starts_with("delete") => {
            if held.refuse_batch {
                return reply(501, "", b"not implemented", true);
            }
            if let Some(scripted) = held.batch_replies.pop_front() {
                return reply(200, "", scripted.as_bytes(), true);
            }
            let text = String::from_utf8_lossy(body).to_string();
            let mut result = String::from("<DeleteResult>");
            let mut rest = text.as_str();
            while let Some(start) = rest.find("<Key>") {
                let after = &rest[start + 5..];
                let Some(end) = after.find("</Key>") else {
                    break;
                };
                let escaped = after[..end].to_string();
                let key = escaped
                    .replace("&lt;", "<")
                    .replace("&gt;", ">")
                    .replace("&quot;", "\"")
                    .replace("&apos;", "'")
                    .replace("&amp;", "&");
                held.objects.remove(&key);
                result.push_str(&format!("<Deleted><Key>{escaped}</Key></Deleted>"));
                rest = &after[end..];
            }
            result.push_str("</DeleteResult>");
            reply(200, "", result.as_bytes(), true)
        }
        _ => reply(405, "", b"method not allowed", true),
    }
}

fn store(endpoint: &str, clock: Arc<TestClock>) -> S3Store {
    S3Store::with_clock(
        &StorageOptions {
            endpoint: endpoint.to_string(),
            bucket: "bucket".into(),
            region: "auto".into(),
            prefix: "komodoc/".into(),
            access_key: "key".into(),
            secret_key: "secret".into(),
            ..StorageOptions::default()
        },
        clock,
    )
}

/* --------------------------------------------------------------- retries */

// Throttling is what a bucket does under load, and it is not a failure: the
// operation waits at least as long as the provider asked and then asks again.
#[tokio::test]
async fn throttling_is_retried_and_honours_the_providers_delay() {
    let (endpoint, bucket) = mock_bucket().await;
    let clock = TestClock::new(1.0);
    let blobs = store(&endpoint, clock.clone());
    blobs
        .put("thing", b"body".to_vec(), "text/plain")
        .await
        .expect("write");
    {
        let mut held = bucket.lock().expect("bucket");
        held.log.clear();
        held.faults.push_back(Fault::Status(503, Some(1)));
        held.faults.push_back(Fault::Status(429, None));
    }
    assert_eq!(blobs.get("thing").await.expect("read"), b"body");
    assert_eq!(bucket.lock().expect("bucket").requests(), 3);
    assert_eq!(
        clock.waits(),
        vec![
            std::time::Duration::from_secs(1),
            std::time::Duration::from_millis(200)
        ],
        "the provider's delay must be a floor and the backoff must grow"
    );
    assert!(clock.waited() <= RetryLimits::IDEMPOTENT.total);
}

// A gateway error before the write reached the object leaves nothing behind,
// so the same conditional write goes out again -- still conditional.
#[tokio::test]
async fn a_transient_server_error_retries_the_same_conditional_write() {
    let (endpoint, bucket) = mock_bucket().await;
    let clock = TestClock::new(0.5);
    let blobs = store(&endpoint, clock.clone());
    bucket
        .lock()
        .expect("bucket")
        .faults
        .push_back(Fault::Status(500, None));
    let version = blobs
        .swap("index.json", b"{}".to_vec(), "")
        .await
        .expect("create-only write");
    assert_eq!(version, version_of(b"{}"));
    let held = bucket.lock().expect("bucket");
    assert_eq!(held.applied, 1, "the object was written more than once");
    assert_eq!(
        held.methods(),
        vec!["PUT", "GET", "PUT"],
        "a retried write must read the object back before repeating it"
    );
}

// The case an unconditional retry gets wrong: the provider applied the write
// and the acknowledgement never arrived. Sending it again would answer 412
// and be reported as somebody else's object; reading it back finds our own
// bytes and reports the write that already succeeded.
#[tokio::test]
async fn a_create_only_write_that_lost_its_answer_is_reconciled_not_repeated() {
    let (endpoint, bucket) = mock_bucket().await;
    let clock = TestClock::new(0.5);
    let blobs = store(&endpoint, clock.clone());
    bucket
        .lock()
        .expect("bucket")
        .faults
        .push_back(Fault::ApplyThenDrop);
    let version = blobs
        .swap("recovery/manifest", b"a recovery point".to_vec(), "")
        .await
        .expect("an applied create-only write was reported as a failure");
    assert_eq!(version, version_of(b"a recovery point"));
    let held = bucket.lock().expect("bucket");
    assert_eq!(held.applied, 1, "the recovery point was published twice");
    assert_eq!(held.methods(), vec!["PUT", "GET"]);
    assert_eq!(
        held.objects
            .get("komodoc/recovery/manifest")
            .map(Vec::as_slice),
        Some(b"a recovery point".as_slice())
    );
}

// The other half of that reconciliation: an object under the key we asked to
// create that is not what we sent belongs to somebody else, and create-only
// publication must still refuse.
#[tokio::test]
async fn a_create_only_write_that_finds_other_bytes_is_a_conflict() {
    let (endpoint, bucket) = mock_bucket().await;
    let clock = TestClock::new(0.5);
    let blobs = store(&endpoint, clock.clone());
    {
        let mut held = bucket.lock().expect("bucket");
        held.objects
            .insert("komodoc/recovery/manifest".into(), b"theirs".to_vec());
        held.faults.push_back(Fault::Drop);
    }
    assert!(matches!(
        blobs.swap("recovery/manifest", b"ours".to_vec(), "").await,
        Err(BlobError::Conflict)
    ));
    let held = bucket.lock().expect("bucket");
    assert_eq!(held.applied, 0);
    assert_eq!(
        held.objects
            .get("komodoc/recovery/manifest")
            .map(Vec::as_slice),
        Some(b"theirs".as_slice())
    );
}

// A compare-and-swap whose object still holds what we conditioned on was not
// applied, so the same If-Match write may go out again. It is never turned
// into an unconditional overwrite.
#[tokio::test]
async fn a_lost_compare_and_swap_retries_against_the_same_version() {
    let (endpoint, bucket) = mock_bucket().await;
    let clock = TestClock::new(0.5);
    let blobs = store(&endpoint, clock.clone());
    let first = blobs
        .swap("index.json", b"one".to_vec(), "")
        .await
        .expect("first write");
    bucket.lock().expect("bucket").faults.push_back(Fault::Drop);
    let second = blobs
        .swap("index.json", b"two".to_vec(), &first)
        .await
        .expect("a lost compare-and-swap was not retried");
    assert_eq!(second, version_of(b"two"));
    let held = bucket.lock().expect("bucket");
    assert_eq!(held.applied, 2, "the retried write applied twice");
    assert_eq!(held.methods(), vec!["PUT", "PUT", "GET", "PUT"]);
}

// A conditional write losing is a definite answer with its own meaning. It is
// never retried, because retrying it can only produce the same refusal more
// slowly -- and the caller's response is to re-read and decide again.
#[tokio::test]
async fn a_conditional_conflict_is_answered_once() {
    let (endpoint, bucket) = mock_bucket().await;
    let clock = TestClock::new(0.5);
    let blobs = store(&endpoint, clock.clone());
    blobs
        .swap("index.json", b"one".to_vec(), "")
        .await
        .expect("first write");
    bucket.lock().expect("bucket").log.clear();
    assert!(matches!(
        blobs.swap("index.json", b"two".to_vec(), "").await,
        Err(BlobError::Conflict)
    ));
    assert!(matches!(
        blobs
            .swap("index.json", b"two".to_vec(), "\"someone else\"")
            .await,
        Err(BlobError::Conflict)
    ));
    assert_eq!(
        bucket.lock().expect("bucket").requests(),
        2,
        "a conditional conflict was retried"
    );
    assert!(clock.waits().is_empty());
}

// Missing objects and refused credentials keep their meanings too.
#[tokio::test]
async fn absence_and_authorization_are_never_retried() {
    let (endpoint, bucket) = mock_bucket().await;
    let clock = TestClock::new(0.5);
    let blobs = store(&endpoint, clock.clone());
    assert!(matches!(
        blobs.get("nothing").await,
        Err(BlobError::NotFound)
    ));
    assert!(!blobs.exists("nothing").await.expect("head"));
    bucket
        .lock()
        .expect("bucket")
        .faults
        .push_back(Fault::Status(403, None));
    assert!(matches!(
        blobs.get("forbidden").await,
        Err(BlobError::Other(_))
    ));
    assert_eq!(bucket.lock().expect("bucket").requests(), 3);
    assert!(clock.waits().is_empty());
}

// A bucket that never recovers must not hold the caller forever. Both bounds
// apply, and the answer is a transient failure rather than an absence or a
// conflict, so no caller draws a durable conclusion from an outage.
#[tokio::test]
async fn exhausted_reads_report_a_transient_failure_within_their_bounds() {
    let (endpoint, bucket) = mock_bucket().await;
    let clock = TestClock::new(1.0);
    let blobs = store(&endpoint, clock.clone());
    for _ in 0..10 {
        bucket
            .lock()
            .expect("bucket")
            .faults
            .push_back(Fault::Status(503, None));
    }
    let failure = blobs
        .get("thing")
        .await
        .expect_err("outage reported success");
    assert!(matches!(&failure, BlobError::Other(message) if message.contains("503")));
    assert_eq!(
        bucket.lock().expect("bucket").requests(),
        RetryLimits::IDEMPOTENT.attempts as usize
    );
    assert!(clock.waited() <= RetryLimits::IDEMPOTENT.total);
}

// A write whose fate is still unknown when the bounds run out is reported as
// unresolved. It is not a conflict and not a success: the caller keeps its
// conservative charges and its recovery record.
#[tokio::test]
async fn an_unresolved_write_is_reported_as_unresolved() {
    let (endpoint, bucket) = mock_bucket().await;
    let clock = TestClock::new(1.0);
    let blobs = store(&endpoint, clock.clone());
    for _ in 0..10 {
        bucket
            .lock()
            .expect("bucket")
            .faults
            .push_back(Fault::Status(503, None));
    }
    let failure = blobs
        .swap("index.json", b"{}".to_vec(), "")
        .await
        .expect_err("an unresolved write was reported as settled");
    assert!(
        matches!(&failure, BlobError::Other(message) if message.contains("unresolved")),
        "unresolved write reported as {failure:?}"
    );
    let held = bucket.lock().expect("bucket");
    assert_eq!(held.applied, 0);
    assert_eq!(held.requests(), RetryLimits::WRITE.attempts as usize);
    assert!(clock.waited() <= RetryLimits::WRITE.total);
}

// Backing off is an await point and nothing more: dropping the future that
// owns an operation stops it there, and no further request goes out.
#[tokio::test]
async fn dropping_an_operation_during_backoff_sends_nothing_more() {
    let (endpoint, bucket) = mock_bucket().await;
    let (clock, hold) = TestClock::held(1.0);
    for _ in 0..10 {
        bucket
            .lock()
            .expect("bucket")
            .faults
            .push_back(Fault::Status(503, None));
    }
    let address = endpoint.clone();
    let running = tokio::spawn(async move {
        let blobs = store(&address, clock);
        blobs.get("thing").await
    });
    // The first attempt has been made and the second is parked in backoff.
    let parked = hold.parked.acquire().await.expect("a sleeper parked");
    assert_eq!(bucket.lock().expect("bucket").requests(), 1);
    running.abort();
    assert!(running.await.is_err(), "the cancelled task still finished");
    drop(parked);
    hold.release.notify_waiters();
    tokio::task::yield_now().await;
    assert_eq!(
        bucket.lock().expect("bucket").requests(),
        1,
        "a cancelled operation made another request"
    );
}

/* ------------------------------------------------------- batch deletion */

// The provider answers 200 and puts the verdicts in the body. Only the keys
// it named as deleted or absent are confirmed; one it refused for a
// transient reason goes into the next request; one it refused outright is a
// failure that keeps its object's charge.
#[tokio::test]
async fn a_partial_failure_inside_a_successful_response_is_reported_per_key() {
    let (endpoint, bucket) = mock_bucket().await;
    let clock = TestClock::new(0.5);
    let blobs = store(&endpoint, clock.clone());
    {
        let mut held = bucket.lock().expect("bucket");
        held.batch_replies.push_back(
            "<DeleteResult>\
             <Deleted><Key>komodoc/one</Key></Deleted>\
             <Error><Key>komodoc/two</Key><Code>NoSuchKey</Code><Message>gone</Message></Error>\
             <Error><Key>komodoc/three</Key><Code>InternalError</Code><Message>later</Message></Error>\
             <Error><Key>komodoc/four</Key><Code>AccessDenied</Code><Message>no</Message></Error>\
             </DeleteResult>"
                .into(),
        );
        held.batch_replies.push_back(
            "<DeleteResult><Deleted><Key>komodoc/three</Key></Deleted></DeleteResult>".into(),
        );
    }
    let keys: Vec<String> = ["one", "two", "three", "four"]
        .iter()
        .map(|key| key.to_string())
        .collect();
    let outcomes = blobs.delete_each(&keys).await.expect("batch delete");
    assert_eq!(
        outcomes,
        vec![
            DeleteOutcome::Deleted,
            DeleteOutcome::Absent,
            DeleteOutcome::Deleted,
            DeleteOutcome::Failed("AccessDenied: no".into()),
        ]
    );
    assert!(outcomes[3].why().contains("AccessDenied"));
    assert_eq!(
        bucket.lock().expect("bucket").methods(),
        vec!["POST", "POST"],
        "only the unresolved key should have been asked for again"
    );
    // `delete` keeps its all-or-nothing meaning for the callers that only ask
    // whether the whole request worked.
    bucket.lock().expect("bucket").batch_replies.push_back(
        "<DeleteResult><Error><Key>komodoc/four</Key><Code>AccessDenied</Code><Message>no</Message></Error></DeleteResult>".into(),
    );
    assert!(blobs.delete(&["four".to_string()]).await.is_err());
}

// A batch request that never gets an answer leaves every key in it unknown.
// Nothing in it may be retired: a repeated DELETE is harmless, but assuming
// it landed would release capacity for bytes nobody has proved are gone.
#[tokio::test]
async fn a_batch_that_never_gets_an_answer_leaves_its_keys_uncertain() {
    let (endpoint, bucket) = mock_bucket().await;
    let clock = TestClock::new(1.0);
    let blobs = store(&endpoint, clock.clone());
    for _ in 0..10 {
        bucket.lock().expect("bucket").faults.push_back(Fault::Drop);
    }
    let keys = vec!["one".to_string(), "two".to_string()];
    let outcomes = blobs.delete_each(&keys).await.expect("batch delete");
    assert!(
        outcomes
            .iter()
            .all(|outcome| matches!(outcome, DeleteOutcome::Uncertain(_))),
        "unanswered deletions reported as {outcomes:?}"
    );
    assert!(outcomes.iter().all(|outcome| !outcome.confirmed()));
    assert_eq!(
        bucket.lock().expect("bucket").requests(),
        RetryLimits::IDEMPOTENT.attempts as usize
    );
    assert!(clock.waited() <= RetryLimits::IDEMPOTENT.total);
}

// A gateway that does not implement multi-object deletion keeps working: the
// store falls back to one DELETE per key and remembers that it must.
#[tokio::test]
async fn a_bucket_without_batch_deletion_falls_back_to_one_request_per_key() {
    let (endpoint, bucket) = mock_bucket().await;
    let clock = TestClock::new(0.5);
    let blobs = store(&endpoint, clock.clone());
    bucket.lock().expect("bucket").refuse_batch = true;
    for index in 0..3 {
        blobs
            .put(&format!("thing/{index}"), vec![b'x'], "text/plain")
            .await
            .expect("write");
    }
    bucket.lock().expect("bucket").log.clear();
    let keys: Vec<String> = (0..3).map(|index| format!("thing/{index}")).collect();
    let outcomes = blobs.delete_each(&keys).await.expect("fallback delete");
    assert!(outcomes.iter().all(|outcome| outcome.confirmed()));
    {
        let held = bucket.lock().expect("bucket");
        assert_eq!(held.methods(), vec!["POST", "DELETE", "DELETE", "DELETE"]);
        assert!(held.objects.is_empty());
    }
    // The refusal is remembered, so the next sweep does not pay for it again.
    for index in 0..2 {
        blobs
            .put(&format!("more/{index}"), vec![b'x'], "text/plain")
            .await
            .expect("write");
    }
    bucket.lock().expect("bucket").log.clear();
    let keys: Vec<String> = (0..2).map(|index| format!("more/{index}")).collect();
    blobs.delete_each(&keys).await.expect("fallback delete");
    assert_eq!(
        bucket.lock().expect("bucket").methods(),
        vec!["DELETE", "DELETE"]
    );
}

// The measurement the spec asks for: what bulk retirement costs before and
// after. Two hundred and fifty objects took two hundred and fifty requests;
// they now take one.
#[tokio::test]
async fn bulk_retirement_costs_one_request_instead_of_one_per_object() {
    const OBJECTS: usize = 250;
    let keys: Vec<String> = (0..OBJECTS)
        .map(|index| format!("bulk/{index:04}"))
        .collect();

    let (endpoint, bucket) = mock_bucket().await;
    let clock = TestClock::new(0.5);
    let blobs = store(&endpoint, clock.clone());
    for key in &keys {
        blobs
            .put(key, vec![b'x'], "text/plain")
            .await
            .expect("write");
    }
    bucket.lock().expect("bucket").log.clear();
    let outcomes = blobs.delete_each(&keys).await.expect("batch delete");
    assert!(outcomes.iter().all(|outcome| outcome.confirmed()));
    let batched = bucket.lock().expect("bucket").requests();

    let (endpoint, bucket) = mock_bucket().await;
    let blobs = store(&endpoint, clock.clone());
    bucket.lock().expect("bucket").refuse_batch = true;
    for key in &keys {
        blobs
            .put(key, vec![b'x'], "text/plain")
            .await
            .expect("write");
    }
    bucket.lock().expect("bucket").log.clear();
    blobs.delete_each(&keys).await.expect("per-key delete");
    // The refused batch request plus one DELETE per object: what every
    // deployment paid before this change.
    let per_key = bucket.lock().expect("bucket").requests() - 1;

    assert_eq!(per_key, OBJECTS, "the fallback is one request per object");
    assert_eq!(batched, 1, "{OBJECTS} objects should retire in one request");
}

// Keys with characters XML cares about survive the round trip, because a key
// that comes back mangled would be reported against the wrong object.
#[tokio::test]
async fn batch_deletion_escapes_keys_and_matches_the_answer_back_to_them() {
    let (endpoint, bucket) = mock_bucket().await;
    let clock = TestClock::new(0.5);
    let blobs = store(&endpoint, clock.clone());
    let keys = vec!["a&b".to_string(), "c<d".to_string()];
    for key in &keys {
        blobs
            .put(key, vec![b'x'], "text/plain")
            .await
            .expect("write");
    }
    let outcomes = blobs.delete_each(&keys).await.expect("batch delete");
    assert_eq!(
        outcomes,
        vec![DeleteOutcome::Deleted, DeleteOutcome::Deleted]
    );
    assert!(bucket.lock().expect("bucket").objects.is_empty());
}
