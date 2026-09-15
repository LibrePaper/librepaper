//! What a document costs to keep.
//!
//! SPEC-loro.md §7 rests on measurements, and measurements nobody can reproduce
//! are assertions with numbers in them. This is the harness behind them.
//!
//! Three questions, in the order §7 asks them: which export mode a stored base
//! should use (§7.1), whether keeping every keystroke is affordable and whether
//! thinning would help (§7.2), and what it costs to read a document back
//! (§7.3).
//!
//! The typing is character by character, because that is how a paper is
//! actually written and it is the case a CRDT is worst at: a thousand
//! keystrokes are a thousand operations where one paste is one.

use std::borrow::Cow;
use std::time::Instant;

use loro::{ExportMode, LoroDoc};

/// Prose to type, read from the repository's own documentation.
///
/// It matters that this is real writing. An earlier version of this harness
/// typed one paragraph over and over, and 300 KB of it compressed to 228 bytes
/// -- which made every ratio below meaningless, because what was measured was
/// the repetition rather than the format.
fn prose() -> String {
    let docs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs");
    let mut files: Vec<_> = std::fs::read_dir(&docs)
        .expect("the docs directory is two levels above this crate")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|kind| kind == "md"))
        .collect();
    files.sort();
    let mut text = String::new();
    for path in files {
        if let Ok(body) = std::fs::read_to_string(&path) {
            text.push_str(&body);
        }
    }
    assert!(!text.is_empty(), "no prose to measure");
    text
}

fn zstd3(bytes: &[u8]) -> usize {
    zstd::stream::encode_all(bytes, 3)
        .expect("prose compresses")
        .len()
}

/// A document typed one character at a time, by `authors` taking turns.
fn typed(source: &str, chars: usize, authors: u64) -> LoroDoc {
    let doc = LoroDoc::new();
    let text = doc.get_text("main.tex");
    let body: String = source.chars().cycle().take(chars).collect();
    for (n, character) in body.chars().enumerate() {
        if authors > 1 && n % 64 == 0 {
            doc.set_peer_id((n as u64 / 64) % authors + 1)
                .expect("a peer id");
        }
        let at = text.len_utf16();
        text.insert_utf16(at, &character.to_string())
            .expect("a keystroke lands");
        // Committed in runs, as an editor does. Committing every character
        // would measure the commit rather than the typing.
        if n % 32 == 0 {
            doc.commit();
        }
    }
    doc.commit();
    doc
}

fn history_of(doc: &LoroDoc) -> Vec<u8> {
    doc.export(ExportMode::Updates {
        from: Cow::Owned(Default::default()),
    })
    .expect("a document exports")
}

fn main() {
    let source = prose();
    println!("prose: {} characters of the repository's own docs\n", source.chars().count());

    println!("== 7.1 export modes, one author, 300 KB");
    let doc = typed(&source, 300_000, 1);
    let history = history_of(&doc);
    let snapshot = doc.export(ExportMode::Snapshot).expect("a snapshot");
    let plain = doc.get_text("main.tex").to_string();
    for (label, bytes) in [
        ("plain text", plain.as_bytes()),
        ("updates (chosen)", history.as_slice()),
        ("snapshot", snapshot.as_slice()),
    ] {
        println!(
            "  {label:18} raw {:>9}  zstd-3 {:>9}",
            bytes.len(),
            zstd3(bytes)
        );
    }

    println!("\n== 7.2 every keystroke kept, against thinning it");
    for (label, chars, authors) in [
        ("drafted, 1 author", 100_000, 1),
        ("revised, 3 authors", 100_000, 3),
    ] {
        let doc = typed(&source, chars, authors);
        let kept = zstd3(&history_of(&doc));
        // Thinning means what a shallow snapshot is: the state, with the
        // history behind it discarded.
        let thinned = zstd3(
            &doc.export(ExportMode::ShallowSnapshot(Cow::Owned(doc.state_frontiers())))
                .expect("a shallow snapshot"),
        );
        println!(
            "  {label:20} history {:>8}  thinned {:>8}  -> {}",
            kept,
            thinned,
            if kept < thinned {
                "history is smaller"
            } else {
                "thinning is smaller"
            }
        );
    }

    println!("\n== 7.3 what it costs to read one back");
    let doc = typed(&source, 176_000, 1);
    let history = history_of(&doc);
    let started = Instant::now();
    let rebuilt = LoroDoc::new();
    rebuilt.import(&history).expect("a history imports");
    println!(
        "  cold start         {:>8.2} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
    let frontiers = doc.state_frontiers();
    let started = Instant::now();
    let _ = doc.fork_at(&frontiers).expect("an old version is reachable");
    println!(
        "  checkout a version {:>8.2} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
}
