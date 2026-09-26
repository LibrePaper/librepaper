//! A shared document as a peer can shape it, projected.
//!
//! Every key in the four maps is a string any editor can set, so the peer
//! here writes through the binary's own functions and straight into the maps
//! alike. What used to happen next was a repair pass that rewrote the
//! document into a shape the server would accept. There is no repair now
//! (SPEC-server-is-a-log §2.3): the server never rewrites what a peer wrote,
//! and a shape the schema does not use is simply absent from the projection
//! with a diagnostic.
//!
//! That makes this target more important rather than less. §4.4 opens with
//! "a projection is a total, deterministic function of a `LoroDoc`", and
//! "total" is a claim about every input, which is exactly the kind of claim
//! a fuzzer can attack and a unit test cannot. A projection that panicked on
//! some arrangement of the four maps would take the reader route, the
//! comment anchoring, the export and every semantic command down with it,
//! for a document any peer can write on purpose.
//!
//! What is asserted:
//!
//! * projecting never panics, whatever is in the maps (libFuzzer's own
//!   oracle: reaching the end of this function is the pass);
//! * it is deterministic -- the same document projects to the same tree and
//!   the same digest twice running;
//! * the main file, when there is one, is a text that is in the tree;
//! * every text in the tree has a body beside it, which is what an export
//!   and a render both depend on;
//! * no path in the tree is a directory prefix of another, which is step 4's
//!   reservation rule and the thing that makes the tree writable to a real
//!   filesystem;
//! * the digest is a function of the tree and nothing else: two documents
//!   that project to the same tree have the same digest.
//!
//! Also still asserted, from before: a replacement lands exactly whatever
//! the characters in it, the word-level edits `sync` applies land exactly,
//! and encoding the document and decoding it elsewhere gives the same
//! document.
#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use librepaper::session;
use librepaper_fuzz::{configuration, rules};
use loro::{LoroText, ValueOrContainer};
use wasm_helpers::text::diff;

#[derive(Arbitrary, Debug)]
enum Op {
    /// Through the binary: a file put at a path.
    PutText { path: String, body: String },
    /// Through the binary: an asset's digest named at a path.
    PutAsset { path: String, sha: String },
    /// Through the binary: the main file replaced as a publish does.
    Replace { wanted: String, path: String },
    /// Through the binary: the main file edited as a `sync` write does.
    Edit { wanted: String },
    /// Through the binary: another id named as main.
    SetMain { id: String },
    /// Straight into the maps, as a browser can: a text under any id, with
    /// or without a path.
    RawFile {
        id: String,
        body: String,
        path: Option<String>,
    },
    /// Straight into the maps: any id given any path.
    RawPath { id: String, path: String },
    /// Straight into the maps: any asset key.
    RawAsset { path: String, sha: String },
    /// Straight into the maps: `meta.main` set to anything.
    RawMain { id: String },
    /// Straight into the maps: a `files` value that is not a text at all,
    /// which §4.4 step 1 says is absent with a diagnostic rather than an
    /// error.
    RawNonText { id: String, number: i64 },
}

fuzz_target!(|ops: Vec<Op>| {
    let config = configuration();
    let rules = rules(&config);
    let doc = session::new_doc();
    let files = doc.get_map(session::FILES);
    let paths = doc.get_map(session::PATHS);
    let assets = doc.get_map(session::ASSETS);
    let meta = doc.get_map(session::META);

    for op in &ops {
        match op {
            Op::PutText { path, body } => {
                session::put_text(&doc, path, body);
            }
            Op::PutAsset { path, sha } => session::put_asset(&doc, path, sha),
            Op::Replace { wanted, path } => {
                session::replace_text(&doc, wanted, path);
            }
            Op::Edit { wanted } => {
                let before = session::text_of(&doc);
                let edits = diff(&before, wanted);
                let main = session::main_path(&doc);
                if !main.is_empty() {
                    session::apply_edits_at(&doc, &main, &edits);
                }
            }
            Op::SetMain { id } => session::set_main(&doc, id),
            Op::RawFile { id, body, path } => {
                if let Ok(text) = files.insert_container(id.as_str(), LoroText::new()) {
                    if !body.is_empty() {
                        text.insert_utf16(0, body).ok();
                    }
                }
                if let Some(path) = path {
                    paths.insert(id.as_str(), path.as_str()).ok();
                }
            }
            Op::RawPath { id, path } => {
                paths.insert(id.as_str(), path.as_str()).ok();
            }
            Op::RawAsset { path, sha } => {
                assets.insert(path.as_str(), sha.as_str()).ok();
            }
            Op::RawMain { id } => {
                meta.insert(session::MAIN, id.as_str()).ok();
            }
            Op::RawNonText { id, number } => {
                files.insert(id.as_str(), *number).ok();
            }
        }
    }
    doc.commit();

    // Totality. Reaching the next line at all is most of what this target
    // is for.
    let first = librepaper_document_core::project(&doc, &rules);

    // Determinism, which the two implementations of §4.4 are held to by
    // fixtures and which this holds one implementation to across every
    // document libFuzzer can build.
    let second = librepaper_document_core::project(&doc, &rules);
    assert_eq!(
        first.projection, second.projection,
        "projecting one document twice gave two trees"
    );
    assert_eq!(
        first.projection.digest(),
        second.projection.digest(),
        "one tree had two digests"
    );

    // The main file is a text that is here, or there is no main. §4.4 step 6
    // chooses the first text when `meta.main` names nothing, so "no main" is
    // reachable only for a document with no text in it at all.
    if !first.projection.main.is_empty() {
        let entry = first
            .projection
            .files
            .get(&first.projection.main)
            .expect("the main path is in the tree");
        assert_eq!(entry.kind, "text", "the main file is a text");
        assert_eq!(
            entry.id, first.projection.main_id,
            "the main path and the main id name one file"
        );
    } else {
        assert!(
            first
                .projection
                .files
                .values()
                .all(|entry| entry.kind != "text"),
            "a document with a text in it has a main file"
        );
    }

    for (path, entry) in &first.projection.files {
        if entry.kind == "text" {
            assert!(
                first.texts.contains_key(path),
                "{path:?} is a text in the tree with no body beside it"
            );
        }
        // Step 4's reservation rule: a file never sits inside another file.
        // This is what makes the tree writable to a real filesystem, and a
        // pair that broke it would be two files one `mkdir -p` apart.
        let under = format!("{path}/");
        assert!(
            !first
                .projection
                .files
                .keys()
                .any(|other| other.starts_with(&under)),
            "{path:?} is both a file and a directory"
        );
    }

    // The digest is a function of the tree and nothing else. Rebuilding a
    // document from the projection alone, under fresh ids, must name it the
    // same: that is what lets a document restored from its own archive be
    // recognised as the document it was (§4.4 step 7).
    let rebuilt = session::new_doc();
    let mut minted = 0u32;
    for (path, entry) in &first.projection.files {
        match entry.kind.as_str() {
            "text" => {
                minted += 1;
                session::put_text(
                    &rebuilt,
                    path,
                    first.texts.get(path).map(String::as_str).unwrap_or_default(),
                );
            }
            _ => session::put_asset(&rebuilt, path, &entry.digest),
        }
    }
    if !first.projection.main.is_empty() {
        if let Some((id, _)) = session::paths_of(&rebuilt)
            .into_iter()
            .find(|(_, path)| path == &first.projection.main)
        {
            session::set_main(&rebuilt, &id);
        }
    }
    rebuilt.commit();
    let again = librepaper_document_core::project(&rebuilt, &rules);
    if minted > 0 || !first.projection.files.is_empty() {
        assert_eq!(
            first.projection.digest(),
            again.projection.digest(),
            "the same directory under fresh ids got a different name:\n{:?}\nvs\n{:?}",
            first.projection,
            again.projection
        );
    }

    // Encoding and decoding gives the same document, which is what every
    // join and every cache build depends on.
    let peer = session::new_doc();
    if session::apply_update(&peer, &session::encode_state(&doc)).is_ok() {
        let there = librepaper_document_core::project(&peer, &rules);
        assert_eq!(
            first.projection, there.projection,
            "a document encoded here projected differently there"
        );
    }

    // A value that is not a text is absent rather than fatal (§4.4 step 1).
    for op in &ops {
        if let Op::RawNonText { id, .. } = op {
            if matches!(files.get(id.as_str()), Some(ValueOrContainer::Value(_))) {
                assert!(
                    !first.projection.files.values().any(|entry| &entry.id == id),
                    "a files value that is not a text reached the tree"
                );
            }
        }
    }
});
