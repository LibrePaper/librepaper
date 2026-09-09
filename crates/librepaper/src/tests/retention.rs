use super::*;
use crate::document::retention::{parse_expire_from, parse_retention};
use crate::document::store::Publication;
use crate::util::format_unix;

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
    for (slug, age_days) in [("old", 10), ("new", 1)] {
        server
            .instance
            .store
            .put(Publication {
                slug: slug.into(),
                title: slug.into(),
                source: "<p>x</p>".into(),
                source_format: "html".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        let catalog = server.instance.store.catalog.as_ref().unwrap();
        let mut document = catalog.document(slug).unwrap().unwrap();
        document.updated_at = format_unix(now - age_days * 86_400);
        document.created_at = document.updated_at.clone();
        catalog.update_document(&document).unwrap();
    }

    let removed = server
        .instance
        .delete_expired(now, 7 * 86_400, "updated")
        .await;
    assert_eq!(removed, 1, "removed {removed} documents, want 1");
    assert!(
        server.instance.store.get("old").await.is_none(),
        "old document survived"
    );
    assert!(
        server.instance.store.get("new").await.is_some(),
        "new document was removed"
    );
}
