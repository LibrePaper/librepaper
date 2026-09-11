//! MCP requests exercise the real HTTP, permission, and durable-view boundary.
use super::*;
use serde_json::{json, Value};

fn request(method: &str, params: Value) -> Value {
    let mut params = params;
    params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}});
    json!({"jsonrpc":"2.0","id":"test","method":method,"params":params})
}

async fn call(base: &str, slug: &str, key: &str, input: &Value) -> (u16, Value) {
    let mut req = client()
        .post(format!("{base}/api/documents/{slug}/mcp"))
        .header("cookie", session_as(TEST_PUBLISHER))
        .header("x-librepaper-client", "1")
        .header(crate::server::LINK_HEADER, key)
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", input["method"].as_str().unwrap_or_default());
    if let Some(name) = input["params"]["name"].as_str() {
        req = req.header("mcp-name", name);
    }
    let response = req.json(input).send().await.expect("MCP response");
    let status = response.status().as_u16();
    let body = response.bytes().await.expect("body");
    (
        status,
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| json!({"text":String::from_utf8_lossy(&body)})),
    )
}

async fn publish_mcp(base: &str) -> Value {
    let (status,value) = post(base,"/api/documents",json!({"title":"MCP paper","source":"# Intro\n\n😀 same\n\nsame\n","source_format":"markdown"})).await;
    assert_eq!(status, 201, "{value}");
    value
}

async fn tool(base: &str, slug: &str, key: &str, name: &str, args: Value) -> Value {
    let (status, response) = call(
        base,
        slug,
        key,
        &request("tools/call", json!({"name":name,"arguments":args})),
    )
    .await;
    assert_eq!(status, 200, "{response}");
    assert!(response.get("error").is_none(), "{response}");
    response["result"]["structuredContent"].clone()
}

#[tokio::test]
async fn mcp_propose_apply_and_retry_have_one_effect_and_bound_authority() {
    let server = new_test_server().await;
    let doc = publish_mcp(&server.url).await;
    let slug = text(&doc, "slug");
    let edit = editor_key(&server.url, &slug).await;
    let read = tool(
        &server.url,
        &slug,
        &edit,
        "document_read",
        json!({"queries":[{"kind":"source"}]}),
    )
    .await;
    let range = &read["results"][0]["blocks"][0]["range_id"];
    assert!(range.is_string(), "{read}");
    let args = json!({"view_id":read["view_id"],"operation":{"epoch":read["operation_epoch"],"id":"proposal-1"},"patches":[{"range_id":range,"find":"😀 same","replacement":"A new sentence"}],"publish":"private"});
    let proposal = tool(&server.url, &slug, &edit, "document_propose", args.clone()).await;
    assert_eq!(proposal["status"], "committed", "{proposal}");
    let replay = tool(&server.url, &slug, &edit, "document_propose", args.clone()).await;
    assert_eq!(replay["candidate_id"], proposal["candidate_id"]);
    assert_eq!(replay["replay"], true);
    let mut changed = args;
    changed["patches"][0]["replacement"] = json!("different");
    let refused = tool(&server.url, &slug, &edit, "document_propose", changed).await;
    assert_eq!(
        refused["error"]["code"], "operation_key_reused",
        "{refused}"
    );
    let apply = json!({"candidate_id":proposal["candidate_id"],"operation":{"epoch":read["operation_epoch"],"id":"apply-1"}});
    let applied = tool(&server.url, &slug, &edit, "document_apply", apply.clone()).await;
    assert_eq!(applied["status"], "committed", "{applied}");
    assert_eq!(
        applied["source_revision_after"],
        proposal["source_revision"]
    );
    let again = tool(&server.url, &slug, &edit, "document_apply", apply).await;
    assert_eq!(again["replay"], true, "{again}");
    let fresh = tool(
        &server.url,
        &slug,
        &edit,
        "document_read",
        json!({"queries":[{"kind":"source"}]}),
    )
    .await;
    assert_eq!(
        fresh["results"][0]["blocks"][0]["source"],
        "# Intro\n\nA new sentence\n\nsame\n"
    );
    let denied = tool(
        &server.url,
        &slug,
        &read_key_of(&doc),
        "document_result",
        json!({"kind":"candidate","id":proposal["candidate_id"]}),
    )
    .await;
    assert_eq!(denied["error"]["code"], "view_expired", "{denied}");
}

#[tokio::test]
async fn mcp_atomic_suggestion_retry_and_conflicting_apply() {
    let server = new_test_server().await;
    let doc = publish_mcp(&server.url).await;
    let slug = text(&doc, "slug");
    let edit = editor_key(&server.url, &slug).await;
    let read = tool(
        &server.url,
        &slug,
        &edit,
        "document_read",
        json!({"queries":[{"kind":"search","query":"same"}]}),
    )
    .await;
    let matches = read["results"][0]["matches"].as_array().expect("matches");
    let args = json!({"view_id":read["view_id"],"operation":{"epoch":read["operation_epoch"],"id":"batch"},"patches":matches.iter().map(|r|json!({"range_id":r["range_id"],"replacement":"changed"})).collect::<Vec<_>>()});
    let proposal = tool(&server.url, &slug, &edit, "document_propose", args.clone()).await;
    assert_eq!(
        proposal["effects"].as_array().expect("effects").len(),
        2,
        "{proposal}"
    );
    let replay = tool(&server.url, &slug, &edit, "document_propose", args).await;
    assert_eq!(replay["effects"], proposal["effects"]);
    let threads = tool(
        &server.url,
        &slug,
        &edit,
        "document_read",
        json!({"queries":[{"kind":"thread"}]}),
    )
    .await;
    assert!(!threads.to_string().contains("error"), "{threads}");
    server
        .instance
        .rooms
        .get(&slug)
        .await
        .set_source("Collaborator edit\n", "markdown")
        .await
        .expect("edit");
    let result=tool(&server.url,&slug,&edit,"document_apply",json!({"candidate_id":proposal["candidate_id"],"operation":{"epoch":read["operation_epoch"],"id":"stale-apply"}})).await;
    assert_eq!(result["error"]["code"], "conflict", "{result}");
}

#[tokio::test]
async fn mcp_discovery_is_standard_stateless_and_schema_validated() {
    let server = new_test_server().await;
    let doc = publish_mcp(&server.url).await;
    let slug = text(&doc, "slug");
    let key = read_key_of(&doc);
    let (status, discovery) = call(
        &server.url,
        &slug,
        &key,
        &request("server/discover", json!({})),
    )
    .await;
    assert_eq!(status, 200, "{discovery}");
    assert_eq!(discovery["result"]["resultType"], "complete");
    assert_eq!(
        discovery["result"]["supportedVersions"],
        json!(["2026-07-28"])
    );
    let (_, tools) = call(&server.url, &slug, &key, &request("tools/list", json!({}))).await;
    assert_eq!(tools["result"]["tools"].as_array().expect("tools").len(), 5);
    let (_, error) = call(
        &server.url,
        &slug,
        &key,
        &request(
            "tools/call",
            json!({"name":"document_read","arguments":{"queries":[{"kind":"source"}],"typo":true}}),
        ),
    )
    .await;
    assert_eq!(error["error"]["code"], -32602);
}

#[tokio::test]
async fn mcp_reads_return_small_exact_views_and_preserve_snapshot_identity() {
    let server = new_test_server().await;
    let doc = publish_mcp(&server.url).await;
    let slug = text(&doc, "slug");
    let key = read_key_of(&doc);
    let (_,read) = call(&server.url,&slug,&key,&request("tools/call",json!({"name":"document_read","arguments":{"queries":[{"kind":"source","path":"main.md"}]}}))).await;
    assert_eq!(read["result"]["isError"], false, "{read}");
    let result = &read["result"]["structuredContent"];
    assert_eq!(result["permissions"]["apply"], false);
    let view = result["view_id"].as_str().expect("view");
    let old = result["source_revision"].clone();
    server
        .instance
        .rooms
        .get(&slug)
        .await
        .set_source("new source\n", "markdown")
        .await
        .expect("edit");
    let (_,again) = call(&server.url,&slug,&key,&request("tools/call",json!({"name":"document_read","arguments":{"snapshot":{"view_id":view},"queries":[{"kind":"source","path":"main.md"}]}}))).await;
    assert_eq!(
        again["result"]["structuredContent"]["source_revision"], old,
        "{again}"
    );
    assert!(
        again.to_string().contains("😀 same\\n\\nsame\\n"),
        "{again}"
    );
    assert!(again.to_string().len() < 12000);
}

#[tokio::test]
async fn mcp_rejects_header_mismatch_and_wrong_document_scope() {
    let server = new_test_server().await;
    let doc = publish_mcp(&server.url).await;
    let slug = text(&doc, "slug");
    let key = read_key_of(&doc);
    let mut input = request("tools/list", json!({}));
    input["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] = json!("wrong");
    let (status, error) = call(&server.url, &slug, &key, &input).await;
    assert_eq!(status, 400);
    assert_eq!(error["error"]["code"], -32020);
    let (_,error) = call(&server.url,&slug,&key,&request("tools/call",json!({"name":"document_read","arguments":{"document_id":"another","queries":[{"kind":"context"}]}}))).await;
    assert_eq!(
        error["result"]["structuredContent"]["error"]["code"],
        "permission_changed"
    );
}

async fn editor_key(base: &str, slug: &str) -> String {
    let (status, value) = post_as(
        &session_as(TEST_PUBLISHER),
        base,
        &format!("/api/documents/{slug}/share"),
        json!({"link":{"role":"editor","until":""}}),
    )
    .await;
    assert_eq!(status, 200, "{value}");
    text(&value, "key")
}

#[tokio::test]
async fn mcp_pagination_covers_escaped_unicode_without_skips() {
    let server = new_test_server().await;
    let source = "😀 \"quoted\" \\ line\n".repeat(1600);
    let (status, doc) = post(
        &server.url,
        "/api/documents",
        json!({"title":"Paged paper","source":source,"source_format":"markdown"}),
    )
    .await;
    assert_eq!(status, 201, "{doc}");
    let slug = text(&doc, "slug");
    let key = read_key_of(&doc);
    let mut args =
        json!({"queries":[{"kind":"source"}],"budget":{"max_bytes":6000,"max_tokens":1500}});
    let mut collected = String::new();
    let mut pages = 0;
    loop {
        let page = tool(&server.url, &slug, &key, "document_read", args.clone()).await;
        assert!(page.to_string().len() <= 6000, "{}", page.to_string().len());
        let block = &page["results"][0]["blocks"][0];
        assert_eq!(
            block["start"].as_u64(),
            Some(collected.len() as u64),
            "{page}"
        );
        let text = block["source"].as_str().expect("nonempty source page");
        assert!(!text.is_empty(), "{page}");
        collected.push_str(text);
        pages += 1;
        if page["results"][0]["complete"] == true {
            break;
        }
        assert!(pages < 100, "pagination must progress");
        args["snapshot"] = json!({"view_id":page["view_id"]});
        args["queries"][0]["cursor"] = page["results"][0]["next_cursor"].clone();
    }
    assert_eq!(collected, source);
    assert!(pages > 1);
}

#[tokio::test]
async fn mcp_multi_file_candidate_is_atomic_and_hashes_the_full_tree() {
    let server = new_test_server().await;
    let doc = publish_mcp(&server.url).await;
    let slug = text(&doc, "slug");
    let key = editor_key(&server.url, &slug).await;
    let room = server.instance.rooms.get(&slug).await;
    room.add_text("refs.bib", "@book{old, title={Old}}\n")
        .await
        .expect("file");
    let read = tool(
        &server.url,
        &slug,
        &key,
        "document_read",
        json!({"queries":[{"kind":"source","path":"main.md"},{"kind":"source","path":"refs.bib"}]}),
    )
    .await;
    let proposal=tool(&server.url,&slug,&key,"document_propose",json!({"view_id":read["view_id"],"operation":{"epoch":read["operation_epoch"],"id":"multi"},"publish":"private","patches":[{"range_id":read["results"][0]["blocks"][0]["range_id"],"replacement":"See [@new].\n"},{"range_id":read["results"][1]["blocks"][0]["range_id"],"replacement":"@book{new, title={New}}\n"}]})).await;
    assert!(proposal["candidate_id"].is_string(), "{proposal}");
    let applied=tool(&server.url,&slug,&key,"document_apply",json!({"candidate_id":proposal["candidate_id"],"operation":{"epoch":read["operation_epoch"],"id":"multi-apply"}})).await;
    assert_eq!(
        applied["source_revision_after"], proposal["source_revision"],
        "{applied}"
    );
    let after = tool(
        &server.url,
        &slug,
        &key,
        "document_read",
        json!({"queries":[{"kind":"source","path":"main.md"},{"kind":"source","path":"refs.bib"}]}),
    )
    .await;
    assert_eq!(after["source_revision"], proposal["source_revision"]);
    assert_eq!(after["results"][0]["blocks"][0]["source"], "See [@new].\n");
    assert_eq!(
        after["results"][1]["blocks"][0]["source"],
        "@book{new, title={New}}\n"
    );
}

#[tokio::test]
async fn mcp_accept_and_checkpoint_replay_retained_effects() {
    let server = new_test_server().await;
    let doc = publish_mcp(&server.url).await;
    let slug = text(&doc, "slug");
    let key = editor_key(&server.url, &slug).await;
    let read = tool(
        &server.url,
        &slug,
        &key,
        "document_read",
        json!({"queries":[{"kind":"source"}]}),
    )
    .await;
    let proposal=tool(&server.url,&slug,&key,"document_propose",json!({"view_id":read["view_id"],"operation":{"epoch":read["operation_epoch"],"id":"accept-proposal"},"patches":[{"range_id":read["results"][0]["blocks"][0]["range_id"],"find":"😀 same","replacement":"Accepted text"}]})).await;
    let id = proposal["effects"][0]["id"].clone();
    assert!(id.is_string(), "{proposal}");
    let thread = tool(
        &server.url,
        &slug,
        &key,
        "document_read",
        json!({"queries":[{"kind":"thread","id":id}]}),
    )
    .await;
    let args = json!({"action":"accept","view_id":thread["view_id"],"comment_id":id,"expected_version":thread["results"][0]["thread"]["comment_version"],"operation":{"epoch":thread["operation_epoch"],"id":"accept-once"}});
    let accepted = tool(&server.url, &slug, &key, "document_comment", args.clone()).await;
    assert_eq!(accepted["status"], "committed", "{accepted}");
    let replay = tool(&server.url, &slug, &key, "document_comment", args).await;
    assert_eq!(replay["replay"], true, "{replay}");
    let after = tool(
        &server.url,
        &slug,
        &key,
        "document_read",
        json!({"queries":[{"kind":"source"},{"kind":"thread","id":id}]}),
    )
    .await;
    assert_eq!(
        after["results"][0]["blocks"][0]["source"],
        "# Intro\n\nAccepted text\n\nsame\n"
    );
    assert_eq!(
        after["results"][1]["thread"]["outcome"], "accepted",
        "{after}"
    );
    let checkpoint = json!({"action":"checkpoint","view_id":after["view_id"],"operation":{"epoch":after["operation_epoch"],"id":"checkpoint-once"}});
    let first = tool(
        &server.url,
        &slug,
        &key,
        "document_comment",
        checkpoint.clone(),
    )
    .await;
    assert_eq!(first["status"], "committed", "{first}");
    let second = tool(&server.url, &slug, &key, "document_comment", checkpoint).await;
    assert_eq!(first["checkpoint_id"], second["checkpoint_id"], "{second}");
}

#[tokio::test]
async fn mcp_captured_selection_and_independent_items_preserve_scope() {
    let server = new_test_server().await;
    let doc = publish_mcp(&server.url).await;
    let slug = text(&doc, "slug");
    let key = editor_key(&server.url, &slug).await;
    let read=tool(&server.url,&slug,&key,"document_read",json!({"queries":[{"kind":"source","selection":{"path":"main.md","exact":"same","position":12}}]})).await;
    assert_eq!(read["results"][0]["blocks"][0]["source"], "same", "{read}");
    let result=tool(&server.url,&slug,&key,"document_propose",json!({"view_id":read["view_id"],"operation":{"epoch":read["operation_epoch"],"id":"independent"},"batch":"independent","patches":[{"range_id":read["results"][0]["blocks"][0]["range_id"],"replacement":"one"},{"range_id":"invalid","replacement":"two"}]})).await;
    assert_eq!(result["items"][0]["status"], "committed", "{result}");
    assert_eq!(
        result["items"][1]["error"]["code"], "invalid_range",
        "{result}"
    );
}

/// Deterministic transport microbenchmark. It deliberately measures bytes
/// and service latency, not provider billing or writing quality.
#[tokio::test]
#[ignore = "explicit protocol transfer benchmark"]
async fn mcp_protocol_transfer_benchmark() {
    let server = new_test_server().await;
    let source = format!(
        "A short selected paragraph.\n\n{}",
        "Background material for a large document.\n".repeat(4000)
    );
    let (status, doc) = post(
        &server.url,
        "/api/documents",
        json!({"title":"Transfer benchmark","source":source,"source_format":"markdown"}),
    )
    .await;
    assert_eq!(status, 201, "{doc}");
    let slug = text(&doc, "slug");
    let key = read_key_of(&doc);
    let start = std::time::Instant::now();
    let response = client()
        .get(format!("{}/api/documents/{slug}/snapshot", server.url))
        .header("cookie", session_as(TEST_PUBLISHER))
        .header("x-librepaper-client", "1")
        .header(crate::server::LINK_HEADER, &key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let baseline: Value = response.json().await.unwrap();
    let baseline_ms = start.elapsed().as_secs_f64() * 1000.0;
    let mut timings = Vec::new();
    let mut bytes = 0;
    let mut args = json!({"queries":[{"kind":"source","start":0,"end":27},{"kind":"context"}]});
    for _ in 0..20 {
        let start = std::time::Instant::now();
        let result = tool(&server.url, &slug, &key, "document_read", args.clone()).await;
        assert!(result["error"].is_null(), "{result}");
        timings.push(start.elapsed().as_secs_f64() * 1000.0);
        bytes = result.to_string().len();
        args["snapshot"] = json!({"view_id":result["view_id"]});
    }
    let cold = timings[0];
    timings.sort_by(f64::total_cmp);
    let baseline_bytes = baseline.to_string().len();
    let reduction = 1.0 - bytes as f64 / baseline_bytes as f64;
    eprintln!(
        "{}",
        json!({"workload":"large_document_selected_read","source_bytes":source.len(),"baseline_snapshot_json_bytes":baseline_bytes,"mcp_structured_json_bytes":bytes,"byte_reduction":reduction,"baseline_ms":baseline_ms,"mcp_cold_ms":cold,"mcp_p50_ms":timings[10],"mcp_p95_ms":timings[18],"samples":20,"provider_cost":"not_measured","compression":"not_measured"})
    );
    assert!(
        reduction >= 0.80,
        "focused source should avoid whole-document transfer"
    );
}

#[tokio::test]
async fn mcp_large_candidate_uses_authenticated_data_path_and_revision_bound_render() {
    use crate::room::Outgoing;
    let server = new_test_server().await;
    let source = format!("Selected paragraph.\n\n{}", "background\n".repeat(5000));
    let (_, doc) = post(
        &server.url,
        "/api/documents",
        json!({"title":"Large candidate", "source":source, "source_format":"markdown"}),
    )
    .await;
    let slug = text(&doc, "slug");
    let key = editor_key(&server.url, &slug).await;
    let read = tool(
        &server.url,
        &slug,
        &key,
        "document_read",
        json!({"queries":[{"kind":"source","start":0,"end":19}]}),
    )
    .await;
    let channel = server.instance.chat.create(&slug).await.unwrap();
    let conversation = channel["id"].as_str().unwrap();
    let token = channel["token"].as_str().unwrap();
    let (sender, mut browser) = tokio::sync::mpsc::channel(32);
    server
        .instance
        .chat
        .attach(&slug, conversation, token, "user", 101, sender)
        .await
        .unwrap();
    let args = json!({"view_id":read["view_id"],"operation":{"epoch":read["operation_epoch"],"id":"large-render"},"patches":[{"range_id":read["results"][0]["blocks"][0]["range_id"],"replacement":"Improved paragraph."}],"validation":"compile","publish":"suggestions"});
    let req = request(
        "tools/call",
        json!({"name":"document_propose","arguments":args}),
    );
    let http = client()
        .post(format!("{}/api/documents/{slug}/mcp", server.url))
        .header("cookie", session_as(TEST_PUBLISHER))
        .header("x-librepaper-client", "1")
        .header(crate::server::LINK_HEADER, &key)
        .header("x-librepaper-conversation", conversation)
        .header("x-librepaper-chat-token", token)
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", "tools/call")
        .header("mcp-name", "document_propose")
        .json(&req);
    let pending =
        tokio::spawn(async move { http.send().await.unwrap().json::<Value>().await.unwrap() });
    let frame = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Some(Outgoing::Text(raw)) = browser.recv().await {
                let frame: Value = serde_json::from_str(&raw).unwrap();
                if frame["type"] == "preview_request" {
                    break frame;
                }
            }
        }
    })
    .await
    .unwrap();
    assert!(
        frame.to_string().len() < 2000,
        "bulk source must not enter the chat frame"
    );
    assert!(frame.get("files").is_none());
    let candidate_id = frame["candidate_id"].as_str().unwrap();
    let path = format!(
        "{}/api/documents/{slug}/agent/candidates/{candidate_id}",
        server.url
    );
    let bare = client()
        .get(&path)
        .header("cookie", session_as(TEST_PUBLISHER))
        .header("x-librepaper-client", "1")
        .header(crate::server::LINK_HEADER, &key)
        .send()
        .await
        .unwrap();
    assert_eq!(
        bare.status(),
        404,
        "a candidate id is not an access credential"
    );
    let fetch = |url: String| {
        client()
            .get(url)
            .header("cookie", session_as(TEST_PUBLISHER))
            .header("x-librepaper-client", "1")
            .header(crate::server::LINK_HEADER, &key)
            .header(
                "x-librepaper-candidate-token",
                frame["candidate_token"].as_str().unwrap(),
            )
    };
    let manifest: Value = fetch(path.clone())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let candidate_source = fetch(format!("{path}/source?path=main.md"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(
        candidate_source,
        source.replacen("Selected paragraph.", "Improved paragraph.", 1)
    );
    assert!(candidate_source.len() > 16 * 1024);
    assert_eq!(manifest["source_revision"], frame["revision"]);
    server
        .instance
        .chat
        .relay(
            &slug,
            conversation,
            token,
            101,
            "user",
            json!({
                "type":"preview_result","id":"render-complete","request_id":frame["id"],"task_id":frame["task_id"],
                "revision":frame["revision"],"base_revision":frame["base_revision"],"ok":true,"diagnostics":[],"output":"html",
                "provenance":{"renderer":"test-browser","environment_reproducible":true}
            }),
        )
        .await
        .unwrap();
    let response = pending.await.unwrap();
    let result = &response["result"]["structuredContent"];
    assert_eq!(result["status"], "committed", "{response}");
    assert_eq!(result["effects"].as_array().unwrap().len(), 1);
    let fresh = tool(
        &server.url,
        &slug,
        &key,
        "document_read",
        json!({"queries":[{"kind":"source","start":0,"end":19}]}),
    )
    .await;
    assert_eq!(
        fresh["results"][0]["blocks"][0]["source"], "Selected paragraph.",
        "verification must not apply source"
    );
}

#[tokio::test]
async fn mcp_cancel_pending_render_is_replayable_and_preserves_committed_outcomes() {
    let server = new_test_server().await;
    let doc = publish_mcp(&server.url).await;
    let slug = text(&doc, "slug");
    let key = editor_key(&server.url, &slug).await;
    let read = tool(
        &server.url,
        &slug,
        &key,
        "document_read",
        json!({"queries":[{"kind":"source"}]}),
    )
    .await;
    let target = json!({"epoch":read["operation_epoch"],"id":"pending-render"});
    let args = json!({"view_id":read["view_id"],"operation":target,"patches":[{"range_id":read["results"][0]["blocks"][0]["range_id"],"replacement":"new text"}],"validation":"compile","publish":"suggestions"});
    let pending = tool(&server.url, &slug, &key, "document_propose", args.clone()).await;
    assert_eq!(
        pending["error"]["code"], "renderer_unavailable",
        "{pending}"
    );
    let candidate = &pending["error"]["recovery"]["candidate_id"];
    assert!(candidate.is_string(), "{pending}");
    let cancellation = json!({"action":"cancel","kind":"render","id":candidate,"target_operation":target,"operation":{"epoch":read["operation_epoch"],"id":"cancel-render"}});
    let first = tool(
        &server.url,
        &slug,
        &key,
        "document_result",
        cancellation.clone(),
    )
    .await;
    assert_eq!(first["status"], "cancel_requested", "{first}");
    let again = tool(
        &server.url,
        &slug,
        &key,
        "document_result",
        cancellation.clone(),
    )
    .await;
    assert_eq!(first, again);
    let recovered = tool(
        &server.url,
        &slug,
        &key,
        "document_result",
        json!({"kind":"operation","target_operation":cancellation["operation"]}),
    )
    .await;
    assert_eq!(
        recovered["status"], "cancel_requested",
        "the cancel effect has its own recoverable receipt: {recovered}"
    );
    let retried = tool(&server.url, &slug, &key, "document_propose", args).await;
    assert_eq!(retried["status"], "cancel_requested", "{retried}");
    let mut altered = cancellation;
    altered["target_operation"]["id"] = json!("different");
    let conflict = tool(&server.url, &slug, &key, "document_result", altered).await;
    assert!(conflict["error"].is_object(), "{conflict}");
    let fresh = tool(
        &server.url,
        &slug,
        &key,
        "document_read",
        json!({"queries":[{"kind":"thread"}]}),
    )
    .await;
    assert!(fresh["results"][0]["threads"]
        .as_array()
        .unwrap()
        .is_empty());
    let committed_key = json!({"epoch":read["operation_epoch"],"id":"committed-private"});
    let committed = tool(&server.url, &slug, &key, "document_propose", json!({"view_id":read["view_id"],"operation":committed_key,"patches":[{"range_id":read["results"][0]["blocks"][0]["range_id"],"replacement":"new text"}],"publish":"private"})).await;
    assert_eq!(committed["status"], "committed");
    let late = tool(&server.url, &slug, &key, "document_result", json!({"action":"cancel","kind":"operation","target_operation":committed_key,"operation":{"epoch":read["operation_epoch"],"id":"cancel-committed"}})).await;
    assert_eq!(late["status"], "already_committed", "{late}");
    assert_eq!(late["rollback"], false);
    assert_eq!(late["outcome"]["candidate_id"], committed["candidate_id"]);
    let reused = tool(&server.url, &slug, &key, "document_result", json!({"action":"cancel","kind":"operation","target_operation":target,"operation":committed_key})).await;
    assert_eq!(
        reused["error"]["code"], "operation_key_reused",
        "all mutation tools share one key namespace: {reused}"
    );
}

#[tokio::test]
async fn mcp_retention_expires_terminal_receipts_but_preserves_unresolved_evidence() {
    let server = new_test_server().await;
    let doc = publish_mcp(&server.url).await;
    let slug = text(&doc, "slug");
    let catalog = server.instance.store.catalog.as_ref().unwrap();
    let storage_id = catalog.document(&slug).unwrap().unwrap().storage_id;
    catalog.with_connection(|db| {
        for (id, status) in [("old-terminal", "committed"), ("old-pending", "prepared")] {
            db.execute("INSERT INTO catalog_operations(storage_id,request_id,kind,request_digest,status,intent,result,created_at)
                VALUES(?1,?2,'agent_apply','test',?3,'{}','{}',?4)", rusqlite::params![storage_id,id,status,crate::auth::now_unix()-8*86400])?;
        }
        db.execute("INSERT INTO agent_cancellations(storage_id,target_request_id,cancel_request_id,request_digest,kind,target_id,status,result,created_at)
            VALUES(?1,'old-pending','old-cancel','test','operation','old-pending','cancel_requested','{}',?2)", rusqlite::params![storage_id,crate::auth::now_unix()-8*86400])?;
        Ok(())
    }).unwrap();
    catalog
        .put_agent_object(
            &slug,
            "test-actor",
            "maintenance-trigger",
            "test",
            b"bounded",
            crate::auth::now_unix() + 60,
        )
        .unwrap();
    assert!(catalog
        .operation(&storage_id, "old-terminal")
        .unwrap()
        .is_none());
    assert!(catalog
        .operation(&storage_id, "old-pending")
        .unwrap()
        .is_some());
    assert!(catalog
        .agent_cancellation(&slug, "old-pending")
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn mcp_replacement_runner_fences_old_epoch_without_affecting_external_agents() {
    let server = new_test_server().await;
    let doc = publish_mcp(&server.url).await;
    let slug = text(&doc, "slug");
    let key = editor_key(&server.url, &slug).await;
    let read = tool(
        &server.url,
        &slug,
        &key,
        "document_read",
        json!({"queries":[{"kind":"source"}]}),
    )
    .await;
    let catalog = server.instance.store.catalog.as_ref().unwrap();
    let conversation = "test-conversation";
    let old = catalog
        .issue_agent_execution_lease(&slug, conversation)
        .unwrap();
    let scoped = |epoch: &str, name: &str, args: Value| {
        client()
            .post(format!("{}/api/documents/{slug}/mcp", server.url))
            .header("cookie", session_as(TEST_PUBLISHER))
            .header("x-librepaper-client", "1")
            .header(crate::server::LINK_HEADER, &key)
            .header("x-librepaper-runner-conversation", conversation)
            .header("x-librepaper-execution-epoch", epoch)
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "tools/call")
            .header("mcp-name", name)
            .json(&request(
                "tools/call",
                json!({"name":name,"arguments":args}),
            ))
    };
    let proposed: Value = scoped(&old, "document_propose", json!({"view_id":read["view_id"],"operation":{"epoch":read["operation_epoch"],"id":"runner-propose"},"patches":[{"range_id":read["results"][0]["blocks"][0]["range_id"],"replacement":"Runner edit\n"}],"publish":"private"}))
        .send().await.unwrap().json().await.unwrap();
    let candidate = &proposed["result"]["structuredContent"]["candidate_id"];
    assert!(candidate.is_string(), "{proposed}");
    let fresh_epoch = catalog
        .issue_agent_execution_lease(&slug, conversation)
        .unwrap();
    let args = json!({"candidate_id":candidate,"operation":{"epoch":read["operation_epoch"],"id":"runner-apply"}});
    let stale: Value = scoped(&old, "document_apply", args.clone())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        stale["result"]["structuredContent"]["error"]["code"], "permission_changed",
        "{stale}"
    );
    let accepted: Value = scoped(&fresh_epoch, "document_apply", args)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        accepted["result"]["structuredContent"]["status"], "committed",
        "{accepted}"
    );
    let external = tool(
        &server.url,
        &slug,
        &key,
        "document_read",
        json!({"queries":[{"kind":"source"}]}),
    )
    .await;
    assert_eq!(
        external["results"][0]["blocks"][0]["source"],
        "Runner edit\n"
    );
}
