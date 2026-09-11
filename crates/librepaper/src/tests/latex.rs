//! Source-only LaTeX publication and direct compiler mirror configuration.
use super::*;
use crate::document::render::{document_format, is_latex, title_from_latex};
use serde_json::{json, Value};

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

// What `librepaper publish paper.tex` leaves in the store. Nothing is
// rendered, because nothing here can render it; what is kept is the source and
// the name the document gave itself.
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
        .header("x-librepaper-client", "1")
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

#[tokio::test]
async fn compiler_url_is_advertised_without_an_origin_distribution_route() {
    let mirror = "https://compiler.example.invalid/";
    let server = test_server_latex(mirror.to_string()).await;
    let (status, config) = get_json(&server.url, "/api/config").await;
    assert_eq!(status, 200);
    assert_eq!(config["latexMirror"], mirror);
    assert_eq!(config["latex"], true);
    assert!(config.get("biberVm").is_none());
    for path in ["/latex/manifest.json", "/latex/engine.wasm"] {
        let response = client()
            .get(format!("{}{path}", server.url))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 404);
    }
}

#[test]
fn mirror_configuration_accepts_only_https() {
    for value in ["/srv/latex", "http://mirror.internal/", "file:///srv/latex"] {
        assert!(crate::server::serve::validate_latex_mirror(value).is_err());
    }
    assert!(crate::server::serve::validate_latex_mirror("https://mirror.example/").is_ok());
}
