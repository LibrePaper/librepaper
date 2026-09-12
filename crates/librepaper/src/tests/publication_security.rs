//! Direct requests must not bypass the rendered-reader boundary.

use super::*;
use serde_json::json;
use sha2::{Digest, Sha256};

fn docs_path(url: &str) -> String {
    let authority = url.split_once("://").expect("absolute publication URL").1;
    authority[authority.find('/').expect("publication path")..].to_string()
}

#[tokio::test]
async fn restricted_links_cannot_reconstruct_the_source_project() {
    let server = new_test_server().await;
    let owner = session_as(TEST_PUBLISHER);
    let source_secret = "PRIVATE-PROJECT-CANARY-81f0";
    let data_secret = "PRIVATE-DATA-CANARY-07be";
    let bibliography_secret = "PRIVATE-BIB-CANARY-fcc1";
    let image_secret = b"PRIVATE-IMAGE-CANARY-289d";
    let map_secret = "PRIVATE-SOURCEMAP-CANARY-9c71";
    let private_image_sha = hex::encode(Sha256::digest(image_secret));
    let form = reqwest::multipart::Form::new()
        .text("title", "Public title")
        .text("main", "main.md")
        .part(
            "file",
            reqwest::multipart::Part::bytes(
                format!("# Public title\n{source_secret}").into_bytes(),
            )
            .file_name("main.md"),
        )
        .part(
            "file",
            reqwest::multipart::Part::bytes(data_secret.as_bytes().to_vec())
                .file_name("data/private.csv"),
        )
        .part(
            "file",
            reqwest::multipart::Part::bytes(bibliography_secret.as_bytes().to_vec())
                .file_name("references/private.bib"),
        )
        .part(
            "file",
            reqwest::multipart::Part::bytes(image_secret.to_vec()).file_name("figures/private.png"),
        );
    let response = client()
        .post(format!("{}/api/documents", server.url))
        .header("cookie", &owner)
        .header("x-librepaper-client", "1")
        .multipart(form)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let created: serde_json::Value = response.json().await.unwrap();
    assert_eq!(status, 201, "{created}");
    let slug = text(&created, "slug");
    // Directory upload already rejects source maps. An editable source
    // session can still contain one, so exercise that stronger boundary.
    let room = server.instance.rooms.get(&slug).await;
    room.add_text("main.js.map", map_secret).await.unwrap();
    room.persist().await.unwrap();
    let public_image = b"PUBLIC-DISPLAY-IMAGE";
    let published = publish_display(
        &server.url,
        &owner,
        &slug,
        b"<!doctype html><p>public rendering</p><img src=\"assets/public.png\">",
        &[("assets/public.png", "image/png", public_image)],
    )
    .await;
    let publication_id = text(&published["publication"], "id");
    let secrets = [
        source_secret,
        data_secret,
        bibliography_secret,
        std::str::from_utf8(image_secret).unwrap(),
        map_secret,
        "data/private.csv",
        "references/private.bib",
        "figures/private.png",
        "main.js.map",
    ];
    let mut reader_key = String::new();
    for role in ["reader", "commenter", "editor"] {
        let (status, links) = post_as(
            &owner,
            &server.url,
            &format!("/api/documents/{slug}/share"),
            json!({"link":{"role":role,"until":"never"}}),
        )
        .await;
        assert_eq!(status, 200, "{links}");
        let key = text(&links, "key");
        assert!(!key.is_empty(), "{links}");
        if role == "reader" {
            reader_key = key.clone();
        }
        // An account used for attribution cannot elevate a restricted link.
        for endpoint in ["source", "snapshot", "history", "state"] {
            let response = client()
                .get(format!("{}/api/documents/{slug}/{endpoint}", server.url))
                .header("cookie", &owner)
                .header(crate::server::LINK_HEADER, &key)
                .header(crate::server::AUTOMATION_HEADER, "1")
                .header("x-librepaper-client", "1")
                .send()
                .await
                .expect("source endpoint response");
            let status = response.status().as_u16();
            let body = response.text().await.expect("response bytes");
            if role != "editor" {
                assert!(
                    matches!(status, 401 | 403 | 404),
                    "{role} read {endpoint}: {status} {body}"
                );
                assert!(
                    secrets.iter().all(|secret| !body.contains(secret)),
                    "private source tree leaked in refusal: {body}"
                );
            } else if matches!(endpoint, "source" | "snapshot") {
                assert_eq!(status, 200, "editor could not read {endpoint}: {body}");
                assert!(body.contains(source_secret), "editor source lost");
            }
        }
        if role != "editor" {
            let response = client()
                .post(format!(
                    "{}/api/documents/{slug}/publication/prepare",
                    server.url
                ))
                .header("cookie", &owner)
                .header(crate::server::LINK_HEADER, &key)
                .header(crate::server::AUTOMATION_HEADER, "1")
                .header("x-librepaper-client", "1")
                .json(&json!({}))
                .send()
                .await
                .expect("publication refusal");
            assert!(
                matches!(response.status().as_u16(), 401 | 403 | 404),
                "{role} admitted publication upload"
            );
        }
    }

    // The reader sees only display metadata and the explicit public asset.
    let (status, metadata) = get_json_keyed(
        "",
        &reader_key,
        &server.url,
        &format!("/api/documents/{slug}"),
    )
    .await;
    assert_eq!(status, 200, "{metadata}");
    let encoded = metadata.to_string();
    assert!(
        secrets.iter().all(|secret| !encoded.contains(secret)),
        "{metadata}"
    );
    let (status, comments) = get_json_keyed(
        "",
        &reader_key,
        &server.url,
        &format!("/api/documents/{slug}/comments"),
    )
    .await;
    assert_eq!(status, 200, "{comments}");
    assert!(
        secrets
            .iter()
            .all(|secret| !comments.to_string().contains(secret)),
        "reader comments leaked private source: {comments}"
    );

    let response = client()
        .get(format!("{}/api/documents/{slug}/publication", server.url))
        .header(crate::server::LINK_HEADER, &reader_key)
        .header("x-librepaper-client", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let reader_publication: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        text(&reader_publication["publication"], "id"),
        publication_id
    );
    let page = docs_path(
        reader_publication["publication"]["html_url"]
            .as_str()
            .unwrap(),
    );
    let response = on_docs_host(&server.url, &page).await;
    assert_eq!(response.status().as_u16(), 200);
    let display_cookie = response
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let display = response.text().await.unwrap();
    assert!(display.contains("assets/public.png"), "{display}");
    assert!(
        secrets.iter().all(|secret| !display.contains(secret)),
        "{display}"
    );

    for guessed in [
        format!("/api/documents/{slug}/assets/{private_image_sha}"),
        format!("/api/documents/{slug}/assets/{}", "0".repeat(64)),
        format!("/published/{slug}/{publication_id}/assets/private.png"),
        format!("/published/{slug}/{publication_id}/assets/main.js.map"),
    ] {
        let mut request = client().get(format!("{}{}", server.url, guessed));
        if guessed.starts_with("/api/") {
            request = request
                .header(crate::server::LINK_HEADER, &reader_key)
                .header("x-librepaper-client", "1");
        } else {
            request = request
                .header(
                    "host",
                    format!("docs.{}", server.url.trim_start_matches("http://")),
                )
                .header("cookie", &display_cookie);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let body = response.text().await.unwrap();
        assert!(
            matches!(status, 401 | 403 | 404),
            "guessed private asset {guessed}: {status} {body}"
        );
        assert!(
            secrets.iter().all(|secret| !body.contains(secret)),
            "private asset refusal leaked source: {body}"
        );
    }
}

#[tokio::test]
async fn reader_socket_cannot_open_source_or_receive_editorial_anchors() {
    let server = new_test_server().await;
    let owner = session_as(TEST_PUBLISHER);
    let secret = "PRIVATE-WS-SOURCE-CANARY-c87e";
    let (status, created) = post_as(
        &owner,
        &server.url,
        "/api/documents",
        json!({"title":"Socket boundary", "source":format!("# Socket boundary\n{secret}"), "source_format":"markdown"}),
    )
    .await;
    assert_eq!(status, 201, "{created}");
    let slug = text(&created, "slug");

    let (status, reader_link) = post_as(
        &owner,
        &server.url,
        &format!("/api/documents/{slug}/share"),
        json!({"link":{"role":"reader","until":"never"}}),
    )
    .await;
    assert_eq!(status, 200, "{reader_link}");
    let reader_key = text(&reader_link, "key");
    let (status, commenter_link) = post_as(
        &owner,
        &server.url,
        &format!("/api/documents/{slug}/share"),
        json!({"link":{"role":"commenter","until":"never"}}),
    )
    .await;
    assert_eq!(status, 200, "{commenter_link}");
    let commenter_key = text(&commenter_link, "key");
    let (status, editor_link) = post_as(
        &owner,
        &server.url,
        &format!("/api/documents/{slug}/share"),
        json!({"link":{"role":"editor","until":"never"}}),
    )
    .await;
    assert_eq!(status, 200, "{editor_link}");
    let editor_key = text(&editor_link, "key");

    let mut reader = dial_websocket_keyed(&server.url, &slug, &reader_key).await;
    assert_eq!(reader.read().await["type"], "hello");
    reader
        .write(json!({"type":"y-open", "request_id":"source-open"}))
        .await;
    let refusal = reader.read().await;
    assert_eq!(refusal["type"], "error", "{refusal}");
    assert!(
        !refusal.to_string().contains(secret),
        "source canary leaked through source-sync refusal: {refusal}"
    );

    // A forged empty publication ID cannot turn a rendered annotation into
    // an unchecked source-era comment, on either transport.
    let (status, refusal) = post_keyed(
        "",
        &commenter_key,
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({"type":"comment","exact":"public selection","body":"note","publication_id":""}),
    )
    .await;
    assert_eq!(status, 409, "{refusal}");
    assert!(
        !refusal.to_string().contains(secret),
        "HTTP refusal leaked source: {refusal}"
    );
    let mut commenter = dial_websocket_keyed(&server.url, &slug, &commenter_key).await;
    assert_eq!(commenter.read().await["type"], "hello");
    commenter
        .write(json!({"type":"comment","exact":"public selection","body":"note","publication_id":"","temp_id":"forged-empty"}))
        .await;
    let refusal = commenter.read().await;
    assert_eq!(refusal["type"], "error", "{refusal}");
    assert!(
        refusal["message"]
            .as_str()
            .unwrap_or_default()
            .contains("publication is required"),
        "unexpected empty-publication refusal: {refusal}"
    );
    assert!(
        !refusal.to_string().contains(secret),
        "WebSocket refusal leaked source: {refusal}"
    );

    let mut editor = dial_websocket_with(
        &server.url,
        &slug,
        &format!(
            "Cookie: {owner}\r\n{}: {editor_key}\r\n",
            crate::server::LINK_HEADER
        ),
    )
    .await
    .unwrap();
    assert_eq!(editor.read().await["type"], "hello");
    editor
        .write(json!({
            "type":"comment", "motivation":"editing", "body":"editorial note",
            "exact":secret, "source":{"path":"main.md","exact":secret},
            "proposed":"public replacement", "temp_id":"editorial-anchor"
        }))
        .await;
    let editor_result = editor.read().await;
    assert_eq!(editor_result["type"], "comment", "{editor_result}");

    let mut reader_event = None;
    for _ in 0..6 {
        let frame = reader.read().await;
        if frame["type"] == "annotation-redacted" {
            reader_event = Some(frame);
            break;
        }
    }
    let reader_event = reader_event.expect("reader did not receive the redacted annotation event");
    assert!(
        !reader_event.to_string().contains(secret),
        "source anchor leaked to reader socket: {reader_event}"
    );

    // Empty publication IDs also identify ordinary editor-local comments,
    // not only suggestions. They and their replies must stay out of reader
    // snapshots and socket events.
    let (status, local) = post_as(
        &owner,
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({"type":"comment","exact":secret,"body":"local source note"}),
    )
    .await;
    assert_eq!(status, 200, "{local}");
    let local_id = text(&local["comment"], "id");
    let local_event = reader.read().await;
    assert_eq!(local_event["type"], "annotation-redacted", "{local_event}");
    assert!(!local_event.to_string().contains(secret), "{local_event}");

    let (status, reply) = post_as(
        &owner,
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({"type":"reply","comment_id":local_id,"body":"local follow-up"}),
    )
    .await;
    assert_eq!(status, 200, "{reply}");
    let reply_event = reader.read().await;
    assert_eq!(reply_event["type"], "annotation-redacted", "{reply_event}");
    assert!(
        !reply_event.to_string().contains("local follow-up"),
        "{reply_event}"
    );

    let (status, reader_comments) = get_json_keyed(
        "",
        &reader_key,
        &server.url,
        &format!("/api/documents/{slug}/comments"),
    )
    .await;
    assert_eq!(status, 200, "{reader_comments}");
    assert!(
        reader_comments["comments"]
            .as_array()
            .unwrap()
            .iter()
            .all(|comment| text(comment, "id") != local_id),
        "reader snapshot exposed an editor-local annotation: {reader_comments}"
    );
}

#[tokio::test]
async fn restricted_automation_routes_refuse_without_source_details() {
    let server = new_test_server().await;
    let owner = session_as(TEST_PUBLISHER);
    let secret = "PRIVATE-AUTOMATION-CANARY-1ed9";
    let (status, document) = post_as(
        &owner,
        &server.url,
        "/api/documents",
        json!({"title":"Automation boundary","source":secret,"source_format":"markdown"}),
    )
    .await;
    assert_eq!(status, 201, "{document}");
    let slug = text(&document, "slug");
    let (status, link) = post_as(
        &owner,
        &server.url,
        &format!("/api/documents/{slug}/share"),
        json!({"link":{"role":"reader","until":"never"}}),
    )
    .await;
    assert_eq!(status, 200, "{link}");
    let key = text(&link, "key");

    let response = client()
        .post(format!("{}/api/documents/{slug}/suggestions", server.url))
        .header("cookie", &owner)
        .header(crate::server::LINK_HEADER, &key)
        .header("x-librepaper-client", "1")
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let body = response.text().await.unwrap();
    assert_eq!(status, 403, "assistant route admitted reader: {body}");
    assert!(
        !body.contains(secret),
        "assistant refusal leaked source: {body}"
    );

    let request = json!({
        "jsonrpc":"2.0", "id":"reader", "method":"tools/list",
        "params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}
    });
    let response = client()
        .post(format!("{}/api/documents/{slug}/mcp", server.url))
        .header("cookie", &owner)
        .header(crate::server::LINK_HEADER, &key)
        .header("x-librepaper-client", "1")
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", "tools/list")
        .json(&request)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let body = response.text().await.unwrap();
    assert_eq!(status, 404, "MCP route admitted reader: {body}");
    assert!(!body.contains(secret), "MCP refusal leaked source: {body}");
}
