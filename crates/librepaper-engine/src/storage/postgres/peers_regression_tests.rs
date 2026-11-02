//! Regressions for peer attribution: the first writer keeps a peer, and the
//! read resolves the name of the account that holds it.

use uuid::Uuid;

use super::super::{NewAccount, NewDocument, PostgresCatalog, PostgresOptions};

async fn catalog() -> PostgresCatalog {
    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
        .expect("set LIBREPAPER_TEST_POSTGRES_URL to a throwaway PostgreSQL database");
    let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
        .await
        .expect("connect to PostgreSQL");
    catalog.migrate().await.expect("run PostgreSQL migrations");
    catalog
}

async fn account(catalog: &PostgresCatalog, name: &str) -> super::super::AccountRecord {
    catalog
        .create_account(NewAccount {
            kind: "anonymous".into(),
            provider: None,
            provider_subject: None,
            handle: format!("peers-{}", Uuid::now_v7()),
            display_name: name.into(),
            email: None,
        })
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn first_writer_keeps_a_peer_and_reads_back_its_name() {
    let catalog = catalog().await;
    let _writer = catalog.claim_writer().await.unwrap();
    let first = account(&catalog, "First writer").await;
    let second = account(&catalog, "Second writer").await;
    let document = catalog
        .create_document(NewDocument {
            slug: format!("peers-{}", Uuid::now_v7()),
            owner_id: first.id,
            owner_session_generation: Some(first.session_generation),
            ownership_mode: "owned".into(),
            title: "Peer test".into(),
            source_format: "markdown".into(),
        })
        .await
        .unwrap();

    catalog
        .record_document_peers(document.id, &[10, 11], Some(first.id), "")
        .await
        .unwrap();
    // Peer 11 is claimed again by another account: the first writer keeps it.
    catalog
        .record_document_peers(document.id, &[11, 12], Some(second.id), "")
        .await
        .unwrap();

    let authors = catalog.document_peer_authors(document.id).await.unwrap();
    let owner_of = |peer: u64| {
        authors
            .iter()
            .find(|author| author.peer == peer)
            .unwrap_or_else(|| panic!("peer {peer} is not recorded"))
    };
    assert_eq!(authors.len(), 3);
    assert_eq!(owner_of(10).account_id, Some(first.id));
    assert_eq!(owner_of(11).account_id, Some(first.id));
    assert_eq!(owner_of(12).account_id, Some(second.id));
    assert_eq!(owner_of(11).name, "First writer");
    assert_eq!(owner_of(12).name, "Second writer");
    assert!(!owner_of(12).agent);
}
