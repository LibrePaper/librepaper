//! PostgreSQL contract for project tags: their creation, renaming and
//! deletion, the built-in Favorites tag, and what a trash does to a tag.

use uuid::Uuid;

use crate::storage::postgres::{Error, NewAccount, NewDocument, PostgresCatalog, PostgresOptions};

async fn test_catalog() -> PostgresCatalog {
    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
        .expect("set LIBREPAPER_TEST_POSTGRES_URL to a throwaway PostgreSQL database");
    let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
        .await
        .expect("connect to PostgreSQL");
    catalog.migrate().await.expect("run PostgreSQL migrations");
    catalog
}

async fn account(catalog: &PostgresCatalog) -> Uuid {
    catalog
        .create_account(NewAccount {
            kind: "anonymous".into(),
            provider: None,
            provider_subject: None,
            handle: format!("tags-test-{}", Uuid::now_v7()),
            display_name: "Tags test".into(),
            email: None,
        })
        .await
        .unwrap()
        .id
}

async fn document(catalog: &PostgresCatalog, owner_id: Uuid) -> Uuid {
    catalog
        .create_document(NewDocument {
            slug: format!("tags-test-{}", Uuid::now_v7()),
            owner_id,
            owner_session_generation: None,
            ownership_mode: "owned".into(),
            title: "Tags test".into(),
            source_format: "markdown".into(),
        })
        .await
        .unwrap()
        .id
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn tags_are_created_renamed_recoloured_and_deleted_by_their_owner() {
    let catalog = test_catalog().await;
    let writer = catalog.claim_writer().await.unwrap();
    let owner = account(&catalog).await;

    let created = catalog
        .create_tag(owner, "  Draft  ", "blue")
        .await
        .unwrap();
    assert_eq!(created.name, "Draft", "the name is kept trimmed");
    assert_eq!(created.colour, "blue");
    assert!(!created.builtin);

    let renamed = catalog
        .update_tag(owner, created.id, Some("Final"), Some("bluish-green"))
        .await
        .unwrap();
    assert_eq!(renamed.name, "Final");
    assert_eq!(renamed.colour, "bluish-green");
    let recoloured = catalog
        .update_tag(owner, created.id, None, Some("vermillion"))
        .await
        .unwrap();
    assert_eq!(
        recoloured.name, "Final",
        "a colour change leaves the name alone"
    );
    assert_eq!(recoloured.colour, "vermillion");

    let listed = catalog.list_tags(owner).await.unwrap();
    assert_eq!(listed.len(), 1);
    catalog.delete_tag(owner, created.id).await.unwrap();
    assert!(catalog.list_tags(owner).await.unwrap().is_empty());
    assert!(matches!(
        catalog.tag_for(owner, created.id).await,
        Err(Error::NotFound)
    ));

    drop(writer);
    catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn names_are_unique_per_account_ignoring_case_and_favorites_is_reserved() {
    let catalog = test_catalog().await;
    let writer = catalog.claim_writer().await.unwrap();
    let owner = account(&catalog).await;
    let other = account(&catalog).await;

    catalog.create_tag(owner, "Draft", "gray").await.unwrap();
    let duplicate = catalog.create_tag(owner, "draft", "vermillion").await;
    assert!(
        matches!(duplicate, Err(Error::Conflict(_))),
        "a name the account already has is a conflict",
    );
    catalog.create_tag(other, "Draft", "gray").await.unwrap();

    let reserved = catalog.create_tag(owner, "FAVORITES", "vermillion").await;
    assert!(matches!(reserved, Err(Error::Invalid(_))));
    let unknown_colour = catalog.create_tag(owner, "Plum", "plum").await;
    assert!(matches!(unknown_colour, Err(Error::Invalid(_))));
    let blank = catalog.create_tag(owner, "   ", "vermillion").await;
    assert!(matches!(blank, Err(Error::Invalid(_))));

    drop(writer);
    catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_built_in_favorites_tag_cannot_be_renamed_recoloured_or_deleted() {
    let catalog = test_catalog().await;
    let writer = catalog.claim_writer().await.unwrap();
    let owner = account(&catalog).await;

    let favorites = catalog.ensure_favorites(owner).await.unwrap();
    let listed = catalog.list_tags(owner).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].builtin);
    assert_eq!(listed[0].name, "Favorites");
    assert_eq!(listed[0].colour, "yellow");

    assert!(matches!(
        catalog
            .update_tag(owner, favorites, Some("Starred"), None)
            .await,
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        catalog
            .update_tag(owner, favorites, None, Some("vermillion"))
            .await,
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        catalog.delete_tag(owner, favorites).await,
        Err(Error::Invalid(_))
    ));
    assert_eq!(catalog.favorites_tag(owner).await.unwrap(), Some(favorites));

    drop(writer);
    catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn starring_creates_the_favorites_row_on_first_use() {
    let catalog = test_catalog().await;
    let writer = catalog.claim_writer().await.unwrap();
    let owner = account(&catalog).await;
    let paper = document(&catalog, owner).await;

    assert_eq!(catalog.favorites_tag(owner).await.unwrap(), None);
    assert!(catalog.list_tags(owner).await.unwrap().is_empty());

    let favorites = catalog.ensure_favorites(owner).await.unwrap();
    catalog
        .set_document_tag(owner, paper, favorites, true)
        .await
        .unwrap();
    assert_eq!(catalog.favorites_tag(owner).await.unwrap(), Some(favorites));
    assert_eq!(
        catalog.ensure_favorites(owner).await.unwrap(),
        favorites,
        "asking for the built-in tag again finds the same row",
    );
    let listed = catalog.list_tags(owner).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].builtin);

    drop(writer);
    catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_tag_survives_its_project_going_to_the_trash_and_coming_back() {
    let catalog = test_catalog().await;
    let writer = catalog.claim_writer().await.unwrap();
    let owner = account(&catalog).await;
    let paper = document(&catalog, owner).await;
    let tag = catalog.create_tag(owner, "Keep", "sky-blue").await.unwrap();
    catalog
        .set_document_tag(owner, paper, tag.id, true)
        .await
        .unwrap();

    assert!(catalog.mark_document_deleting(paper).await.unwrap());
    assert_eq!(catalog.trashed_documents(owner, 50).await.unwrap().len(), 1);
    assert_eq!(
        catalog
            .tags_for_documents(owner, &[paper])
            .await
            .unwrap()
            .len(),
        1,
        "a trashed project keeps its tags",
    );

    assert!(catalog.restore_document(paper).await.unwrap());
    let restored = catalog.tags_for_documents(owner, &[paper]).await.unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].tag_id, tag.id);

    drop(writer);
    catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn another_accounts_tag_id_is_not_found() {
    let catalog = test_catalog().await;
    let writer = catalog.claim_writer().await.unwrap();
    let owner = account(&catalog).await;
    let other = account(&catalog).await;
    let paper = document(&catalog, owner).await;
    let tag = catalog
        .create_tag(owner, "Mine", "reddish-purple")
        .await
        .unwrap();

    assert!(matches!(
        catalog.tag_for(other, tag.id).await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        catalog
            .update_tag(other, tag.id, Some("Theirs"), None)
            .await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        catalog.delete_tag(other, tag.id).await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        catalog.set_document_tag(other, paper, tag.id, true).await,
        Err(Error::NotFound)
    ));
    assert!(catalog.list_tags(other).await.unwrap().is_empty());
    assert!(
        catalog
            .tags_for_documents(other, &[paper])
            .await
            .unwrap()
            .is_empty(),
        "the owner's tags on a document are invisible to another account",
    );

    drop(writer);
    catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn two_creates_cannot_both_take_the_last_place() {
    let catalog = test_catalog().await;
    let writer = catalog.claim_writer().await.unwrap();
    let owner = account(&catalog).await;
    for number in 0..99 {
        catalog
            .create_tag(owner, &format!("Tag {number}"), "gray")
            .await
            .unwrap();
    }

    let (first, second) = tokio::join!(
        catalog.create_tag(owner, "Last one", "vermillion"),
        catalog.create_tag(owner, "Also last", "blue"),
    );
    let results = [first, second];
    let succeeded = results.iter().filter(|result| result.is_ok()).count();
    assert_eq!(succeeded, 1, "exactly one create takes the hundredth place");
    let refused = results
        .iter()
        .find(|result| result.is_err())
        .expect("one create is refused");
    assert!(matches!(refused, Err(Error::Invalid(_))));
    assert_eq!(catalog.list_tags(owner).await.unwrap().len(), 100);

    drop(writer);
    catalog.close().await;
}
