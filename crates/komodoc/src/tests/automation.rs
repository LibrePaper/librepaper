//! The HTTP boundary used by link-scoped automation peers.

use std::collections::HashMap;

use serde_json::{json, Value};

use super::*;

#[tokio::test]
async fn bundled_agent_documents_are_allowlisted() {
    let server = new_test_server().await;
    for path in [
        "/skills/komodoc/SKILL.md",
        "/docs/protocol/room-v1.md",
        "/docs/protocol/chat.md",
    ] {
        let response = client()
            .get(format!("{}{path}", server.url))
            .send()
            .await
            .expect("a response");
        assert_eq!(response.status(), 200, "{path}");
        assert!(!response.text().await.unwrap().is_empty(), "{path}");
    }
}

async fn automation_get(cookie: &str, key: &str, base: &str, path: &str) -> (u16, Value) {
    let mut request = client()
        .get(format!("{base}{path}"))
        .header("x-komodoc-client", "1")
        .header(crate::server::AUTOMATION_HEADER, "1");
    if !cookie.is_empty() {
        request = request.header("cookie", cookie);
    }
    if !key.is_empty() {
        request = request.header(crate::server::LINK_HEADER, key);
    }
    let response = request.send().await.expect("a response");
    let status = response.status().as_u16();
    let body = response.bytes().await.unwrap_or_default();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

async fn automation_post(
    cookie: &str,
    key: &str,
    base: &str,
    path: &str,
    payload: Value,
) -> (u16, Value) {
    let mut headers = HashMap::new();
    headers.insert("content-type", "application/json".to_string());
    headers.insert("x-komodoc-client", "1".to_string());
    headers.insert(crate::server::AUTOMATION_HEADER, "1".to_string());
    if !cookie.is_empty() {
        headers.insert("cookie", cookie.to_string());
    }
    if !key.is_empty() {
        headers.insert(crate::server::LINK_HEADER, key.to_string());
    }
    raw_post(base, path, headers, payload).await
}

async fn mint_role(base: &str, slug: &str, role: &str) -> String {
    let (status, payload) = post_as(
        &session_as(TEST_PUBLISHER),
        base,
        &format!("/api/documents/{slug}/share"),
        json!({"link": {"role": role, "until": "never"}}),
    )
    .await;
    assert_eq!(status, 200, "minting {role} link: {payload}");
    let key = text(&payload, "key");
    assert!(!key.is_empty(), "minting {role} link returned no key");
    key
}

#[tokio::test]
async fn automation_snapshot_is_link_scoped_and_consistent() {
    let server = new_test_server().await;
    let document = publish_test_document(&server.url).await;
    let slug = text(&document, "slug");
    let key = read_key_of(&document);

    // A cached owner session cannot turn a reader link into an owner.
    let (status, snapshot) = automation_get(
        &session_as(TEST_PUBLISHER),
        &key,
        &server.url,
        &format!("/api/documents/{slug}/snapshot"),
    )
    .await;
    assert_eq!(status, 200, "{snapshot}");
    assert_eq!(snapshot["role"], "reader");
    assert_eq!(snapshot["capabilities"]["edit"], false);
    assert_eq!(
        snapshot["source_sha"],
        crate::store::digest_of(snapshot["source"].as_str().unwrap())
    );
    assert_eq!(snapshot["main"], snapshot["tree"]["main"]);
    assert_eq!(
        snapshot["texts"][snapshot["main"].as_str().unwrap()],
        snapshot["source"]
    );

    // The same cached owner session cannot use a link minted for another
    // document, and a revoked/expired link is removed from an active read.
    let other = publish_test_document(&server.url).await;
    let other_slug = text(&other, "slug");
    let (status, _) = automation_get(
        &session_as(TEST_PUBLISHER),
        &key,
        &server.url,
        &format!("/api/documents/{other_slug}/snapshot"),
    )
    .await;
    assert_eq!(status, 404);
    server
        .instance
        .store
        .modify(&slug, |entry| {
            if let Some(link) = entry.links.iter_mut().find(|link| link.key == key) {
                link.until = crate::clock::format_unix(crate::clock::now_unix() - 1);
            }
            Ok(())
        })
        .await
        .expect("expire the test link");
    let (status, _) = automation_get(
        &session_as(TEST_PUBLISHER),
        &key,
        &server.url,
        &format!("/api/documents/{slug}/snapshot"),
    )
    .await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn automation_annotation_is_attributed_to_the_link_and_deduplicated() {
    let server = new_test_server().await;
    let document = publish_test_document(&server.url).await;
    let slug = text(&document, "slug");
    let key = mint_role(&server.url, &slug, "commenter").await;
    let endpoint = format!("/api/documents/{slug}/comments");
    let payload = json!({
        "type": "comment",
        "exact": "hello",
        "body": "agent note",
        "motivation": "commenting",
        "temp_id": "11111111-1111-4111-8111-111111111111",
        "request_id": "annotation-1",
    });
    let (status, first) = automation_post(
        &session_as(TEST_PUBLISHER),
        &key,
        &server.url,
        &endpoint,
        payload.clone(),
    )
    .await;
    assert_eq!(status, 200, "{first}");
    assert_eq!(first["request_id"], "annotation-1");
    assert_eq!(first["version"], 1);
    assert_eq!(first["comment"]["creator"], "vincent");
    // The same signed-in person still owns this comment in the ordinary
    // browser, where requests do not carry the automation marker.
    let browser: Value = client()
        .get(format!("{}{}", server.url, endpoint))
        .header("cookie", session_as(TEST_PUBLISHER))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(browser["comments"][0]["mine"], true, "{browser}");
    let (status, retry) = automation_post(
        &session_as(TEST_PUBLISHER),
        &key,
        &server.url,
        &endpoint,
        payload,
    )
    .await;
    assert_eq!(status, 200, "{retry}");
    assert_eq!(retry["noop"], true);
    assert_eq!(retry["comment"]["id"], first["comment"]["id"]);

    let mut socket = dial_websocket_keyed(&server.url, &slug, &key).await;
    assert_eq!(socket.read().await["type"], "hello");
    socket
        .write(json!({"type":"y-checkpoint","request_id":"checkpoint-denied"}))
        .await;
    let error = socket.read().await;
    assert_eq!(error["type"], "error");
    assert_eq!(error["request_id"], "checkpoint-denied");
}

#[tokio::test]
async fn automation_checkpoint_is_explicit_and_durable_or_noop() {
    let server = new_test_server().await;
    let document = publish_test_document(&server.url).await;
    let slug = text(&document, "slug");
    let key = mint_role(&server.url, &slug, "editor").await;
    let mut socket = dial_websocket_with(
        &server.url,
        &slug,
        &format!(
            "{}: {key}\r\nx-komodoc-automation: 1\r\ncookie: {}\r\n",
            crate::server::LINK_HEADER,
            session_as(TEST_PUBLISHER)
        ),
    )
    .await
    .unwrap();
    assert_eq!(socket.read().await["type"], "hello");
    socket
        .write(json!({"type":"y-checkpoint","why":"sync","request_id":"checkpoint-1"}))
        .await;
    let result = socket.read().await;
    assert_eq!(result["type"], "y-checkpoint", "{result}");
    assert_eq!(result["request_id"], "checkpoint-1");
    assert_eq!(result["durable"], true, "{result}");
    assert!(
        result["noop"] == true || result["sha"].as_str().is_some(),
        "{result}"
    );
}

#[tokio::test]
async fn automation_never_elevates_an_unowned_or_example_document() {
    let server = test_server_with(
        crate::config::Configuration::default(),
        crate::auth::Policy::parse("anyone"),
        crate::auth::Policy::parse("anyone"),
        true,
    )
    .await;
    let (status, document) = post_as(
        "",
        &server.url,
        "/api/documents",
        json!({"title": "Unowned", "html": "<p>unowned</p>"}),
    )
    .await;
    assert_eq!(status, 201, "{document}");
    let slug = text(&document, "slug");
    let key = read_key_of(&document);
    let (status, snapshot) = automation_get(
        &session_as("cached-owner"),
        &key,
        &server.url,
        &format!("/api/documents/{slug}/snapshot"),
    )
    .await;
    assert_eq!(status, 200, "{snapshot}");
    assert_eq!(snapshot["role"], "reader");
    assert_eq!(snapshot["capabilities"]["edit"], false);

    // An example is public to read, but an automation request without an
    // explicit editor link still cannot turn the public fallback into edit.
    server
        .instance
        .store
        .modify(&slug, |entry| {
            entry.example = true;
            entry.links.clear();
            Ok(())
        })
        .await
        .expect("mark the document as an example");
    let (status, snapshot) = automation_get(
        &session_as("cached-owner"),
        "",
        &server.url,
        &format!("/api/documents/{slug}/snapshot"),
    )
    .await;
    assert_eq!(status, 200, "{snapshot}");
    assert_eq!(snapshot["role"], "reader");
    assert_eq!(snapshot["capabilities"]["comment"], false);
    assert_eq!(snapshot["capabilities"]["edit"], false);
}

#[tokio::test]
async fn mailbox_requires_both_current_document_access_and_chat_token() {
    let server = new_test_server().await;
    let document = publish_test_document(&server.url).await;
    let slug = text(&document, "slug");
    let key = read_key_of(&document);
    let endpoint = format!("{}/api/documents/{slug}/chat", server.url);
    let create = client()
        .post(&endpoint)
        .header("x-komodoc-client", "1")
        .header(crate::server::AUTOMATION_HEADER, "1")
        .header(crate::server::LINK_HEADER, &key)
        .send()
        .await
        .unwrap();
    assert_eq!(create.status(), 200);
    let created: Value = create.json().await.unwrap();
    let endpoint = format!("{endpoint}/{}", text(&created, "id"));
    let token = text(&created, "token");
    for (read_key, chat_key, status) in [
        (&key, &token, 200),
        (&key, &String::new(), 404),
        (&String::new(), &token, 404),
    ] {
        let response = client()
            .get(&endpoint)
            .header("x-komodoc-client", "1")
            .header(crate::server::AUTOMATION_HEADER, "1")
            .header(crate::server::LINK_HEADER, read_key)
            .header("x-komodoc-chat-token", chat_key)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
    let response = client()
        .post(&endpoint)
        .header("x-komodoc-client", "1")
        .header(crate::server::AUTOMATION_HEADER, "1")
        .header(crate::server::LINK_HEADER, &key)
        .header("x-komodoc-chat-token", &token)
        .json(&json!({"id":"one","role":"user","text":"Hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let (status, _) = post_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("/api/documents/{slug}/share"),
        json!({"revoke":"reader"}),
    )
    .await;
    assert_eq!(status, 200);
    // Document access is reevaluated even for a previously accepted chat token.
    let response = client()
        .get(&endpoint)
        .header("x-komodoc-client", "1")
        .header(crate::server::AUTOMATION_HEADER, "1")
        .header(crate::server::LINK_HEADER, &key)
        .header("x-komodoc-chat-token", &token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
}
