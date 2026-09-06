//! A v1 update as it arrives on the socket, checked against the admission
//! ceilings. The update is a real one -- the encoded state of a document a
//! peer built -- and then damaged the way a bad client or a corrupt frame
//! would damage it: truncated, bytes flipped, bytes dropped or added. So the
//! mutator works on the encoding's structure rather than on noise that never
//! decodes.
//!
//! What is asserted: a refusal of any kind leaves the document exactly as it
//! was, and an admitted update applies.
#![no_main]

use arbitrary::Arbitrary;
use komodoc::session::{self, Admission};
use libfuzzer_sys::fuzz_target;

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
    /// The ceilings, small so that the scratch-copy path is reached often.
    ceiling: u16,
    max_files: u8,
}

fuzz_target!(|input: Input| {
    // libFuzzer's harness aborts the process from the panic hook, before any
    // `catch_unwind` runs, and `session::decode` catches the panics yrs makes
    // on a damaged update. A hook that only prints leaves the catch working;
    // a panic that does escape still aborts at the C boundary and is a crash.
    static HOOK: std::sync::Once = std::sync::Once::new();
    HOOK.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            eprintln!("{info}\n{}", std::backtrace::Backtrace::capture());
        }))
    });

    // The peer's document, encoded whole.|input: Input| {
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

    let ceiling = usize::from(input.ceiling);
    let max_files = usize::from(input.max_files);
    match session::admit_update(&doc, &update, ceiling, max_files) {
        Admission::Fits => {
            session::apply_update(&doc, &update).expect("an admitted update applies");
        }
        Admission::Malformed | Admission::TooLarge | Admission::TooMany => {
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
