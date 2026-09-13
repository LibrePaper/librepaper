use super::*;
use crate::document::retention::{parse_expire_from, parse_retention};
use serde_json::json;

#[test]
fn parse_retention_takes_durations_and_days() {
    for (input, want) in [
        ("", 0),
        ("never", 0),
        ("24h", 24 * 3600),
        ("30d", 30 * 86_400),
        ("90m", 5400),
        ("45s", 45),
    ] {
        assert_eq!(
            parse_retention(input).unwrap(),
            want,
            "parse_retention({input:?})"
        );
    }
    assert!(
        parse_retention("tomorrow").is_err(),
        "invalid retention was accepted"
    );
    assert!(parse_retention("0d").is_err());
    assert_eq!(parse_expire_from("").unwrap(), "updated");
    assert_eq!(parse_expire_from("Created").unwrap(), "created");
    assert!(parse_expire_from("yesterday").is_err());
}

#[tokio::test]
async fn delete_expired_removes_only_what_is_old() {
    let server = new_test_server().await;
    let now = crate::util::now_unix();
    let mut slugs = Vec::new();
    for (title, age_days) in [("Old retention fixture", 10), ("New retention fixture", 1)] {
        let (status, response) = post(
            &server.url,
            "/api/documents",
            json!({"title": title, "html": "<!doctype html><p>x</p>"}),
        )
        .await;
        assert_eq!(status, 201, "fixture publication failed: {response}");
        let slug = text(&response, "slug");
        assert!(!slug.is_empty(), "fixture publication omitted slug: {response}");
        let catalog = server.instance.store.catalog.as_ref().unwrap();
        let document = catalog.document(&slug).unwrap().unwrap();
        let old_at = (now - age_days * 86_400) * 1_000;
        catalog
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE documents SET updated_at=?1,created_at=?1 WHERE id=?2",
                    rusqlite::params![old_at, document.storage_id],
                )?;
                Ok(())
            })
            .unwrap();
        slugs.push(slug);
    }

    let removed = server
        .instance
        .delete_expired(now, 7 * 86_400, "updated")
        .await;
    assert_eq!(removed, 1, "removed {removed} documents, want 1");
    assert!(
        server.instance.store.get(&slugs[0]).await.is_none(),
        "old document survived"
    );
    assert!(
        server.instance.store.get(&slugs[1]).await.is_some(),
        "new document was removed"
    );
}
