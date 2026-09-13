use super::*;
use crate::auth::{Identity, Policy};
use crate::config::Configuration;
use serde_json::json;

#[tokio::test]
async fn account_examples_resume_after_admission_failure() {
    use rusqlite::OptionalExtension;

    let mut config = Configuration::default();
    config.set_counts(Some(4), Some(30)).unwrap();
    let server = test_server_with(config, Policy::parse("any"), Policy::parse("any"), true).await;
    let mut who = Identity::github("alice", "alice");
    get_json_as(&session_as("alice"), &server.url, "/api/me").await;
    who.session_generation = "test-session-generation".into();
    assert!(server
        .instance
        .initialize_account_examples(&who)
        .await
        .is_err());
    let catalog = server.instance.store.catalog.as_ref().unwrap();
    assert_eq!(catalog.pending_account_examples(&who.id).unwrap().len(), 1);
    let entries = server.instance.store.list().await;
    assert_eq!(entries.len(), 4);
    let removed = &entries[0].slug;
    server.instance.store.remove(removed).await.unwrap();
    // A v2 remove withdraws the document immediately, but its object charge
    // remains until the bounded worker has confirmed the physical deletes.
    // Drive that durable phase here so the retry exercises the intended
    // admission-after-delete path rather than depending on a background
    // maintenance tick.
    let catalog = server.instance.store.catalog.as_ref().unwrap().clone();
    let worker = crate::storage::maintenance::DeletionWorker::new(
        catalog.clone(),
        server.instance.store.blobs.clone(),
        crate::storage::maintenance::DeletionLimits::default(),
    )
    .unwrap();
    let mut gc_now = crate::util::now_millis().saturating_add(900_001);
    for _ in 0..8 {
        worker.run_v2_once(gc_now).await.unwrap();
        let status: Option<String> = catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT status FROM documents WHERE slug=?1",
                        [removed.as_str()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .unwrap();
        if status.is_none() {
            break;
        }
        gc_now = gc_now.saturating_add(900_001);
    }
    assert!(catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT 1 FROM documents WHERE slug=?1",
                    [removed.as_str()],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap()
        .is_none());
    server
        .instance
        .initialize_account_examples(&who)
        .await
        .unwrap();
    assert!(catalog
        .pending_account_examples(&who.id)
        .unwrap()
        .is_empty());
    assert!(server
        .instance
        .store
        .get_result(removed)
        .await
        .unwrap()
        .is_none());
    assert_eq!(server.instance.store.list().await.len(), 4);
}

#[tokio::test]
async fn account_examples_are_private_owned_and_created_once() {
    let server = test_server_with(
        Configuration::default(),
        Policy::parse("anyone"),
        Policy::parse("anyone"),
        true,
    )
    .await;
    let mut alice = Identity::github("alice", "alice");
    let mut bob = Identity::github("bob", "bob");
    // Establish normal account rows through the test harness's signed cookies.
    get_json_as(&session_as("alice"), &server.url, "/api/me").await;
    get_json_as(&session_as("bob"), &server.url, "/api/me").await;
    alice.session_generation = "test-session-generation".into();
    bob.session_generation = "test-session-generation".into();
    let (first, concurrent) = tokio::join!(
        server.instance.initialize_account_examples(&alice),
        server.instance.initialize_account_examples(&alice),
    );
    first.unwrap();
    concurrent.unwrap();
    server
        .instance
        .initialize_account_examples(&bob)
        .await
        .unwrap();
    let entries = server.instance.store.list().await;
    assert_eq!(entries.len(), 10);
    for who in [&alice, &bob] {
        let owned: Vec<_> = entries
            .iter()
            .filter(|e| e.publisher_id == who.id)
            .collect();
        assert_eq!(owned.len(), 5);
        let mut formats: Vec<_> = owned.iter().map(|e| e.source_format.as_str()).collect();
        formats.sort();
        assert_eq!(formats, ["html", "latex", "markdown", "quarto", "typst"]);
        for entry in owned {
            assert!(!entry.example && !entry.unowned);
            assert!(entry.links.is_empty());
            if entry.source_format == "quarto" {
                assert_eq!(entry.main, "librepaper.qmd");
                assert_eq!(
                    server.instance.rooms.get(&entry.slug).await.source().await,
                    include_str!("../../../../docs/examples/tutorial-quarto/librepaper.qmd")
                );
            }
            let tree = server.instance.rooms.get(&entry.slug).await.tree().await;
            assert_eq!(tree.files.len(), 3);
            assert!(tree.files.contains_key("librepaper-icon.png"));
            assert!(tree
                .files
                .keys()
                .any(|path| path.starts_with("sections/rendering.")));
            assert!(!server
                .instance
                .rooms
                .get(&entry.slug)
                .await
                .source()
                .await
                .is_empty());
            assert_eq!(
                get_json_as("", &server.url, &format!("/api/documents/{}", entry.slug))
                    .await
                    .0,
                404
            );
        }
    }
    let paper = entries
        .iter()
        .find(|e| e.publisher_id == alice.id && e.source_format == "markdown")
        .unwrap();
    let room = server.instance.rooms.get(&paper.slug).await;
    room.set_main_file("# My edited example", "markdown", "librepaper.md")
        .await
        .unwrap();
    room.checkpoint("test", "alice").await.unwrap();
    for role in ["reader", "commenter", "editor"] {
        let (status, sharing) = post_as(
            &session_as("alice"),
            &server.url,
            &format!("/api/documents/{}/share", paper.slug),
            json!({"link": {"role": role, "until": "never"}}),
        )
        .await;
        assert_eq!(status, 200, "{sharing}");
        let key = text(&sharing["links"][role], "key");
        let (status, document) = get_json_keyed(
            "",
            &key,
            &server.url,
            &format!("/api/documents/{}", paper.slug),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(
            text(&document, "role"),
            if role == "editor" { "commenter" } else { role },
            "an anonymous link cannot grant source editing"
        );
        assert_eq!(
            get_json_keyed(
                "",
                &key,
                &server.url,
                &format!("/api/documents/{}/share", paper.slug)
            )
            .await
            .0,
            404
        );
    }
    let removed = entries
        .iter()
        .find(|e| e.publisher_id == alice.id && e.source_format == "quarto")
        .unwrap();
    server.instance.store.remove(&removed.slug).await.unwrap();
    server
        .instance
        .initialize_account_examples(&alice)
        .await
        .unwrap();
    assert_eq!(server.instance.store.list().await.len(), 9);
    assert_eq!(room.source().await, "# My edited example");
    let reopened =
        crate::storage::catalog::Catalog::open(server.dir.path().join("catalog.sqlite")).unwrap();
    assert!(reopened
        .pending_account_examples(&alice.id)
        .unwrap()
        .is_empty());
}
