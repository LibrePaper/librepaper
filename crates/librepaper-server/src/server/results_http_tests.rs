//! HTTP coverage for shared Quarto results.
//!
//! `/api/documents/{slug}/results`: GET (Read), PUT and DELETE (Edit). The
//! record is not a project file; its bytes count toward the owner's storage.
//!
//! The quota refusal (413) is not covered here: the shared fixture builds the
//! catalogue with the default 50 MiB owner policy, and filling it needs
//! several 20 MiB documents.
//!
//! Needs `LIBREPAPER_TEST_POSTGRES_URL` and is skipped without it.

use std::net::SocketAddr;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::net::TcpListener;
use uuid::Uuid;

use super::http_test_support::{deployment as http_deployment, owner_bearer, Deployment};
use super::Server;
use crate::server::origins::Origins;
use librepaper_base::auth::{sign_device, Identity, PROVIDER_GITHUB};
use librepaper_base::role::Role;
use librepaper_engine::storage::postgres::{NewAccount, WriterLease};
use librepaper_engine::storage::store::DocumentInput;

const READER: &str = "https://paper.example";
const DOCS: &str = "https://docs.paper.example";
const PAPER: &str = "# Results\n\nA document with shared results.\n";

async fn deployment(slug: &str) -> Option<Deployment> {
    http_deployment(
        DocumentInput {
            slug: slug.into(),
            title: "Results Test".into(),
            source_format: "markdown".into(),
        },
        vec![("paper.md".into(), PAPER.as_bytes().to_vec())],
        librepaper_base::auth::Policy::allowing("github", &["owner"]),
        librepaper_base::auth::Policy::default(),
    )
    .await
}

struct Served {
    base: String,
    deployment_slug: String,
    owner: String,
    catalog: Arc<librepaper_engine::storage::postgres::PostgresCatalog>,
    _stop: tokio::sync::oneshot::Sender<()>,
    _writer: WriterLease,
    _objects: tempfile::TempDir,
}

async fn serve(deployment: Deployment) -> Served {
    let owner = owner_bearer(&deployment)
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let Deployment {
        mut server,
        slug,
        catalog,
        _writer,
        _objects,
        ..
    } = deployment;
    server.origins = Origins::configure(READER, Some(DOCS)).unwrap();
    let server: Arc<Server> = Arc::new(server);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address: SocketAddr = listener.local_addr().unwrap();
    let router = server.clone().router();
    let (shutdown, stopped) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .await
        .unwrap();
    });
    Served {
        base: format!("http://{address}"),
        deployment_slug: slug,
        owner,
        catalog,
        _stop: shutdown,
        _writer,
        _objects,
    }
}

impl Served {
    fn url(&self) -> String {
        format!("{}/api/documents/{}/results", self.base, self.deployment_slug)
    }

    async fn call(&self, method: &str, authorization: &str, body: Option<String>) -> (u16, Value) {
        let client = reqwest::Client::new();
        let mut request = client
            .request(method.parse().unwrap(), self.url())
            .header("host", "paper.example")
            .header("authorization", authorization);
        if let Some(body) = body {
            request = request
                .header("content-type", "application/json")
                .body(body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let value = response.json::<Value>().await.unwrap_or(Value::Null);
        (status, value)
    }

    async fn document_id(&self) -> Uuid {
        let row: Uuid = sqlx::query_scalar("SELECT id FROM documents WHERE slug=$1")
            .bind(&self.deployment_slug)
            .fetch_one(self.catalog.pool())
            .await
            .unwrap();
        row
    }

    /// A signed-in account granted `role` on the project.
    async fn bearer_with(&self, role: Role) -> String {
        let account = self
            .catalog
            .create_account(NewAccount {
                kind: "registered".into(),
                provider: Some("test".into()),
                provider_subject: Some(format!("results-{}", Uuid::new_v4().simple())),
                handle: "guest".into(),
                display_name: "Guest".into(),
                email: None,
            })
            .await
            .unwrap();
        self.catalog
            .set_grant(self.document_id().await, account.id, role)
            .await
            .unwrap();
        let identity = Identity {
            provider: PROVIDER_GITHUB.into(),
            id: account.id.to_string(),
            handle: "guest".into(),
            name: "Guest".into(),
            picture: String::new(),
            session_generation: account.session_generation.to_string(),
        };
        let token = sign_device(
            &[0; 32],
            &identity,
            librepaper_base::util::now_unix() + 3600,
        );
        format!("Bearer {token}")
    }

    async fn storage(&self) -> Value {
        let response = reqwest::Client::new()
            .get(format!("{}/api/account/storage", self.base))
            .header("host", "paper.example")
            .header("authorization", &self.owner)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        response.json::<Value>().await.unwrap()
    }
}

fn fingerprint(seed: char) -> String {
    seed.to_string().repeat(64)
}

fn record() -> Value {
    json!({
        "version": 1,
        "path": "main.qmd",
        "cells": [
            {"fingerprint": fingerprint('a'), "label": "tbl-head", "html": "<p>one</p>"},
            {"fingerprint": fingerprint('b'), "label": null, "html": "<p>two</p>"},
        ],
    })
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_owner_puts_and_gets_a_record() {
    let Some(deployment) = deployment("results-round-trip").await else {
        return;
    };
    let served = serve(deployment).await;
    let owner = served.owner.clone();

    let (status, _) = served.call("GET", &owner, None).await;
    assert_eq!(status, 404, "no record yet");

    let (status, put) = served
        .call("PUT", &owner, Some(record().to_string()))
        .await;
    assert_eq!(status, 200, "{put}");
    assert!(put["bytes"].as_i64().unwrap() > 0);
    assert!(put["updatedAt"].is_string());

    let (status, got) = served.call("GET", &owner, None).await;
    assert_eq!(status, 200, "{got}");
    assert_eq!(got["version"], 1);
    assert_eq!(got["path"], "main.qmd");
    assert_eq!(got["cells"].as_array().unwrap().len(), 2);
    assert_eq!(got["cells"][0]["label"], "tbl-head");
    assert_eq!(got["cells"][1]["label"], Value::Null);
    assert_eq!(got["uploaderName"], "Owner");
    assert_eq!(got["bytes"], put["bytes"]);
    assert!(got["updatedAt"].is_string());
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_reader_can_get_but_not_put_or_delete() {
    let Some(deployment) = deployment("results-reader").await else {
        return;
    };
    let served = serve(deployment).await;
    let owner = served.owner.clone();
    let reader = served.bearer_with(Role::Reader).await;

    let (status, _) = served
        .call("PUT", &owner, Some(record().to_string()))
        .await;
    assert_eq!(status, 200);

    let (status, got) = served.call("GET", &reader, None).await;
    assert_eq!(status, 200, "a reader reads the record");
    assert_eq!(got["path"], "main.qmd");

    let (status, _) = served
        .call("PUT", &reader, Some(record().to_string()))
        .await;
    // Lacking the rung is refused as not found, like every authorize::<Edit> route.
    assert_eq!(status, 404, "a reader cannot replace the record");
    let (status, _) = served.call("DELETE", &reader, None).await;
    assert_eq!(status, 404, "a reader cannot delete the record");

    let (status, _) = served.call("GET", &owner, None).await;
    assert_eq!(status, 200, "the record survived");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn an_invalid_record_is_refused() {
    let Some(deployment) = deployment("results-invalid").await else {
        return;
    };
    let served = serve(deployment).await;
    let owner = served.owner.clone();

    let mut bad_fingerprint = record();
    bad_fingerprint["cells"][0]["fingerprint"] = json!("ABC");
    let mut upper = record();
    upper["cells"][0]["fingerprint"] = json!("A".repeat(64));
    let mut unknown = record();
    unknown["extra"] = json!(true);
    let mut unknown_cell = record();
    unknown_cell["cells"][0]["extra"] = json!(1);
    let mut wrong_version = record();
    wrong_version["version"] = json!(2);
    let mut empty_path = record();
    empty_path["path"] = json!("");

    for (name, body) in [
        ("short fingerprint", bad_fingerprint),
        ("uppercase fingerprint", upper),
        ("unknown field", unknown),
        ("unknown cell field", unknown_cell),
        ("wrong version", wrong_version),
        ("empty path", empty_path),
    ] {
        let (status, _) = served.call("PUT", &owner, Some(body.to_string())).await;
        assert_eq!(status, 400, "{name}");
    }
    let (status, _) = served
        .call("PUT", &owner, Some("not json".to_string()))
        .await;
    assert_eq!(status, 400, "not json");
    let (status, _) = served.call("GET", &owner, None).await;
    assert_eq!(status, 404, "nothing was stored");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn delete_reports_the_freed_bytes_and_the_record_is_gone() {
    let Some(deployment) = deployment("results-delete").await else {
        return;
    };
    let served = serve(deployment).await;
    let owner = served.owner.clone();

    let (status, deleted) = served.call("DELETE", &owner, None).await;
    assert_eq!(status, 200);
    assert_eq!(deleted["bytes"], 0, "nothing to free");

    let (_, put) = served
        .call("PUT", &owner, Some(record().to_string()))
        .await;
    let (status, deleted) = served.call("DELETE", &owner, None).await;
    assert_eq!(status, 200);
    assert_eq!(deleted["bytes"], put["bytes"]);
    let (status, _) = served.call("GET", &owner, None).await;
    assert_eq!(status, 404);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_storage_status_counts_shared_results() {
    let Some(deployment) = deployment("results-storage").await else {
        return;
    };
    let served = serve(deployment).await;
    let owner = served.owner.clone();
    let entry = |status: &Value, slug: &str| -> Value {
        status["usage"]["documents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["slug"] == slug)
            .unwrap()
            .clone()
    };

    let before = served.storage().await;
    let slug = served.deployment_slug.clone();
    assert_eq!(entry(&before, &slug)["resultsBytes"], 0);

    let (_, put) = served
        .call("PUT", &owner, Some(record().to_string()))
        .await;
    let bytes = put["bytes"].as_i64().unwrap();
    let after = served.storage().await;
    assert_eq!(entry(&after, &slug)["resultsBytes"], bytes);
    assert_eq!(
        after["usage"]["chargedBytes"].as_i64().unwrap()
            - before["usage"]["chargedBytes"].as_i64().unwrap(),
        bytes,
        "the owner is charged for the record"
    );

    served.call("DELETE", &owner, None).await;
    let gone = served.storage().await;
    assert_eq!(
        gone["usage"]["chargedBytes"],
        before["usage"]["chargedBytes"]
    );
}
