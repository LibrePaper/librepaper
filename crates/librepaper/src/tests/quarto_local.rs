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

#[tokio::test]
#[ignore = "requires installed Quarto, R, knitr and rmarkdown"]
async fn quarto_managed_job_uses_local_data_and_returns_publishable_bundle() {
    use base64::Engine;
    let source = include_str!("../../../../examples/quarto-r.qmd")
        .replace("x <- 1:8", "x <- read.csv('local.csv')$x");
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
