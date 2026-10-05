//! An update as it arrives on the socket, taken through the live import
//! path. The update is a real one -- the encoded state of a document a peer
//! built -- and then damaged the way a bad client or a corrupt frame would
//! damage it: truncated, bytes flipped, bytes dropped or added. So the
//! mutator works on the encoding's structure rather than on noise that never
//! decodes.
//!
//! This used to drive `session::admit_update`, a size-ceiling check the
//! server stopped calling when the log took over admission (§9.1 bounds the
//! log, not the document). Fuzzing it kept an implementation alive that
//! nothing shipped, and left the path bytes really do take -- decode,
//! import, project -- with no coverage at all. What is asserted now is what
//! that path promises: a refused update leaves the document exactly as it
//! was, an accepted one applies, and whatever comes out still projects
//! without panicking, because the very next thing the server does with an
//! imported document is read it.
#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use librepaper::session;

#[derive(Arbitrary, Debug)]
enum Damage {
    Truncate(u16),
    Flip { at: u16, bits: u8 },
    Drop(u16),
    Add { at: u16, byte: u8 },
}

#[derive(Arbitrary, Debug)]
struct Input {
    /// The files the peer's document holds.
    files: Vec<(String, String)>,
    /// What to do to the encoding of it.
    damage: Vec<Damage>,
}

fuzz_target!(|input: Input| {
    // Loro's `import` returns a `Result` instead of panicking on malformed
    // input, so `session::apply_update` handles errors cleanly. A panic hook
    // is still set for any unexpected panics from the fuzzer infrastructure;
    // a panic that escapes still aborts at the C boundary and is a crash.
    static HOOK: std::sync::Once = std::sync::Once::new();
    HOOK.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            eprintln!("{info}\n{}", std::backtrace::Backtrace::capture());
        }))
    });

    // The peer's document, encoded whole.
    let theirs = session::new_doc();
    for (path, body) in input.files.iter().take(8) {
        session::put_text(&theirs, path, body);
    }
    let mut update = session::encode_state(&theirs);
    for damage in input.damage.iter().take(16) {
        if update.is_empty() {
            break;
        }
        match *damage {
            Damage::Truncate(at) => update.truncate(usize::from(at) % update.len()),
            Damage::Flip { at, bits } => {
                let at = usize::from(at) % update.len();
                update[at] ^= bits;
            }
            Damage::Drop(at) => {
                update.remove(usize::from(at) % update.len());
            }
            Damage::Add { at, byte } => update.insert(usize::from(at) % (update.len() + 1), byte),
        }
    }

    // The room's document, with something already in it.
    let doc = session::new_doc();
    let id = session::put_text(&doc, "main.typ", "= Title\n\nA paragraph of prose.");
    session::set_main(&doc, &id);
    session::put_text(&doc, "chapters/one.typ", "one");
    let before = session::encode_state(&doc);

    // `decode_update` is the socket's first act on the bytes: it rehearses
    // the import somewhere that is not the live document, so that a frame
    // which cannot be read never touches the text every reader is looking
    // at. Its answer must agree with what applying them actually does.
    let decoded = session::decode_update(&update).is_ok();
    match session::apply_update(&doc, &update) {
        Ok(()) => {
            assert!(
                decoded,
                "an update that applied was refused by the rehearsal that guards the document"
            );
            // The projection is the next thing the server reads, and it
            // reads whatever the peer's bytes just put there.
            let _ = session::paths_of(&doc);
            let _ = session::text_of(&doc);
            let _ = session::main_path(&doc);
        }
        Err(_) => {
            assert!(!decoded, "the rehearsal admitted bytes the import refused");
            assert_eq!(
                session::encode_state(&doc),
                before,
                "a refused update changed the document"
            );
        }
    }

    // And a fresh document accepts or refuses it without panicking.
    let _ = session::apply_update(&session::new_doc(), &update);
});
