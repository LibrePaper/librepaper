//! Assets: the bytes of a document that nobody edits in place.
//!
//! A figure is where the bytes of a paper actually go -- a directory of images
//! is an order of magnitude larger than its text -- so most of what is checked
//! here is the bounding: what one figure may be, what a document's figures may
//! come to, what the owner's quota says, how fast they may arrive, and what
//! happens to the ones nothing refers to any more.
//!
//! The other half is that reading one is exactly as hard as reading the
//! document: a private paper's figures are private, and that is not a separate
//! rule but the same one asked in another place.

use serde_json::{json, Value};

use super::*;
use crate::auth::Policy;
use crate::config::Configuration;

/// A small PNG, as bytes. Real enough to be a figure and small enough to sit
/// in a test: the eight-byte signature and a minimal IHDR.
fn png(seed: u8) -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    bytes.extend_from_slice(b"IHDRkomodoc");
    bytes.push(seed);
    bytes
}

async fn put_asset(cookie: &str, base: &str, slug: &str, body: Vec<u8>) -> (u16, Value) {
    let mut request = client()
        .put(format!("{base}/api/documents/{slug}/assets"))
        .header("x-komodoc-client", "1")
        .body(body);
    if !cookie.is_empty() {
        request = request.header("cookie", cookie);
    }
    let response = request.send().await.expect("a response");
    let status = response.status().as_u16();
    let raw = response.bytes().await.unwrap_or_default();
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

async fn get_asset(cookie: &str, base: &str, slug: &str, sha: &str) -> (u16, Vec<u8>) {
    get_asset_keyed(cookie, "", base, slug, sha).await
}

/// The same, carrying a link key too, for a caller who is not the owner.
async fn get_asset_keyed(
    cookie: &str,
    key: &str,
    base: &str,
    slug: &str,
    sha: &str,
) -> (u16, Vec<u8>) {
    // The header a browser cannot attach across origins without a preflight
    // that is never granted, which is what marks a same-origin request. Every
    // API route refuses without it, and a figure is not an exception.
    let mut request = client()
        .get(format!("{base}/api/documents/{slug}/assets/{sha}"))
        .header("x-komodoc-client", "1");
    if !cookie.is_empty() {
        request = request.header("cookie", cookie);
    }
    if !key.is_empty() {
        request = request.header(crate::server::LINK_HEADER, key);
    }
    let response = request.send().await.expect("a response");
    let status = response.status().as_u16();
    (status, response.bytes().await.unwrap_or_default().to_vec())
}

/// A GET carrying the marker header a browser attaches to a same-origin
/// request. Every route below refuses without it, which is rule A and not a
/// property of figures.
async fn get_same_origin(cookie: &str, base: &str, path: &str) -> (u16, Value) {
    get_same_origin_keyed(cookie, "", base, path).await
}

/// The same, carrying a link key too, for a caller who is not the owner.
async fn get_same_origin_keyed(cookie: &str, key: &str, base: &str, path: &str) -> (u16, Value) {
    let mut request = client()
        .get(format!("{base}{path}"))
        .header("x-komodoc-client", "1");
    if !cookie.is_empty() {
        request = request.header("cookie", cookie);
    }
    if !key.is_empty() {
        request = request.header(crate::server::LINK_HEADER, key);
    }
    let response = request.send().await.expect("a response");
    let status = response.status().as_u16();
    let raw = response.bytes().await.unwrap_or_default();
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}
/* ------------------------------------------------------------ the round trip */

#[tokio::test]
async fn a_figure_goes_up_and_comes_back() {
    let server = new_test_server().await;
    let document = publish_test_document(&server.url).await;
    let slug = text(&document, "slug");
    let key = read_key_of(&document);
    let bytes = png(1);

    let (status, answer) = put_asset(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &slug,
        bytes.clone(),
    )
    .await;
    assert_eq!(status, 200, "{answer}");
    let sha = text(&answer, "sha");
    assert_eq!(
        sha,
        crate::store::digest_of_bytes(&bytes),
        "named by digest"
    );
    assert_eq!(answer["size"], bytes.len());

    // A reader holds the link `publish` printed, not merely the slug.
    let (status, back) = get_asset_keyed("", &key, &server.url, &slug, &sha).await;
    assert_eq!(status, 200);
    assert_eq!(back, bytes, "what came back is not what went up");
}

#[tokio::test]
async fn a_figure_is_cached_for_a_year_because_its_name_is_its_bytes() {
    let server = new_test_server().await;
    let document = publish_test_document(&server.url).await;
    let slug = text(&document, "slug");
    let key = read_key_of(&document);
    let (_, answer) = put_asset(&session_as(TEST_PUBLISHER), &server.url, &slug, png(2)).await;
    let sha = text(&answer, "sha");

    let response = client()
        .get(format!("{}/api/documents/{slug}/assets/{sha}", server.url))
        .header("x-komodoc-client", "1")
        .header(crate::server::LINK_HEADER, &key)
        .send()
        .await
        .expect("a response");
    let caching = response.headers()["cache-control"].to_str().unwrap();
    assert!(caching.contains("immutable"), "{caching}");
    assert!(caching.contains("max-age=31536000"), "{caching}");
}

#[tokio::test]
async fn the_same_figure_twice_is_stored_once() {
    let server = new_test_server().await;
    let slug = text(&publish_test_document(&server.url).await, "slug");
    let bytes = png(3);
    let cookie = session_as(TEST_PUBLISHER);

    let (_, first) = put_asset(&cookie, &server.url, &slug, bytes.clone()).await;
    let (status, again) = put_asset(&cookie, &server.url, &slug, bytes.clone()).await;
    assert_eq!(status, 200);
    assert_eq!(text(&first, "sha"), text(&again, "sha"));

    let stored = server
        .instance
        .store
        .blobs
        .list(&crate::blob::asset_prefix(&slug))
        .await
        .unwrap();
    assert_eq!(stored.len(), 1, "the same bytes were stored twice");
}

/* ------------------------------------------------------------------- rights */

#[tokio::test]
async fn only_an_editor_may_put_a_figure() {
    let server = new_test_server().await;
    let slug = text(&publish_test_document(&server.url).await, "slug");

    // A stranger, and a reader with no account: neither may put bytes on the
    // server. Both are told the document is not there, on the same reasoning
    // the delete route follows.
    for cookie in ["", &session_as("stranger")] {
        let (status, _) = put_asset(cookie, &server.url, &slug, png(4)).await;
        assert_eq!(status, 404, "a caller who may not edit was not refused");
    }
    let stored = server
        .instance
        .store
        .blobs
        .list(&crate::blob::asset_prefix(&slug))
        .await
        .unwrap();
    assert!(stored.is_empty(), "a refused upload stored something");
}

#[tokio::test]
async fn a_documents_figures_are_as_closed_as_its_text() {
    let server = new_test_server().await;
    let owner = session_as(TEST_PUBLISHER);
    let slug = text(&publish_test_document(&server.url).await, "slug");
    let (_, answer) = put_asset(&owner, &server.url, &slug, png(5)).await;
    let sha = text(&answer, "sha");

    // A stranger gets what they get for the document itself: nothing, and
    // no hint that there is anything to get.
    assert_eq!(
        get_asset("", &server.url, &slug, &sha).await.0,
        404,
        "a document's figure was served to a stranger"
    );
    assert_eq!(
        get_asset(&session_as("stranger"), &server.url, &slug, &sha)
            .await
            .0,
        404
    );
    // And the owner still has it.
    assert_eq!(get_asset(&owner, &server.url, &slug, &sha).await.0, 200);
}

#[tokio::test]
async fn a_figure_named_by_something_that_is_not_a_digest_is_refused() {
    let server = new_test_server().await;
    let slug = text(&publish_test_document(&server.url).await, "slug");
    // A key is never built from something a caller shapes. These are the
    // shapes that would matter if it were.
    for name in ["../../index.json", "..%2F..%2Findex.json", "abc", ""] {
        let (status, _) = get_asset("", &server.url, &slug, name).await;
        assert!(status == 404 || status == 400, "{name} answered {status}");
    }
}

/* ---------------------------------------------------------------- the bounds */

#[tokio::test]
async fn one_figure_may_be_only_so_large() {
    let config = Configuration {
        max_asset: 1024,
        max_assets: 1024 * 1024,
        ..Configuration::default()
    };
    let server = test_server_with(
        config,
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
        true,
    )
    .await;
    let slug = text(&publish_test_document(&server.url).await, "slug");
    let cookie = session_as(TEST_PUBLISHER);

    // At the boundary it is taken, and past it refused.
    let (status, answer) = put_asset(&cookie, &server.url, &slug, vec![7u8; 1024]).await;
    assert_eq!(status, 200, "{answer}");
    let (status, _) = put_asset(&cookie, &server.url, &slug, vec![8u8; 1025]).await;
    assert_eq!(status, 413, "a figure over the ceiling was taken");
}

#[tokio::test]
async fn a_documents_figures_may_come_to_only_so_much() {
    let config = Configuration {
        max_asset: 4096,
        max_assets: 4096,
        ..Configuration::default()
    };
    let server = test_server_with(
        config,
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
        true,
    )
    .await;
    let slug = text(&publish_test_document(&server.url).await, "slug");
    let cookie = session_as(TEST_PUBLISHER);

    let (status, _) = put_asset(&cookie, &server.url, &slug, vec![1u8; 3000]).await;
    assert_eq!(status, 200);
    // The second would carry the document past what it may keep in figures,
    // even though it is itself inside `max_asset`.
    let (status, said) = put_asset(&cookie, &server.url, &slug, vec![2u8; 3000]).await;
    assert_eq!(status, 413, "{said}");
    assert!(
        text(&said, "error").contains("figures"),
        "the refusal should name the ceiling it met: {said}"
    );
}

#[tokio::test]
async fn figures_count_against_the_owners_quota() {
    let mut config = Configuration {
        max_asset: 8192,
        max_assets: 8192,
        ..Configuration::default()
    };
    config.storage.per_owner = 4096;
    let server = test_server_with(
        config,
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
        true,
    )
    .await;
    let slug = text(&publish_test_document(&server.url).await, "slug");
    let cookie = session_as(TEST_PUBLISHER);

    let (status, said) = put_asset(&cookie, &server.url, &slug, vec![3u8; 8000]).await;
    assert_eq!(status, 507, "{said}");
    assert!(text(&said, "error").contains("quota"), "{said}");
}

#[tokio::test]
async fn a_figure_is_an_upload_and_is_rate_limited_as_one() {
    let mut config = Configuration::default();
    config.storage.uploads_per_hour = 2;
    let server = test_server_with(
        config,
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
        true,
    )
    .await;
    let slug = text(&publish_test_document(&server.url).await, "slug");
    let cookie = session_as(TEST_PUBLISHER);

    assert_eq!(put_asset(&cookie, &server.url, &slug, png(10)).await.0, 200);
    assert_eq!(put_asset(&cookie, &server.url, &slug, png(11)).await.0, 200);
    let (status, said) = put_asset(&cookie, &server.url, &slug, png(12)).await;
    assert_eq!(status, 429, "{said}");
}

/* ------------------------------------------------------------------ pruning */

#[tokio::test]
async fn a_figure_nothing_refers_to_is_pruned_once_its_grace_has_passed() {
    // No grace, so the test is about what is referred to rather than about
    // waiting. The grace itself is the next test.
    let config = Configuration {
        asset_grace: 0,
        ..Configuration::default()
    };
    let server = test_server_with(
        config,
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
        true,
    )
    .await;
    let slug = text(&publish_test_document(&server.url).await, "slug");
    let cookie = session_as(TEST_PUBLISHER);

    let (_, named) = put_asset(&cookie, &server.url, &slug, png(20)).await;
    let (_, orphan) = put_asset(&cookie, &server.url, &slug, png(21)).await;
    let kept = text(&named, "sha");
    let dropped = text(&orphan, "sha");

    // One of them is named in the document; the other is not.
    let room = server.instance.rooms.get(&slug).await;
    {
        let state = room.state.lock().await;
        crate::session::put_asset(&state.session.doc, "fig/one.png", &kept);
    }
    // `quiet` rather than `cli`: a checkpoint asked for from outside inside
    // the defer window waits, and a checkpoint that has not happened has
    // pruned nothing. The publish a moment ago took one.
    room.checkpoint("quiet", TEST_PUBLISHER)
        .await
        .expect("a checkpoint")
        .expect("not deferred");

    assert!(
        room.read_asset(&kept).await.is_some(),
        "the named figure went"
    );
    assert!(
        room.read_asset(&dropped).await.is_none(),
        "the figure nothing refers to is still there"
    );
}

#[tokio::test]
async fn a_figure_just_uploaded_is_kept_while_its_name_is_still_coming() {
    // Uploading and naming are two requests. Between them the figure is
    // referred to by nothing, and pruning it would delete what somebody had
    // just successfully uploaded.
    let server = new_test_server().await; // the default hour of grace
    let slug = text(&publish_test_document(&server.url).await, "slug");
    let cookie = session_as(TEST_PUBLISHER);
    let (_, answer) = put_asset(&cookie, &server.url, &slug, png(22)).await;
    let sha = text(&answer, "sha");

    let room = server.instance.rooms.get(&slug).await;
    room.checkpoint("quiet", TEST_PUBLISHER)
        .await
        .expect("a checkpoint");
    assert!(
        room.read_asset(&sha).await.is_some(),
        "a figure uploaded a moment ago was pruned before it could be named"
    );
}

#[tokio::test]
async fn a_figure_a_kept_checkpoint_names_survives_being_removed_from_the_document() {
    // Restoring Tuesday has to find Tuesday's figures. An asset is kept while
    // any retained checkpoint names it, not merely while the live document
    // does.
    let config = Configuration {
        asset_grace: 0,
        ..Configuration::default()
    };
    let server = test_server_with(
        config,
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
        true,
    )
    .await;
    let slug = text(&publish_test_document(&server.url).await, "slug");
    let cookie = session_as(TEST_PUBLISHER);
    let (_, answer) = put_asset(&cookie, &server.url, &slug, png(23)).await;
    let sha = text(&answer, "sha");

    let room = server.instance.rooms.get(&slug).await;
    {
        let state = room.state.lock().await;
        crate::session::put_asset(&state.session.doc, "fig/one.png", &sha);
    }
    room.checkpoint("quiet", TEST_PUBLISHER)
        .await
        .expect("first")
        .expect("not deferred");

    // Taken out of the live document, and a second checkpoint recorded.
    {
        let state = room.state.lock().await;
        let assets = yrs::Doc::get_or_insert_map(&state.session.doc, crate::session::ASSETS);
        let mut txn = yrs::Transact::transact_mut(&state.session.doc);
        yrs::Map::remove(&assets, &mut txn, "fig/one.png");
    }
    // The text has to differ too, or the checkpoint is the same tree.
    room.set_source("# changed\n\nso the tree differs.\n", "markdown")
        .await;
    room.checkpoint("quiet", TEST_PUBLISHER)
        .await
        .expect("second")
        .expect("not deferred");

    assert!(
        room.read_asset(&sha).await.is_some(),
        "a figure the first checkpoint still names was pruned"
    );
}

#[tokio::test]
async fn destroying_a_document_takes_its_figures() {
    let server = new_test_server().await;
    let slug = text(&publish_test_document(&server.url).await, "slug");
    let cookie = session_as(TEST_PUBLISHER);
    put_asset(&cookie, &server.url, &slug, png(30)).await;

    let (status, _) = post_as(
        &cookie,
        &server.url,
        &format!("/api/documents/{slug}/delete"),
        json!({}),
    )
    .await;
    assert_eq!(status, 200);
    let left = server
        .instance
        .store
        .blobs
        .list(&crate::blob::asset_prefix(&slug))
        .await
        .unwrap();
    assert!(left.is_empty(), "destroy left {left:?}");
}

/* --------------------------------------------------- publishing a directory */

/// Sends a directory the way `komodoc publish <dir>` does: one multipart
/// request, a part per file named by its path, and a `main` field saying
/// which of them is the document.
async fn post_directory(
    cookie: &str,
    base: &str,
    title: &str,
    main: &str,
    files: &[(&str, &[u8])],
) -> (u16, Value) {
    let mut form = reqwest::multipart::Form::new()
        .text("title", title.to_string())
        .text("main", main.to_string());
    for (at, bytes) in files {
        form = form.part(
            "file",
            reqwest::multipart::Part::bytes(bytes.to_vec()).file_name(at.to_string()),
        );
    }
    let mut request = client()
        .post(format!("{base}/api/documents"))
        .header("x-komodoc-client", "1")
        .multipart(form);
    if !cookie.is_empty() {
        request = request.header("cookie", cookie);
    }
    let response = request.send().await.expect("a response");
    let status = response.status().as_u16();
    let raw = response.bytes().await.unwrap_or_default();
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

#[tokio::test]
async fn a_whole_directory_arrives_as_one_document() {
    let server = new_test_server().await;
    let cookie = session_as(TEST_PUBLISHER);
    let (status, document) = post_directory(
        &cookie,
        &server.url,
        "A Modular Paper",
        "paper.typ",
        &[
            (
                "paper.typ",
                b"#import \"lib.typ\": word\n= A Modular Paper\n#word\n",
            ),
            ("lib.typ", b"#let word = \"sibling\"\n"),
            ("chapters/03.typ", b"The third chapter.\n"),
            ("refs.bib", b"@book{a,title={A}}\n"),
            ("fig/one.png", b"\x89PNG\r\n\x1a\nIHDR"),
        ],
    )
    .await;
    assert_eq!(status, 201, "{document}");
    let slug = text(&document, "slug");

    // Every text is in the shared document, under the name it was sent as,
    // and the figure is named beside its digest rather than being in it.
    let room = server.instance.rooms.get(&slug).await;
    let state = room.state.lock().await;
    let texts = crate::session::texts_of(&state.session.doc);
    assert_eq!(
        texts.keys().collect::<Vec<_>>(),
        vec!["chapters/03.typ", "lib.typ", "paper.typ", "refs.bib"]
    );
    assert_eq!(texts["lib.typ"], "#let word = \"sibling\"\n");
    assert_eq!(crate::session::main_path(&state.session.doc), "paper.typ");
    let figures = crate::session::assets_of(&state.session.doc);
    assert_eq!(figures.len(), 1);
    let sha = figures["fig/one.png"].clone();
    drop(state);

    // And the figure's bytes are in the store, under their own digest.
    assert_eq!(
        room.read_asset(&sha).await.as_deref(),
        Some(&b"\x89PNG\r\n\x1a\nIHDR"[..])
    );

    // The index says which file is the document, so a reader that has not
    // joined the session yet still knows what it is looking at.
    let entry = server
        .instance
        .store
        .get(&slug)
        .await
        .expect("in the index");
    assert_eq!(entry.main, "paper.typ");
    assert_eq!(entry.source_format, "typst");
}

#[tokio::test]
async fn a_directory_whose_main_file_is_not_in_it_is_refused() {
    let server = new_test_server().await;
    let (status, said) = post_directory(
        &session_as(TEST_PUBLISHER),
        &server.url,
        "A Paper",
        "absent.typ",
        &[("paper.typ", b"= A Paper\n")],
    )
    .await;
    assert_eq!(status, 400, "{said}");
    assert!(text(&said, "error").contains("absent.typ"), "{said}");
}

#[tokio::test]
async fn a_directory_carrying_a_path_the_rules_refuse_is_refused_by_name() {
    let server = new_test_server().await;
    for bad in [
        "../escape.typ",
        "fig/../secret.png",
        "main.aux",
        "notes.rtf",
    ] {
        let (status, said) = post_directory(
            &session_as(TEST_PUBLISHER),
            &server.url,
            "A Paper",
            "paper.typ",
            &[("paper.typ", b"= A Paper\n"), (bad, b"whatever\n")],
        )
        .await;
        assert_eq!(status, 400, "{bad} was taken: {said}");
        assert!(
            text(&said, "error").contains(bad),
            "the refusal should name the file: {said}"
        );
    }
}

#[tokio::test]
async fn a_one_file_publish_is_still_a_directory_of_one_file() {
    // The shape this route has always taken, unchanged: no `main` field and
    // one part. What it produces is a directory with one file in it.
    let server = new_test_server().await;
    let document = publish_test_document(&server.url).await;
    let slug = text(&document, "slug");
    let entry = server
        .instance
        .store
        .get(&slug)
        .await
        .expect("in the index");
    assert_eq!(entry.main, "main.html");
    let room = server.instance.rooms.get(&slug).await;
    let state = room.state.lock().await;
    assert_eq!(crate::session::texts_of(&state.session.doc).len(), 1);
}

/* ------------------------------------------------------------- the timeline */

#[tokio::test]
async fn the_history_says_which_paths_each_checkpoint_moved() {
    let server = new_test_server().await;
    let cookie = session_as(TEST_PUBLISHER);
    let (status, document) = post_directory(
        &cookie,
        &server.url,
        "A Paper",
        "paper.md",
        &[
            ("paper.md", b"# A Paper\n\nThe opening.\n"),
            ("chapters/03.md", b"The third chapter.\n"),
        ],
    )
    .await;
    assert_eq!(status, 201, "{document}");
    let slug = text(&document, "slug");

    // One chapter edited, and a checkpoint taken.
    let room = server.instance.rooms.get(&slug).await;
    room.add_text("chapters/03.md", "The third chapter, revised.\n")
        .await;
    room.checkpoint("quiet", TEST_PUBLISHER)
        .await
        .expect("a checkpoint")
        .expect("not deferred");

    let (status, timeline) = get_same_origin(
        &cookie,
        &server.url,
        &format!("/api/documents/{slug}/history"),
    )
    .await;
    assert_eq!(status, 200, "{timeline}");
    assert_eq!(text(&timeline, "main"), "paper.md");
    let points = timeline["checkpoints"].as_array().expect("a list");
    assert_eq!(points.len(), 2, "{timeline}");

    // The first names every path, because there was nothing before it; the
    // second names only what moved, which is what makes a timeline of a
    // directory readable rather than a wall of identical rows.
    let first: Vec<&str> = points[0]["changed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|one| one.as_str().unwrap())
        .collect();
    assert_eq!(first, vec!["chapters/03.md", "paper.md"]);
    let second: Vec<&str> = points[1]["changed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|one| one.as_str().unwrap())
        .collect();
    assert_eq!(
        second,
        vec!["chapters/03.md"],
        "the untouched file was listed"
    );
    assert_eq!(points[1]["tree"], true);
}

#[tokio::test]
async fn a_history_is_as_readable_as_the_document_it_belongs_to() {
    let server = new_test_server().await;
    let owner = session_as(TEST_PUBLISHER);
    let document = publish_test_document(&server.url).await;
    let slug = text(&document, "slug");
    let path = format!("/api/documents/{slug}/history");
    let key = read_key_of(&document);

    // Whoever holds the read link `publish` printed can read what the
    // document used to say, exactly as they can read the document.
    assert_eq!(
        get_same_origin_keyed("", &key, &server.url, &path).await.0,
        200,
        "a link holder was refused the history"
    );

    // The bare URL never opened the history for a stranger, and revoking the
    // read link -- the only thing that ever let a stranger in -- leaves them
    // with nothing, same as it does for the document.
    let (status, said) = post_as(
        &owner,
        &server.url,
        &format!("/api/documents/{slug}/share"),
        json!({"revoke": "reader"}),
    )
    .await;
    assert_eq!(status, 200, "{said}");
    assert_eq!(
        get_same_origin_keyed("", &key, &server.url, &path).await.0,
        404,
        "a revoked link still read the history"
    );
    assert_eq!(get_same_origin(&owner, &server.url, &path).await.0, 200);
}
