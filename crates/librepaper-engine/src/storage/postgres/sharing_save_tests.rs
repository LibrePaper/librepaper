//! A sharing save compares whole rows, not just hashes, and never brings a
//! revoked token back.

use time::OffsetDateTime;
use uuid::Uuid;

use super::{
    Authority, Error, LinkState, NewAccount, NewDocument, PostgresCatalog, PostgresOptions,
};

type Link = (
    String,
    [u8; 32],
    Vec<u8>,
    String,
    Option<OffsetDateTime>,
    Option<i64>,
    String,
    String,
    bool,
);

async fn setup() -> (PostgresCatalog, Uuid, Authority) {
    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
        .expect("set LIBREPAPER_TEST_POSTGRES_URL to a throwaway PostgreSQL database");
    let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
        .await
        .unwrap();
    catalog.migrate().await.unwrap();
    crate::testing::reset(&catalog).await;
    let tag = Uuid::now_v7();
    let account = catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some("test".into()),
            provider_subject: Some(format!("sharing-{tag}")),
            handle: format!("sharing-{tag}"),
            display_name: "Sharing test".into(),
            email: None,
        })
        .await
        .unwrap();
    let document = catalog
        .create_document(NewDocument {
            slug: format!("sharing-{tag}"),
            owner_id: account.id,
            owner_session_generation: None,
            ownership_mode: "owned".into(),
            title: "Sharing test".into(),
            source_format: "markdown".into(),
        })
        .await
        .unwrap();
    let actor = Authority {
        agent_label: String::new(),
        principal_key: account.id.to_string(),
        account_id: Some(account.id),
        session_generation: Some(account.session_generation),
        link_hash: None,
        policy_edit: true,
        policy_comment: true,
        automation: false,
    };
    (catalog, document.id, actor)
}

fn link(digit: u8, expires: Option<OffsetDateTime>) -> Link {
    (
        "reader".into(),
        [digit; 32],
        Vec::new(),
        String::new(),
        expires,
        None,
        "link".into(),
        String::new(),
        false,
    )
}

fn state(link: &Link) -> LinkState {
    (
        link.1.to_vec(),
        link.0.clone(),
        link.3.clone(),
        link.4.map(|at| at.unix_timestamp()),
        link.5,
        link.6.clone(),
        link.7.clone(),
        link.8,
    )
}

fn hashes(links: &[Link]) -> Vec<Vec<u8>> {
    links.iter().map(|link| link.1.to_vec()).collect()
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_save_cannot_bring_back_a_revoked_token() {
    let (catalog, id, actor) = setup().await;
    let first = link(1, None);
    let second = link(2, None);
    let both = [first.clone(), second.clone()];
    catalog
        .save_document_sharing(id, &actor, &both, &hashes(&both), &[])
        .await
        .unwrap();
    // Dropping the first revokes it.
    let only_second = [second.clone()];
    catalog
        .save_document_sharing(
            id,
            &actor,
            &only_second,
            &hashes(&only_second),
            &[state(&first), state(&second)],
        )
        .await
        .unwrap();
    // A save that names it again is refused and writes nothing.
    let error = catalog
        .save_document_sharing(id, &actor, &both, &hashes(&both), &[state(&second)])
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Revoked), "{error:?}");
    let live = catalog.share_links(id).await.unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].token_hash, vec![2u8; 32]);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_change_to_one_field_of_a_row_makes_an_older_read_stale() {
    let (catalog, id, actor) = setup().await;
    let soon =
        OffsetDateTime::from_unix_timestamp(OffsetDateTime::now_utc().unix_timestamp() + 3600)
            .unwrap();
    let before = link(3, Some(soon));
    catalog
        .save_document_sharing(
            id,
            &actor,
            std::slice::from_ref(&before),
            &hashes(std::slice::from_ref(&before)),
            &[],
        )
        .await
        .unwrap();
    // A second writer shortens the expiry; the hash set is unchanged.
    let sooner = OffsetDateTime::from_unix_timestamp(soon.unix_timestamp() - 3000).unwrap();
    let after = link(3, Some(sooner));
    catalog
        .save_document_sharing(
            id,
            &actor,
            std::slice::from_ref(&after),
            &hashes(std::slice::from_ref(&after)),
            &[state(&before)],
        )
        .await
        .unwrap();
    // A save built from the first read still expects the old expiry.
    let error = catalog
        .save_document_sharing(
            id,
            &actor,
            std::slice::from_ref(&before),
            &hashes(std::slice::from_ref(&before)),
            &[state(&before)],
        )
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Stale), "{error:?}");
    let live = catalog.share_links(id).await.unwrap();
    assert_eq!(
        live[0].expires_at.map(|at| at.unix_timestamp()),
        Some(sooner.unix_timestamp())
    );
}
