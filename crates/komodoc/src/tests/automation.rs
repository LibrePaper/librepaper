//! The HTTP boundary used by link-scoped automation peers.

use std::collections::HashMap;

use serde_json::{json, Value};

use super::*;

#[tokio::test]
async fn bundled_agent_documents_are_allowlisted() {
    let server = new_test_server().await;
    for path in [
        "/skills/komodoc-document/SKILL.md",
        "/skills/komodoc-document/references/install.md",
        "/skills/komodoc-document/references/editing.md",
        "/skills/komodoc-pair/SKILL.md",
        "/skills/komodoc-pair/references/install.md",
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
async fn automation_cannot_use_cached_owner_for_publish_list_or_delete() {
    let server = new_test_server().await;
    let document = publish_test_document(&server.url).await;
    let slug = text(&document, "slug");
    let cookie = session_as(TEST_PUBLISHER);

    let (status, answer) = automation_get(&cookie, "", &server.url, "/api/list").await;
    assert_eq!(status, 403, "automation listed documents: {answer}");
    assert_eq!(
        answer["error"],
        "automation is link-scoped and cannot publish or enumerate documents"
    );

    let (status, answer) = automation_post(
        &cookie,
        "",
        &server.url,
        "/api/documents",
        json!({"title": "automation must not publish", "html": "<p>nope</p>"}),
    )
    .await;
    assert_eq!(status, 403, "automation published a document: {answer}");

    let response = client()
        .post(format!("{}/api/documents/{slug}/delete", server.url))
        .header("x-komodoc-client", "1")
        .header(crate::server::AUTOMATION_HEADER, "1")
        .header("cookie", cookie)
        .send()
        .await
        .expect("delete response");
    assert_eq!(response.status(), 403, "automation deleted a document");
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
        crate::document::store::digest_of(snapshot["source"].as_str().unwrap())
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
                link.until = crate::util::format_unix(crate::util::now_unix() - 1);
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

type ChatSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn chat_socket(endpoint: &str, key: &str, token: &str, role: &str) -> ChatSocket {
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
    let mut request = format!("{}/socket", endpoint.replacen("http:", "ws:", 1))
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("x-komodoc-key", key.parse().unwrap());
    request
        .headers_mut()
        .insert("x-komodoc-automation", "1".parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    socket
        .send(Message::Text(
            json!({"type":"join","token":token,"role":role})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    socket
}

async fn chat_frame(socket: &mut ChatSocket, kind: &str) -> Value {
    use futures_util::StreamExt;
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            match socket.next().await {
                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                    let value: Value = serde_json::from_str(&text).unwrap();
                    if value["type"] == kind {
                        return value;
                    }
                    assert_ne!(value["type"], "error", "unexpected frame: {value}");
                }
                frame => panic!("socket ended waiting for {kind}: {frame:?}"),
            }
        }
    })
    .await
    .expect("chat frame timeout")
}

#[tokio::test]
async fn live_agent_channel_requires_access_token_and_presence() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
    let server = new_test_server().await;
    let document = publish_test_document(&server.url).await;
    let slug = text(&document, "slug");
    let key = read_key_of(&document);
    let endpoint = format!("{}/api/documents/{slug}/chat", server.url);
    let create = client()
        .post(&endpoint)
        .header("x-komodoc-client", "1")
        .header("x-komodoc-key", &key)
        .send()
        .await
        .unwrap();
    assert_eq!(create.status(), 200);
    let created: Value = create.json().await.unwrap();
    let endpoint = format!("{endpoint}/{}", text(&created, "id"));
    let token = text(&created, "token");

    // Document access is required before the WebSocket is upgraded.
    let request = format!("{}/socket", endpoint.replacen("http:", "ws:", 1))
        .into_client_request()
        .unwrap();
    assert!(tokio_tungstenite::connect_async(request).await.is_err());
    let mut stranger = chat_socket(&endpoint, &key, "wrong", "agent").await;
    assert_eq!(chat_frame(&mut stranger, "error").await["status"], 404);

    let mut browser = chat_socket(&endpoint, &key, &token, "user").await;
    assert_eq!(chat_frame(&mut browser, "ready").await["listening"], false);
    browser
        .send(Message::Text(
            json!({"type":"message","id":"early","text":"hi"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    assert_eq!(chat_frame(&mut browser, "error").await["status"], 409);

    let mut agent = chat_socket(&endpoint, &key, &token, "agent").await;
    chat_frame(&mut agent, "ready").await;
    assert_eq!(
        chat_frame(&mut browser, "presence").await["listening"],
        true
    );
    browser
        .send(Message::Text(
            json!({"type":"message","id":"one","role":"agent","text":"Hello"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let delivered = chat_frame(&mut agent, "message").await;
    assert_eq!(
        delivered["message"]["role"], "user",
        "sender role is derived from the joined socket"
    );
    assert_eq!(
        chat_frame(&mut browser, "message").await["message"]["text"],
        "Hello"
    );
    chat_frame(&mut browser, "ack").await;
    agent.close(None).await.unwrap();
    assert_eq!(
        chat_frame(&mut browser, "presence").await["listening"],
        false
    );

    // No retained transcript is available to a later agent socket.
    let mut agent = chat_socket(&endpoint, &key, &token, "agent").await;
    chat_frame(&mut agent, "ready").await;
    chat_frame(&mut browser, "presence").await;
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), agent.next())
            .await
            .is_err()
    );

    let response = client()
        .post(&endpoint)
        .header("x-komodoc-client", "1")
        .header("x-komodoc-key", &key)
        .header("x-komodoc-chat-token", &token)
        .json(&json!({"id":"reply","text":"Done"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        chat_frame(&mut browser, "message").await["message"]["role"],
        "agent"
    );
    assert_eq!(
        chat_frame(&mut agent, "message").await["message"]["text"],
        "Done"
    );

    let (status, _) = post_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("/api/documents/{slug}/share"),
        json!({"revoke":"reader"}),
    )
    .await;
    assert_eq!(status, 200);
    let next = tokio::time::timeout(std::time::Duration::from_secs(3), browser.next())
        .await
        .unwrap();
    assert!(
        !matches!(next, Some(Ok(Message::Text(_)))),
        "revocation closes the channel"
    );
    let response = client()
        .post(&endpoint)
        .header("x-komodoc-client", "1")
        .header("x-komodoc-key", &key)
        .header("x-komodoc-chat-token", &token)
        .json(&json!({"id":"late","text":"Too late"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn human_chat_is_live_permissioned_and_uses_server_message_ids() {
    let server = new_test_server().await;
    let document = publish_test_document(&server.url).await;
    let slug = text(&document, "slug");
    let reader_key = read_key_of(&document);
    let commenter_key = mint_role(&server.url, &slug, "commenter").await;
    let mut reader = dial_websocket_keyed(&server.url, &slug, &reader_key).await;
    let mut commenter = dial_websocket_keyed(&server.url, &slug, &commenter_key).await;
    reader.read().await;
    commenter.read().await;
    reader
        .write(json!({"type":"chat","temp_id":"forbidden","body":"no"}))
        .await;
    assert_eq!(reader.read().await["type"], "error");
    commenter
        .write(json!({"type":"chat","temp_id":"local","body":"hello"}))
        .await;
    let message = reader.read().await;
    assert_eq!(message["type"], "chat");
    assert_eq!(message["text"], "hello");
    assert_ne!(message["id"], "local");
    assert!(
        message.get("temp_id").is_none(),
        "private retry ids are not broadcast"
    );
    let echo = commenter.read().await;
    assert_eq!(echo["temp_id"], "local");
    commenter
        .write(json!({"type":"chat","temp_id":"local","body":"hello"}))
        .await;
    assert_eq!(commenter.read().await["type"], "chat-ack");
    let mut late = dial_websocket_keyed(&server.url, &slug, &reader_key).await;
    let hello = late.read().await;
    assert_eq!(hello["type"], "hello");
    assert!(
        hello["comments"].as_array().unwrap().is_empty(),
        "chat must not appear in hello state"
    );
}
