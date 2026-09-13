//! Regression coverage for the account quota/retention API and its advisory
//! policy recalculation boundary.

use std::collections::HashMap;

use serde_json::{json, Value};

use super::*;
use crate::document::quota::QuotaPreferences;
use crate::storage::catalog::{Checkpoint, Comment};

const ACCOUNT: &str = "github:vincent";

async fn quota_status(cookie: &str, base: &str) -> (u16, Value) {
    let mut request = client()
        .get(format!("{base}/api/account/storage"))
        .header("x-librepaper-client", "shell");
    if !cookie.is_empty() {
        request = request.header("cookie", cookie);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap())
}

fn preferences_value(preferences: &QuotaPreferences) -> Value {
    serde_json::to_value(preferences).expect("quota preferences serialize")
}

async fn publish_and_slug(server: &TestServer) -> String {
    let document = publish_test_document(&server.url).await;
    let slug = text(&document, "slug");
    assert!(
        !slug.is_empty(),
        "publication did not return a slug: {document}"
    );
    slug
}

fn set_checkpoint_time(server: &TestServer, slug: &str, sha: &str, at: i64) {
    let catalog = server.instance.store.catalog.as_ref().unwrap();
    catalog
        .with_connection(|connection| {
            connection
                .execute(
                    "UPDATE checkpoints SET created_at=?1
                     WHERE document_id=(SELECT id FROM documents WHERE slug=?2) AND id=?3",
                    rusqlite::params![at.to_string(), slug, sha],
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .expect("checkpoint timestamp update");
}

fn add_checkpoint(server: &TestServer, slug: &str, sha: &str, parent: &str, at: i64) {
    server
        .instance
        .store
        .catalog
        .as_ref()
        .unwrap()
        .insert_checkpoint(&Checkpoint {
            slug: slug.into(),
            sha: sha.into(),
            seq: -1,
            durable_seq: 0,
            tree_sha: format!("tree-{sha}"),
            parent: parent.into(),
            at: at.to_string(),
            by: TEST_PUBLISHER.into(),
            by_account: Some(ACCOUNT.into()),
            why: "automatic".into(),
            source_format: "html".into(),
            size: 1,
            label: String::new(),
            git_commit: String::new(),
            dirty: false,
            changed: None,
        })
        .expect("insert checkpoint fixture");
}

/// Make two routine points occupy one old bucket, plus a current newest
/// point.  The first old point is the candidate; the second is the bucket
/// winner.  Integer timestamps are accepted by the history parser and avoid
/// coupling this fixture to display timezone formatting.
fn seed_old_bucket(server: &TestServer, slug: &str) -> (String, String, String) {
    let catalog = server.instance.store.catalog.as_ref().unwrap();
    let initial = catalog
        .checkpoints(slug, None, 10)
        .expect("initial checkpoint")
        .into_iter()
        .next()
        .expect("publication checkpoint");
    let now = crate::util::now_unix();
    // Anchor the old points away from the two-day preset boundary so the
    // fixture cannot become flaky when the test starts near a boundary.
    let old_at = now.div_euclid(172_800) * 172_800 + 100 - 4 * 172_800;
    set_checkpoint_time(server, slug, &initial.sha, old_at);
    let old_loser = "old-loser";
    add_checkpoint(server, slug, old_loser, &initial.sha, old_at + 5);
    let old_winner = "old-winner";
    add_checkpoint(server, slug, old_winner, old_loser, old_at + 10);
    let newest = "newest-checkpoint";
    add_checkpoint(server, slug, newest, old_winner, now);
    (initial.sha, old_loser.into(), newest.into())
}

fn open_annotation(slug: &str, revision: &str) -> Comment {
    Comment {
        slug: slug.into(),
        id: "open-annotation".into(),
        seq: 0,
        motivation: "commenting".into(),
        body: "keep this revision available".into(),
        creator: "reviewer".into(),
        author: "reviewer".into(),
        via: "web".into(),
        created: "2026-01-01T00:00:00.000Z".into(),
        publication_id: String::new(),
        exact: String::new(),
        prefix: String::new(),
        suffix: String::new(),
        position: None,
        point: false,
        color: None,
        region: None,
        quarto_output: None,
        source_path: None,
        source_exact: None,
        source_prefix: None,
        source_suffix: None,
        source_position: None,
        proposed: None,
        pass: String::new(),
        outcome: String::new(),
        accept_request: String::new(),
        revision: revision.into(),
        resolved: false,
        resolved_at: None,
        resolved_in: String::new(),
    }
}

async fn preview(
    server: &TestServer,
    cookie: &str,
    revision: i64,
    preferences: &QuotaPreferences,
) -> (u16, Value) {
    post_as(
        cookie,
        &server.url,
        "/api/account/storage/preview",
        json!({"revision": revision, "preferences": preferences_value(preferences)}),
    )
    .await
}

async fn apply(
    server: &TestServer,
    cookie: &str,
    revision: i64,
    preferences: &QuotaPreferences,
) -> (u16, Value) {
    post_as(
        cookie,
        &server.url,
        "/api/account/storage/apply",
        json!({
            "revision": revision,
            "preferences": preferences_value(preferences),
        }),
    )
    .await
}

#[tokio::test]
async fn quota_routes_reject_anonymous_link_only_invalid_session_and_cross_site_mutations() {
    let server = new_test_server().await;
    let unauthenticated = quota_status("", &server.url).await;
    assert_eq!(
        unauthenticated.0, 401,
        "anonymous storage read: {:?}",
        unauthenticated.1
    );

    let link_only = post_keyed(
        "",
        "not-a-quota-link",
        &server.url,
        "/api/account/storage/apply",
        json!({}),
    )
    .await;
    assert_eq!(
        link_only.0, 401,
        "link-only quota mutation: {:?}",
        link_only.1
    );

    let invalid_session = post_as(
        "librepaper_session=invalid",
        &server.url,
        "/api/account/storage/preview",
        json!({}),
    )
    .await;
    assert_eq!(
        invalid_session.0, 401,
        "invalid session: {:?}",
        invalid_session.1
    );

    let mut headers = HashMap::new();
    headers.insert("origin", "https://evil.example".to_string());
    headers.insert("cookie", session_as(TEST_PUBLISHER));
    let cross_site = raw_post(
        &server.url,
        "/api/account/storage/preview",
        headers,
        json!({}),
    )
    .await;
    assert_eq!(
        cross_site.0, 403,
        "cross-site quota mutation: {:?}",
        cross_site.1
    );
}

#[tokio::test]
async fn storage_status_exposes_balanced_v1_utc_tiers_without_changing_hard_quota() {
    let server = new_test_server().await;
    publish_and_slug(&server).await;

    let (status, payload) = quota_status(&session_as(TEST_PUBLISHER), &server.url).await;
    assert_eq!(status, 200, "storage status: {payload}");
    assert_eq!(payload["preferences"]["displayTimezone"], "UTC");
    assert_eq!(payload["effective"]["profile"], "balanced");
    assert_eq!(
        payload["constraints"]["hardQuotaBytes"],
        server.instance.store.config.storage.per_owner
    );
    let widths: Vec<i64> = payload["effective"]["retention"]["tiers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tier| tier["bucketSeconds"].as_i64().unwrap())
        .collect();
    assert_eq!(widths, [300, 3_600, 21_600, 86_400]);
}

#[tokio::test]
async fn preview_timezone_is_presentation_only_and_apply_is_advisory() {
    let server = new_test_server().await;
    let slug = publish_and_slug(&server).await;
    let (old, _, _) = seed_old_bucket(&server, &slug);
    let cookie = session_as(TEST_PUBLISHER);

    let utc = QuotaPreferences::default();
    let mut toronto = utc.clone();
    toronto.display_timezone = "America/Toronto".into();
    let utc_preview = preview(&server, &cookie, 0, &utc).await;
    let local_preview = preview(&server, &cookie, 0, &toronto).await;
    assert_eq!(utc_preview.0, 200, "UTC preview: {}", utc_preview.1);
    assert_eq!(local_preview.0, 200, "local preview: {}", local_preview.1);
    assert_eq!(
        utc_preview.1["effective"]["retention"],
        local_preview.1["effective"]["retention"]
    );
    assert_eq!(
        utc_preview.1["affectedCount"],
        local_preview.1["affectedCount"]
    );
    assert!(utc_preview.1["generation"].is_null());
    assert!(utc_preview.1["affectedCount"].as_u64().unwrap() >= 1);

    server
        .instance
        .store
        .catalog
        .as_ref()
        .unwrap()
        .label_checkpoint(&slug, &old, "important")
        .expect("label old checkpoint");
    let saved = apply(&server, &cookie, 0, &utc).await;
    assert_eq!(saved.0, 200, "advisory apply: {}", saved.1);
    assert_eq!(saved.1["status"], "saved");
    assert_eq!(saved.1["revision"], 1);
    assert_eq!(saved.1["graceMs"], 86_400_000);
}

#[tokio::test]
async fn advisory_apply_rejects_stale_revision_without_durable_job() {
    let server = new_test_server().await;
    let cookie = session_as(TEST_PUBLISHER);
    let preferences = QuotaPreferences::default();

    let accepted = apply(&server, &cookie, 0, &preferences).await;
    assert_eq!(accepted.0, 200, "first apply: {}", accepted.1);
    let stale = apply(&server, &cookie, 0, &preferences).await;
    assert_eq!(stale.0, 409, "stale apply: {}", stale.1);

    let catalog = server.instance.store.catalog.as_ref().unwrap();
    let due: i64 = catalog.with_connection(|connection| {
        connection.query_row(
            "SELECT retention_due_at FROM documents WHERE owner_id=?1 AND status='active' ORDER BY id LIMIT 1",
            [ACCOUNT], |row| row.get(0),
        ).map_err(crate::storage::catalog::CatalogError::from)
    }).unwrap();
    assert_eq!(
        due, 0,
        "policy apply did not enqueue document recalculation"
    );
}

#[tokio::test]
async fn newer_policy_restarts_due_document_evaluation() {
    let server = new_test_server().await;
    publish_and_slug(&server).await;
    let cookie = session_as(TEST_PUBLISHER);
    let first = apply(&server, &cookie, 0, &QuotaPreferences::default()).await;
    assert_eq!(first.0, 200, "first apply: {}", first.1);
    let second = apply(
        &server,
        &cookie,
        1,
        &QuotaPreferences {
            retention_profile: "manual".into(),
            ..QuotaPreferences::default()
        },
    )
    .await;
    assert_eq!(second.0, 200, "second apply: {}", second.1);
    assert!(second.1["generation"].is_null());
}

#[tokio::test]
async fn thinning_preserves_open_annotation_and_newest_checkpoint() {
    let server = new_test_server().await;
    let slug = publish_and_slug(&server).await;
    let (protected, routine_candidate, newest) = seed_old_bucket(&server, &slug);
    let catalog = server.instance.store.catalog.as_ref().unwrap();
    catalog
        .insert_comment(&open_annotation(&slug, &protected))
        .expect("open annotation fixture");
    let cookie = session_as(TEST_PUBLISHER);
    let preferences = QuotaPreferences {
        retention_profile: "custom".into(),
        custom_retention: Some(crate::document::quota::CustomRetention {
            max_routine_count: Some(1),
            max_age_ms: None,
        }),
        ..QuotaPreferences::default()
    };
    let applied = apply(&server, &cookie, 0, &preferences).await;
    assert_eq!(applied.0, 200, "apply: {}", applied.1);

    let now = crate::util::now_millis();
    catalog
        .schedule_document_balanced(
            &slug,
            now,
            crate::document::quota::RetentionBounds::default(),
        )
        .expect("schedule retention");
    catalog.with_connection(|connection| {
        connection.execute(
            "UPDATE checkpoints SET eligible_after=?1 WHERE document_id=(SELECT id FROM documents WHERE slug=?2) AND id=?3",
            rusqlite::params![now - 1, slug, routine_candidate],
        ).map_err(crate::storage::catalog::CatalogError::from)?;
        connection.execute(
            "UPDATE documents SET retention_due_at=?1 WHERE slug=?2",
            rusqlite::params![now, slug],
        ).map_err(crate::storage::catalog::CatalogError::from)
    }).unwrap();
    let pass = catalog.run_retention_pass(now, 32).expect("retention pass");
    assert!(
        !pass.removed.iter().any(|(_, sha)| sha == &protected),
        "open annotation checkpoint was removed: {:?}",
        pass.removed
    );
    assert!(
        pass.removed
            .iter()
            .any(|(_, sha)| sha == &routine_candidate),
        "routine checkpoint was not removed: {:?}",
        pass.removed
    );
    assert!(catalog.checkpoint(&slug, &protected).unwrap().is_some());
    assert!(catalog.checkpoint(&slug, &newest).unwrap().is_some());
}
