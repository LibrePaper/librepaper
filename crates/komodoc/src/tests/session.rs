//! The shared document as the server holds it: restores that keep a peer's
//! words, edits at UTF-16 edges, and the limits admission enforces.
use std::collections::{BTreeMap, HashMap};

use crate::document::history::{Tree, TreeEntry};
use crate::document::session;
use yrs::{Map, Transact};

/// Restoring a target after a peer update must leave a disjoint peer word in
/// place. The peer update is encoded and applied through Yrs exactly as the
/// browser path does; the room's three-way merge then supplies the body that
/// the path-based restore applies as separate word edits.
#[test]
fn restore_keeps_a_disjoint_peer_word() {
    let doc = session::new_doc();
    let base = "alpha beta gamma";
    let id = session::put_text(&doc, "main.md", base);

    // A second Yrs peer starts from the same state and inserts a word beside
    // the region the selected checkpoint will replace.
    let peer = session::new_doc();
    session::apply_update(&peer, &session::encode_state(&doc)).expect("peer state");
    session::replace_text(&peer, "alpha beta PEER gamma", "main.md");
    session::apply_update(&doc, &session::encode_state(&peer)).expect("peer update");

    let target = "alpha TARGET gamma";
    let merged = komodoc_text::merge(base, &session::text_of(&doc), target).text;
    assert_eq!(merged, "alpha TARGET PEER gamma");
    let sha = crate::document::store::digest_of(target);
    let mut files = BTreeMap::new();
    files.insert(
        "main.md".to_string(),
        TreeEntry {
            kind: "text".to_string(),
            id,
            sha: sha.clone(),
            size: target.len() as i64,
        },
    );
    let tree = Tree {
        main: "main.md".to_string(),
        files,
        settings: None,
    };
    let mut bodies = HashMap::new();
    bodies.insert("main.md".to_string(), merged);
    session::restore_by_path(&doc, &tree, &bodies);
    assert_eq!(session::text_of(&doc), "alpha TARGET PEER gamma");
}

/// R14: `edit_text` computed its common prefix/suffix over raw UTF-16 code
/// units. Two distinct emoji sharing a surrogate could make that boundary
/// fall inside a pair, which the probe `review_replacing_emoji_splits_surrogate_pair`
/// showed panicking inside Yrs. The fix chooses boundaries on `char`
/// (Unicode scalar) positions first, so this must now simply succeed and
/// read back the replacement text.
#[test]
fn replacing_emoji_splits_surrogate_pair() {
    let doc = session::new_doc();
    session::replace_text(&doc, "\u{1F600}", "main.md"); // 😀
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        session::replace_text(&doc, "\u{1F601}", "main.md"); // 😁
    }));
    assert!(
        outcome.is_ok(),
        "emoji replacement sharing a surrogate must not panic"
    );
    assert_eq!(session::text_of(&doc), "\u{1F601}");
}

/// Two ordinary emoji that are simply adjacent to each other exercise the
/// same boundary code without any surrogate pair split at stake; it must
/// keep working exactly as it did before the fix.
#[test]
fn adjacent_emoji_replacement() {
    let doc = session::new_doc();
    session::replace_text(&doc, "\u{1F600}\u{1F601}", "main.md"); // 😀😁
    session::replace_text(&doc, "\u{1F601}\u{1F600}", "main.md"); // 😁😀
    assert_eq!(session::text_of(&doc), "\u{1F601}\u{1F600}");
}

/// Inserting and deleting text immediately beside an astral character must
/// not disturb the character itself, and must not offer Yrs a boundary
/// inside its surrogate pair.
#[test]
fn insertion_and_deletion_around_emoji() {
    let doc = session::new_doc();
    session::replace_text(&doc, "a\u{1F600}b", "main.md"); // a😀b
    session::replace_text(&doc, "aXX\u{1F600}b", "main.md"); // insert before
    assert_eq!(session::text_of(&doc), "aXX\u{1F600}b");
    session::replace_text(&doc, "aXX\u{1F600}", "main.md"); // delete after
    assert_eq!(session::text_of(&doc), "aXX\u{1F600}");
    session::replace_text(&doc, "\u{1F600}", "main.md"); // delete before
    assert_eq!(session::text_of(&doc), "\u{1F600}");
}

/// A round trip through the same encode/decode path a browser's Yjs update
/// takes must also survive an emoji-to-emoji replacement: apply the edit,
/// then rebuild a fresh document from the encoded state and confirm it reads
/// the replacement rather than failing to decode/apply.
#[test]
fn emoji_replacement_round_trips_through_encoded_update() {
    let doc = session::new_doc();
    session::replace_text(&doc, "\u{1F600}", "main.md");
    session::replace_text(&doc, "\u{1F601}", "main.md");
    let update = session::encode_state(&doc);
    let copy = session::new_doc();
    session::apply_update(&copy, &update).expect("encoded update must apply cleanly");
    assert_eq!(session::text_of(&copy), "\u{1F601}");
}

/// R18: `measure` counted only text bodies and a handful of keys, so a large
/// metadata value could ride along under the ceiling on the very path meant
/// to catch it exactly. `admit_update`'s rehearsal now sums every retained
/// string value, including `meta`, so this must be refused as `TooLarge`.
#[test]
fn metadata_bypasses_byte_limit() {
    let doc = session::new_doc();
    let meta = doc.get_or_insert_map(session::META);
    meta.insert(&mut doc.transact_mut(), "payload", "x".repeat(200_000));
    let empty = session::new_doc();
    let update = session::encode_state(&doc);
    assert!(update.len() > 200_000);
    assert_eq!(
        session::admit_update(&empty, &update, 1024, 200),
        session::Admission::TooLarge,
    );
}

/// R19(a): the cheap admission branch used to ask only whether the
/// document's *old* file count was under `max_files`, so one update that
/// creates many files at once passed regardless of how many it added. A
/// fresh batch of 20 files must now be rejected as `TooMany` under a ceiling
/// of 5.
#[test]
fn file_count_fast_path_rejects_oversized_batch() {
    let doc = session::new_doc();
    for i in 0..20 {
        session::put_text(&doc, &format!("{i}.txt"), "x");
    }
    let empty = session::new_doc();
    let update = session::encode_state(&doc);
    assert_eq!(
        session::admit_update(&empty, &update, 100_000, 5),
        session::Admission::TooMany,
    );
}

/// R19(b): the exact measurement counted a text file's `files` entry and its
/// `paths` entry as two files, so a valid document with exactly `max_files`
/// legitimate files rejected even a no-op update. A document with 3 files
/// under a ceiling of 5 must admit a no-op update.
#[test]
fn file_count_no_op_on_valid_document_fits() {
    let legitimate = session::new_doc();
    for i in 0..3 {
        session::put_text(&legitimate, &format!("{i}.txt"), "x");
    }
    let noop = yrs::Update::EMPTY_V1;
    assert_eq!(
        session::admit_update(&legitimate, noop, 100_000, 5),
        session::Admission::Fits,
    );
}
