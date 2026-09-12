//! HTTP delivery tests for the source-free published-document origin.

use super::*;
use serde_json::json;
use sha2::{Digest, Sha256};

fn docs_path(url: &str) -> String {
    // The harness uses an IP Host and simulates docs.<ip> without DNS.
    // Extract the path without treating that synthetic Host as a DNS name.
    let authority = url.split_once("://").expect("absolute publication URL").1;
    authority[authority.find('/').expect("publication path")..].to_string()
}

#[tokio::test]
async fn published_delivery_excludes_source_and_revalidates_stable_assets() {
    let server = new_test_server().await;
    let owner = session_as(TEST_PUBLISHER);
    let (status, document) = post_as(
        &owner,
        &server.url,
        "/api/documents",
        json!({
            "title":"Display", "source":"PRIVATE-SOURCE-CANARY", "source_format":"markdown"
        }),
    )
    .await;
    assert_eq!(status, 201, "{document}");
    let slug = text(&document, "slug");
    let response = client()
        .get(format!("{}/api/documents/{slug}/publication", server.url))
        .header("cookie", &owner)
        .header("x-librepaper-client", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let meta: serde_json::Value = response.json().await.unwrap();
    assert!(meta["publication"].is_null());

    let image = b"not-a-private-input";
    let result = publish_display(&server.url, &owner, &slug,
        b"<!doctype html><img src=\"assets/image.png\"><script>window.auth='no credentials'</script>",
        &[("assets/image.png", "image/png", image), ("figures/café.png", "image/png", image)]).await;
    let page = docs_path(
        result["publication"]["html_url"]
            .as_str()
            .expect("html URL"),
    );
    let response = on_docs_host(&server.url, &page).await;
    assert_eq!(response.status().as_u16(), 200);
    let cookie = response
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let body = response.text().await.unwrap();
    assert!(!body.contains("PRIVATE-SOURCE-CANARY"));
    assert!(body.contains("window.auth") && body.contains("agent.js"));

    let encoded = client()
        .get(format!("{}{}", server.url, page))
        .header(
            "host",
            format!("docs.{}", server.url.trim_start_matches("http://")),
        )
        .header("accept-encoding", "gzip")
        .send()
        .await
        .unwrap();
    assert_eq!(encoded.headers().get("content-encoding").unwrap(), "gzip");
    let compressed = encoded.bytes().await.unwrap();
    let mut decoded = String::new();
    std::io::Read::read_to_string(
        &mut flate2::read::GzDecoder::new(compressed.as_ref()),
        &mut decoded,
    )
    .unwrap();
    assert_eq!(decoded, body);
    let identity = client()
        .get(format!("{}{}", server.url, page))
        .header(
            "host",
            format!("docs.{}", server.url.trim_start_matches("http://")),
        )
        .header("accept-encoding", "gzip;q=0")
        .send()
        .await
        .unwrap();
    assert!(identity.headers().get("content-encoding").is_none());

    let asset = format!("/published/{slug}/assets/image.png");
    let named_asset = format!("/published/{slug}/figures/caf%C3%A9.png");
    let named = client()
        .get(format!("{}{}", server.url, named_asset))
        .header(
            "host",
            format!("docs.{}", server.url.trim_start_matches("http://")),
        )
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(
        named.status(),
        200,
        "manifest paths are decoded once within the publication namespace"
    );
    assert_eq!(named.bytes().await.unwrap().as_ref(), image);
    let first = client()
        .get(format!("{}{}", server.url, asset))
        .header(
            "host",
            format!("docs.{}", server.url.trim_start_matches("http://")),
        )
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(first.status().as_u16(), 200);
    let etag = first
        .headers()
        .get("etag")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(
        first.headers().get("cache-control").unwrap(),
        "private, max-age=0, must-revalidate"
    );
    assert_eq!(first.bytes().await.unwrap().as_ref(), image);
    let conditional = client()
        .get(format!("{}{}", server.url, asset))
        .header(
            "host",
            format!("docs.{}", server.url.trim_start_matches("http://")),
        )
        .header("cookie", &cookie)
        .header("if-none-match", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(conditional.status().as_u16(), 304);
    assert!(conditional.bytes().await.unwrap().is_empty());

    // Publishing a small text change does not invalidate an unchanged
    // image, including lazy requests from the still-open older page.
    publish_display(
        &server.url,
        &owner,
        &slug,
        b"<!doctype html><p>Updated</p><img src=\"assets/image.png\">",
        &[("assets/image.png", "image/png", image)],
    )
    .await;
    let warm = client()
        .get(format!("{}{}", server.url, asset))
        .header(
            "host",
            format!("docs.{}", server.url.trim_start_matches("http://")),
        )
        .header("cookie", &cookie)
        .header("if-none-match", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(
        warm.status(),
        304,
        "unchanged display assets reuse cached bytes"
    );
    assert!(warm.bytes().await.unwrap().is_empty());
    let removed = client()
        .get(format!("{}{}", server.url, named_asset))
        .header(
            "host",
            format!("docs.{}", server.url.trim_start_matches("http://")),
        )
        .header("cookie", &cookie)
        .header("if-none-match", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(
        removed.status(),
        404,
        "cached bytes outside the current manifest are not reauthorized"
    );
    // A valid manifest is enough to revalidate an unchanged asset. Remove
    // the object after the final publish: a conditional response must not
    // read it, while a normal delivery would fail here.
    let entry = server.instance.store.get(&slug).await.unwrap();
    let object = crate::server::publication::PublicationStore::object_key(
        &entry.storage_id,
        &hex::encode(Sha256::digest(image)),
    );
    std::fs::remove_file(server.dir.path().join("objects").join(object)).unwrap();
    let bodyless = client()
        .get(format!("{}{}", server.url, asset))
        .header(
            "host",
            format!("docs.{}", server.url.trim_start_matches("http://")),
        )
        .header("cookie", &cookie)
        .header("if-none-match", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(
        bodyless.status(),
        304,
        "asset revalidation does not read its body"
    );
    assert!(bodyless.bytes().await.unwrap().is_empty());
    assert_eq!(
        on_docs_host(&server.url, &page).await.status(),
        404,
        "an old display capability does not retain an old full HTML document"
    );
}

#[tokio::test]
async fn publication_capability_fails_after_link_revocation_even_conditionally() {
    let server = new_test_server().await;
    let owner = session_as(TEST_PUBLISHER);
    let document = publish_test_document(&server.url).await;
    let slug = text(&document, "slug");
    let result = publish_display(
        &server.url,
        &owner,
        &slug,
        b"<p>published</p><img src=\"assets/revoked.png\">",
        &[("assets/revoked.png", "image/png", b"revoked asset")],
    )
    .await;
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
        .get(format!("{}/api/documents/{slug}/publication", server.url))
        .header(crate::server::LINK_HEADER, &key)
        .header("x-librepaper-client", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let meta: serde_json::Value = response.json().await.unwrap();
    let page = docs_path(meta["publication"]["html_url"].as_str().unwrap());
    let response = on_docs_host(&server.url, &page).await;
    assert_eq!(response.status().as_u16(), 200);
    let etag = response
        .headers()
        .get("etag")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let cookie = response
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let asset = format!("/published/{slug}/assets/revoked.png");
    let asset_etag = client()
        .get(format!("{}{}", server.url, asset))
        .header(
            "host",
            format!("docs.{}", server.url.trim_start_matches("http://")),
        )
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .headers()
        .get("etag")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    // Rotating the reader link revokes the old hash. A conditional request
    // must not turn that into a 304 from a shared cache.
    let (status, _) = post_as(
        &owner,
        &server.url,
        &format!("/api/documents/{slug}/share"),
        json!({"link":{"role":"reader","until":"never"}}),
    )
    .await;
    assert_eq!(status, 200);
    let denied = client()
        .get(format!("{}{}", server.url, page))
        .header(
            "host",
            format!("docs.{}", server.url.trim_start_matches("http://")),
        )
        .header("if-none-match", etag)
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status().as_u16(), 404);
    let denied_asset = client()
        .get(format!("{}{}", server.url, asset))
        .header(
            "host",
            format!("docs.{}", server.url.trim_start_matches("http://")),
        )
        .header("cookie", cookie)
        .header("if-none-match", asset_etag)
        .send()
        .await
        .unwrap();
    assert_eq!(
        denied_asset.status().as_u16(),
        404,
        "revocation is checked before an asset ETag can return 304"
    );
    let _ = result;
}
