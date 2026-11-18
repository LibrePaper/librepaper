//! The connector's `POST /mcp/{slug}` endpoint, driven over HTTP against a
//! real server with tokens minted through the share route.
//!
//! Needs `LIBREPAPER_TEST_POSTGRES_URL` and is skipped without it.

use std::net::SocketAddr;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::net::TcpListener;

use super::http_test_support::{deployment as http_deployment, owner_bearer, Deployment};
use crate::server::origins::Origins;
use librepaper_engine::storage::postgres::WriterLease;
use librepaper_engine::storage::store::DocumentInput;

const READER: &str = "https://paper.example";
const DOCS: &str = "https://docs.paper.example";
const PAPER: &str = "# Interval estimates\n\nThe interval covers the mean of the posterior.\n";

struct Served {
    base: String,
    slug: String,
    owner: String,
    _stop: tokio::sync::oneshot::Sender<()>,
    _writer: WriterLease,
    _objects: tempfile::TempDir,
}

async fn serve(slug: &str, connectors: bool) -> Option<Served> {
    let mut deployment = http_deployment(
        DocumentInput {
            slug: slug.into(),
            title: "Connector paper".into(),
            source_format: "markdown".into(),
        },
        vec![("paper.md".into(), PAPER.as_bytes().to_vec())],
        librepaper_base::auth::Policy::allowing("github", &["owner"]),
        librepaper_base::auth::Policy::allowing("github", &["owner"]),
    )
    .await?;
    sqlx::query("UPDATE accounts SET provider='github' WHERE id=$1")
        .bind(deployment.owner_id)
        .execute(deployment.catalog.pool())
        .await
        .unwrap();
    let owner = owner_bearer(&deployment)
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    deployment.server.origins = Origins::configure(READER, Some(DOCS)).unwrap();
    deployment.server.agent_connectors = connectors;
    let Deployment {
        server,
        slug,
        _writer,
        _objects,
        ..
    } = deployment;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address: SocketAddr = listener.local_addr().unwrap();
    let router = Arc::new(server).router();
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
    Some(Served {
        base: format!("http://{address}"),
        slug,
        owner,
        _stop: shutdown,
        _writer,
        _objects,
    })
}

impl Served {
    async fn mint_token(&self, role: &str) -> String {
        let response = reqwest::Client::new()
            .post(format!("{}/api/documents/{}/share", self.base, self.slug))
            .header("host", "paper.example")
            .header("authorization", &self.owner)
            .json(&json!({"create": {"kind": "token", "role": role, "name": "Chat app", "until": "1d"}}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 200);
        let body: Value = response.json().await.unwrap();
        body["created"]["token"].as_str().unwrap().to_string()
    }

    /// One request to the endpoint, with the token when given.
    async fn send(
        &self,
        method: reqwest::Method,
        token: Option<&str>,
        body: Option<Value>,
    ) -> reqwest::Response {
        let mut request = reqwest::Client::new()
            .request(method, format!("{}/mcp/{}", self.base, self.slug))
            .header("host", "paper.example");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        request.send().await.unwrap()
    }

    /// One JSON-RPC request under a token: the HTTP status and the body.
    async fn rpc(&self, token: &str, body: Value) -> (u16, Value) {
        let response = self
            .send(reqwest::Method::POST, Some(token), Some(body))
            .await;
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    /// Every comment thread of the project, as the text a read returns.
    async fn threads(&self, token: &str) -> String {
        let result = self
            .call(
                token,
                "document_read",
                json!({"queries": [{"kind": "thread", "include": "all"}]}),
            )
            .await;
        assert_eq!(result["isError"], false, "{result}");
        result.to_string()
    }

    async fn call(&self, token: &str, tool: &str, arguments: Value) -> Value {
        let (status, body) = self
            .rpc(
                token,
                json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
                    "params": {"name": tool, "arguments": arguments}}),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["id"], 7);
        body["result"].clone()
    }
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_missing_or_foreign_token_is_sent_to_the_authorization_server() {
    let Some(served) = serve("mcp-auth", true).await else {
        return;
    };
    let response = served
        .send(
            reqwest::Method::POST,
            None,
            Some(json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})),
        )
        .await;
    assert_eq!(response.status().as_u16(), 401);
    assert_eq!(
        response.headers()["www-authenticate"],
        format!(
            "Bearer resource_metadata=\"{READER}/.well-known/oauth-protected-resource/mcp/mcp-auth\""
        )
    );
    let body: Value = response.json().await.unwrap();
    assert!(body["error"].is_string());

    // A token that names another project is refused on this one.
    let foreign = librepaper_base::ids::AgentToken {
        origin: READER.into(),
        slug: "other-paper".parse().unwrap(),
        key: "key".into(),
    }
    .to_string();
    let response = served
        .send(
            reqwest::Method::POST,
            Some(&foreign),
            Some(json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})),
        )
        .await;
    assert_eq!(response.status().as_u16(), 401);
    assert_eq!(
        response.headers()["www-authenticate"],
        format!(
            "Bearer resource_metadata=\"{READER}/.well-known/oauth-protected-resource/mcp/mcp-auth\""
        ),
        "a token for another project is pointed at this project's authorization server"
    );

    // A well-formed token for this project that names no row is refused with
    // the same pointer to the authorization server.
    let unknown = librepaper_base::ids::AgentToken {
        origin: READER.into(),
        slug: served.slug.parse().unwrap(),
        key: "no-such-key".into(),
    }
    .to_string();
    let response = served
        .send(
            reqwest::Method::POST,
            Some(&unknown),
            Some(json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})),
        )
        .await;
    assert_eq!(response.status().as_u16(), 401);
    assert!(response.headers().contains_key("www-authenticate"));
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_handshake_and_the_tool_list_follow_the_protocol() {
    let Some(served) = serve("mcp-handshake", true).await else {
        return;
    };
    let token = served.mint_token("comment").await;

    let (status, body) = served
        .rpc(
            &token,
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                    "clientInfo": {"name": "test", "version": "1"}}}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["id"], 1);
    assert_eq!(body["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(body["result"]["serverInfo"]["name"], "librepaper");
    assert!(body["result"]["capabilities"]["tools"].is_object());
    let instructions = body["result"]["instructions"].as_str().unwrap();
    assert!(!instructions.contains("operation_epoch"));
    assert!(instructions.contains("there is no main file"));
    assert!(instructions.contains("document_compile"));
    assert!(instructions.contains("Run directory"));

    // An unknown revision is answered with the newest one this server speaks.
    let (_, body) = served
        .rpc(
            &token,
            json!({"jsonrpc": "2.0", "id": 2, "method": "initialize",
                "params": {"protocolVersion": "1999-01-01"}}),
        )
        .await;
    assert_ne!(body["result"]["protocolVersion"], "1999-01-01");

    let response = served
        .send(
            reqwest::Method::POST,
            Some(&token),
            Some(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})),
        )
        .await;
    assert_eq!(response.status().as_u16(), 202);
    assert!(response.bytes().await.unwrap().is_empty());

    let (_, body) = served
        .rpc(&token, json!({"jsonrpc": "2.0", "id": 3, "method": "ping"}))
        .await;
    assert_eq!(body["result"], json!({}));

    let (status, body) = served
        .rpc(
            &token,
            json!({"jsonrpc": "2.0", "id": 4, "method": "tools/list"}),
        )
        .await;
    assert_eq!(status, 200);
    let names: Vec<String> = body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect();
    let served_by_route: Value = reqwest::Client::new()
        .get(format!(
            "{}/api/documents/{}/tools",
            served.base, served.slug
        ))
        .header("host", "paper.example")
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let expected: Vec<String> = served_by_route["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, expected);
    assert!(names.contains(&"document_comment".to_string()));
    assert!(!names.contains(&"document_propose".to_string()));
    for tool in body["result"]["tools"].as_array().unwrap() {
        assert!(tool["inputSchema"]["properties"].get("operation").is_none());
    }
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn tools_are_called_and_writes_need_no_operation_from_the_caller() {
    let Some(served) = serve("mcp-tools", true).await else {
        return;
    };
    let comment = served.mint_token("comment").await;
    let read = served.mint_token("read").await;

    let result = served
        .call(
            &comment,
            "document_read",
            json!({"queries": [{"kind": "source", "path": "paper.md", "start": 0, "end": 5}]}),
        )
        .await;
    assert_eq!(result["isError"], false, "{result}");
    assert!(result["structuredContent"]["view_id"].is_string());
    let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text, result["structuredContent"]);

    // No `operation`, and no read before it: the endpoint supplies both.
    let result = served
        .call(
            &comment,
            "document_comment",
            json!({"action": "create", "body": "A remark"}),
        )
        .await;
    assert_eq!(result["isError"], false, "{result}");

    // A Read token is refused the write, and told so as a tool error.
    let result = served
        .call(
            &read,
            "document_comment",
            json!({"action": "create", "body": "A remark"}),
        )
        .await;
    assert_eq!(result["isError"], true, "{result}");

    let result = served.call(&comment, "document_nonsense", json!({})).await;
    assert_eq!(result["isError"], true, "{result}");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn malformed_methods_and_verbs_are_answered_as_the_protocol_asks() {
    let Some(served) = serve("mcp-errors", true).await else {
        return;
    };
    let token = served.mint_token("read").await;

    let (_, body) = served
        .rpc(
            &token,
            json!([{"jsonrpc": "2.0", "id": 1, "method": "ping"}]),
        )
        .await;
    assert_eq!(body["error"]["code"], -32600);

    let (_, body) = served
        .rpc(
            &token,
            json!({"jsonrpc": "2.0", "id": 2, "method": "resources/list"}),
        )
        .await;
    assert_eq!(body["error"]["code"], -32601);
    assert_eq!(body["id"], 2);

    for method in [reqwest::Method::GET, reqwest::Method::DELETE] {
        let response = served.send(method, Some(&token), None).await;
        assert_eq!(response.status().as_u16(), 405);
        assert_eq!(response.headers()["allow"], "POST");
    }
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_endpoint_is_absent_when_connectors_are_switched_off() {
    let Some(served) = serve("mcp-off", false).await else {
        return;
    };
    let token = served.mint_token("read").await;
    let response = served
        .send(
            reqwest::Method::POST,
            Some(&token),
            Some(json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})),
        )
        .await;
    assert_eq!(response.status().as_u16(), 404);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_token_still_works_nowhere_else() {
    let Some(served) = serve("mcp-scope", true).await else {
        return;
    };
    let token = served.mint_token("comment").await;
    for path in [
        "/api/list",
        "/api/account/storage",
        "/mcp",
        "/mcp/other-paper",
    ] {
        let response = reqwest::Client::new()
            .get(format!("{}{path}", served.base))
            .header("host", "paper.example")
            .header("authorization", format!("Bearer {token}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 401, "{path}");
    }
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_valid_token_is_not_found_on_the_documents_host() {
    let Some(served) = serve("mcp-docs-host", true).await else {
        return;
    };
    let token = served.mint_token("read").await;
    let response = reqwest::Client::new()
        .post(format!("{}/mcp/{}", served.base, served.slug))
        .header("host", "docs.paper.example")
        .header("authorization", format!("Bearer {token}"))
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 404);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_message_that_is_not_json_rpc_two_runs_nothing() {
    let Some(served) = serve("mcp-envelope", true).await else {
        return;
    };
    let token = served.mint_token("comment").await;

    // No `jsonrpc` member: refused, and the comment is not made.
    let (status, body) = served
        .rpc(
            &token,
            json!({"id": 1, "method": "tools/call",
                "params": {"name": "document_comment",
                    "arguments": {"action": "create", "body": "Should not exist"}}}),
        )
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], -32600);
    assert_eq!(body["id"], Value::Null);
    // The same read sees a comment a valid request made, so its silence
    // about the refused one means something.
    let made = served
        .call(
            &token,
            "document_comment",
            json!({"action": "create", "body": "Does exist"}),
        )
        .await;
    assert_eq!(made["isError"], false, "{made}");
    let listed = served.threads(&token).await;
    assert!(listed.contains("Does exist"), "{listed}");
    assert!(!listed.contains("Should not exist"), "{listed}");

    // An object as the id would be echoed into the answer.
    let (status, body) = served
        .rpc(
            &token,
            json!({"jsonrpc": "2.0", "id": {"a": 1}, "method": "ping"}),
        )
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], -32600);

    // A notification with a wrong version is refused, not accepted.
    let response = served
        .send(
            reqwest::Method::POST,
            Some(&token),
            Some(json!({"jsonrpc": "1.0", "method": "notifications/initialized"})),
        )
        .await;
    assert_eq!(response.status().as_u16(), 400);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_protocol_version_the_endpoint_does_not_speak_runs_nothing() {
    let Some(served) = serve("mcp-version", true).await else {
        return;
    };
    let token = served.mint_token("comment").await;
    let create = |body: &str, id: u64| {
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": "document_comment",
                "arguments": {"action": "create", "body": body}}})
    };
    let with_version = |version: &'static str, body: Value| {
        let token = token.clone();
        let base = served.base.clone();
        let slug = served.slug.clone();
        async move {
            reqwest::Client::new()
                .post(format!("{base}/mcp/{slug}"))
                .header("host", "paper.example")
                .header("authorization", format!("Bearer {token}"))
                .header("mcp-protocol-version", version)
                .json(&body)
                .send()
                .await
                .unwrap()
        }
    };

    let response = with_version("1999-01-01", create("Should not exist", 1)).await;
    assert_eq!(response.status().as_u16(), 400);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], -32600, "{body}");
    assert_eq!(body["id"], Value::Null);
    // The same read sees a comment a valid request makes, so its silence
    // about the refused one means something.
    let made = served
        .call(
            &token,
            "document_comment",
            json!({"action": "create", "body": "Does exist"}),
        )
        .await;
    assert_eq!(made["isError"], false, "{made}");
    let listed = served.threads(&token).await;
    assert!(listed.contains("Does exist"), "{listed}");
    assert!(!listed.contains("Should not exist"), "{listed}");

    // A supported revision works, and so does no header at all.
    let response = with_version("2025-06-18", create("Also exists", 2)).await;
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["result"]["isError"], false, "{body}");
}

#[tokio::test]
async fn a_failed_tool_list_keeps_its_status_and_never_looks_like_a_tool_result() {
    use super::mcp_http::list_failure;
    let origin = "https://paper.example";
    let read = |reply: super::Reply| async move {
        let status = reply.status().as_u16();
        let challenge = reply
            .headers()
            .get("www-authenticate")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let retry = reply
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let bytes = axum::body::to_bytes(reply.into_body(), 1 << 20)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        (status, challenge, retry, body)
    };

    let (status, challenge, _, body) = read(list_failure(
        origin,
        "paper",
        json!(3),
        401,
        &json!({"error": "that token was revoked"}),
    ))
    .await;
    assert_eq!(status, 401);
    assert_eq!(
        challenge.as_deref(),
        Some(super::mcp_http::mcp_challenge(origin, "paper").as_str())
    );
    assert_eq!(body["error"], "that token was revoked");
    assert!(body.get("result").is_none(), "{body}");

    let (status, _, retry, body) = read(list_failure(
        origin,
        "paper",
        json!(3),
        503,
        &json!({"error": "store unavailable"}),
    ))
    .await;
    assert_eq!(status, 503);
    assert_eq!(retry.as_deref(), Some("5"));
    assert!(body.get("result").is_none(), "{body}");

    let (status, challenge, _, body) = read(list_failure(
        origin,
        "paper",
        json!(3),
        500,
        &json!({"error": {"code": "internal", "message": "it broke"}}),
    ))
    .await;
    assert_eq!(status, 200);
    assert!(challenge.is_none());
    assert_eq!(body["id"], 3);
    assert_eq!(body["error"]["code"], -32603);
    assert_eq!(body["error"]["message"], "it broke");
    assert!(body.get("result").is_none(), "{body}");
}
