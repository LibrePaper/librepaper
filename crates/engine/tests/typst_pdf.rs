use std::path::Path;

use komodoc_engine::diagnostic::RenderedDocument;
use komodoc_engine::typst::{compile_pdf, no_files};

const PAPER: &str = include_str!("../tests/typst-corpus/paper.typ");
const LONG: &str = include_str!("../tests/typst-corpus/long.typ");
const BROKEN: &str = include_str!("../tests/typst-corpus/broken.typ");

fn corpus_file(path: &Path) -> Option<Vec<u8>> {
    match path.to_str()? {
        "lib.typ" => Some(include_bytes!("typst-corpus/lib.typ").to_vec()),
        "refs.bib" => Some(include_bytes!("typst-corpus/refs.bib").to_vec()),
        "asset.svg" => Some(include_bytes!("typst-corpus/asset.svg").to_vec()),
        _ => None,
    }
}

#[test]
fn corpus_compiles_to_typed_pdf_with_imports_and_assets() {
    let compiled = compile_pdf(PAPER, "paper.typ", &corpus_file, None);
    assert!(
        compiled.errors().next().is_none(),
        "{:?}",
        compiled.diagnostics
    );
    let Some(RenderedDocument::Pdf(pdf)) = compiled.output else {
        panic!("expected PDF output, got {:?}", compiled.output);
    };
    assert!(pdf.starts_with(b"%PDF-"), "missing PDF header");
    assert!(pdf.len() > 1_000, "fixture PDF is unexpectedly tiny");
}

#[test]
fn long_fixture_has_multiple_pages_and_repeated_compiles_are_independent() {
    let first = compile_pdf(LONG, "long.typ", &no_files, None);
    let second = compile_pdf(LONG, "long.typ", &no_files, None);
    for compiled in [first, second] {
        let Some(RenderedDocument::Pdf(pdf)) = compiled.output else {
            panic!("expected PDF output, got {:?}", compiled.diagnostics);
        };
        assert!(pdf.starts_with(b"%PDF-"));
        assert!(
            pdf.len() > 4_000,
            "long fixture did not produce a paged-sized PDF"
        );
    }
}

#[test]
fn missing_import_is_a_structured_error_without_partial_pdf() {
    let compiled = compile_pdf(BROKEN, "broken.typ", &no_files, None);
    assert!(compiled.output.is_none());
    let diagnostic = compiled.errors().next().expect("missing diagnostic");
    assert!(!diagnostic.message.is_empty());
    assert!(compiled.diagnostics_json().contains("error"));
}
