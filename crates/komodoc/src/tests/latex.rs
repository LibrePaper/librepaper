//! LaTeX as a document: what a `.tex` file becomes when it is published, what
//! a deployment says about its mirror, and what the mirror route serves.
//!
//! Nothing here compiles anything. There is no TeX in this binary and there
//! never will be -- the compiler is in the browser, and these are the answers
//! the browser needs before it can go and get one.

use serde_json::{json, Value};

use super::*;
use crate::latex::Mirror;
use crate::render::{document_format, is_latex, title_from_latex};

/// A mirror on disk, in the shape `latex/tools/mirror.mjs` writes: the manifest at
/// the top and everything else under a directory named by a digest.
fn mirror_directory() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temporary directory");
    std::fs::write(
        dir.path().join("manifest.json"),
        r#"{"version":1,"distributions":{}}"#,
    )
    .expect("manifest");
    let release = dir.path().join("swiftlatex-pdftex/2dfb2fc534b459b5");
    std::fs::create_dir_all(&release).expect("release directory");
    std::fs::write(release.join("engine.wasm"), b"\0asm\x01\0\0\0").expect("module");
    std::fs::write(release.join("engine.js"), b"// the loader\n").expect("loader");
    // A file nobody may reach through the route, one directory above the
    // mirror, for the escape test below.
    std::fs::write(dir.path().join("secrets"), b"not yours").expect("secret");
    dir
}

/// The bucket, stood in for by an ordinary HTTP server on a free port.
///
/// `Mirror::open` refuses a plain HTTP URL, and it is right to: a browser will
/// not load a compiler over one. That refusal is a startup policy about what
/// an operator may configure, not a rule about what `get` can fetch, so a test
/// that wants the upstream path builds the variant directly rather than
/// standing up TLS to prove a proxy proxies.
async fn upstream_stand_in(files: Vec<(&'static str, &'static str)>) -> (Mirror, String) {
    use axum::routing::get;
    let mut router = axum::Router::new();
    for (path, body) in files {
        router = router.route(path, get(move || async move { body }));
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a free port");
    let address = listener.local_addr().expect("an address");
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    let base = format!("http://{address}/");
    (
        Mirror::Upstream {
            base: base.clone(),
            client: reqwest::Client::new(),
        },
        base,
    )
}

async fn fetch(base: &str, path: &str) -> reqwest::Response {
    client()
        .get(format!("{base}{path}"))
        .send()
        .await
        .expect("the mirror route answers")
}

fn header(response: &reqwest::Response, name: &str) -> String {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/* --------------------------------------------------------- what a .tex is */

// The title scan, on the two shapes that matter: a title dressed in the
// macros a paper actually carries, and a document that never named itself.
#[test]
fn a_latex_title_is_the_title_macro_with_its_dressing_removed() {
    // The acknowledgement goes with its argument, and the line break goes.
    // Written both ways because papers are: a `\thanks` hard against the
    // title, and one after the break that starts the second line.
    assert_eq!(title_from_latex("\\title{A\\\\thanks{x} B}"), "A B");
    assert_eq!(title_from_latex("\\title{A\\thanks{x} B}"), "A B");
    assert_eq!(title_from_latex("\\title{A\\\\ B}"), "A B");
    assert_eq!(
        title_from_latex("\\documentclass{article}\n\\title[Short]{The \\textbf{Long} One}\n"),
        "The Long One"
    );
    assert_eq!(
        title_from_latex("\\title{Nested {braces} survive}"),
        "Nested braces survive"
    );
    assert_eq!(
        title_from_latex("\\documentclass{article}\n\\titlepage\n"),
        ""
    );
    assert_eq!(title_from_latex("no title here"), "");
    assert_eq!(title_from_latex("% \\title{Commented out}"), "");
}

#[test]
fn tex_and_ltx_are_latex_and_nothing_else_is() {
    assert!(is_latex("paper.tex"));
    assert!(is_latex("PAPER.TEX"));
    assert!(is_latex("chapters/one.ltx"));
    assert!(!is_latex("paper.typ"));
    assert!(!is_latex("paper.texinfo"));
    // One place decides, and it decides this the same way.
    assert_eq!(document_format("paper.tex"), Some("latex"));
    assert_eq!(document_format("paper.bib"), None);
}

// What `komodoc publish paper.tex` leaves in the store. Nothing is rendered,
// because nothing here can render it; what is kept is the source and the name
// the document gave itself.
#[tokio::test]
async fn a_tex_publish_is_stored_as_latex_with_its_own_title() {
    let server = new_test_server().await;
    let source = "\\documentclass{article}\n\\title{On the Bootstrap}\n\\begin{document}\n\\maketitle\n\\end{document}\n";
    let (status, document) = post_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        "/api/documents",
        json!({"source": source, "source_format": "latex", "title": "On the Bootstrap"}),
    )
    .await;
    assert_eq!(status, 201, "{document}");

    let (status, entry) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("/api/documents/{}", text(&document, "slug")),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(text(&entry, "source_format"), "latex");
}

// The upload route derives the format from the main file's name, through the
// one function that decides it, so a directory whose main file is `.tex`
// arrives as latex rather than as HTML.
#[tokio::test]
async fn a_tex_directory_upload_arrives_as_latex() {
    let server = new_test_server().await;
    let source = "\\documentclass{article}\n\\title{A Paper With Chapters}\n\\begin{document}\n\\input{one}\n\\end{document}\n";
    let form = reqwest::multipart::Form::new()
        .text("main", "paper.tex")
        .part(
            "file",
            reqwest::multipart::Part::text(source).file_name("paper.tex"),
        )
        .part(
            "file",
            reqwest::multipart::Part::text("A chapter.\n").file_name("one.tex"),
        );
    let response = client()
        .post(format!("{}/api/documents", server.url))
        .header("x-komodoc-client", "1")
        .header("cookie", session_as(TEST_PUBLISHER))
        .multipart(form)
        .send()
        .await
        .expect("upload");
    assert_eq!(response.status().as_u16(), 201);
    let document: Value = response.json().await.expect("json");
    let (status, entry) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("/api/documents/{}", text(&document, "slug")),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(text(&entry, "source_format"), "latex");
    assert_eq!(
        text(&entry, "title"),
        "A Paper With Chapters",
        "the title came from the \\title of the main file"
    );
}

/* -------------------------------------------------- what a deployment says */

// The reader asks two questions before it offers anything: can this deployment
// render latex again, and is there a mirror to fetch a compiler from. Both are
// no on a deployment started without `--latex`, whatever the binary can do.
#[tokio::test]
async fn a_deployment_reports_latex_only_when_it_has_a_mirror() {
    let plain = new_test_server().await;
    let (status, config) = get_json(&plain.url, "/api/config").await;
    assert_eq!(status, 200);
    assert_eq!(config.get("latex"), Some(&json!(false)));
    assert!(!plain.instance.renderers().contains(&"latex".to_string()));

    let dir = mirror_directory();
    let with_mirror =
        test_server_latex(Mirror::open(&dir.path().display().to_string()).expect("a directory"))
            .await;
    let (status, config) = get_json(&with_mirror.url, "/api/config").await;
    assert_eq!(status, 200);
    assert_eq!(config.get("latex"), Some(&json!(true)));
    assert!(with_mirror
        .instance
        .renderers()
        .contains(&"latex".to_string()));
    // Whether, never where. The mirror may be a bucket whose URL is the
    // operator's business, and the browser has no use for it.
    let printed = config.to_string();
    assert!(
        !printed.contains(&dir.path().display().to_string()),
        "{printed}"
    );

    // And a document is offered an editor either way -- `latex` has been a
    // storable source format since it was one at all, which is what keeps a
    // `.tex` file readable on a deployment with no mirror.
    assert!(crate::config::Configuration::default().storable_source("latex"));
}

/* ------------------------------------------------------- the mirror route */

// The two cache lives a mirror has. The manifest is the only file whose URL
// carries no digest, so it is the only one that may not be cached for ever;
// everything else is named by a digest of its own bytes.
#[tokio::test]
async fn a_directory_mirror_is_served_with_the_headers_its_urls_earn() {
    let dir = mirror_directory();
    let server =
        test_server_latex(Mirror::open(&dir.path().display().to_string()).expect("a directory"))
            .await;

    let response = fetch(&server.url, "/latex/manifest.json").await;
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(header(&response, "cache-control"), "no-cache");
    assert!(header(&response, "content-type").starts_with("application/json"));
    assert!(response.text().await.expect("body").contains("\"version\""));

    let response = fetch(
        &server.url,
        "/latex/swiftlatex-pdftex/2dfb2fc534b459b5/engine.wasm",
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        header(&response, "cache-control"),
        "public, max-age=31536000, immutable"
    );
    // `WebAssembly.instantiateStreaming` refuses anything else.
    assert_eq!(header(&response, "content-type"), "application/wasm");

    let response = fetch(
        &server.url,
        "/latex/swiftlatex-pdftex/2dfb2fc534b459b5/engine.js",
    )
    .await;
    assert!(header(&response, "content-type").starts_with("text/javascript"));
}

#[tokio::test]
async fn an_upstream_mirror_is_proxied_with_the_same_headers_and_its_own_status() {
    let (mirror, _base) = upstream_stand_in(vec![
        ("/manifest.json", "{\"version\":1}"),
        ("/dist/abc123/engine.wasm", "not really wasm"),
    ])
    .await;
    let server = test_server_latex(mirror).await;

    let response = fetch(&server.url, "/latex/manifest.json").await;
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(header(&response, "cache-control"), "no-cache");

    let response = fetch(&server.url, "/latex/dist/abc123/engine.wasm").await;
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        header(&response, "cache-control"),
        "public, max-age=31536000, immutable"
    );
    assert_eq!(header(&response, "content-type"), "application/wasm");

    // The upstream's own answer, passed through rather than translated: a
    // bucket whose policy is wrong should not read as a missing file.
    let response = fetch(&server.url, "/latex/dist/abc123/absent.wasm").await;
    assert_eq!(response.status().as_u16(), 404);
}

// Every way out of the mirror, through the route rather than through the
// function, because a route is where one would be tried.
#[tokio::test]
async fn nothing_outside_the_mirror_is_reachable_through_it() {
    let dir = mirror_directory();
    let server =
        test_server_latex(Mirror::open(&dir.path().display().to_string()).expect("a directory"))
            .await;

    for escape in [
        "/latex/../secrets",
        "/latex/swiftlatex-pdftex/../../secrets",
        "/latex/%2e%2e/secrets",
        "/latex/nothing-of-the-sort.wasm",
        "/latex/",
    ] {
        let response = fetch(&server.url, escape).await;
        assert_eq!(
            response.status().as_u16(),
            404,
            "{escape} was answered {}",
            response.status()
        );
        let body = response.text().await.unwrap_or_default();
        assert!(!body.contains("not yours"), "{escape} served the secret");
    }
}

// A deployment with no mirror has no route either. The reader has already been
// told `latex: false` and has no reason to ask, but a page cached from a
// deployment that did have one would.
#[tokio::test]
async fn a_deployment_without_a_mirror_serves_no_mirror() {
    let server = new_test_server().await;
    let response = fetch(&server.url, "/latex/manifest.json").await;
    assert_eq!(response.status().as_u16(), 404);
}

// The refusal an operator gets rather than a browser: the shell's CSP permits
// https and blob and nothing else, so an http mirror would fail in every
// browser with nothing on screen to say why.
#[test]
fn a_plain_http_mirror_is_refused_at_startup() {
    let err = Mirror::open("http://mirror.internal/latex/").expect_err("http was accepted");
    assert!(err.contains("plain HTTP"), "{err}");
    assert!(err.contains("https"), "{err}");
    assert!(Mirror::open("https://example.invalid/latex/").is_ok());
}

// A mirror that is merely unreachable is a warning, not a death: a deployment
// that serves markdown has no business refusing to start because a bucket is
// still filling.
#[tokio::test]
async fn an_unreachable_mirror_warns_and_does_not_die() {
    let mirror = Mirror::open("https://mirror.invalid.example/latex/").expect("https is accepted");
    let warning = mirror.probe().await.expect("an unreachable mirror warns");
    assert!(warning.starts_with("warning:"), "{warning}");

    let empty = tempfile::tempdir().expect("a temporary directory");
    let warning = Mirror::open(&empty.path().display().to_string())
        .expect("an empty directory is still a directory")
        .probe()
        .await
        .expect("a mirror with no manifest warns");
    assert!(warning.contains("manifest.json"), "{warning}");

    // And a mirror that is there says nothing at all.
    let dir = mirror_directory();
    assert!(Mirror::open(&dir.path().display().to_string())
        .expect("a directory")
        .probe()
        .await
        .is_none());
}
