//! Explicit runtime tests for the same adapter the loopback service invokes.

use std::collections::BTreeMap;

use crate::local::{protocol::*, quarto::*};

fn fixture(source: &str) -> (tempfile::TempDir, BindingStore, JobRequest, Workspace) {
    // Quarto excludes dot-prefixed directories from project discovery. Use
    // an ordinary project path so freezer tests exercise the project engine.
    let directory = tempfile::Builder::new()
        .prefix("librepaper-quarto-")
        .tempdir()
        .unwrap();
    let project = directory.path().join("project");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(project.join("paper.qmd"), source).unwrap();
    let bindings = BindingStore::new(&directory.path().join("config"));
    let binding = bindings
        .grant("https://paper.example", "document", &project, "paper.qmd")
        .unwrap();
    let request = JobRequest {
        protocol: 1,
        kind: "quarto".into(),
        project: "document".into(),
        origin: "https://paper.example".into(),
        snapshot: "saved-revision".into(),
        generation: 1,
        engine: String::new(),
        main: "paper.qmd".into(),
        stem: String::new(),
        quarto: Some(QuartoJobOptions {
            binding_id: binding.id,
            main: "paper.qmd".into(),
            ..Default::default()
        }),
        manifest: vec![ManifestEntry {
            path: "paper.qmd".into(),
            sha256: crate::quarto::sha256(source.as_bytes()),
            size: source.len() as u64,
        }],
        options: JobOptions {
            deadline_seconds: 60,
            ..Default::default()
        },
        builder: None,
        workspace: None,
        entrypoint: None,
        output: None,
        builder_options: None,
        preset: None,
    };
    let workspace = Workspace {
        root: directory.path().join("job"),
    };
    std::fs::create_dir(&workspace.root).unwrap();
    (directory, bindings, request, workspace)
}

fn make_frozen_cache(directory: &tempfile::TempDir) {
    let project = directory.path().join("project");
    std::fs::write(
        project.join("_quarto.yml"),
        "project:\n  type: default\nexecute:\n  freeze: true\n",
    )
    .unwrap();
    let first = std::process::Command::new("quarto")
        .args(["render", "paper.qmd", "--no-execute-daemon"])
        .current_dir(&project)
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let marker = project.join("marker.txt");
    if marker.is_file() {
        std::fs::remove_file(marker).unwrap();
    }
}

fn remove_pngs(root: &std::path::Path) {
    for entry in std::fs::read_dir(root).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            remove_pngs(&path);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("png") {
            std::fs::remove_file(path).unwrap();
        }
    }
}

fn add_manifest_file(directory: &tempfile::TempDir, request: &mut JobRequest, path: &str) {
    let bytes = std::fs::read(directory.path().join("project").join(path)).unwrap();
    request.manifest.push(ManifestEntry {
        path: path.into(),
        sha256: crate::quarto::sha256(&bytes),
        size: bytes.len() as u64,
    });
}

#[tokio::test]
#[ignore = "requires installed Quarto, R, knitr and rmarkdown"]
async fn quarto_managed_job_uses_local_data_and_returns_publishable_bundle() {
    use base64::Engine;
    let source =
        include_str!("fixtures/quarto/r.qmd").replace("x <- 1:8", "x <- read.csv('local.csv')$x");
    let (directory, bindings, request, workspace) = fixture(&source);
    std::fs::write(
        directory.path().join("project/local.csv"),
        "x\n1\n2\n3\n4\n5\n6\n7\n8\n",
    )
    .unwrap();
    let (_sender, cancel) = tokio::sync::watch::channel(false);
    let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let outcome = run_job_with_bindings(request, workspace, cancel, progress, &bindings).await;
    assert_eq!(outcome.status.status, "done", "{:?}", outcome.status);
    let manifest: crate::quarto::BundleManifest =
        serde_json::from_slice(outcome.files.get("quarto-bundle.json").expect("manifest")).unwrap();
    manifest.validate().unwrap();
    assert!(!manifest
        .assets
        .iter()
        .any(|asset| asset.path.ends_with("local.csv")));
    let first = manifest
        .cells
        .iter()
        .find(|cell| cell.label == "fig-first")
        .unwrap();
    let second = manifest
        .cells
        .iter()
        .find(|cell| cell.label == "fig-second")
        .unwrap();
    assert_eq!(first.coverage, crate::quarto::CellCoverage::Captured);
    assert_ne!(
        first.outputs[0].content_sha256,
        second.outputs[0].content_sha256
    );
    assert!(first.outputs[0].caption.is_some());
    let mut uploads = BTreeMap::new();
    let artifact = manifest.artifact.as_ref().unwrap();
    uploads.insert(
        artifact.sha256.clone(),
        crate::quarto::BlobUpload {
            sha256: artifact.sha256.clone(),
            mime: artifact.mime.clone(),
            data: base64::engine::general_purpose::STANDARD
                .encode(outcome.files.get("artifact.html").expect("artifact")),
        },
    );
    for asset in &manifest.assets {
        let bytes = outcome
            .files
            .get(&format!("asset:{}", asset.path))
            .expect("every declared dependency returned");
        uploads.insert(
            asset.sha256.clone(),
            crate::quarto::BlobUpload {
                sha256: asset.sha256.clone(),
                mime: asset.mime.clone(),
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
            },
        );
    }
    crate::quarto::decode_uploads(&manifest, &uploads.into_values().collect::<Vec<_>>()).unwrap();
}

#[tokio::test]
#[ignore = "requires installed Quarto, R, knitr and rmarkdown"]
async fn quarto_managed_job_captures_inline_value_with_context_identity() {
    let source = r#"---
format: html
---

The answer is `r 1 + 1`.

```{r}
#| label: inline-cell
1 + 1
```
"#;
    let (directory, bindings, request, workspace) = fixture(source);
    let (_sender, cancel) = tokio::sync::watch::channel(false);
    let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let outcome = run_job_with_bindings(request, workspace, cancel, progress, &bindings).await;
    assert_eq!(outcome.status.status, "done", "{:?}", outcome.status);
    let manifest: crate::quarto::BundleManifest =
        serde_json::from_slice(outcome.files.get("quarto-bundle.json").unwrap()).unwrap();
    let expected = crate::quarto::parse_qmd(source, "paper.qmd")
        .inline_records
        .into_iter()
        .next()
        .expect("inline source record");
    let captured = manifest
        .inline_results
        .iter()
        .find(|result| result.id == expected.id)
        .expect("captured inline result");
    assert_eq!(captured.expression, expected.expression);
    assert_eq!(captured.source_path, "paper.qmd");
    assert_eq!(captured.line, expected.line);
    assert_eq!(captured.column, expected.column);
    assert_eq!(captured.value.trim(), "2");
    assert_eq!(
        captured.context_sha256.as_deref(),
        Some(manifest.context.computation_sha256.as_str())
    );
    // The browser parser uses the same source occurrence identity, so the
    // durable result can only attach to this exact inline occurrence.
    assert_eq!(captured.id, "paper.qmd#inline-5-0");
    assert!(directory.path().join("project/paper.qmd").is_file());
}

#[tokio::test]
#[ignore = "requires installed Quarto, R, knitr and rmarkdown"]
async fn quarto_typed_parameters_reach_r_without_string_coercion() {
    let source = r#"---
format: html
params:
  number: 0
  string: ''
  boolean: false
  nothing: null
---
```{r}
#| label: typed-parameters
stopifnot(is.numeric(params$number), params$number == 7)
stopifnot(is.character(params$string), params$string == 'quoted "value"')
stopifnot(isTRUE(params$boolean), is.null(params$nothing))
cat('typed-parameters-ok')
```
"#;
    let (directory, bindings, mut request, workspace) = fixture(source);
    let options = request.quarto.as_mut().unwrap();
    options
        .parameters
        .insert("number".into(), serde_json::json!(7));
    options
        .parameters
        .insert("string".into(), serde_json::json!(r#"quoted "value""#));
    options
        .parameters
        .insert("boolean".into(), serde_json::json!(true));
    options
        .parameters
        .insert("nothing".into(), serde_json::Value::Null);
    let (_sender, cancel) = tokio::sync::watch::channel(false);
    let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let outcome = run_job_with_bindings(request, workspace, cancel, progress, &bindings).await;
    assert_eq!(outcome.status.status, "done", "{:?}", outcome.status);
    let bundle: crate::quarto::BundleManifest =
        serde_json::from_slice(outcome.files.get("quarto-bundle.json").unwrap()).unwrap();
    assert!(bundle.artifact.is_some(), "HTML artifact");
    let bytes = outcome.files.get("artifact.html").expect("artifact bytes");
    let html = String::from_utf8_lossy(bytes);
    assert!(html.contains("typed-parameters-ok"), "{html}");
    assert!(!directory.path().join("project/marker.txt").exists());
}

/// The hosted binding, end to end with real Quarto: no grant, the uploads
/// staged by the service become the workspace, the render succeeds, and a
/// second job with new source updates the same workspace in place.
#[tokio::test]
#[ignore = "requires installed Quarto"]
async fn quarto_hosted_workspace_renders_from_uploads_without_a_grant() {
    let directory = tempfile::Builder::new()
        .prefix("librepaper-quarto-hosted-")
        .tempdir()
        .unwrap();
    let workspaces = directory.path().join("workspaces");
    let bindings = BindingStore::new(&directory.path().join("config"))
        .with_hosted_workspaces(workspaces.clone());
    let run = |source: &str, job: &str| {
        let source = source.to_string();
        let workspace = Workspace {
            root: directory.path().join(job),
        };
        let bindings = bindings.clone();
        async move {
            // What the service does at admission: the uploads, staged under
            // the job's own `project/`.
            std::fs::create_dir_all(workspace.project()).unwrap();
            std::fs::write(workspace.project().join("paper.qmd"), &source).unwrap();
            let request = JobRequest {
                protocol: 1,
                kind: "quarto".into(),
                project: "document".into(),
                origin: "https://paper.example".into(),
                snapshot: "saved-revision".into(),
                generation: 1,
                engine: String::new(),
                main: "paper.qmd".into(),
                stem: String::new(),
                quarto: Some(QuartoJobOptions {
                    binding_id: HOSTED_BINDING.into(),
                    main: "paper.qmd".into(),
                    ..Default::default()
                }),
                manifest: vec![ManifestEntry {
                    path: "paper.qmd".into(),
                    sha256: crate::quarto::sha256(source.as_bytes()),
                    size: source.len() as u64,
                }],
                options: JobOptions {
                    deadline_seconds: 60,
                    ..Default::default()
                },
                builder: None,
                workspace: None,
                entrypoint: None,
                output: None,
                builder_options: None,
                preset: None,
            };
            let (_sender, cancel) = tokio::sync::watch::channel(false);
            let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
            run_job_with_bindings(request, workspace, cancel, progress, &bindings).await
        }
    };

    let first = run("---\ntitle: Hosted\n---\n\nHello *hosted*.\n", "job-1").await;
    assert_eq!(first.status.status, "done", "{:?}", first.status);
    let html = String::from_utf8_lossy(first.files.get("artifact.html").expect("artifact"));
    assert!(html.contains("hosted"));
    let workspace = bindings
        .get_scoped(HOSTED_BINDING, "https://paper.example", "document")
        .expect("hosted binding")
        .root;
    assert!(workspace.starts_with(std::fs::canonicalize(&workspaces).unwrap()));
    assert!(
        workspace.join("paper.qmd").is_file(),
        "the upload became the workspace"
    );

    let second = run("---\ntitle: Hosted\n---\n\nSecond *version*.\n", "job-2").await;
    assert_eq!(second.status.status, "done", "{:?}", second.status);
    let html = String::from_utf8_lossy(second.files.get("artifact.html").expect("artifact"));
    assert!(html.contains("Second"));
    assert_eq!(
        std::fs::read_to_string(workspace.join("paper.qmd")).unwrap(),
        "---\ntitle: Hosted\n---\n\nSecond *version*.\n"
    );
    // A granted binding was never involved.
    assert!(bindings
        .list_scoped("https://paper.example", "document")
        .is_empty());
}

#[tokio::test]
#[ignore = "requires installed Quarto"]
async fn quarto_changed_shared_input_is_refused_before_execution() {
    let (directory, bindings, mut request, workspace) =
        fixture("```{r}\nwriteLines('executed', 'marker.txt')\n```\n");
    request.manifest[0].sha256 = crate::quarto::sha256(b"another version");
    let (_sender, cancel) = tokio::sync::watch::channel(false);
    let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let outcome = run_job_with_bindings(request, workspace, cancel, progress, &bindings).await;
    assert_eq!(outcome.status.status, "failed");
    assert!(outcome
        .status
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("synchronize"));
    assert!(!directory.path().join("project/marker.txt").exists());
}

#[tokio::test]
#[ignore = "requires installed Quarto and R"]
async fn quarto_managed_job_cancels_without_publishing_output() {
    let (_directory, bindings, request, workspace) = fixture("```{r}\nSys.sleep(30)\n```\n");
    let (sender, cancel) = tokio::sync::watch::channel(false);
    let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let execution = run_job_with_bindings(request, workspace, cancel, progress, &bindings);
    let cancellation = async {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        sender.send(true).unwrap();
    };
    let (outcome, ()) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(execution, cancellation)
    })
    .await
    .expect("cancellation finishes promptly");
    assert_eq!(outcome.status.status, "canceled");
    assert!(outcome.files.is_empty());
}

#[tokio::test]
#[ignore = "requires installed Quarto, R, knitr and rmarkdown"]
async fn quarto_engine_cache_survives_jobs_and_refresh_recomputes() {
    let source = "---\nformat: html\nexecute:\n  cache: true\n---\n```{r}\n#| label: fig-counter\nn <- if (file.exists('counter.txt')) as.integer(readLines('counter.txt')) + 1 else 1\nwriteLines(as.character(n), 'counter.txt')\nplot(seq_len(n))\n```\n";
    let (directory, bindings, request, _) = fixture(source);
    for (index, expected) in [1, 1, 2].into_iter().enumerate() {
        let mut next = request.clone();
        if index == 2 {
            next.quarto.as_mut().unwrap().policy = QuartoRenderPolicy::RefreshComputations;
        }
        let workspace = Workspace {
            root: directory.path().join(format!("render-{index}")),
        };
        std::fs::create_dir(&workspace.root).unwrap();
        let (_sender, cancel) = tokio::sync::watch::channel(false);
        let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let outcome = run_job_with_bindings(next, workspace, cancel, progress, &bindings).await;
        assert_eq!(outcome.status.status, "done", "{:?}", outcome.status);
        assert_eq!(
            std::fs::read_to_string(directory.path().join("project/counter.txt"))
                .unwrap()
                .trim(),
            expected.to_string()
        );
        let manifest: crate::quarto::BundleManifest =
            serde_json::from_slice(outcome.files.get("quarto-bundle.json").unwrap()).unwrap();
        assert_eq!(
            manifest.provenance.computation,
            crate::quarto::ComputationEvidence::CacheUseUnknown
        );
    }
}

#[tokio::test]
#[ignore = "requires installed Quarto, R, knitr and rmarkdown"]
async fn quarto_frozen_rebuild_reuses_a_plot_without_running_code() {
    let source = "---\nformat: html\n---\n```{r}\n#| label: fig-frozen\nwriteLines('executed', 'marker.txt')\nplot(1:5)\n```\n";
    let (directory, bindings, mut request, workspace) = fixture(source);
    let project = directory.path().join("project");
    std::fs::write(
        project.join("_quarto.yml"),
        "project:\n  type: default\nexecute:\n  freeze: true\n",
    )
    .unwrap();
    let first = std::process::Command::new("quarto")
        .args(["render", "paper.qmd", "--no-execute-daemon"])
        .current_dir(&project)
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        project.join("marker.txt").is_file(),
        "{}\n{}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    std::fs::remove_file(project.join("marker.txt")).unwrap();
    request.quarto.as_mut().unwrap().policy = QuartoRenderPolicy::Frozen;
    let (_sender, cancel) = tokio::sync::watch::channel(false);
    let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let result = run_job_with_bindings(request, workspace, cancel, progress, &bindings).await;
    assert_eq!(result.status.status, "done", "{:?}", result.status);
    assert!(
        !project.join("marker.txt").exists(),
        "a frozen rebuild executed code"
    );
    let manifest: crate::quarto::BundleManifest =
        serde_json::from_slice(&result.files["quarto-bundle.json"]).unwrap();
    assert!(
        manifest
            .assets
            .iter()
            .any(|asset| asset.path.ends_with(".png")),
        "artifact={:?} assets={:?}",
        manifest.artifact,
        manifest.assets
    );
    assert_eq!(manifest.provenance.policy, "frozen");
    assert_ne!(
        manifest.provenance.computation,
        crate::quarto::ComputationEvidence::Refreshed
    );
}

#[tokio::test]
#[ignore = "requires installed Quarto, R, knitr and rmarkdown"]
async fn quarto_frozen_rebuild_verifies_profiles_and_typed_parameters() {
    let source = "---\nformat: html\nparams:\n  answer: 0\n---\n```{r}\n#| label: fig-profile\nstopifnot(identical(as.numeric(params$answer), 7))\nwriteLines('executed', 'marker.txt')\nplot(1:5)\n```\n";
    let (directory, bindings, mut request, _workspace) = fixture(source);
    let project = directory.path().join("project");
    std::fs::write(
        project.join("_quarto.yml"),
        "project:\n  type: default\nexecute:\n  freeze: true\n",
    )
    .unwrap();
    std::fs::write(project.join("_quarto-review.yml"), "format: html\n").unwrap();
    add_manifest_file(&directory, &mut request, "_quarto-review.yml");
    let options = request.quarto.as_mut().unwrap();
    options.profile = Some("review".into());
    options
        .parameters
        .insert("answer".into(), serde_json::json!(7));
    let first_workspace = Workspace {
        root: directory.path().join("render-profile-first"),
    };
    std::fs::create_dir(&first_workspace.root).unwrap();
    let (_sender, cancel) = tokio::sync::watch::channel(false);
    let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let first = run_job_with_bindings(
        request.clone(),
        first_workspace,
        cancel,
        progress,
        &bindings,
    )
    .await;
    assert_eq!(first.status.status, "done", "{:?}", first.status);
    assert!(project.join("marker.txt").is_file());
    let cache = project.join("_freeze/paper/execute-results/html.json");
    let cache_json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(cache).unwrap()).unwrap();
    assert!(cache_json.get("librepaper_context").is_some());
    std::fs::remove_file(project.join("marker.txt")).unwrap();
    let mut frozen = request;
    frozen.quarto.as_mut().unwrap().policy = QuartoRenderPolicy::Frozen;
    let frozen_workspace = Workspace {
        root: directory.path().join("render-profile-frozen"),
    };
    std::fs::create_dir(&frozen_workspace.root).unwrap();
    let (_sender, cancel) = tokio::sync::watch::channel(false);
    let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let result = run_job_with_bindings(frozen, frozen_workspace, cancel, progress, &bindings).await;
    assert_eq!(result.status.status, "done", "{:?}", result.status);
    assert!(!project.join("marker.txt").exists());
    let manifest: crate::quarto::BundleManifest =
        serde_json::from_slice(&result.files["quarto-bundle.json"]).unwrap();
    assert!(manifest
        .assets
        .iter()
        .any(|asset| asset.path.ends_with(".png")));
    assert!(String::from_utf8_lossy(&result.files["artifact.html"]).contains("fig-profile"));
}

#[tokio::test]
#[ignore = "requires installed Quarto, R, knitr and rmarkdown"]
async fn quarto_frozen_rebuild_verifies_nested_included_sources() {
    let source = "---\nformat: html\n---\n{{< include child.qmd >}}\n```{r}\n#| label: fig-included\nwriteLines('executed', 'marker.txt')\nplot(1:5)\n```\n";
    let (directory, bindings, mut request, _workspace) = fixture(source);
    let project = directory.path().join("project");
    std::fs::write(
        project.join("child.qmd"),
        "Child page.\n{{< include nested.md >}}\n",
    )
    .unwrap();
    std::fs::write(project.join("nested.md"), "Nested included text.\n").unwrap();
    add_manifest_file(&directory, &mut request, "child.qmd");
    add_manifest_file(&directory, &mut request, "nested.md");
    std::fs::write(
        project.join("_quarto.yml"),
        "project:\n  type: default\nexecute:\n  freeze: true\n",
    )
    .unwrap();
    let first_workspace = Workspace {
        root: directory.path().join("render-include-first"),
    };
    std::fs::create_dir(&first_workspace.root).unwrap();
    let (_sender, cancel) = tokio::sync::watch::channel(false);
    let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let first = run_job_with_bindings(
        request.clone(),
        first_workspace,
        cancel,
        progress,
        &bindings,
    )
    .await;
    assert_eq!(first.status.status, "done", "{:?}", first.status);
    std::fs::remove_file(project.join("marker.txt")).unwrap();
    let mut frozen = request;
    frozen.quarto.as_mut().unwrap().policy = QuartoRenderPolicy::Frozen;
    let frozen_workspace = Workspace {
        root: directory.path().join("render-include-frozen"),
    };
    std::fs::create_dir(&frozen_workspace.root).unwrap();
    let (_sender, cancel) = tokio::sync::watch::channel(false);
    let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let result = run_job_with_bindings(frozen, frozen_workspace, cancel, progress, &bindings).await;
    assert_eq!(result.status.status, "done", "{:?}", result.status);
    assert!(!project.join("marker.txt").exists());
    let manifest: crate::quarto::BundleManifest =
        serde_json::from_slice(&result.files["quarto-bundle.json"]).unwrap();
    assert!(manifest
        .assets
        .iter()
        .any(|asset| asset.path.ends_with(".png")));
}

#[tokio::test]
#[ignore = "requires installed Quarto"]
async fn quarto_project_scope_collects_pages_and_nested_web_resources() {
    let source = "---\ntitle: Home\nformat: html\n---\n<link rel=\"stylesheet\" href=\"styles.css\">\n<img src=\"assets/logo.svg\" alt=\"logo\">\nHome page.\n";
    let (directory, bindings, mut request, workspace) = fixture(source);
    let project = directory.path().join("project");
    std::fs::write(
        project.join("_quarto.yml"),
        "project:\n  type: website\nwebsite:\n  title: Test site\n  navbar:\n    left:\n      - href: paper.qmd\n        text: Home\n      - href: about.qmd\n        text: About\nformat: html\n",
    )
    .unwrap();
    std::fs::write(
        project.join("about.qmd"),
        "---\ntitle: About\n---\nAbout page.\n",
    )
    .unwrap();
    std::fs::create_dir_all(project.join("assets")).unwrap();
    std::fs::create_dir_all(project.join("theme")).unwrap();
    std::fs::write(
        project.join("styles.css"),
        "@import url('theme/extra.css'); body { color: #222; }\n",
    )
    .unwrap();
    std::fs::write(project.join("theme/extra.css"), "body { margin: 0; }\n").unwrap();
    std::fs::write(
        project.join("assets/logo.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"8\" height=\"8\"><circle cx=\"4\" cy=\"4\" r=\"3\"/></svg>\n",
    )
    .unwrap();
    for path in [
        "_quarto.yml",
        "about.qmd",
        "styles.css",
        "theme/extra.css",
        "assets/logo.svg",
    ] {
        add_manifest_file(&directory, &mut request, path);
    }
    request.quarto.as_mut().unwrap().render_scope = QuartoRenderScope::Project;
    let (_sender, cancel) = tokio::sync::watch::channel(false);
    let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let result = run_job_with_bindings(request, workspace, cancel, progress, &bindings).await;
    assert_eq!(result.status.status, "done", "{:?}", result.status);
    let manifest: crate::quarto::BundleManifest =
        serde_json::from_slice(&result.files["quarto-bundle.json"]).unwrap();
    let artifact = manifest.artifact.as_ref().expect("project artifact");
    assert!(artifact.entrypoint == "paper.html" || artifact.entrypoint == "index.html");
    assert!(
        manifest
            .assets
            .iter()
            .any(|asset| asset.path == "about.html"),
        "project assets: {:?}",
        manifest
            .assets
            .iter()
            .map(|asset| &asset.path)
            .collect::<Vec<_>>()
    );
    assert!(manifest
        .assets
        .iter()
        .any(|asset| asset.path.ends_with("extra.css")));
    assert!(manifest
        .assets
        .iter()
        .any(|asset| asset.path.ends_with("logo.svg")));
    assert_eq!(
        manifest.coverage.cell_outputs,
        crate::quarto::CoverageLevel::None
    );
}

#[tokio::test]
#[ignore = "requires installed Quarto, R, knitr and rmarkdown"]
async fn quarto_frozen_refuses_missing_or_corrupt_cache_before_execution() {
    let source =
        "---\nformat: html\n---\n```{r}\nwriteLines('executed', 'marker.txt')\nplot(1:5)\n```\n";
    let (directory, bindings, mut request, workspace) = fixture(source);
    request.quarto.as_mut().unwrap().policy = QuartoRenderPolicy::Frozen;
    let (_sender, cancel) = tokio::sync::watch::channel(false);
    let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let missing = run_job_with_bindings(
        request.clone(),
        workspace.clone(),
        cancel,
        progress,
        &bindings,
    )
    .await;
    assert_eq!(missing.status.status, "failed");
    assert!(!directory.path().join("project/marker.txt").exists());

    let (partial_dir, partial_bindings, mut partial_request, partial_workspace) = fixture(source);
    make_frozen_cache(&partial_dir);
    remove_pngs(&partial_dir.path().join("project/_freeze"));
    remove_pngs(&partial_dir.path().join("project/paper_files"));
    partial_request.quarto.as_mut().unwrap().policy = QuartoRenderPolicy::Frozen;
    let (_sender, cancel) = tokio::sync::watch::channel(false);
    let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let partial = run_job_with_bindings(
        partial_request,
        partial_workspace,
        cancel,
        progress,
        &partial_bindings,
    )
    .await;
    assert_eq!(partial.status.status, "failed");
    assert!(!partial_dir.path().join("project/marker.txt").exists());

    make_frozen_cache(&directory);
    let cache = directory
        .path()
        .join("project/_freeze/paper/execute-results/html.json");
    std::fs::write(&cache, b"not-json").unwrap();
    let (_sender, cancel) = tokio::sync::watch::channel(false);
    let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let corrupt = run_job_with_bindings(request, workspace, cancel, progress, &bindings).await;
    assert_eq!(corrupt.status.status, "failed");
    assert!(!directory.path().join("project/marker.txt").exists());

    let included_source =
        "{{< include child.qmd >}}\n```{r}\nwriteLines('executed', 'marker.txt')\n```\n";
    let (included_dir, included_bindings, mut included_request, included_workspace) =
        fixture(included_source);
    included_request.quarto.as_mut().unwrap().policy = QuartoRenderPolicy::Frozen;
    let (_sender, cancel) = tokio::sync::watch::channel(false);
    let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let included = run_job_with_bindings(
        included_request,
        included_workspace,
        cancel,
        progress,
        &included_bindings,
    )
    .await;
    assert_eq!(included.status.status, "failed");
    assert!(!included_dir.path().join("project/marker.txt").exists());
}

#[tokio::test]
#[ignore = "requires installed Quarto, R, knitr and rmarkdown"]
async fn quarto_frozen_refuses_changed_source_before_execution() {
    let source =
        "---\nformat: html\n---\n```{r}\nwriteLines('executed', 'marker.txt')\nplot(1:5)\n```\n";
    let (directory, bindings, mut request, workspace) = fixture(source);
    make_frozen_cache(&directory);
    let changed = format!("{source}\nChanged prose.\n");
    std::fs::write(directory.path().join("project/paper.qmd"), &changed).unwrap();
    request.manifest[0].sha256 = crate::quarto::sha256(changed.as_bytes());
    request.manifest[0].size = changed.len() as u64;
    request.quarto.as_mut().unwrap().policy = QuartoRenderPolicy::Frozen;
    let (_sender, cancel) = tokio::sync::watch::channel(false);
    let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let outcome = run_job_with_bindings(request, workspace, cancel, progress, &bindings).await;
    assert_eq!(outcome.status.status, "failed");
    assert!(outcome
        .status
        .error
        .as_deref()
        .is_some_and(|error| error.contains("does not match the current source")));
    assert!(!directory.path().join("project/marker.txt").exists());
}

#[tokio::test]
#[ignore = "requires installed Quarto, R, knitr and rmarkdown"]
async fn quarto_isolated_snapshot_does_not_write_the_bound_project() {
    let source = "# snapshot\n\n```{r}\n#| label: snapshot-data\nstopifnot(read.csv('local.csv')$x == 1)\nwriteLines('snapshot only', 'marker.txt')\nprint('snapshot data read')\n```\n";
    let (directory, bindings, mut request, workspace) = fixture(source);
    std::fs::write(directory.path().join("project/local.csv"), "x\n1\n").unwrap();
    let options = request.quarto.as_mut().unwrap();
    options.execution_mode = QuartoExecutionMode::IsolatedSnapshot;
    options.shared_inventory_complete = true;
    options.shared_tree_sha256 = Some(crate::quarto::sha256(b"snapshot-tree"));
    options.data_inputs = vec!["local.csv".into()];
    let (_sender, cancel) = tokio::sync::watch::channel(false);
    let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let result = run_job_with_bindings(request, workspace, cancel, progress, &bindings).await;
    assert_eq!(result.status.status, "done", "{:?}", result.status);
    assert!(!directory.path().join("project/quarto-bundle.json").exists());
    assert!(!directory.path().join("project/marker.txt").exists());
    assert!(!directory.path().join("project/.quarto").exists());
    let manifest: crate::quarto::BundleManifest =
        serde_json::from_slice(&result.files["quarto-bundle.json"]).unwrap();
    assert_eq!(
        manifest.source.verification,
        crate::results::Verification::IsolatedSnapshot
    );
    let expected_tree = crate::quarto::sha256(b"snapshot-tree");
    assert_eq!(
        manifest.source.tree_sha256.as_deref(),
        Some(expected_tree.as_str())
    );
    let parameters_sha256 = crate::results::parameters_sha256(&BTreeMap::new());
    let expected_context = crate::local::engine_adapter::quarto_computation_fingerprint(
        source,
        "paper.qmd",
        "html",
        &[],
        Some(&parameters_sha256),
        &[],
    );
    assert_eq!(manifest.context.computation_sha256, expected_context);
}

#[tokio::test]
#[ignore = "requires installed Quarto"]
async fn quarto_book_scope_keeps_chapter_navigation() {
    let source = "# Preface\n\nBook introduction.\n";
    let (directory, bindings, mut request, workspace) = fixture(source);
    let project = directory.path().join("project");
    std::fs::rename(project.join("paper.qmd"), project.join("index.qmd")).unwrap();
    std::fs::write(
        project.join("chapter.qmd"),
        "# A chapter\n\nChapter content.\n",
    )
    .unwrap();
    std::fs::write(project.join("_quarto.yml"), "project:\n  type: book\nbook:\n  title: Example book\n  chapters:\n    - index.qmd\n    - chapter.qmd\nformat: html\n").unwrap();
    let binding = bindings
        .grant(&request.origin, &request.project, &project, "index.qmd")
        .unwrap();
    request.manifest[0].path = "index.qmd".into();
    request.main = "index.qmd".into();
    let options = request.quarto.as_mut().unwrap();
    options.binding_id = binding.id;
    options.main = "index.qmd".into();
    options.render_scope = QuartoRenderScope::Project;
    for path in ["chapter.qmd", "_quarto.yml"] {
        add_manifest_file(&directory, &mut request, path);
    }
    let (_sender, cancel) = tokio::sync::watch::channel(false);
    let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let result = run_job_with_bindings(request, workspace, cancel, progress, &bindings).await;
    assert_eq!(result.status.status, "done", "{:?}", result.status);
    let manifest: crate::quarto::BundleManifest =
        serde_json::from_slice(&result.files["quarto-bundle.json"]).unwrap();
    assert_eq!(manifest.artifact.as_ref().unwrap().entrypoint, "index.html");
    assert!(manifest
        .assets
        .iter()
        .any(|asset| asset.path == "chapter.html"));
    assert!(String::from_utf8_lossy(&result.files["artifact.html"]).contains("chapter.html"));
}

#[tokio::test]
#[ignore = "requires installed Quarto"]
async fn quarto_nested_document_entrypoint_collects_flattened_or_nested_output() {
    let source = "---\ntitle: Intro\nformat: html\n---\n# Introduction\n\nA nested entrypoint.\n";
    let (directory, bindings, mut request, workspace) = fixture(source);
    let project = directory.path().join("project");
    std::fs::create_dir_all(project.join("chapters")).unwrap();
    std::fs::rename(
        project.join("paper.qmd"),
        project.join("chapters/intro.qmd"),
    )
    .unwrap();
    let binding = bindings
        .grant(
            &request.origin,
            &request.project,
            &project,
            "chapters/intro.qmd",
        )
        .unwrap();
    request.manifest[0].path = "chapters/intro.qmd".into();
    request.quarto.as_mut().unwrap().binding_id = binding.id;
    request.quarto.as_mut().unwrap().main = "chapters/intro.qmd".into();
    request.main = "chapters/intro.qmd".into();
    let (_sender, cancel) = tokio::sync::watch::channel(false);
    let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let result = run_job_with_bindings(request, workspace, cancel, progress, &bindings).await;
    assert_eq!(result.status.status, "done", "{:?}", result.status);
    let manifest: crate::quarto::BundleManifest =
        serde_json::from_slice(&result.files["quarto-bundle.json"]).unwrap();
    let artifact = manifest.artifact.expect("nested HTML artifact");
    assert!(
        artifact.entrypoint == "chapters/intro.html" || artifact.entrypoint == "intro.html",
        "unexpected nested artifact: {}",
        artifact.entrypoint
    );
}
