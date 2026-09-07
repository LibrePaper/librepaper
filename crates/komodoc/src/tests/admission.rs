//! Admission regression tests and a small release-mode timing probe.

use std::time::Instant;

use yrs::{Map, Text, Transact};

use crate::session::{self, Admission, DecodedAdmission};

fn insertion_update(doc: &yrs::Doc, value: &str) -> Vec<u8> {
    let scratch = session::new_doc();
    session::apply_update(&scratch, &session::encode_state(doc)).expect("copy must apply");
    let before = session::encode_vector(&scratch);
    let source = scratch.get_or_insert_text(session::SOURCE);
    let mut txn = scratch.transact_mut();
    source.insert(&mut txn, 0, value);
    drop(txn);
    session::encode_diff(&scratch, &before).expect("the diff must encode")
}

fn new_file_update(doc: &yrs::Doc, path: &str) -> Vec<u8> {
    let scratch = session::new_doc();
    session::apply_update(&scratch, &session::encode_state(doc)).expect("copy must apply");
    let before = session::encode_vector(&scratch);
    session::put_text(&scratch, path, "x");
    session::encode_diff(&scratch, &before).expect("the diff must encode")
}

fn files_doc(count: usize) -> yrs::Doc {
    let doc = session::new_doc();
    for index in 0..count {
        session::put_text(&doc, &format!("{index}.txt"), "");
    }
    doc
}

#[test]
fn decoded_admission_can_be_applied_without_decoding_again() {
    let source = session::new_doc();
    let update = insertion_update(&source, "hello");
    let decoded = session::decode_update(&update).expect("the update must decode");
    let target = session::new_doc();
    let admitted = session::admit_decoded_update(&target, decoded, &update, 1024, 200);
    let DecodedAdmission::Fits(decoded) = admitted else {
        panic!("small decoded update should fit");
    };
    session::apply_decoded_update(&target, decoded).expect("admitted update must apply");
    assert_eq!(session::text_of(&target), "hello");
}

#[test]
fn unknown_roots_still_count_towards_the_byte_ceiling() {
    let source = session::new_doc();
    let future = source.get_or_insert_map("future-root");
    future.insert(&mut source.transact_mut(), "payload", "x".repeat(4096));
    let update = session::encode_state(&source);
    assert_eq!(
        session::admit_update(&session::new_doc(), &update, 1024, 200),
        Admission::TooLarge
    );
}

#[test]
fn pending_update_is_rehearsed_before_a_predecessor_can_cross_the_limit() {
    let source = session::new_doc();
    let before = session::encode_vector(&source);
    let text = source.get_or_insert_text(session::SOURCE);
    text.insert(&mut source.transact_mut(), 0, &"p".repeat(256));
    let predecessor = session::encode_diff(&source, &before).expect("predecessor must encode");

    let before = session::encode_vector(&source);
    text.insert(&mut source.transact_mut(), 256, &"z".repeat(256));
    let successor = session::encode_diff(&source, &before).expect("successor must encode");

    let target = session::new_doc();
    let ceiling = successor.len() + 8;
    assert_eq!(
        session::admit_update(&target, &successor, ceiling, usize::MAX),
        Admission::Fits,
        "the out-of-order successor itself is within the conservative wire bound"
    );
    session::apply_update(&target, &successor).expect("successor is retained as pending");
    assert_eq!(session::text_of(&target), "");

    assert_eq!(
        session::admit_update(&target, &predecessor, ceiling, usize::MAX),
        Admission::TooLarge,
        "integrating the pending successor must be included in quota admission"
    );
    assert_eq!(session::text_of(&target), "");
}

#[test]
fn a_noop_at_the_two_hundred_file_boundary_is_still_admitted() {
    let doc = files_doc(199);
    assert_eq!(
        session::admit_update(&doc, yrs::Update::EMPTY_V1, 1 << 20, 200),
        Admission::Fits
    );
}

#[test]
fn admission_probe_covers_realistic_file_and_paste_sizes() {
    for file_count in [0, 100, 200] {
        let doc = files_doc(file_count);
        let keystroke = insertion_update(&doc, "x");
        let paste = insertion_update(&doc, &"x".repeat(200));
        let create = new_file_update(&doc, "new.txt");

        assert_eq!(
            session::admit_update(&doc, yrs::Update::EMPTY_V1, 4 * 1024 * 1024, 200),
            Admission::Fits,
            "a no-op must fit at {file_count} files"
        );
        assert_eq!(
            session::admit_update(&doc, &keystroke, 4 * 1024 * 1024, 200),
            Admission::Fits,
            "a keystroke must fit at {file_count} files"
        );
        assert_eq!(
            session::admit_update(&doc, &paste, 4 * 1024 * 1024, 200),
            Admission::Fits,
            "a 200-byte paste must fit at {file_count} files"
        );
        assert_eq!(
            session::admit_update(&doc, &create, 4 * 1024 * 1024, 200),
            if file_count == 200 {
                Admission::TooMany
            } else {
                Admission::Fits
            },
            "a new file must respect the 200-file ceiling at {file_count} files"
        );

        let started = Instant::now();
        for _ in 0..10 {
            let _ = session::admit_update(&doc, &keystroke, 4 * 1024 * 1024, 200);
        }
        let keystroke_time = started.elapsed() / 10;

        let started = Instant::now();
        for _ in 0..3 {
            let _ = session::admit_update(&doc, &paste, 4 * 1024 * 1024, 200);
        }
        let paste_time = started.elapsed() / 3;
        eprintln!(
            "admission probe: {file_count} files, keystroke {keystroke_time:?}, paste {paste_time:?}"
        );
    }
}
