//! Regression coverage for the account quota/retention API and its durable
//! retention jobs.  These tests deliberately exercise the HTTP boundary and
//! then inspect the catalogue to verify the durable operation identity.

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
                    "UPDATE checkpoints SET at=?1 WHERE slug=?2 AND sha=?3",
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
    generation: &str,
    preferences: &QuotaPreferences,
    confirmed: bool,
) -> (u16, Value) {
    post_as(
        cookie,
        &server.url,
        "/api/account/storage/apply",
        json!({
            "revision": revision,
            "generation": generation,
            "preferences": preferences_value(preferences),
            "confirmed": confirmed,
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
async fn preview_timezone_is_presentation_only_and_stale_label_changes_are_rejected() {
    let server = new_test_server().await;
    let slug = publish_and_slug(&server).await;
    let (old, _, _) = seed_old_bucket(&server, &slug);
    let cookie = session_as(TEST_PUBLISHER);

    let utc = QuotaPreferences {
        retention_profile: "useLessStorage".into(),
        ..QuotaPreferences::default()
    };
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

    let generation = text(&utc_preview.1, "generation");
    assert!(!generation.is_empty());
    assert!(utc_preview.1["affectedCount"].as_u64().unwrap() >= 1);
    server
        .instance
        .store
        .catalog
        .as_ref()
        .unwrap()
        .label_checkpoint(&slug, &old, "important")
        .expect("label old checkpoint");
    let stale = apply(&server, &cookie, 0, &generation, &utc, true).await;
    assert_eq!(
        stale.0, 409,
        "label mutation must stale preview: {}",
        stale.1
    );
    assert!(text(&stale.1, "error").contains("stale"));
}

#[tokio::test]
async fn destructive_apply_requires_confirmation_enters_grace_and_replays_idempotently() {
    let server = new_test_server().await;
    let slug = publish_and_slug(&server).await;
    seed_old_bucket(&server, &slug);
    let cookie = session_as(TEST_PUBLISHER);
    let preferences = QuotaPreferences {
        retention_profile: "useLessStorage".into(),
        ..QuotaPreferences::default()
    };
    let preview = preview(&server, &cookie, 0, &preferences).await;
    assert_eq!(preview.0, 200, "preview: {}", preview.1);
    let generation = text(&preview.1, "generation");

    let not_confirmed = apply(&server, &cookie, 0, &generation, &preferences, false).await;
    assert_eq!(
        not_confirmed.0, 400,
        "missing confirmation: {}",
        not_confirmed.1
    );
    assert!(text(&not_confirmed.1, "error").contains("confirmation"));

    let accepted = apply(&server, &cookie, 0, &generation, &preferences, true).await;
    assert_eq!(accepted.0, 202, "confirmed apply: {}", accepted.1);
    assert_eq!(accepted.1["thinning"], "grace");
    assert!(accepted.1["graceSeconds"].as_i64().unwrap() > 0);

    let replay = apply(&server, &cookie, 0, &generation, &preferences, true).await;
    assert_eq!(replay.0, 202, "idempotent replay: {}", replay.1);
    assert_eq!(replay.1["generation"], generation);
    assert_eq!(replay.1["revision"], 1);
    let job_count: i64 = server
        .instance
        .store
        .catalog
        .as_ref()
        .unwrap()
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM quota_retention_jobs WHERE account_id=?1 AND revision=1",
                    [ACCOUNT],
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert_eq!(job_count, 1, "replay created another durable job");
}

#[tokio::test]
async fn newer_policy_marks_an_older_grace_job_stale() {
    let server = new_test_server().await;
    let slug = publish_and_slug(&server).await;
    seed_old_bucket(&server, &slug);
    let cookie = session_as(TEST_PUBLISHER);

    let narrower = QuotaPreferences {
        retention_profile: "useLessStorage".into(),
        ..QuotaPreferences::default()
    };
    let first_preview = preview(&server, &cookie, 0, &narrower).await;
    let first_generation = text(&first_preview.1, "generation");
    let first_apply = apply(&server, &cookie, 0, &first_generation, &narrower, true).await;
    assert_eq!(first_apply.0, 202, "first apply: {}", first_apply.1);

    let broader = QuotaPreferences::default();
    let second_preview = preview(&server, &cookie, 1, &broader).await;
    assert_eq!(
        second_preview.0, 200,
        "second preview: {}",
        second_preview.1
    );
    let second_generation = text(&second_preview.1, "generation");
    let second_apply = apply(&server, &cookie, 1, &second_generation, &broader, true).await;
    assert_eq!(second_apply.0, 202, "second apply: {}", second_apply.1);
    assert_ne!(second_generation, first_generation);

    let catalog = server.instance.store.catalog.as_ref().unwrap();
    assert_eq!(
        catalog
            .retention_job(ACCOUNT, &first_generation)
            .unwrap()
            .unwrap()
            .status,
        "stale"
    );
    assert_eq!(
        catalog
            .latest_retention_job(ACCOUNT)
            .unwrap()
            .unwrap()
            .generation,
        second_generation
    );
}

#[tokio::test]
async fn thinning_removes_routine_bucket_loser_but_preserves_open_annotation_and_newest() {
    let server = new_test_server().await;
    let slug = publish_and_slug(&server).await;
    let (protected, routine_candidate, newest) = seed_old_bucket(&server, &slug);
    let catalog = server.instance.store.catalog.as_ref().unwrap();
    catalog
        .insert_comment(&open_annotation(&slug, &protected))
        .expect("open annotation fixture");
    let cookie = session_as(TEST_PUBLISHER);
    let preferences = QuotaPreferences {
        retention_profile: "useLessStorage".into(),
        ..QuotaPreferences::default()
    };
    let preview = preview(&server, &cookie, 0, &preferences).await;
    assert_eq!(preview.0, 200, "preview: {}", preview.1);
    assert!(preview.1["affectedCount"].as_u64().unwrap() >= 1);
    let generation = text(&preview.1, "generation");
    let applied = apply(&server, &cookie, 0, &generation, &preferences, true).await;
    assert_eq!(applied.0, 202, "apply: {}", applied.1);

    // The HTTP contract supplies a bounded grace period.  Advancing the
    // durable clock here lets the test exercise the same pass the worker uses
    // without making a wall-clock test wait for a day.
    catalog
        .with_connection(|connection| {
            connection
                .execute(
                    "UPDATE quota_retention_jobs SET grace_until=0 WHERE account_id=?1 AND generation=?2",
                    rusqlite::params![ACCOUNT, generation],
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            connection
                .execute(
                    "UPDATE quota_retention_candidates SET grace_until=0 WHERE account_id=?1 AND generation=?2",
                    rusqlite::params![ACCOUNT, generation],
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    let pass = catalog
        .run_retention_pass(crate::util::now_unix() + 1, 100)
        .expect("retention pass");
    assert!(
        !pass.removed.iter().any(|(_, sha)| sha == &protected),
        "open annotation checkpoint was removed: {:?}",
        pass.removed
    );
    assert!(
        pass.removed
            .iter()
            .any(|(_, sha)| sha == &routine_candidate),
        "routine bucket loser was not removed: {:?}",
        pass.removed
    );
    assert!(catalog.checkpoint(&slug, &protected).unwrap().is_some());
    assert!(catalog.checkpoint(&slug, &newest).unwrap().is_some());
}
