use super::*;
use tempfile::tempdir;

#[test]
fn collector_keeps_unlabelled_cells_distinct() {
    let dir = tempdir().expect("tempdir");
    let source = "```{r}\nplot(x)\n```\n\n```{r}\nplot(x)\n```\n";
    std::fs::write(dir.path().join("paper.qmd"), source).expect("source");
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).expect("out");
    std::fs::write(out.join("paper.html"), b"<html></html>").expect("artifact");
    let options = QuartoJobOptions {
        binding_id: "b".into(),
        main: "paper.qmd".into(),
        ..Default::default()
    };
    let inventory = inventory_tree(dir.path()).expect("inventory");
    let bundle = collect_bundle(dir.path(), &options, &out, &inventory, None, Instant::now())
        .expect("bundle");
    assert_eq!(bundle.cells.len(), 2);
    assert_ne!(bundle.cells[0].id, bundle.cells[1].id);
    assert_eq!(
        bundle.artifact.as_ref().map(|a| a.entrypoint.as_str()),
        Some("paper.html")
    );
}

#[test]
fn revealjs_bundle_preserves_format_and_freshness_context() {
    let dir = tempdir().expect("tempdir");
    let source = "```{r}\n1 + 1\n```\n";
    std::fs::write(dir.path().join("paper.qmd"), source).expect("source");
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).expect("out");
    std::fs::write(out.join("paper.html"), b"<html></html>").expect("artifact");
    let options = QuartoJobOptions {
        binding_id: "b".into(),
        main: "paper.qmd".into(),
        format: "revealjs".into(),
        profile: Some("review".into()),
        parameters: BTreeMap::from([("seed".into(), serde_json::json!(42))]),
        shared_tree_sha256: Some(sha256(source.as_bytes())),
        ..Default::default()
    };
    let inventory = inventory_tree(dir.path()).expect("inventory");
    let bundle = collect_bundle(dir.path(), &options, &out, &inventory, None, Instant::now())
        .expect("bundle");
    let manifest = bundle.to_storage_manifest("doc", "revision");
    manifest.validate().expect("valid bundle");
    assert_eq!(
        manifest.context.format,
        crate::results::OutputFormat::Revealjs
    );
    assert_eq!(
        manifest.artifact.as_ref().expect("artifact").kind,
        crate::results::ArtifactKind::Html
    );
    assert_eq!(
        serde_json::to_value(&manifest).expect("JSON")["context"]["format"],
        "revealjs"
    );
    let parsed = crate::quarto::parse_qmd(source, "paper.qmd");
    assert_eq!(
        crate::quarto::classify_freshness(
            &manifest,
            &parsed,
            "paper.qmd",
            &["review".into()],
            manifest.context.parameters_sha256.as_deref()
        ),
        crate::quarto::Freshness::MatchesRecordedInputs
    );
}

#[test]
fn shared_dependency_hashes_change_computation_context() {
    let dir = tempdir().expect("tempdir");
    let source = "```{r}\nplot(x)\n```\n";
    std::fs::write(dir.path().join("paper.qmd"), source).expect("source");
    std::fs::write(dir.path().join("data.csv"), b"x\n1\n").expect("data");
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).expect("out");
    std::fs::write(out.join("paper.html"), b"<html></html>").expect("artifact");
    let options = QuartoJobOptions {
        binding_id: "b".into(),
        main: "paper.qmd".into(),
        ..Default::default()
    };
    let inventory = inventory_tree(dir.path()).expect("inventory");
    let first = collect_bundle_with_dependencies(
        dir.path(),
        &options,
        &out,
        &inventory,
        None,
        Instant::now(),
        &[format!("data.csv\0{}", sha256(b"x\n1\n"))],
    )
    .expect("bundle");
    let second = collect_bundle_with_dependencies(
        dir.path(),
        &options,
        &out,
        &inventory,
        None,
        Instant::now(),
        &[format!("data.csv\0{}", sha256(b"x\n2\n"))],
    )
    .expect("bundle");
    assert_ne!(
        first.context.computation_sha256,
        second.context.computation_sha256
    );
}

#[test]
fn binding_store_scopes_ids_to_origin_and_project() {
    let config = tempdir().expect("tempdir");
    let root = tempdir().expect("root");
    std::fs::write(root.path().join("paper.qmd"), "# hi").expect("source");
    let store = BindingStore::new(config.path());
    let binding = store
        .grant("https://Example.test/", "p", root.path(), "paper.qmd")
        .expect("grant");
    assert!(store
        .get_scoped(&binding.id, "https://example.test", "p")
        .is_some());
    assert!(store
        .get_scoped(&binding.id, "https://other.test", "p")
        .is_none());
}

#[test]
fn typed_options_reject_shell_and_path_injection() {
    let mut options = QuartoJobOptions {
        binding_id: "binding".into(),
        main: "../run.qmd".into(),
        ..Default::default()
    };
    assert!(options.validate().is_err());
    options.main = "paper.qmd".into();
    options.profile = Some("default; touch /tmp/pwned".into());
    assert!(options.validate().is_err());
    options.profile = None;
    options.format = "html; echo bad".into();
    assert!(options.validate().is_err());
}

#[test]
fn typed_quarto_parameters_preserve_scalar_types() {
    let mut options = QuartoJobOptions {
        binding_id: "binding".into(),
        ..Default::default()
    };
    options
        .parameters
        .insert("number".into(), serde_json::json!(1));
    options
        .parameters
        .insert("text".into(), serde_json::json!("1"));
    options
        .parameters
        .insert("boolean".into(), serde_json::json!(true));
    options
        .parameters
        .insert("nothing".into(), serde_json::Value::Null);
    assert!(options.validate().is_ok());
    options
        .parameters
        .insert("object".into(), serde_json::json!({"unsafe": true}));
    assert!(options.validate().is_err());
}

#[test]
fn policies_match_quarto_minor_version() {
    assert_eq!(
        supported_policies(Some("Quarto 1.2.9")),
        vec!["project-defaults"]
    );
    assert!(supported_policies(Some("Quarto 1.3.0")).contains(&"frozen".into()));
    assert!(supported_policies(Some("Quarto 2.0.0")).contains(&"refresh-computations".into()));
    assert_eq!(supported_policies(None), vec!["project-defaults"]);
}

#[test]
fn imported_html_only_includes_referenced_assets() {
    let dir = tempdir().expect("tempdir");
    std::fs::write(dir.path().join("paper.html"), b"<img src=\"plot.png\">").expect("html");
    std::fs::write(dir.path().join("plot.png"), b"plot").expect("plot");
    std::fs::write(dir.path().join("private.csv"), b"secret").expect("private");
    let bundle = import_artifact(dir.path(), "paper.html", "html").expect("import");
    assert_eq!(bundle.assets.len(), 1);
    assert_eq!(bundle.assets[0].path, "plot.png");
    assert_eq!(bundle.coverage.cell_outputs, "unknown");
    let manifest = bundle.to_storage_manifest("doc", "revision");
    manifest.validate().expect("shared manifest validates");
}

#[test]
fn imported_html_follows_nested_css_dependencies() {
    let dir = tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("css/fonts")).expect("css");
    std::fs::write(
        dir.path().join("paper.html"),
        b"<link href=\"css/site.css\"><img src=\"plot.png\">",
    )
    .expect("html");
    std::fs::write(
        dir.path().join("css/site.css"),
        b"@font-face{src:url('./fonts/body.woff2')} body{background:url(../paper.png)}",
    )
    .expect("css");
    std::fs::write(dir.path().join("css/fonts/body.woff2"), b"font").expect("font");
    std::fs::write(dir.path().join("paper.png"), b"paper").expect("paper");
    std::fs::write(dir.path().join("plot.png"), b"plot").expect("plot");
    std::fs::write(dir.path().join("private.csv"), b"secret").expect("private");
    let bundle = import_artifact(dir.path(), "paper.html", "html").expect("import");
    let paths: BTreeSet<_> = bundle
        .assets
        .iter()
        .map(|asset| asset.path.as_str())
        .collect();
    assert_eq!(
        paths,
        BTreeSet::from([
            "css/site.css",
            "css/fonts/body.woff2",
            "paper.png",
            "plot.png"
        ])
    );
}

#[test]
fn managed_dependency_closure_does_not_publish_private_fallback_data() {
    let dir = tempdir().expect("project");
    let output = dir.path().join("output");
    std::fs::create_dir(&output).expect("output directory");
    std::fs::write(
        output.join("paper.html"),
        b"<img src=\"data/private.csv\">private data",
    )
    .expect("artifact");
    std::fs::create_dir_all(dir.path().join("data")).expect("data directory");
    std::fs::write(dir.path().join("data/private.csv"), b"secret").expect("private data");
    let empty_shared = Vec::new();
    let error = referenced_resource_closure(
        &output,
        Some(dir.path()),
        Some(&empty_shared),
        "paper.html",
        b"<img src=\"data/private.csv\">private data",
    )
    .unwrap_err();
    assert!(error.contains("required artifact dependency"));

    let shared = vec!["data/private.csv".to_string()];
    let references = referenced_resource_closure(
        &output,
        Some(dir.path()),
        Some(&shared),
        "paper.html",
        b"<img src=\"data/private.csv\">private data",
    )
    .expect("explicitly shared data");
    assert!(references.contains("data/private.csv"));
}

#[test]
fn managed_dependency_closure_skips_missing_or_unshared_navigation() {
    let dir = tempdir().expect("project");
    let output = dir.path().join("output");
    std::fs::create_dir(&output).expect("output directory");
    std::fs::write(
        output.join("paper.html"),
        b"<a href=\"chapters/missing.html\">next</a><a href=\"private.html\">private</a>",
    )
    .expect("artifact");
    std::fs::write(dir.path().join("private.html"), b"local page").expect("private page");
    let references = referenced_resource_closure(
        &output,
        Some(dir.path()),
        Some(&[]),
        "paper.html",
        b"<a href=\"chapters/missing.html\">next</a><a href=\"private.html\">private</a>",
    )
    .expect("navigation links do not require publication inputs");
    assert!(references.is_empty());
}

#[test]
fn navigation_kind_is_taken_from_html_tag_not_file_extension() {
    let dir = tempdir().expect("project");
    let output = dir.path().join("output");
    std::fs::create_dir(&output).expect("output directory");
    let navigation =
            b"<a href=\"data/raw.csv\">download</a><area href=\"route\"><script src=\"loader\"></script>";
    let error = referenced_resource_closure(
        &output,
        Some(dir.path()),
        Some(&[]),
        "paper.html",
        navigation,
    )
    .unwrap_err();
    assert!(error.contains("loader"));
    let navigation_only = b"<a href=\"data/raw.csv\">download</a><area href=\"route\">";
    let references = referenced_resource_closure(
        &output,
        Some(dir.path()),
        Some(&[]),
        "paper.html",
        navigation_only,
    )
    .expect("all navigation links are optional");
    assert!(references.is_empty());
}

#[test]
fn required_resource_is_not_hidden_by_an_earlier_navigation_link() {
    let dir = tempdir().expect("project");
    let output = dir.path().join("output");
    std::fs::create_dir(&output).expect("output directory");
    std::fs::write(
        output.join("paper.html"),
        b"<A href=\"second.html\">next</A>",
    )
    .expect("artifact");
    std::fs::write(
        output.join("second.html"),
        b"<script src=\"same-route\"></script>",
    )
    .expect("linked page");
    let error = referenced_resource_closure(
        &output,
        None,
        None,
        "paper.html",
        b"<A href=\"second.html\">next</A>",
    )
    .unwrap_err();
    assert!(error.contains("same-route"));
}

#[test]
fn managed_dependency_closure_still_requires_unshared_display_resources() {
    let dir = tempdir().expect("project");
    let output = dir.path().join("output");
    std::fs::create_dir(&output).expect("output directory");
    std::fs::write(output.join("paper.html"), b"<img src=\"private.png\">").expect("artifact");
    std::fs::write(dir.path().join("private.png"), b"private image").expect("private image");
    let error = referenced_resource_closure(
        &output,
        Some(dir.path()),
        Some(&[]),
        "paper.html",
        b"<img src=\"private.png\">",
    )
    .unwrap_err();
    assert!(error.contains("required artifact dependency"));
}

#[test]
fn nested_document_entrypoint_selects_nested_html_artifact() {
    assert!(is_artifact(
        "chapters/intro.html",
        "chapters/intro.qmd",
        "html",
        protocol::QuartoRenderScope::Document,
    ));
    assert!(is_artifact(
        "intro.html",
        "chapters/intro.qmd",
        "html",
        protocol::QuartoRenderScope::Document,
    ));
}

#[test]
fn frozen_cache_md5_matches_rfc_vectors_and_multi_block_source() {
    assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
    assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    assert_eq!(
        md5_hex(&vec![b'a'; 1000]),
        "cabe45dcc9ae5b66ba86600cca6b8ba8"
    );
}

fn valid_frozen_project() -> tempfile::TempDir {
    let directory = tempdir().expect("project");
    std::fs::write(
        directory.path().join("_quarto.yml"),
        "project:\n  type: default\nexecute:\n  freeze: true\n",
    )
    .expect("config");
    directory
}

#[test]
fn frozen_project_config_parses_yaml_and_rejects_execution_variants() {
    let quoted = valid_frozen_project();
    std::fs::write(
        quoted.path().join("_quarto.yml"),
        "project: {type: default}\nexecute: {freeze: true}\n'pre-render': [run.R]\n",
    )
    .expect("quoted hook");
    assert!(frozen_project_config(quoted.path(), "paper.qmd")
        .unwrap_err()
        .contains("configuration key"));

    let inherited = valid_frozen_project();
    std::fs::create_dir(inherited.path().join("chapter")).expect("chapter");
    std::fs::write(
        inherited.path().join("chapter/_metadata.yml"),
        "execute: {freeze: false}\n",
    )
    .expect("metadata");
    assert!(frozen_project_config(inherited.path(), "paper.qmd")
        .unwrap_err()
        .contains("inherited metadata"));

    let profile = valid_frozen_project();
    std::fs::write(
        profile.path().join("_quarto.yml"),
        "project: {type: website}\nexecute: {freeze: true}\n",
    )
    .expect("project type");
    assert!(frozen_project_config(profile.path(), "paper.qmd")
        .unwrap_err()
        .contains("project.type"));

    let profile_file = valid_frozen_project();
    std::fs::write(
        profile_file.path().join("_quarto-dev.yml"),
        "execute: {freeze: true}\n",
    )
    .expect("profile");
    assert!(frozen_project_config(profile_file.path(), "paper.qmd")
        .unwrap_err()
        .contains("profiles"));
}

#[test]
fn frozen_project_config_rejects_selected_environment_profile() {
    let project = valid_frozen_project();
    let error = frozen_project_config_with_profile(project.path(), "paper.qmd", true).unwrap_err();
    assert!(error.contains("QUARTO_PROFILE"));
}

#[test]
fn frozen_document_metadata_rejects_quoted_and_flow_filters() {
    let quoted = crate::quarto::parse_qmd(
        "---\nformat: html\n'filters': [custom.lua]\n---\ntext\n",
        "paper.qmd",
    );
    assert!(reject_frozen_front_matter(&quoted, "html")
        .unwrap_err()
        .contains("execution-bearing"));

    let flow = crate::quarto::parse_qmd(
        "---\nformat: {html: {filters: [custom.lua]}}\n---\ntext\n",
        "paper.qmd",
    );
    assert!(reject_frozen_front_matter(&flow, "html").is_err());
}

#[test]
fn frozen_cache_identity_covers_profiles_parameters_and_dependencies() {
    let source = "```{r}\n1 + 1\n```\n";
    let parameters = BTreeMap::from([("seed".into(), serde_json::json!(7))]);
    let profiles = vec!["review".to_string()];
    let dependencies = vec!["data.csv\0deadbeef".to_string()];
    let parameters_sha256 = crate::results::parameters_sha256(&parameters);
    let computation = engine_adapter::quarto_computation_fingerprint(
        source,
        "paper.qmd",
        "html",
        &profiles,
        Some(&parameters_sha256),
        &dependencies,
    );
    let cache = serde_json::json!({
        "librepaper_context": {
            "source_md5": md5_hex(source.as_bytes()),
            "format": "html",
            "computation_sha256": computation,
            "profiles": profiles,
            "parameters_sha256": parameters_sha256,
            "dependencies": dependencies,
        }
    });
    verify_frozen_cache_identity(
        &cache,
        source,
        "paper.qmd",
        "html",
        Some(&"review".to_string()),
        &parameters,
        &["data.csv\0deadbeef".to_string()],
    )
    .expect("matching context identity");
    let mut changed = parameters.clone();
    changed.insert("seed".into(), serde_json::json!(8));
    assert!(verify_frozen_cache_identity(
        &cache,
        source,
        "paper.qmd",
        "html",
        Some(&"review".to_string()),
        &changed,
        &["data.csv\0deadbeef".to_string()],
    )
    .is_err());
}

#[test]
fn managed_render_persists_frozen_context_identity_atomically() {
    let project = tempdir().expect("project");
    let cache = project.path().join("_freeze/paper/execute-results");
    std::fs::create_dir_all(&cache).expect("cache");
    let source = "```{r}\n1 + 1\n```\n";
    let options = QuartoJobOptions {
        binding_id: "binding".into(),
        main: "paper.qmd".into(),
        profile: Some("review".into()),
        parameters: BTreeMap::from([("seed".into(), serde_json::json!(7))]),
        ..Default::default()
    };
    let dependencies = vec!["data.csv\0deadbeef".to_string()];
    let path = cache.join("html.json");
    std::fs::write(
        &path,
        serde_json::json!({
            "hash": md5_hex(source.as_bytes()),
            "result": {"markdown": "# result", "supporting": []}
        })
        .to_string(),
    )
    .expect("cache record");
    persist_frozen_cache_identity(project.path(), &options, source, &dependencies)
        .expect("persist identity");
    let cache: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).expect("read cache")).expect("JSON");
    assert_eq!(
        cache["librepaper_context"]["source_md5"],
        md5_hex(source.as_bytes())
    );
    assert_eq!(
        cache["librepaper_context"]["dependencies"],
        serde_json::json!(dependencies)
    );
}

#[test]
fn isolated_snapshot_copies_only_verified_regular_inputs() {
    let source = tempdir().expect("source");
    let destination = tempdir().expect("destination");
    std::fs::create_dir_all(source.path().join("data")).expect("data");
    std::fs::write(source.path().join("paper.qmd"), b"# paper").expect("paper");
    std::fs::write(source.path().join("data/input.csv"), b"x\n1\n").expect("input");
    std::fs::write(source.path().join("private.txt"), b"secret").expect("private");
    let manifest = vec![
        protocol::ManifestEntry {
            path: "paper.qmd".into(),
            sha256: sha256(b"# paper"),
            size: 7,
        },
        protocol::ManifestEntry {
            path: "data/input.csv".into(),
            sha256: sha256(b"x\n1\n"),
            size: 4,
        },
    ];
    copy_isolated_snapshot(source.path(), destination.path(), &manifest, "paper.qmd")
        .expect("copy snapshot");
    assert!(destination.path().join("paper.qmd").is_file());
    assert!(destination.path().join("data/input.csv").is_file());
    assert!(!destination.path().join("private.txt").exists());
}

#[cfg(unix)]
#[test]
fn isolated_snapshot_root_is_canonicalized_before_use() {
    let real = tempdir().expect("real snapshot root");
    let alias = tempdir().expect("alias parent");
    let alias_path = alias.path().join("snapshot");
    std::os::unix::fs::symlink(real.path(), &alias_path).expect("snapshot alias");
    assert_eq!(canonical_snapshot_root(&alias_path).unwrap(), real.path());
}

#[test]
fn computation_fingerprint_includes_declared_snapshot_inputs() {
    let source = "```{r}\nread.csv('data/input.csv')\n```\n";
    let parameters = crate::results::parameters_sha256(&BTreeMap::new());
    let dependencies = vec!["data/input.csv\0deadbeef".to_string()];
    let adapter = engine_adapter::quarto_computation_fingerprint(
        source,
        "paper.qmd",
        "html",
        &[],
        Some(&parameters),
        &dependencies,
    );
    let mut parsed = crate::quarto::parse_qmd(source, "paper.qmd");
    parsed.dependencies = dependencies;
    let browser_contract = crate::quarto::computation_fingerprint_for_format(
        &parsed,
        "paper.qmd",
        "html",
        &[],
        Some(&parameters),
    );
    assert_eq!(adapter, browser_contract);
}

#[cfg(unix)]
#[test]
fn isolated_snapshot_rejects_symlink_inputs() {
    let source = tempdir().expect("source");
    let destination = tempdir().expect("destination");
    std::fs::write(source.path().join("paper.qmd"), b"# paper").expect("paper");
    std::os::unix::fs::symlink("/etc/passwd", source.path().join("data.csv")).expect("symlink");
    let manifest = vec![
        protocol::ManifestEntry {
            path: "paper.qmd".into(),
            sha256: sha256(b"# paper"),
            size: 7,
        },
        protocol::ManifestEntry {
            path: "data.csv".into(),
            sha256: "0".repeat(64),
            size: 1,
        },
    ];
    let error = copy_isolated_snapshot(source.path(), destination.path(), &manifest, "paper.qmd")
        .unwrap_err();
    assert!(error.contains("symlink"));
}

#[test]
fn project_scope_accepts_only_website_or_book() {
    let project = tempdir().expect("project");
    std::fs::write(
        project.path().join("_quarto.yml"),
        "project:\n  type: website\n",
    )
    .expect("config");
    validate_project_scope(project.path()).expect("website");
    std::fs::write(
        project.path().join("_quarto.yml"),
        "project:\n  type: default\n",
    )
    .expect("config");
    assert!(validate_project_scope(project.path()).is_err());
}
