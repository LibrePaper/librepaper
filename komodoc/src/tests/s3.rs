//! A bucket, in memory, speaking enough S3 to exercise the store against it:
//! conditional writes, listing, and ETags. It is not a conformance test of S3
//! -- it is a check that what komodoc sends is what an S3 answers to, and that
//! the store reads the answers correctly.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::{to_bytes, Body};
use axum::extract::State;
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use serde_json::json;

use super::*;
use crate::blob::{
    document_key, document_prefix, source_key, version_of, BlobError, BlobStore, INDEX_KEY,
};
use crate::s3::{canonical_query, escape_path, signing_key_for, S3Store};
use crate::storage::StorageOptions;

type Objects = Arc<Mutex<HashMap<String, Vec<u8>>>>;

pub async fn fake_bucket() -> (String, Objects) {
    let objects: Objects = Arc::new(Mutex::new(HashMap::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = Router::new().fallback(bucket).with_state(objects.clone());
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{address}"), objects)
}

async fn bucket(State(objects): State<Objects>, request: Request<Body>) -> Response<Body> {
    // Every request must be signed: an unsigned one would mean the store is
    // talking to a bucket that happens not to check.
    let authorization = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    if !authorization.starts_with("AWS4-HMAC-SHA256 Credential=") {
        return text_response(403, "unsigned");
    }
    let key = request
        .uri()
        .path()
        .trim_start_matches("/bucket/")
        .to_string();
    let query: HashMap<String, String> = request
        .uri()
        .query()
        .map(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .into_owned()
                .collect()
        })
        .unwrap_or_default();
    let method = request.method().clone();
    let if_match = request
        .headers()
        .get("if-match")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let if_none_match = request
        .headers()
        .get("if-none-match")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();

    match method.as_str() {
        "GET" if query.get("list-type").map(String::as_str) == Some("2") => {
            let prefix = query.get("prefix").cloned().unwrap_or_default();
            let mut body = String::from("<ListBucketResult><IsTruncated>false</IsTruncated>");
            for (name, content) in objects.lock().unwrap().iter() {
                if name.starts_with(&prefix) {
                    body.push_str(&format!(
                        "<Contents><Key>{name}</Key><Size>{}</Size><ETag>{}</ETag></Contents>",
                        content.len(),
                        version_of(content).replace('"', "&quot;")
                    ));
                }
            }
            body.push_str("</ListBucketResult>");
            let mut response = Response::new(Body::from(body));
            response
                .headers_mut()
                .insert("content-type", "application/xml".parse().unwrap());
            response
        }
        "GET" => {
            let content = objects.lock().unwrap().get(&key).cloned();
            match content {
                None => text_response(404, "no such key"),
                Some(content) => {
                    let tag = version_of(&content);
                    let mut response = Response::new(Body::from(content));
                    response.headers_mut().insert("etag", tag.parse().unwrap());
                    response
                }
            }
        }
        "PUT" => {
            {
                let held = objects.lock().unwrap();
                let current = held.get(&key);
                if !if_match.is_empty() && current.map(|c| version_of(c)) != Some(if_match) {
                    return text_response(412, "precondition failed");
                }
                if if_none_match == "*" && current.is_some() {
                    return text_response(412, "precondition failed");
                }
            }
            let Ok(body) = to_bytes(request.into_body(), 64 << 20).await else {
                return text_response(400, "short read");
            };
            let tag = version_of(&body);
            objects.lock().unwrap().insert(key, body.to_vec());
            let mut response = Response::new(Body::empty());
            response.headers_mut().insert("etag", tag.parse().unwrap());
            response
        }
        "DELETE" => {
            objects.lock().unwrap().remove(&key);
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::NO_CONTENT;
            response
        }
        _ => text_response(405, "method not allowed"),
    }
}

fn text_response(status: u16, body: &str) -> Response<Body> {
    let mut response = Response::new(Body::from(body.to_string()));
    *response.status_mut() = StatusCode::from_u16(status).unwrap();
    response
}

pub fn bucket_options(endpoint: &str) -> StorageOptions {
    StorageOptions {
        endpoint: endpoint.to_string(),
        bucket: "bucket".into(),
        region: "auto".into(),
        prefix: "komodoc/".into(),
        access_key: "key".into(),
        secret_key: "secret".into(),
        ..StorageOptions::default()
    }
}

#[tokio::test]
async fn s3_store_against_a_bucket() {
    let (endpoint, objects) = fake_bucket().await;
    let blobs = S3Store::new(&bucket_options(&endpoint));

    // Everything komodoc writes lands under its own prefix, so a bucket can be
    // shared and a clear has a bounded blast radius.
    blobs
        .put(
            &document_key("a-paper", "abc"),
            b"<p>hi</p>".to_vec(),
            "text/html",
        )
        .await
        .unwrap();
    assert!(
        objects
            .lock()
            .unwrap()
            .contains_key("komodoc/documents/a-paper/abc.html"),
        "the object landed outside the prefix"
    );

    // And callers speak in unprefixed keys, in both directions.
    assert_eq!(
        blobs.get(&document_key("a-paper", "abc")).await.unwrap(),
        b"<p>hi</p>"
    );
    let found = blobs.list(&document_prefix("a-paper")).await.unwrap();
    assert!(
        found.len() == 1 && found[0].key == document_key("a-paper", "abc"),
        "listing gave {found:?}"
    );

    // The conditional write, which is what the index depends on.
    let at = blobs
        .swap(INDEX_KEY, br#"{"a":1}"#.to_vec(), "")
        .await
        .unwrap();
    assert!(matches!(
        blobs.swap(INDEX_KEY, br#"{"b":2}"#.to_vec(), "").await,
        Err(BlobError::Conflict)
    ));
    blobs
        .swap(INDEX_KEY, br#"{"c":3}"#.to_vec(), &at)
        .await
        .expect("a write against the current version was refused");

    // The probe is what stands between a working deployment and one that loses
    // an index update in silence.
    let report = blobs.probe().await;
    assert!(
        report.reachable && report.conditional_writes,
        "the probe did not recognise a working bucket: {report:?}"
    );
    assert!(
        !objects
            .lock()
            .unwrap()
            .contains_key("komodoc/.komodoc-probe"),
        "the probe left its scratch object behind"
    );

    blobs
        .delete(&[document_key("a-paper", "abc")])
        .await
        .unwrap();
    assert!(matches!(
        blobs.get(&document_key("a-paper", "abc")).await,
        Err(BlobError::NotFound)
    ));
}

// A bucket without conditional writes must be recognised as such, because
// running on one unknowingly means losing index updates without a sign.
#[tokio::test]
async fn probe_catches_a_bucket_that_ignores_conditions() {
    let objects: Objects = Arc::new(Mutex::new(HashMap::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    // Every write accepted, whatever it was conditional on.
    let router = Router::new()
        .fallback(
            |State(objects): State<Objects>, request: Request<Body>| async move {
                let key = request
                    .uri()
                    .path()
                    .trim_start_matches("/bucket/")
                    .to_string();
                match request.method().as_str() {
                    "PUT" => {
                        objects.lock().unwrap().insert(key, b"stored".to_vec());
                        let mut response = Response::new(Body::empty());
                        response
                            .headers_mut()
                            .insert("etag", "\"whatever\"".parse().unwrap());
                        response
                    }
                    "GET" => match objects.lock().unwrap().get(&key) {
                        None => text_response(404, "no"),
                        Some(body) => {
                            let mut response = Response::new(Body::from(body.clone()));
                            response
                                .headers_mut()
                                .insert("etag", "\"whatever\"".parse().unwrap());
                            response
                        }
                    },
                    _ => {
                        objects.lock().unwrap().remove(&key);
                        Response::new(Body::empty())
                    }
                }
            },
        )
        .with_state(objects);
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let blobs = S3Store::new(&bucket_options(&format!("http://{address}")));
    let report = blobs.probe().await;
    assert!(
        !report.conditional_writes,
        "a bucket that ignores conditions was reported as safe"
    );
    assert!(
        !report.why.is_empty(),
        "the probe refused without saying why"
    );
}

// A presigned URL is what lets a reader's browser fetch a document without
// the bytes passing through this process. It has to carry its own credentials
// and its own expiry, since nothing else will vouch for it.
#[test]
fn presigned_get_carries_its_own_credentials() {
    let mut options = bucket_options("https://bucket.example");
    options.access_key = "AKIAEXAMPLE".into();
    let blobs = S3Store::new(&options);
    let link = blobs
        .presign_get(&document_key("a-paper", "abc"), 120)
        .expect("a link");
    for wanted in [
        "https://bucket.example/bucket/komodoc/documents/a-paper/abc.html",
        "X-Amz-Algorithm=AWS4-HMAC-SHA256",
        "X-Amz-Credential=AKIAEXAMPLE",
        "X-Amz-Expires=120",
        "X-Amz-Signature=",
    ] {
        assert!(
            link.contains(wanted),
            "the presigned link is missing {wanted:?}:\n{link}"
        );
    }
}

// The signing key derivation, against the vector AWS publishes for it.
// Signing is the part of this that cannot be checked by reading it, and a
// wrong signature is a deployment that does not work at all.
#[test]
fn signing_key_matches_the_aws_vector() {
    let key = signing_key_for(
        "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
        "20150830",
        "us-east-1",
        "iam",
    );
    assert_eq!(
        hex::encode(key),
        "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9"
    );
}

// Paths and queries are canonicalised the way SigV4 says: a space is %20, and
// a slash between path segments stays a slash.
#[test]
fn canonical_encoding() {
    assert_eq!(
        escape_path("/komodoc/documents/a b/c.html"),
        "/komodoc/documents/a%20b/c.html"
    );
    let query = vec![
        ("b".to_string(), "2".to_string()),
        ("a".to_string(), "1".to_string()),
        ("prefix".to_string(), "komodoc/documents/".to_string()),
    ];
    assert_eq!(
        canonical_query(&query),
        "a=1&b=2&prefix=komodoc%2Fdocuments%2F"
    );
}

/* ------------------------------------------------------ a server on it */

// The point of the whole exercise: a server whose durable state is somebody
// else's bucket. Publishing, reading, editing, commenting and deleting all go
// through the same paths they always did -- what changed is where the bytes
// land, and this is the test that says the paths do not care.
#[tokio::test]
async fn a_server_backed_by_a_bucket() {
    use crate::assets::load_shell;
    use crate::auth::{GithubApp, Policy};
    use crate::config::Configuration;
    use crate::room::RoomSet;
    use crate::server::Server;
    use crate::store::Store;

    let (endpoint, objects) = fake_bucket().await;
    let blobs: Arc<dyn BlobStore> = Arc::new(S3Store::new(&bucket_options(&endpoint)));
    let config = Arc::new(Configuration::default());
    let store = Store::open(blobs.clone(), config.clone()).await.unwrap();
    let rooms = RoomSet::new(blobs.clone(), config.clone());
    let instance = Server::new(
        store,
        rooms,
        load_shell(&config).unwrap(),
        GithubApp {
            client_id: "test-client".into(),
            client_secret: "test-secret".into(),
        },
        TEST_KEY.to_vec(),
        config.clone(),
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
    );
    let server = serve_instance(Arc::new(instance), tempfile::tempdir().unwrap()).await;

    // Publish, the ordinary way.
    let markdown = crate::tests::edit::TEST_MARKDOWN;
    let html = crate::render::render_markdown_document(markdown, "My Paper");
    let (status, document) = post(
        &server.url,
        "/api/documents",
        json!({"title": "My Paper", "html": html, "source": markdown, "source_format": "markdown"}),
    )
    .await;
    assert_eq!(
        status, 201,
        "publishing to a bucket returned {status}: {document}"
    );
    let slug = text(&document, "slug");

    // Everything is in the bucket, under komodoc's own prefix, in the layout
    // the store expects.
    for wanted in [
        "komodoc/index.json".to_string(),
        // The live document, and the checkpoint the index names. No rendered
        // page and no second copy of the source: nothing derived is stored.
        format!("komodoc/sessions/{slug}"),
        format!("komodoc/history/{slug}/{}", text(&document, "sha")),
        format!("komodoc/history/{slug}/index.json"),
    ] {
        assert!(
            objects.lock().unwrap().contains_key(&wanted),
            "nothing at {wanted}"
        );
    }

    // A comment, which is the state an operator would most hate to lose, and
    // the reason rooms went into the bucket too.
    let (status, _) = post(
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({"type": "comment", "exact": "world", "body": "from a bucket", "creator": "Reader"}),
    )
    .await;
    assert_eq!(status, 200);
    assert!(
        objects
            .lock()
            .unwrap()
            .contains_key(&format!("komodoc/rooms/{slug}.json")),
        "the comment did not reach the bucket"
    );

    // A second server, reading the same bucket, sees the same documents: this
    // is what "the server holds nothing" is worth.
    let second = Store::open(blobs.clone(), config.clone()).await.unwrap();
    let entry = second
        .get(&slug)
        .await
        .expect("a fresh server did not find the document");
    assert_eq!(entry.title, "My Paper");
    // The document itself is the checkpoint the index names, which a fresh
    // server reads from the bucket without having to hold anything.
    assert_eq!(
        second
            .blobs
            .get(&crate::blob::checkpoint_key(&slug, &entry.sha))
            .await
            .unwrap(),
        markdown.as_bytes()
    );

    // And deleting takes komodoc's keys and nothing else.
    objects
        .lock()
        .unwrap()
        .insert("komodoc-other-thing".into(), b"not ours".to_vec());
    let (status, _) = post(
        &server.url,
        &format!("/api/documents/{slug}/delete"),
        json!({}),
    )
    .await;
    assert_eq!(status, 200);
    let held = objects.lock().unwrap();
    assert!(
        !held.keys().any(|name| name.contains(&slug)),
        "something survived the delete: {:?}",
        held.keys()
    );
    assert!(
        held.contains_key("komodoc-other-thing"),
        "deleting a document removed something that was not part of it"
    );
}

// The index is written conditionally, so a second writer cannot overwrite a
// document it never saw. That is what the whole compare-and-swap is for, and
// on a bucket it is the only thing standing between two servers and a lost
// document.
#[tokio::test]
async fn a_second_writer_cannot_clobber_the_index() {
    use crate::config::Configuration;
    use crate::store::{Publication, Store};

    let (endpoint, _) = fake_bucket().await;
    let options = bucket_options(&endpoint);
    let config = Arc::new(Configuration::default());
    let blobs: Arc<dyn BlobStore> = Arc::new(S3Store::new(&options));

    let first = Store::open(blobs.clone(), config.clone()).await.unwrap();
    first
        .put(Publication {
            slug: "a-paper".into(),
            title: "A".into(),
            source: "<p>a</p>".into(),
            source_format: "html".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    // A second server that read the index before that write still holds the
    // version it read, and its own write must be refused rather than silently
    // dropping the first server's document.
    let stale = Store::open(Arc::new(S3Store::new(&options)), config.clone())
        .await
        .unwrap();
    stale.state.lock().await.index_version = String::new();
    assert!(
        stale
            .put(Publication {
                slug: "b-paper".into(),
                title: "B".into(),
                source: "<p>b</p>".into(),
                source_format: "html".into(),
                ..Default::default()
            })
            .await
            .is_err(),
        "a write against a version the index has moved past was accepted"
    );

    // The first document is still there, which is the whole point.
    let after = Store::open(Arc::new(S3Store::new(&options)), config)
        .await
        .unwrap();
    assert!(
        after.get("a-paper").await.is_some(),
        "the first document was lost"
    );
}

// With one writer asserted, the index is written unconditionally -- which is
// the only way to run against a bucket that has no conditional writes, and is
// exactly what the flag promises.
#[tokio::test]
async fn single_writer_writes_unconditionally() {
    let (endpoint, _) = fake_bucket().await;
    let mut options = bucket_options(&endpoint);
    options.single_writer = true;
    let blobs = S3Store::new(&options);

    blobs
        .swap(INDEX_KEY, br#"{"a":1}"#.to_vec(), "")
        .await
        .unwrap();
    // The same write again, against a version that is now wrong: accepted,
    // because the operator said they are the only writer.
    blobs
        .swap(INDEX_KEY, br#"{"b":2}"#.to_vec(), "\"stale\"")
        .await
        .expect("a single-writer deployment refused its own write");
}

#[tokio::test]
async fn a_bucket_source_round_trips() {
    let (endpoint, _) = fake_bucket().await;
    let blobs = S3Store::new(&bucket_options(&endpoint));
    blobs
        .put(
            &source_key("a-paper", "sha1"),
            b"# hello".to_vec(),
            "text/plain",
        )
        .await
        .unwrap();
    assert_eq!(
        blobs.get(&source_key("a-paper", "sha1")).await.unwrap(),
        b"# hello"
    );
}
