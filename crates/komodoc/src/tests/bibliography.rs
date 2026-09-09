use serde_json::{json, Value};

#[test]
fn bibliography_diagnostics_include_citation_source_context() {
    let source = "---\nbibliography: refs.bib\n---\n\nEvidence [@missing].\n";
    let snapshot = crate::cli::peer::Snapshot {
        format: "markdown".into(),
        main: "paper/main.md".into(),
        source: source.into(),
        texts: json!({ "paper/main.md": source, "paper/refs.bib": "@article{known,title={Known},year={2020}}" }),
        ..Default::default()
    };
    let output: Value =
        serde_json::from_str(&crate::cli::peer::diagnostics_json(&snapshot).unwrap()).unwrap();
    let missing = output
        .as_array()
        .unwrap()
        .iter()
        .find(|item| {
            item["message"]
                .as_str()
                .unwrap_or_default()
                .contains("missing")
        })
        .unwrap();
    assert_eq!(missing["path"], "paper/main.md");
    assert_eq!(missing["source_line"], "Evidence [@missing].");
}

#[test]
fn bibliography_diagnostics_report_a_missing_resource() {
    let snapshot = crate::cli::peer::Snapshot {
        format: "markdown".into(),
        main: "paper.md".into(),
        source: "---\nbibliography: absent.bib\n---\nHello".into(),
        ..Default::default()
    };
    assert!(crate::cli::peer::diagnostics_json(&snapshot)
        .unwrap()
        .contains("absent.bib"));
}
