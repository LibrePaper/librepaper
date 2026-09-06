use serde_json::json;

use super::*;
use crate::config::Configuration;
use crate::store::Publication;

#[tokio::test]
async fn upload_needs_a_signed_in_publisher() {
    let server = new_test_server().await;
    let (status, payload) = post_as(
        "",
        &server.url,
        "/api/documents",
        json!({"title": "x", "html": "<p>x</p>"}),
    )
    .await;
    assert_eq!(status, 401, "anonymous upload got {status} {payload}");
    assert_eq!(text(&payload, "error"), "sign in to publish");

    // Signed in, but not one of the allowed logins.
    let (status, payload) = post_as(
        &session_as("stranger"),
        &server.url,
        "/api/documents",
        json!({"title": "x", "html": "<p>x</p>"}),
    )
    .await;
    assert_eq!(status, 403, "stranger's upload got {status} {payload}");
    assert!(text(&payload, "error").contains("stranger may not publish"));

    // A forged cookie is not a sign-in.
    let (status, _) = post_as(
        &format!("{}=nonsense.nonsense", crate::auth::SESSION_COOKIE),
        &server.url,
        "/api/documents",
        json!({"title": "x", "html": "<p>x</p>"}),
    )
    .await;
    assert_eq!(status, 401, "forged cookie got {status}");
}

#[tokio::test]
async fn publish_then_serve_document() {
    let server = new_test_server().await;
    let document = publish_test_document(&server.url).await;
    let config = Configuration::default();

    let slug = text(&document, "slug");
    assert!(
        slug.starts_with("my-paper-") && slug.len() == "my-paper-".len() + config.suffix_length,
        "slug {slug}"
    );
    assert_eq!(text(&document, "url"), format!("/docs/{slug}"));

    let shell_path = format!("/raw/{slug}/");

    // Asking the reader's own host for a document sends you to the other one.
    let redirect = client()
        .get(format!("{}/raw/{slug}", server.url))
        .send()
        .await
        .unwrap();
    assert_eq!(redirect.status().as_u16(), 302);
    let location = redirect
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(location.contains("docs."), "redirected to {location}");

    // On the document host the frame is served, with the agent in it and a CSP
    // that lets a document run its own scripts while pinning who may frame it.
    // This document's format is `html`, so it is served as the page it is --
    // that is the one format whose renderer is the identity, and a page with
    // scripts of its own has to be a page rather than an innerHTML of one.
    // It is served only to a frame URL carrying the token the reader fetched
    // on the origin that knows who is asking, which here is the owner.
    let query = frame_query(&session_as(TEST_PUBLISHER), "", &server.url, &slug).await;
    let response = on_docs_host(&server.url, &format!("{shell_path}?{query}")).await;
    let csp = response
        .headers()
        .get("content-security-policy")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let body = response.text().await.unwrap();
    assert!(
        body.contains("<script src=\"/agent.js?reader="),
        "the in-frame agent was not injected: {body}"
    );
    assert!(
        body.contains("hello world"),
        "an html document is not served as itself: {body}"
    );
    assert!(
        csp.contains("script-src 'self'") && csp.contains("frame-ancestors http://"),
        "document CSP was {csp}"
    );

    // The agent itself is served from the document origin, and nothing else is.
    assert_eq!(
        on_docs_host(&server.url, "/agent.js")
            .await
            .status()
            .as_u16(),
        200
    );
    for path in ["/", "/api/me", &format!("/docs/{slug}"), "/komodoc.css"] {
        assert_eq!(
            on_docs_host(&server.url, path).await.status().as_u16(),
            404,
            "{path} exists on the document origin"
        );
    }

    // The stable URL redirects to the document's own frame, on the document
    // origin. There is one version, so there is no digest in it.
    let redirect = client()
        .get(format!("{}/raw/{slug}", server.url))
        .send()
        .await
        .unwrap();
    assert_eq!(redirect.status().as_u16(), 302);
    let location = redirect
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(location.ends_with(&shell_path), "got {location}");
}

#[tokio::test]
async fn republish_keeps_slug_and_comments() {
    let server = new_test_server().await;
    let first = publish_test_document(&server.url).await;
    let slug = text(&first, "slug");
    let key = comment_key(&session_as(TEST_PUBLISHER), &server.url, &slug).await;

    let mut socket = dial_websocket_keyed(&server.url, &slug, &key).await;
    socket.read().await; // hello
    socket
        .write(json!({"type": "comment", "exact": "hello", "body": "a note", "creator": "Reader"}))
        .await;
    socket.read().await;

    let (status, second) = post(
        &server.url,
        "/api/documents",
        json!({"title": "My Paper", "slug": slug, "html": "<!doctype html><p>hello revised world</p>"}),
    )
    .await;
    assert_eq!(status, 201, "republish returned {status}: {second}");
    assert_eq!(text(&second, "slug"), slug);
    // The document says what was published, whether or not the moment earned a
    // second mark in the timeline: a publish within thirty seconds of the last
    // checkpoint is deferred, so the checkpoint the index names may still be
    // the first one. What must have moved is the text.
    assert_eq!(
        server.instance.rooms.get(&slug).await.source().await,
        "<!doctype html><p>hello revised world</p>",
    );
    assert_eq!(
        text(&second, "created_at"),
        text(&first, "created_at"),
        "republish should keep the creation time"
    );

    let (_, document) =
        get_json_keyed("", &key, &server.url, &format!("/api/documents/{slug}")).await;
    assert_eq!(
        document["comment_count"],
        json!(1),
        "comment did not survive the republish: {document}"
    );
    // The directory's paths travel with the document, for the landing page's
    // search: a project is found by its files as well as by its title.
    assert_eq!(
        document["files"],
        json!(["main.html"]),
        "the document endpoint should list the directory: {document}"
    );
}

#[tokio::test]
async fn comments_broadcast_to_every_reader() {
    let server = new_test_server().await;
    let slug = text(&publish_test_document(&server.url).await, "slug");
    // The author holds a commenter link; the watcher only needs to read, so
    // the same link -- which carries at least a reader's rung -- does for
    // both.
    let key = comment_key(&session_as(TEST_PUBLISHER), &server.url, &slug).await;

    let mut author = dial_websocket_keyed(&server.url, &slug, &key).await;
    let mut watcher = dial_websocket_keyed(&server.url, &slug, &key).await;
    for socket in [&mut author, &mut watcher] {
        let hello = socket.read().await;
        assert_eq!(hello["type"], "hello");
    }

    // The "creator" the socket sends is never trusted; with no visitor
    // cookie on this connection there is nothing to key a pseudonym on, so
    // the comment lands as "Anonymous" regardless of what was typed here.
    author
        .write(json!({"type": "comment", "exact": "hello", "body": "a note", "creator": "Reader", "temp_id": "t1"}))
        .await;

    // The sender gets the echo too, which is how it reconciles the row it
    // drew optimistically.
    for socket in [&mut author, &mut watcher] {
        let event = socket.read().await;
        assert_eq!(event["type"], "comment", "got {event}");
        let comment = &event["comment"];
        assert_eq!(comment["body"], "a note");
        assert_eq!(comment["creator"], "Anonymous");
        assert_eq!(comment["exact"], "hello");
        assert_eq!(comment["resolved"], false);
        assert_eq!(comment["seq"], 1);
    }

    // Replies and resolves reach everyone the same way.
    let (_, listing) = get_json_keyed(
        "",
        &key,
        &server.url,
        &format!("/api/documents/{slug}/comments"),
    )
    .await;
    let comments = listing["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 1);
    let identifier = text(&comments[0], "id");

    author
        .write(json!({"type": "resolve", "comment_id": identifier, "resolved": true}))
        .await;
    let event = watcher.read().await;
    assert_eq!(event["type"], "resolve");
    assert_eq!(event["resolved"], true);
    assert!(
        !event["resolved_at"].is_null(),
        "resolve came back as {event}"
    );
}

#[tokio::test]
async fn comment_validation() {
    let server = new_test_server().await;
    let slug = text(&publish_test_document(&server.url).await, "slug");
    // A commenter link: the caller has to hold enough to comment at all
    // before what it typed can be checked for validity.
    let key = comment_key(&session_as(TEST_PUBLISHER), &server.url, &slug).await;
    let mut socket = dial_websocket_keyed(&server.url, &slug, &key).await;
    socket.read().await;

    let cases = [
        (
            "no body",
            json!({"type": "comment", "exact": "hello"}),
            "comment body is required",
        ),
        (
            "no anchor",
            json!({"type": "comment", "body": "x"}),
            "select some text or part of a figure to comment on",
        ),
        (
            "unknown type",
            json!({"type": "wat", "body": "x"}),
            "unknown message type",
        ),
        (
            "unknown comment",
            json!({"type": "reply", "comment_id": "nope", "body": "x"}),
            "unknown comment",
        ),
    ];
    for (name, message, want) in cases {
        socket.write(message).await;
        let event = socket.read().await;
        assert_eq!(event["type"], "error", "{name}: got {event}");
        assert_eq!(event["message"], want, "{name}: got {event}");
    }
}

#[tokio::test]
async fn listing_and_delete() {
    let server = new_test_server().await;
    let slug = text(&publish_test_document(&server.url).await, "slug");

    // Knowing a document's link must not reveal the others.
    let (status, payload) = post_as("", &server.url, "/api/list", json!({})).await;
    assert_eq!(status, 401, "anonymous listing returned {status} {payload}");
    let (status, payload) = post(&server.url, "/api/list", json!({})).await;
    assert_eq!(status, 200);
    assert_eq!(payload["documents"].as_array().unwrap().len(), 1);

    let (status, deleted) = post(
        &server.url,
        &format!("/api/documents/{slug}/delete"),
        json!({}),
    )
    .await;
    assert_eq!(status, 200, "delete returned {status} {deleted}");
    assert_eq!(deleted["deleted"], slug);
    // There are no stored versions any more, so none is counted as removed;
    // what goes is the document, its history and its comments.
    assert_eq!(deleted["versions_removed"], 0);
    let (status, _) = get_json(&server.url, &format!("/api/documents/{slug}")).await;
    assert_eq!(status, 404, "document still present after delete");
}

#[tokio::test]
async fn shell_routes() {
    let server = new_test_server().await;
    // The reader is only served for a document that exists.
    let slug = "a-paper-abcdefghij";
    server
        .instance
        .store
        .put(Publication {
            slug: slug.into(),
            title: "A Paper".into(),
            source: "<p>p</p>".into(),
            source_format: "html".into(),
            ..Publication::default()
        })
        .await
        .unwrap();
    // The pages, and the bundle the reader loads: the bundler decides that
    // file's name, so this reads it out of the page rather than knowing it.
    let shell = crate::assets::load_shell(&Configuration::default()).unwrap();
    let bundle = regex::Regex::new(r#"src="(/assets/[^"]+\.js)""#)
        .unwrap()
        .captures(&shell["/reader.html"].text())
        .expect("the reader page names no bundle")[1]
        .to_string();
    for (path, want) in [
        ("/", "<!doctype html"),
        (&format!("/docs/{slug}"), "<!doctype html"),
        ("/documentation", "<!doctype html"),
        (bundle.as_str(), "/api/documents/"),
    ] {
        let response = client()
            .get(format!("{}{path}", server.url))
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let body = response.text().await.unwrap().to_lowercase();
        assert!(
            status == 200 && body.contains(want),
            "{path} returned {status}, body did not contain {want:?}"
        );
    }
}

// A link nobody can follow used to answer 200 with the reader shell, which
// left the reader looking broken rather than the link. It gets the 404 page,
// with the status to match.
#[tokio::test]
async fn unknown_paths_get_the_not_found_page() {
    let server = new_test_server().await;
    for path in ["/docs/no-such-document", "/nothing-here"] {
        let response = client()
            .get(format!("{}{path}", server.url))
            .header("accept", "text/html")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 404, "{path}");
        let body = response.text().await.unwrap();
        assert!(
            body.contains("Nothing here"),
            "{path} did not serve the 404 page: {:.80}",
            body
        );
    }
}

#[tokio::test]
async fn oversized_upload_is_refused_by_size() {
    let server = new_test_server().await;
    let html = format!(
        "<p>{}</p>",
        "x".repeat(Configuration::default().max_document)
    );
    let (status, payload) = post(
        &server.url,
        "/api/documents",
        json!({"title": "Big", "html": html}),
    )
    .await;
    assert_eq!(status, 413, "got {status} {payload}");
    assert_eq!(text(&payload, "error"), "document too large");
}

#[tokio::test]
async fn document_origin_is_isolated() {
    let server = new_test_server().await;
    let slug = text(&publish_test_document(&server.url).await, "slug");

    // The reader is told where to frame documents from, and it is not itself.
    let (_, payload) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("/api/documents/{slug}"),
    )
    .await;
    let origin = text(&payload, "docs_origin");
    assert!(
        origin.starts_with("http://docs."),
        "docs_origin was {origin}"
    );
    assert!(!origin.trim_start_matches("http://docs.").is_empty() && origin != server.url);

    // A session cookie counts for nothing on the document origin: it serves
    // no API at all, so there is nothing there to authorise.
    let host = format!("docs.{}", server.url.trim_start_matches("http://"));
    let refused = client()
        .post(format!("{}/api/documents", server.url))
        .header("host", host)
        .header("cookie", session_as(TEST_PUBLISHER))
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status().as_u16(), 404);
}

#[tokio::test]
async fn markdown_upload_is_stored_as_markdown() {
    let server = new_test_server().await;
    let part = reqwest::multipart::Part::text("# Notes\n\nA *point* worth making.\n")
        .file_name("notes.md");
    let form = reqwest::multipart::Form::new().part("file", part);
    let response = client()
        .post(format!("{}/api/documents", server.url))
        .header("cookie", session_as(TEST_PUBLISHER))
        .header("x-komodoc-client", "1")
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status().as_u16(),
        201,
        "markdown upload returned {}",
        response.status()
    );
    let document: serde_json::Value = response.json().await.unwrap();
    // The first heading names it when no title was given.
    assert_eq!(document["title"], "Notes");
    // What is stored is the markdown, as markdown. Nothing derived is kept:
    // the browser showing it renders it, with the same module the editor
    // previews with.
    let slug = text(&document, "slug");
    let (status, payload) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("/api/documents/{slug}/source"),
    )
    .await;
    assert_eq!(status, 200, "{payload}");
    assert_eq!(text(&payload, "format"), "markdown");
    assert!(
        text(&payload, "source").contains("# Notes"),
        "the markdown was not kept: {payload}"
    );
}

// The editor is useless without the renderer, so the server has to serve it:
// as WebAssembly, and cacheable for a year.
#[tokio::test]
async fn the_renderer_is_served_as_wasm() {
    let server = new_test_server().await;
    let response = client()
        .get(format!(
            "{}{}",
            server.url,
            crate::assets::module_url("markdown").expect("a markdown module")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/wasm"
    );
    assert!(response
        .headers()
        .get("cache-control")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("immutable"));
    let raw = response.bytes().await.unwrap();
    assert!(
        raw.len() > 8 && &raw[..4] == b"\0asm",
        "the module is not WebAssembly"
    );
    assert!(
        raw.len() < 3 * 1024 * 1024,
        "the markdown module is {} bytes; it should be small",
        raw.len()
    );
}

/// The next frame of this kind, skipping the peer counts a join broadcasts to
/// everyone already in the session.
async fn wait_for(socket: &mut Socket, kind: &str) -> serde_json::Value {
    for _ in 0..6 {
        let frame = socket.read().await;
        if frame["type"] == kind {
            return frame;
        }
    }
    panic!("no {kind} frame arrived");
}

// Two people editing the same document see each other: where the other one's
// caret is, and what they are called. Awareness is relayed to the rest of the
// session and to nobody else, and never kept -- it describes who is here now,
// which is worth nothing to whoever arrives next.
#[tokio::test]
async fn awareness_reaches_the_other_editors_and_is_not_kept() {
    let server = new_test_server().await;
    let slug = text(
        &crate::tests::edit::publish_with_source(&server.url).await,
        "slug",
    );

    // Editing is for whoever may replace the document, so both sockets carry
    // the publisher, the way the editor in a signed-in browser does.
    let cookie = format!("Cookie: {}\r\n", session_as(TEST_PUBLISHER));
    let mut alice = dial_websocket_with(&server.url, &slug, &cookie)
        .await
        .expect("alice");
    let mut bob = dial_websocket_with(&server.url, &slug, &cookie)
        .await
        .expect("bob");
    alice.read().await; // hello
    bob.read().await;

    alice.write(json!({"type": "y-open"})).await;
    assert_eq!(alice.read().await["type"], "y-state");
    bob.write(json!({"type": "y-open"})).await;
    // Bob is told the session state, and both are told the session grew.
    wait_for(&mut bob, "y-state").await;

    alice
        .write(json!({"type": "y-awareness", "update": "AQID"}))
        .await;
    let seen = wait_for(&mut bob, "y-awareness").await;
    assert_eq!(
        seen["update"], "AQID",
        "the awareness update was changed on the way through"
    );

    // A third editor arriving is given the document's state, and nothing about
    // where anyone's caret was: that is not a fact about the document.
    let mut carol = dial_websocket_with(&server.url, &slug, &cookie)
        .await
        .expect("carol");
    carol.read().await; // hello
    carol.write(json!({"type": "y-open"})).await;
    let state = wait_for(&mut carol, "y-state").await;
    assert!(
        !state.to_string().contains("AQID"),
        "a session remembered a caret: {state}"
    );
}

#[tokio::test]
async fn browser_submission_retries_are_idempotent_and_author_scoped() {
    use crate::room::Message;
    let server = new_test_server().await;
    let published = publish_test_document(&server.url).await;
    let slug = text(&published, "slug");
    let room = server.instance.rooms.get(&slug).await;
    let request = Message {
        kind: "comment".into(),
        exact: "passage".into(),
        body: "A comment whose acknowledgment was lost".into(),
        temp_id: crate::util::new_id(),
        ..Default::default()
    };
    let (first, ok) = room
        .apply(request.clone(), "", "visitor:alice", "", None, false)
        .await;
    assert!(ok);
    assert_eq!(first["comment"]["id"], request.temp_id);
    let (again, ok) = room
        .apply(request.clone(), "", "visitor:alice", "", None, false)
        .await;
    assert!(ok);
    assert_eq!(first, again);
    assert_eq!(room.counts().await.0, 1);
    let (_, ok) = room
        .apply(request.clone(), "", "visitor:bob", "", None, true)
        .await;
    assert!(
        !ok,
        "even a moderator cannot reuse another author's submission ID"
    );
    let reply = Message {
        kind: "reply".into(),
        comment_id: request.temp_id,
        body: "A reply".into(),
        temp_id: crate::util::new_id(),
        ..Default::default()
    };
    let (first, ok) = room
        .apply(reply.clone(), "", "visitor:alice", "", None, false)
        .await;
    assert!(ok);
    let (again, ok) = room
        .apply(reply, "", "visitor:alice", "", None, false)
        .await;
    assert!(ok);
    assert_eq!(first, again);
}
