use super::*;
use crate::document::render::{document_format, main_path_for, title_from_quarto};
use serde_json::json;

fn imported_bundle(slug: &str, render: &str, html: &str) -> serde_json::Value {
    use base64::Engine;
    let digest = crate::quarto::sha256(html.as_bytes());
    json!({
        "manifest": {
            "schema": "librepaper-quarto-bundle/v1", "render_id": render, "document_id": slug,
            "engine": "quarto",
            "source": {"revision":"", "tree_sha256":null, "main":"main.qmd", "verification":"imported"},
            "context": {"id":"html", "fingerprint_version":1,"computation_sha256":crate::quarto::sha256(b"unknown"),"format":"html"},
            "provenance": {"kind":"imported", "computation":"no-execution", "external_inputs":"unknown"},
            "artifact": {"kind":"html", "entrypoint":"paper.html", "sha256":digest, "size":html.len(), "mime":"text/html"},
            "cells":[], "assets":[], "coverage":{"full_artifact":true,"cell_outputs":"none"}
        },
        "blobs":[{"sha256":digest,"mime":"text/html","data":base64::engine::general_purpose::STANDARD.encode(html)}],
        "select":true,"expected_generation":0
    })
}

async fn publish_quarto(base: &str) -> serde_json::Value {
    let (status, document) = post(
        base,
        "/api/documents",
        json!({
            "title":"Qmd", "source":"# Paper\n", "source_format":"quarto"
        }),
    )
    .await;
    assert_eq!(status, 201, "{document}");
    document
}

#[tokio::test]
async fn quarto_render_checkpoint_is_durable_and_refuses_different_inputs() {
    let server = new_test_server().await;
    let slug = text(&publish_quarto(&server.url).await, "slug");
    let room = server.instance.rooms.try_get(&slug).await.unwrap();
    let digest = room.tree().await.digest();
    let route = format!("/api/documents/{slug}/quarto/checkpoint");
    let (status, response) = post(&server.url, &route, json!({"tree_sha256":digest})).await;
    assert_eq!(status, 200, "{response}");
    let revision = text(&response, "revision");
    let point = room.checkpoint_by_sha(&revision).await.unwrap().unwrap();
    assert_eq!(point.content_sha(), digest);
    let (status, _) = post(
        &server.url,
        &route,
        json!({"tree_sha256":crate::quarto::sha256(b"different")}),
    )
    .await;
    assert_eq!(status, 409);
    let (status, _) = post_as("", &server.url, &route, json!({"tree_sha256":digest})).await;
    assert_eq!(status, 403);
}

#[tokio::test]
#[ignore = "requires installed Quarto, R, knitr and rmarkdown"]
async fn quarto_managed_render_publishes_against_a_real_durable_checkpoint() {
    use crate::local::{protocol::*, quarto::*};
    use base64::Engine;
    let source = include_str!("fixtures/quarto/r.qmd");
    let server = new_test_server().await;
    let (status, document) = post(
        &server.url,
        "/api/documents",
        json!({"title":"Managed Quarto", "source":source,"source_format":"quarto"}),
    )
    .await;
    assert_eq!(status, 201, "{document}");
    let slug = text(&document, "slug");
    let tree = server
        .instance
        .rooms
        .try_get(&slug)
        .await
        .unwrap()
        .tree()
        .await;
    let (status, checkpoint) = post(
        &server.url,
        &format!("/api/documents/{slug}/quarto/checkpoint"),
        json!({"tree_sha256":tree.digest()}),
    )
    .await;
    assert_eq!(status, 200, "{checkpoint}");
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("project");
    let workspace = Workspace {
        root: directory.path().join("job"),
    };
    std::fs::create_dir(&project).unwrap();
    std::fs::create_dir(&workspace.root).unwrap();
    std::fs::write(project.join(&tree.main), source).unwrap();
    let bindings = BindingStore::new(&directory.path().join("config"));
    let binding = bindings
        .grant(&server.url, &slug, &project, &tree.main)
        .unwrap();
    let request = JobRequest {
        protocol: 1,
        kind: "quarto".into(),
        project: slug.clone(),
        origin: server.url.clone(),
        snapshot: text(&checkpoint, "revision"),
        generation: 1,
        engine: String::new(),
        main: tree.main.clone(),
        stem: String::new(),
        quarto: Some(QuartoJobOptions {
            binding_id: binding.id,
            main: tree.main.clone(),
            shared_tree_sha256: Some(tree.digest()),
            ..Default::default()
        }),
        manifest: vec![ManifestEntry {
            path: tree.main.clone(),
            sha256: crate::quarto::sha256(source.as_bytes()),
            size: source.len() as u64,
        }],
        options: JobOptions {
            deadline_seconds: 60,
            ..Default::default()
        },
    };
    let (_sender, cancel) = tokio::sync::watch::channel(false);
    let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let outcome = run_job_with_bindings(request, workspace, cancel, progress, &bindings).await;
    assert_eq!(outcome.status.status, "done", "{:?}", outcome.status);
    let manifest: crate::quarto::BundleManifest =
        serde_json::from_slice(&outcome.files["quarto-bundle.json"]).unwrap();
    assert_eq!(
        manifest.source.tree_sha256.as_deref(),
        Some(tree.digest().as_str())
    );
    let artifact = manifest.artifact.as_ref().unwrap();
    let mut uploads = std::collections::BTreeMap::new();
    uploads.insert(artifact.sha256.clone(), json!({"sha256":artifact.sha256,"mime":artifact.mime,"data":base64::engine::general_purpose::STANDARD.encode(&outcome.files["artifact.html"])}));
    for asset in &manifest.assets {
        uploads.insert(asset.sha256.clone(), json!({"sha256":asset.sha256,"mime":asset.mime,"data":base64::engine::general_purpose::STANDARD.encode(&outcome.files[&format!("asset:{}",asset.path)])}));
    }
    let (status, receipt) = post(
        &server.url,
        &format!("/api/documents/{slug}/quarto/bundles"),
        json!({"manifest":manifest,"blobs":uploads.into_values().collect::<Vec<_>>(),"select":true,"expected_generation":0}),
    ).await;
    assert_eq!(status, 201, "{receipt}");
    assert_eq!(
        receipt["manifest"]["source"]["revision"],
        checkpoint["revision"]
    );
}

#[tokio::test]
async fn quarto_bundle_publication_round_trips_and_is_idempotent() {
    let server = new_test_server().await;
    let slug = text(&publish_quarto(&server.url).await, "slug");
    let payload = imported_bundle(&slug, "render-one", "<!doctype html><p>Saved result</p>");
    let route = format!("/api/documents/{slug}/quarto/bundles");
    let (status, first) = post(&server.url, &route, payload.clone()).await;
    assert_eq!(status, 201, "{first}");
    let (status, second) = post(&server.url, &route, payload).await;
    assert!(
        matches!(status, 200 | 201),
        "idempotent retry: {status} {second}"
    );
    assert_eq!(
        first["selection"]["generation"],
        second["selection"]["generation"]
    );
    let (status, selected) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("{route}/selected/html"),
    )
    .await;
    assert_eq!(status, 200, "{selected}");
    assert_eq!(selected["manifest"]["render_id"], "render-one");
}

#[tokio::test]
async fn document_execution_engine_is_validated_on_create_and_update() {
    let server = new_test_server().await;
    for (slug, engine) in [("calepin-create", "calepin"), ("none-qmd-create", "none")] {
        let (status, body) = post(
            &server.url,
            "/api/documents",
            json!({
                "slug": slug,
                "title": "Rejected engine",
                "source": "# Paper\n",
                "source_format": "quarto",
                "execution_engine": engine,
                "draft_format": "markdown"
            }),
        )
        .await;
        assert_eq!(status, 400, "{engine}: {body}");
    }

    let (status, created) = post(
        &server.url,
        "/api/documents",
        json!({
            "slug": "explicit-quarto",
            "title": "Explicit Quarto",
            "source": "# Paper\n",
            "source_format": "quarto",
            "execution_engine": "quarto",
            "draft_format": "markdown"
        }),
    )
    .await;
    assert_eq!(status, 201, "{created}");
    let slug = text(&created, "slug");
    let (status, document) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("/api/documents/{slug}"),
    )
    .await;
    assert_eq!(status, 200, "{document}");
    assert_eq!(document["execution_engine"], "quarto");
    assert_eq!(document["draft_format"], "markdown");

    for (engine, draft_format) in [
        ("calepin", "markdown"),
        ("none", "markdown"),
        ("quarto", "typst"),
    ] {
        let (status, body) = post(
            &server.url,
            "/api/documents",
            json!({
                "slug": slug,
                "title": "Explicit Quarto",
                "source": "# Changed\n",
                "source_format": "quarto",
                "execution_engine": engine,
                "draft_format": draft_format
            }),
        )
        .await;
        assert_eq!(status, 400, "{engine}/{draft_format}: {body}");
    }
}

#[tokio::test]
async fn invalid_bundle_engines_and_reserved_assets_do_not_change_selection() {
    use base64::Engine;
    let server = new_test_server().await;
    let slug = text(&publish_quarto(&server.url).await, "slug");
    let route = format!("/api/documents/{slug}/quarto/bundles");
    let (status, first) = post(
        &server.url,
        &route,
        imported_bundle(&slug, "stable-engine", "<p>Stable</p>"),
    )
    .await;
    assert_eq!(status, 201, "{first}");

    for (render, engine) in [("engine-none", "none"), ("engine-calepin", "calepin")] {
        let mut payload = imported_bundle(&slug, render, "<p>Rejected</p>");
        payload["manifest"]["engine"] = json!(engine);
        let (status, body) = post(&server.url, &route, payload).await;
        assert_eq!(status, 400, "{engine}: {body}");
    }

    let css = b".paper { color: red }";
    let digest = crate::quarto::sha256(css);
    let mut reserved = imported_bundle(&slug, "draft-dependency", "<p>Rejected</p>");
    reserved["manifest"]["assets"] = json!([{
        "path": "runtime.css",
        "sha256": digest,
        "mime": "text/css",
        "size": css.len(),
        "role": "draft-dependency"
    }]);
    reserved["blobs"].as_array_mut().unwrap().push(json!({
        "sha256": digest,
        "mime": "text/css",
        "data": base64::engine::general_purpose::STANDARD.encode(css)
    }));
    let (status, body) = post(&server.url, &route, reserved).await;
    assert_eq!(status, 400, "reserved draft dependency: {body}");

    let (status, selected) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("{route}/selected/html"),
    )
    .await;
    assert_eq!(status, 200, "{selected}");
    assert_eq!(selected["manifest"]["render_id"], "stable-engine");
    assert_eq!(
        selected["selection"]["generation"],
        first["selection"]["generation"]
    );
}

#[tokio::test]
async fn published_bundle_remains_readable_after_document_format_change() {
    use base64::Engine;
    let server = new_test_server().await;
    let document = publish_quarto(&server.url).await;
    let slug = text(&document, "slug");
    let route = format!("/api/documents/{slug}/quarto/bundles");
    let html = "<p>Persisted Quarto result</p>";
    let css = b".paper { color: blue }";
    let css_digest = crate::quarto::sha256(css);
    let mut bundle = imported_bundle(&slug, "before-format-change", html);
    bundle["manifest"]["assets"] = json!([{
        "path": "runtime.css",
        "sha256": css_digest,
        "mime": "text/css",
        "size": css.len()
        ,"role": "display"
    }]);
    bundle["blobs"].as_array_mut().unwrap().push(json!({
        "sha256": css_digest,
        "mime": "text/css",
        "data": base64::engine::general_purpose::STANDARD.encode(css)
    }));
    let (status, published) = post(&server.url, &route, bundle).await;
    assert_eq!(status, 201, "{published}");

    let (status, comment) = post(
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({
            "type": "comment",
            "body": "Keep this output available after a source migration.",
            "creator": "Reviewer",
            "temp_id": "22222222-2222-4222-8222-222222222222",
            "output_anchor": {
                "render_id": "before-format-change",
                "cell_id": "",
                "output_ordinal": 0,
                "content_sha256": crate::quarto::sha256(html.as_bytes()),
                "coordinate_system": "pixel",
                "width": 800,
                "height": 600
            }
        }),
    )
    .await;
    assert_eq!(status, 200, "{comment}");

    let (status, updated) = post(
        &server.url,
        "/api/documents",
        json!({
            "slug": slug,
            "title": "Qmd",
            "source": "# Markdown now\n",
            "source_format": "markdown",
            "execution_engine": "none",
            "draft_format": "markdown"
        }),
    )
    .await;
    assert_eq!(status, 201, "{updated}");

    let (status, manifest) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("{route}/before-format-change"),
    )
    .await;
    assert_eq!(status, 200, "{manifest}");
    assert_eq!(manifest["render_id"], "before-format-change");
    let asset = client()
        .get(format!(
            "{}/api/documents/{slug}/quarto/bundles/before-format-change/asset?path=runtime.css",
            server.url
        ))
        .header("cookie", session_as(TEST_PUBLISHER))
        .send()
        .await
        .unwrap();
    assert_eq!(asset.status(), 200);
    assert_eq!(asset.bytes().await.unwrap().as_ref(), css);
    let response = client()
        .get(format!(
            "{}/api/documents/{slug}/quarto/bundles/before-format-change/artifact",
            server.url
        ))
        .header("cookie", session_as(TEST_PUBLISHER))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await.unwrap(), html);
    let (status, comments) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("/api/documents/{slug}/comments"),
    )
    .await;
    assert_eq!(status, 200, "{comments}");
    assert_eq!(
        comments["comments"][0]["output_anchor"]["render_id"],
        "before-format-change"
    );
}

#[tokio::test]
async fn quarto_unchanged_artifact_bytes_need_no_second_upload() {
    let server = new_test_server().await;
    let slug = text(&publish_quarto(&server.url).await, "slug");
    let route = format!("/api/documents/{slug}/quarto/bundles");
    let (status, first) = post(
        &server.url,
        &route,
        imported_bundle(&slug, "original", "<p>Same bytes</p>"),
    )
    .await;
    assert_eq!(status, 201, "{first}");
    let mut next = imported_bundle(&slug, "next-render", "<p>Same bytes</p>");
    next["blobs"] = json!([]);
    next["expected_generation"] = json!(1);
    let (status, selected) = post(&server.url, &route, next).await;
    assert_eq!(status, 201, "{selected}");
    assert_eq!(selected["selection"]["generation"], 2);
}

#[tokio::test]
async fn quarto_artifact_api_cannot_execute_html_on_application_origin() {
    let server = new_test_server().await;
    let slug = text(&publish_quarto(&server.url).await, "slug");
    let html = "<!doctype html><script>fetch('/api/me')</script>";
    let (status, result) = post(
        &server.url,
        &format!("/api/documents/{slug}/quarto/bundles"),
        imported_bundle(&slug, "hostile-html", html),
    )
    .await;
    assert_eq!(status, 201, "{result}");
    let response = client()
        .get(format!(
            "{}/api/documents/{slug}/quarto/bundles/hostile-html/artifact",
            server.url
        ))
        .header("cookie", session_as(TEST_PUBLISHER))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(response
        .headers()
        .get("content-disposition")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("attachment")));
    assert!(response
        .headers()
        .get("content-security-policy")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("sandbox")));
    assert_eq!(response.text().await.unwrap(), html);
}

#[tokio::test]
async fn quarto_bad_blob_cannot_publish_a_manifest() {
    let server = new_test_server().await;
    let slug = text(&publish_quarto(&server.url).await, "slug");
    let mut payload = imported_bundle(&slug, "bad-bytes", "<p>Expected</p>");
    payload["blobs"][0]["data"] = json!("YWJj");
    let (status, result) = post(
        &server.url,
        &format!("/api/documents/{slug}/quarto/bundles"),
        payload,
    )
    .await;
    assert_eq!(status, 400, "{result}");
    let (status, _) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("/api/documents/{slug}/quarto/bundles/bad-bytes"),
    )
    .await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn quarto_late_selection_does_not_replace_newer_output() {
    let server = new_test_server().await;
    let slug = text(&publish_quarto(&server.url).await, "slug");
    let route = format!("/api/documents/{slug}/quarto/bundles");
    let (status, first) = post(
        &server.url,
        &route,
        imported_bundle(&slug, "first", "<p>First</p>"),
    )
    .await;
    assert_eq!(status, 201, "{first}");
    let (status, conflict) = post(
        &server.url,
        &route,
        imported_bundle(&slug, "late", "<p>Late</p>"),
    )
    .await;
    assert_eq!(status, 409, "{conflict}");
    let (_, selected) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("{route}/selected/html"),
    )
    .await;
    assert_eq!(selected["manifest"]["render_id"], "first");
    // The completed artifact remains available even when selection loses.
    let (status, saved) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("{route}/late"),
    )
    .await;
    assert_eq!(status, 200, "{saved}");
}

#[tokio::test]
async fn catalog_selection_survives_a_mismatched_transport_blob() {
    let server = new_test_server().await;
    let slug = text(&publish_quarto(&server.url).await, "slug");
    let route = format!("/api/documents/{slug}/quarto/bundles");
    let (status, _) = post(
        &server.url,
        &route,
        imported_bundle(&slug, "stable", "<p>Stable</p>"),
    )
    .await;
    assert_eq!(status, 201);
    let room = server.instance.rooms.get(&slug).await;
    let key = crate::quarto::scoped_selection_key(room.quarto_scope(), &slug, "html");
    let forged = crate::quarto::Selection {
        document_id: slug.clone(),
        context_id: "html".into(),
        generation: 99,
        render_id: "forged".into(),
        source_revision: String::new(),
    };
    server
        .instance
        .store
        .blobs
        .put(
            &key,
            serde_json::to_vec(&forged).unwrap(),
            "application/json",
        )
        .await
        .unwrap();
    let (status, selected) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("{route}/selected/html"),
    )
    .await;
    assert_eq!(status, 200, "{selected}");
    assert_eq!(selected["manifest"]["render_id"], "stable");
}

#[tokio::test]
async fn revoked_publisher_cannot_expose_a_paused_bundle_after_failed_rollback() {
    use super::room::{fixture, HookStore};
    use crate::config::Configuration;
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    store
        .blobs
        .delete(&[crate::storage::blob::room_lock_key("probe")])
        .await
        .unwrap();
    let hooked = HookStore::new(store.blobs.clone());
    let (base, instance) =
        server_over_blobs_catalog(hooked.clone(), Configuration::default()).await;
    let slug = text(&publish_quarto(&base).await, "slug");
    let route = format!("/api/documents/{slug}/quarto/bundles");
    let (status, _) = post(
        &base,
        &route,
        imported_bundle(&slug, "before-revoke", "<p>Old</p>"),
    )
    .await;
    assert_eq!(status, 201);
    let room = instance.rooms.get(&slug).await;
    let manifest_key =
        crate::quarto::scoped_manifest_key(room.quarto_scope(), &slug, "after-revoke");
    *hooked.pause.lock().unwrap() = Some(("swap".into(), manifest_key.clone()));
    let request = tokio::spawn({
        let base = base.clone();
        let route = route.clone();
        let slug_for_request = slug.clone();
        async move {
            post(
                &base,
                &route,
                imported_bundle(&slug_for_request, "after-revoke", "<p>New</p>"),
            )
            .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), hooked.reached.notified())
        .await
        .expect("manifest swap was paused");
    instance
        .store
        .catalog
        .as_ref()
        .unwrap()
        .revoke_sessions("github:vincent", "revoked-generation")
        .unwrap();
    *hooked.fail_delete.lock().unwrap() = Some(manifest_key.clone());
    hooked.resume.notify_one();
    let (status, _) = request.await.unwrap();
    assert_eq!(status, 403);
    let (status, _) = get_json_as("", &base, &format!("{route}/after-revoke")).await;
    assert_eq!(
        status, 404,
        "a revoked publisher cannot read the staged manifest"
    );
    let (selection, _) = room
        .quarto_selection(&slug, "html")
        .await
        .unwrap()
        .expect("previous selection remains durable");
    assert_eq!(selection.render_id, "before-revoke");
    assert!(!room.quarto_object_committed(&manifest_key).await.unwrap());
    instance
        .store
        .catalog
        .as_ref()
        .unwrap()
        .revoke_sessions("github:vincent", "test-session-generation")
        .unwrap();
    let (status, _) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &base,
        &format!("{route}/after-revoke"),
    )
    .await;
    assert_eq!(
        status, 404,
        "an authorized retry cannot read an uncommitted transport manifest"
    );
    let mut retry_bundle = imported_bundle(&slug, "after-revoke", "<p>New</p>");
    retry_bundle["expected_generation"] = json!(1);
    let (status, repaired) = post(&base, &route, retry_bundle).await;
    assert_eq!(status, 201, "{repaired}");
    assert!(room.quarto_object_committed(&manifest_key).await.unwrap());
}

#[tokio::test]
async fn failed_quarto_manifest_deletion_preserves_the_bundle_and_dependencies() {
    use super::room::{fixture, HookStore};
    use crate::config::Configuration;
    use base64::Engine;
    let mut config = Configuration::default();
    config.session.history_max = 1;
    let (_dir, store, _rooms) = fixture(config.clone()).await;
    store
        .blobs
        .delete(&[crate::storage::blob::room_lock_key("probe")])
        .await
        .unwrap();
    let hooked = HookStore::new(store.blobs.clone());
    let (base, instance) = server_over_blobs_legacy(hooked.clone(), config).await;
    let slug = text(&publish_quarto(&base).await, "slug");
    let route = format!("/api/documents/{slug}/quarto/bundles");
    let room = instance.rooms.get(&slug).await;
    let doomed = crate::quarto::scoped_manifest_key(room.quarto_scope(), &slug, "failed-delete");
    for (render, completed) in [
        ("keep", "1"),
        ("failed-delete", "2"),
        ("newest", "3"),
        ("pruned", "4"),
    ] {
        if render == "failed-delete" {
            // The publication completion pass prunes immediately. Install the
            // failure before the next bundle arrives so this older bundle is
            // exercised by that pass as well as by the later checkpoint.
            *hooked.fail_delete.lock().unwrap() = Some(doomed.clone());
        }
        let mut bundle = imported_bundle(&slug, render, &format!("<p>{render}</p>"));
        bundle["manifest"]["provenance"]["completed_at"] = json!(completed);
        if render != "keep" {
            bundle["select"] = json!(false);
        }
        if render == "failed-delete" {
            let css = b".paper { color: red }";
            let digest = crate::quarto::sha256(css);
            bundle["manifest"]["assets"] = json!([{
                "path": "styles.css",
                "sha256": digest,
                "mime": "text/css",
                "size": css.len(),
                "role": "display"
            }]);
            bundle["blobs"].as_array_mut().unwrap().push(json!({
                "sha256": digest,
                "mime": "text/css",
                "data": base64::engine::general_purpose::STANDARD.encode(css)
            }));
        }
        let (status, body) = post(&base, &route, bundle).await;
        assert_eq!(status, 201, "{body}");
    }
    // Publication failure cleanup is scheduled when the HTTP handler exits;
    // it must not depend on a later source edit or checkpoint to run. Wait
    // for the successful follow-up publication's sweep to prune `newest`.
    let pruned = crate::quarto::scoped_manifest_key(room.quarto_scope(), &slug, "newest");
    for _ in 0..40 {
        if matches!(
            hooked.inner.get(&pruned).await,
            Err(crate::storage::blob::BlobError::NotFound)
        ) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(
        hooked.inner.get(&doomed).await.is_ok(),
        "a failed manifest deletion must preserve its manifest and dependency graph"
    );
    let failed_blob = crate::quarto::scoped_blob_key(
        room.quarto_scope(),
        &crate::quarto::sha256(b".paper { color: red }"),
    );
    assert_eq!(
        hooked.inner.get(&failed_blob).await.unwrap(),
        b".paper { color: red }",
        "dependencies of an unconfirmed deletion must remain readable"
    );
    assert!(
        matches!(
            hooked.inner.get(&pruned).await,
            Err(crate::storage::blob::BlobError::NotFound)
        ),
        "an unreferenced bundle should still be pruned"
    );
}

#[tokio::test]
async fn quarto_retention_bounds_repeated_renders_of_one_source_revision() {
    use crate::config::Configuration;
    let mut config = Configuration::default();
    config.session.history_max = 1;
    let server = test_server_with(
        config,
        crate::auth::Policy::parse(TEST_PUBLISHER),
        crate::auth::Policy::parse("anyone"),
        true,
    )
    .await;
    let slug = text(&publish_quarto(&server.url).await, "slug");
    let room = server.instance.rooms.get(&slug).await;
    let tree = room.tree().await;
    let route = format!("/api/documents/{slug}/quarto/bundles");
    for render in ["same-1", "same-2", "same-3"] {
        let mut bundle = imported_bundle(&slug, render, &format!("<p>{render}</p>"));
        bundle["manifest"]["source"]["tree_sha256"] = json!(tree.digest());
        bundle["manifest"]["source"]["main"] = json!(tree.main);
        bundle["select"] = json!(false);
        bundle["manifest"]["provenance"]["completed_at"] = json!(render);
        let (status, body) = post(&server.url, &route, bundle).await;
        assert_eq!(status, 201, "{body}");
    }
    room.set_source("changed", "quarto").await.unwrap();
    room.checkpoint_now("cli", "alice").await.unwrap();
    let old = crate::quarto::scoped_manifest_key(room.quarto_scope(), &slug, "same-1");
    let newest = crate::quarto::scoped_manifest_key(room.quarto_scope(), &slug, "same-3");
    assert!(matches!(
        server.instance.store.blobs.get(&old).await,
        Err(crate::storage::blob::BlobError::NotFound)
    ));
    assert!(server.instance.store.blobs.get(&newest).await.is_ok());
}

#[tokio::test]
async fn quarto_output_comment_keeps_its_immutable_render_and_round_trips() {
    let mut config = crate::config::Configuration::default();
    config.session.history_max = 1;
    let server = test_server_with(
        config,
        crate::auth::Policy::parse(TEST_PUBLISHER),
        crate::auth::Policy::parse("anyone"),
        true,
    )
    .await;
    let slug = text(&publish_quarto(&server.url).await, "slug");
    let route = format!("/api/documents/{slug}/quarto/bundles");
    let mut old_bundle = imported_bundle(&slug, "commented-render", "<p>Old result</p>");
    old_bundle["manifest"]["provenance"]["completed_at"] = json!("2026-01-01T00:00:00Z");
    let (status, first) = post(&server.url, &route, old_bundle).await;
    assert_eq!(status, 201, "{first}");
    let content_sha256 = crate::quarto::sha256(b"<p>Old result</p>");
    let (status, comment) = post(
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({
            "type": "comment",
            "body": "This conclusion needs checking.",
            "creator": "Reviewer",
            "temp_id": "11111111-1111-4111-8111-111111111111",
            "output_anchor": {
                "render_id": "commented-render",
                "cell_id": "",
                "output_ordinal": 0,
                "content_sha256": content_sha256,
                "coordinate_system": "pixel",
                "width": 800,
                "height": 600
            }
        }),
    )
    .await;
    assert_eq!(status, 200, "{comment}");
    let mut second = imported_bundle(&slug, "new-render", "<p>New result</p>");
    second["manifest"]["provenance"]["completed_at"] = json!("2026-01-02T00:00:00Z");
    second["expected_generation"] = json!(1);
    let (status, published) = post(&server.url, &route, second).await;
    assert_eq!(status, 201, "{published}");
    // One selected render and one spare fill the normal retention allowance.
    // Only the comment reference can keep the older original alive.
    let mut third = imported_bundle(&slug, "latest-render", "<p>Latest result</p>");
    third["manifest"]["provenance"]["completed_at"] = json!("2026-01-03T00:00:00Z");
    third["expected_generation"] = json!(2);
    let (status, published) = post(&server.url, &route, third).await;
    assert_eq!(status, 201, "{published}");
    let room = server.instance.rooms.get(&slug).await;
    room.prune_quarto_now().await;
    let first_key =
        crate::quarto::scoped_manifest_key(room.quarto_scope(), &slug, "commented-render");
    assert!(
        server.instance.store.blobs.get(&first_key).await.is_ok(),
        "a comment must retain the immutable render it names"
    );
    let (status, comments) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("/api/documents/{slug}/comments"),
    )
    .await;
    assert_eq!(status, 200, "{comments}");
    assert_eq!(
        comments["comments"][0]["output_anchor"]["render_id"],
        "commented-render"
    );
}

#[tokio::test]
async fn restoring_source_clears_quarto_selection_explicitly() {
    let server = new_test_server().await;
    let document = publish_quarto(&server.url).await;
    let slug = text(&document, "slug");
    let room = server.instance.rooms.try_get(&slug).await.unwrap();
    let (status, checkpoint) = post(
        &server.url,
        &format!("/api/documents/{slug}/quarto/checkpoint"),
        json!({"tree_sha256": room.tree().await.digest()}),
    )
    .await;
    assert_eq!(status, 200, "{checkpoint}");
    let (status, published) = post(
        &server.url,
        &format!("/api/documents/{slug}/quarto/bundles"),
        imported_bundle(&slug, "restore-selected", "<p>Saved</p>"),
    )
    .await;
    assert_eq!(status, 201, "{published}");
    let (status, restored) = post(
        &server.url,
        &format!("/api/documents/{slug}/restore"),
        json!({"sha": text(&checkpoint, "revision")}),
    )
    .await;
    assert_eq!(status, 200, "{restored}");
    assert_eq!(restored["quarto_selection"]["status"], "cleared");
    let (status, selected) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("/api/documents/{slug}/quarto/bundles/selected/html"),
    )
    .await;
    assert_eq!(status, 404, "{selected}");
    assert_eq!(selected["generation"], 2);
}

#[tokio::test]
async fn restoring_source_reselects_a_retained_matching_bundle() {
    let server = new_test_server().await;
    let document = publish_quarto(&server.url).await;
    let slug = text(&document, "slug");
    let room = server.instance.rooms.try_get(&slug).await.unwrap();
    let tree = room.tree().await;
    let (status, checkpoint) = post(
        &server.url,
        &format!("/api/documents/{slug}/quarto/checkpoint"),
        json!({"tree_sha256": tree.digest()}),
    )
    .await;
    assert_eq!(status, 200, "{checkpoint}");
    let mut bundle = imported_bundle(&slug, "restore-retained", "<p>Saved</p>");
    bundle["manifest"]["source"]["tree_sha256"] = json!(tree.digest());
    let (status, published) = post(
        &server.url,
        &format!("/api/documents/{slug}/quarto/bundles"),
        bundle,
    )
    .await;
    assert_eq!(status, 201, "{published}");
    let (status, restored) = post(
        &server.url,
        &format!("/api/documents/{slug}/restore"),
        json!({"sha": text(&checkpoint, "revision")}),
    )
    .await;
    assert_eq!(status, 200, "{restored}");
    assert_eq!(restored["quarto_selection"]["status"], "reselected");
    let (status, selected) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("/api/documents/{slug}/quarto/bundles/selected/html"),
    )
    .await;
    assert_eq!(status, 200, "{selected}");
    assert_eq!(selected["manifest"]["render_id"], "restore-retained");

    room.set_source("# Changed\n", "quarto").await.unwrap();
    let changed_revision = room.checkpoint_now("cli", "alice").await.unwrap().unwrap();
    let (status, cleared) = post(
        &server.url,
        &format!("/api/documents/{slug}/restore"),
        json!({"sha": changed_revision}),
    )
    .await;
    assert_eq!(status, 200, "{cleared}");
    assert_eq!(cleared["quarto_selection"]["status"], "cleared");
    let (status, restored_again) = post(
        &server.url,
        &format!("/api/documents/{slug}/restore"),
        json!({"sha": text(&checkpoint, "revision")}),
    )
    .await;
    assert_eq!(status, 200, "{restored_again}");
    assert_eq!(restored_again["quarto_selection"]["status"], "reselected");
}

#[tokio::test]
async fn quarto_artifacts_and_mutations_require_document_access() {
    let server = new_test_server().await;
    let slug = text(&publish_quarto(&server.url).await, "slug");
    let route = format!("/api/documents/{slug}/quarto/bundles");
    let (status, result) = post(
        &server.url,
        &route,
        imported_bundle(&slug, "private", "<p>Private</p>"),
    )
    .await;
    assert_eq!(status, 201, "{result}");
    for suffix in ["private", "private/artifact", "selected/html"] {
        let (status, _) = get_json_as("", &server.url, &format!("{route}/{suffix}")).await;
        assert_eq!(status, 404, "{suffix}");
    }
    let (status, denied) = post_as(
        "",
        &server.url,
        &route,
        imported_bundle(&slug, "unauthorized", "<p>No</p>"),
    )
    .await;
    assert_eq!(status, 403, "{denied}");
}

#[tokio::test]
#[ignore = "requires installed Quarto, R, knitr and rmarkdown"]
async fn quarto_real_html_import_uploads_required_resources() {
    use base64::Engine;
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("paper.qmd"),
        include_str!("fixtures/quarto/r.qmd"),
    )
    .unwrap();
    let rendered = std::process::Command::new("quarto")
        .args(["render", "paper.qmd", "--to", "html", "--no-execute-daemon"])
        .current_dir(directory.path())
        .output()
        .unwrap();
    assert!(
        rendered.status.success(),
        "{}",
        String::from_utf8_lossy(&rendered.stderr)
    );
    // An unrelated sibling must not enter an artifact import.
    std::fs::write(directory.path().join("private.csv"), "private data").unwrap();
    let imported =
        crate::local::quarto::import_artifact(directory.path(), "paper.html", "html").unwrap();
    let server = new_test_server().await;
    let slug = text(&publish_quarto(&server.url).await, "slug");
    let mut manifest = imported.to_storage_manifest(&slug, "");
    manifest.source.main = "main.qmd".into();
    manifest.source.verification = crate::quarto::Verification::Imported;
    manifest.provenance.kind = crate::quarto::ProvenanceKind::Imported;
    manifest.validate().unwrap();
    assert!(!manifest
        .assets
        .iter()
        .any(|asset| asset.path == "private.csv"));
    assert!(manifest
        .assets
        .iter()
        .any(|asset| asset.path.ends_with(".css")));
    assert!(manifest
        .assets
        .iter()
        .any(|asset| asset.path.ends_with(".png")));
    assert!(
        manifest
            .assets
            .iter()
            .any(|asset| asset.path.ends_with(".woff") || asset.path.ends_with(".woff2")),
        "CSS font dependencies must be included"
    );
    let mut blobs = Vec::new();
    let artifact = manifest.artifact.as_ref().unwrap();
    for (path, digest, mime) in
        std::iter::once((&artifact.entrypoint, &artifact.sha256, &artifact.mime)).chain(
            manifest
                .assets
                .iter()
                .map(|asset| (&asset.path, &asset.sha256, &asset.mime)),
        )
    {
        let bytes = std::fs::read(directory.path().join(path)).unwrap();
        blobs.push(crate::quarto::BlobUpload {
            sha256: digest.clone(),
            mime: mime.clone(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        });
    }
    let payload = crate::quarto::PublishRequest {
        manifest,
        blobs,
        select: true,
        expected_generation: Some(0),
    };
    let (status, published) = post(
        &server.url,
        &format!("/api/documents/{slug}/quarto/bundles"),
        serde_json::to_value(payload).unwrap(),
    )
    .await;
    assert_eq!(status, 201, "{published}");
}

#[test]
fn quarto_format_and_front_matter_title_are_preserved() {
    assert_eq!(document_format("chapter/PAPER.QMD"), Some("quarto"));
    assert_eq!(document_format("paper.md"), Some("markdown"));
    assert_eq!(main_path_for("", "quarto"), "main.qmd");
    assert_eq!(
        title_from_quarto("---\ntitle: 'Study: results'\n---\n# Introduction\n"),
        "Study: results"
    );
    assert_eq!(
        title_from_quarto("---\ntitle: [unfinished\n---\n# Draft\n"),
        "Draft"
    );
}

#[tokio::test]
async fn quarto_source_round_trips_without_execution_or_serialization() {
    let server = new_test_server().await;
    let source = "---\ntitle: 'Study'\nformat: html\n---\n\n```{r}\n#| label: fig-private\nstop('must never execute while publishing')\n```\n";
    let (status, document) = post(
        &server.url,
        "/api/documents",
        json!({
            "title": "Study", "source": source, "source_format": "quarto"
        }),
    )
    .await;
    assert_eq!(status, 201, "{document}");
    let slug = text(&document, "slug");
    let (status, payload) = get_json_keyed(
        &session_as(TEST_PUBLISHER),
        "",
        &server.url,
        &format!("/api/documents/{slug}/source"),
    )
    .await;
    assert_eq!(status, 200, "{payload}");
    assert_eq!(text(&payload, "format"), "quarto");
    assert_eq!(text(&payload, "source"), source);
    let response = client()
        .get(format!("{}/api/documents/{slug}/snapshot", server.url))
        .header("cookie", session_as(TEST_PUBLISHER))
        .header("x-librepaper-client", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let snapshot: serde_json::Value = response.json().await.unwrap();
    assert_eq!(text(&snapshot, "main"), "main.qmd");
}

#[tokio::test]
async fn quarto_empty_dependencies_publish_and_remain_readable() {
    let server = new_test_server().await;
    let slug = text(&publish_quarto(&server.url).await, "slug");
    let mut bundle = imported_bundle(&slug, "empty-dependency", "<p>Saved</p>");
    let digest = crate::quarto::sha256(b"");
    bundle["manifest"]["assets"] = json!([{"path":"paper_files/empty.css", "sha256":digest, "mime":"text/css", "size":0, "role":"display"}]);
    bundle["blobs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"sha256":digest,"mime":"text/css","data":""}));
    let (status, published) = post(
        &server.url,
        &format!("/api/documents/{slug}/quarto/bundles"),
        bundle,
    )
    .await;
    assert_eq!(status, 201, "{published}");
    let catalog = server.instance.store.catalog.as_ref().unwrap();
    let storage = catalog.document(&slug).unwrap().unwrap().storage_id;
    let body = server
        .instance
        .store
        .blobs
        .get(&crate::quarto::scoped_blob_key(&storage, &digest))
        .await
        .unwrap();
    assert!(body.is_empty());
}

#[tokio::test]
async fn quarto_objects_are_reclaimed_by_direct_and_resumed_deletion() {
    for resumed in [false, true] {
        let server = new_test_server().await;
        let slug = text(&publish_quarto(&server.url).await, "slug");
        let (status, published) = post(
            &server.url,
            &format!("/api/documents/{slug}/quarto/bundles"),
            imported_bundle(&slug, "delete-me", "<p>Saved</p>"),
        )
        .await;
        assert_eq!(status, 201, "{published}");
        let catalog = server.instance.store.catalog.as_ref().unwrap();
        let storage = catalog.document(&slug).unwrap().unwrap().storage_id;
        let prefixes = crate::storage::maintenance::document_object_prefixes(&slug, &storage);
        let quarto_prefixes: Vec<_> = prefixes
            .iter()
            .filter(|prefix| prefix.starts_with("quarto/"))
            .collect();
        for prefix in &quarto_prefixes {
            assert!(!server
                .instance
                .store
                .blobs
                .list(prefix)
                .await
                .unwrap()
                .is_empty());
        }
        let unrelated = "quarto/blobs/another-storage/keep";
        server
            .instance
            .store
            .blobs
            .put(unrelated, vec![1], "application/octet-stream")
            .await
            .unwrap();
        if resumed {
            catalog.begin_delete(&slug).unwrap();
            let worker = crate::storage::maintenance::DeletionWorker::new(
                catalog.clone(),
                server.instance.store.blobs.clone(),
                crate::storage::maintenance::DeletionLimits {
                    max_jobs: 1000,
                    max_object_requests: 1000,
                    max_read_bytes: 2_000_000,
                },
            )
            .unwrap();
            let retirements = crate::storage::maintenance::JournalRetirementWorker::new(
                catalog.clone(),
                server.instance.store.blobs.clone(),
                1000,
            )
            .unwrap();
            let started = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;
            for now in started..started + 20 {
                worker.run_once(now).await.unwrap();
                retirements.run_once(now).await.unwrap();
                if catalog.document(&slug).unwrap().is_none() {
                    break;
                }
            }
            assert!(catalog.document(&slug).unwrap().is_none());
        } else {
            server.instance.store.remove(&slug).await.unwrap();
        }
        for prefix in quarto_prefixes {
            assert!(
                server
                    .instance
                    .store
                    .blobs
                    .list(prefix)
                    .await
                    .unwrap()
                    .is_empty(),
                "leftover prefix {prefix}"
            );
        }
        assert_eq!(
            server.instance.store.blobs.get(unrelated).await.unwrap(),
            vec![1]
        );
    }
}

#[tokio::test]
async fn quarto_restore_does_not_promote_a_never_selected_bundle() {
    let server = new_test_server().await;
    let slug = text(&publish_quarto(&server.url).await, "slug");
    let room = server.instance.rooms.try_get(&slug).await.unwrap();
    let digest = room.tree().await.digest();
    let (_, checkpoint) = post(
        &server.url,
        &format!("/api/documents/{slug}/quarto/checkpoint"),
        json!({"tree_sha256":digest}),
    )
    .await;
    let mut bundle = imported_bundle(&slug, "unselected", "<p>Saved but not chosen</p>");
    bundle["manifest"]["source"]["tree_sha256"] = json!(digest);
    bundle["select"] = json!(false);
    let (status, published) = post(
        &server.url,
        &format!("/api/documents/{slug}/quarto/bundles"),
        bundle,
    )
    .await;
    assert_eq!(status, 201, "{published}");
    let (status, restored) = post(
        &server.url,
        &format!("/api/documents/{slug}/restore"),
        json!({"sha":text(&checkpoint,"revision")}),
    )
    .await;
    assert_eq!(status, 200, "{restored}");
    assert_eq!(restored["quarto_selection"]["status"], "cleared");
    let (status, _) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("/api/documents/{slug}/quarto/bundles/selected/html"),
    )
    .await;
    assert_eq!(status, 404);
}
