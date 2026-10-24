//! The starter project a new account is given: one project holding the
//! tutorial in every format, provisioned exactly once.

use std::collections::BTreeSet;

use librepaper_base::auth::Identity;
use librepaper_engine::storage::store::DocumentInput;

use crate::server::http_test_support::deployment;

const EXPECTED: [&str; 12] = [
    "librepaper.md",
    "sections/rendering.md",
    "librepaper.typ",
    "sections/rendering.typ",
    "librepaper.tex",
    "sections/rendering.tex",
    "librepaper.html",
    "sections/rendering.html",
    "librepaper.qmd",
    "sections/rendering.qmd",
    "librepaper-icon.png",
    "references.bib",
];

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_new_account_receives_one_project_holding_every_tutorial() {
    let Some(d) = deployment(
        DocumentInput {
            slug: "onboarding-fixture".into(),
            title: "Fixture".into(),
            source_format: "markdown".into(),
        },
        vec![("fixture.md".into(), "# Fixture\n".as_bytes().to_vec())],
        librepaper_base::auth::Policy::allowing("github", &["*"]),
        librepaper_base::auth::Policy::anonymous(),
    )
    .await
    else {
        return;
    };
    let catalog = &d.catalog;
    assert!(catalog.account_examples_pending(d.owner_id).await.unwrap());

    let who = Identity {
        provider: "test".into(),
        id: d.owner_id.to_string(),
        handle: "owner".into(),
        name: "Owner".into(),
        picture: String::new(),
        session_generation: d.owner_session_generation.clone(),
    };
    d.server.initialize_account_examples(&who).await.unwrap();
    assert!(!catalog.account_examples_pending(d.owner_id).await.unwrap());

    let slug = format!("starter-{}", d.owner_id.simple());
    let entry = d
        .server
        .store
        .get_result(&slug)
        .await
        .unwrap()
        .expect("starter project exists");
    assert_eq!(entry.title, "Explore LibrePaper");
    assert_eq!(entry.publisher_id, d.owner_id.to_string());
    let record = catalog
        .document_by_slug(&slug)
        .await
        .unwrap()
        .expect("starter catalogue row exists");
    assert_eq!(record.source_format, "markdown");
    assert_eq!(record.owner_id, d.owner_id);

    let files = d
        .server
        .store
        .project_files(&slug)
        .await
        .unwrap()
        .expect("starter files exist");
    let paths: BTreeSet<String> = files.iter().map(|(path, _)| path.clone()).collect();
    let expected: BTreeSet<String> = EXPECTED.iter().map(|p| p.to_string()).collect();
    assert_eq!(paths, expected, "starter project file set");
    let bytes = |path: &str| {
        files
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, b)| b.as_slice())
    };
    assert_eq!(
        bytes("librepaper.typ"),
        Some(
            include_str!(concat!(
                env!("LIBREPAPER_DOCS"),
                "/examples/tutorial-typst/librepaper.typ"
            ))
            .as_bytes()
        ),
        "Typst tutorial bytes"
    );
    assert_eq!(
        bytes("librepaper-icon.png"),
        Some(
            include_bytes!(concat!(
                env!("LIBREPAPER_DOCS"),
                "/examples/tutorial-markdown/librepaper-icon.png"
            ))
            .as_slice()
        ),
        "icon asset bytes"
    );

    // A repeat sign-in must not provision a second project.
    d.server.initialize_account_examples(&who).await.unwrap();
    let owned = catalog
        .list_documents(None, 200)
        .await
        .unwrap()
        .into_iter()
        .filter(|record| record.owner_id == d.owner_id)
        .count();
    assert_eq!(owned, 2, "fixture and starter only");
}
