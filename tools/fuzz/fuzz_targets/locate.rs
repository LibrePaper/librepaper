//! Where a reader's selection came from in the source.
//!
//! Both sides of this are outside input: the selection is whatever the
//! browser sends, and the files are whatever the author uploaded. Between
//! them runs the one piece of index arithmetic in the crate that crosses
//! encodings -- a UTF-8 file walked byte by byte, producing UTF-16 offsets a
//! browser will count in -- with an HTML flattener in the middle that skips
//! tags and decodes entities. That is a lot of offsets to get right, and the
//! corpus test next to it only ever tries the tutorials.
//!
//! What is asserted:
//!
//! * nothing panics, on any bytes, for any file name;
//! * a flattening maps every character it kept to a real UTF-16 offset in the
//!   file, the offsets never go backwards, and the one past the end is the
//!   length of the file;
//! * a run of whitespace is one space, so a needle with one space in it can
//!   match a haystack with two;
//! * a range that comes back is a range of the file it names, and the quoted
//!   text is the text at that range -- the property a wrong answer breaks,
//!   and a wrong answer is a comment about the wrong sentence.
#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use librepaper::locate::{flatten, is_html, locate, Candidate, Quote};

#[derive(Arbitrary, Debug)]
struct Input {
    /// The document's files: a name, which decides whether it is read as
    /// HTML, and a body.
    files: Vec<(String, String)>,
    /// What the browser says was selected, and what was around it.
    exact: String,
    prefix: String,
    suffix: String,
}

fuzz_target!(|input: Input| {
    // The flattening on its own, over every file and both readings of it.
    for (path, text) in input.files.iter().take(8) {
        for html in [false, true] {
            check_flat(text, html);
        }
        // And the name decides which, so that path is walked too.
        let _ = is_html(path);
    }

    let files: Vec<Candidate<'_>> = input
        .files
        .iter()
        .take(8)
        .enumerate()
        .map(|(nth, (path, text))| Candidate {
            // The ids are distinct so that a range can be traced back to the
            // file it names; a document's ids are distinct too.
            file_id: match nth {
                0 => "f0",
                1 => "f1",
                2 => "f2",
                3 => "f3",
                4 => "f4",
                5 => "f5",
                6 => "f6",
                _ => "f7",
            },
            path,
            text,
        })
        .collect();

    let quote = Quote {
        exact: &input.exact,
        prefix: &input.prefix,
        suffix: &input.suffix,
    };
    let Ok(found) = locate(&files, &quote) else {
        // A refusal is an answer, and the only one that is ever allowed to be
        // uncertain. Nothing else to check.
        return;
    };

    let file = files
        .iter()
        .find(|candidate| candidate.file_id == found.file_id.0)
        .expect("a range names a file of this document");
    let units: Vec<u16> = file.text.encode_utf16().collect();
    let start = found.start_utf16 as usize;
    let end = found.end_utf16 as usize;
    assert!(
        start <= end && end <= units.len(),
        "a range came back outside the file it names: {start}..{end} of {}",
        units.len()
    );
    assert_eq!(
        String::from_utf16_lossy(&units[start..end]),
        found.exact,
        "the quoted text is not the text at its own range"
    );
    // The context kept beside a range is the source on either side of it,
    // cut at the same place the module cuts it: a quarter of a line each way,
    // which is also what a client may send. Decoded the same way on both
    // sides, because a cut can land between the halves of a surrogate pair
    // and what comes back then is a replacement character on both sides.
    const CONTEXT: usize = 64;
    assert_eq!(
        String::from_utf16_lossy(&units[start.saturating_sub(CONTEXT)..start]),
        found.prefix,
        "the prefix kept is not the source before the range"
    );
    assert_eq!(
        String::from_utf16_lossy(&units[end..(end + CONTEXT).min(units.len())]),
        found.suffix,
        "the suffix kept is not the source after the range"
    );
});

/// Flattening a file gives a map into it that actually points at it.
fn check_flat(text: &str, html: bool) {
    let flat = flatten(text, html);
    let total = text.encode_utf16().count() as u32;

    assert_eq!(
        flat.from.len(),
        flat.chars.len() + 1,
        "a flattening lost its one-past-the-end offset"
    );
    assert_eq!(
        flat.from.last().copied(),
        Some(total),
        "a flattening does not end at the end of the file"
    );
    let mut previous = 0;
    for offset in &flat.from {
        assert!(*offset >= previous, "a flattening's offsets went backwards");
        assert!(
            *offset <= total,
            "a flattening points past the end of the file"
        );
        previous = *offset;
    }
    // A run of whitespace is one space. Without this a selection made on the
    // page, where the browser has already collapsed the run, would not match
    // the source it came from.
    for pair in flat.chars.windows(2) {
        assert!(
            !(pair[0] == ' ' && pair[1] == ' '),
            "a flattening left two spaces in a row"
        );
    }
}
