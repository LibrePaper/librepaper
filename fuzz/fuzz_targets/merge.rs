//! The word diff and the three-way merge, over any three strings. The
//! invariants are the ones `crates/text/src/tests.rs` checks over its random
//! token soups; the difference is the generator, which here produces every
//! string the mutator can reach: combining marks, astral characters, lone
//! whitespace of every kind, texts of some kilobytes.
#![no_main]

use librepaper_fuzz::{partition, round_trip};
use wasm_helpers::text::merge;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: (&str, &str, &str)| {
    let (base, local, remote) = input;
    partition(base);
    partition(local);
    round_trip(base, local);
    round_trip(local, base);

    let merged = merge(base, local, remote);
    // The session is edited towards the merged text, so that diff must apply.
    round_trip(remote, &merged.text);
    let units: Vec<u16> = merged.text.encode_utf16().collect();
    for clash in &merged.conflicts {
        assert!(
            clash.at + clash.len <= units.len(),
            "a conflict runs off the end of the merged text"
        );
        let region = String::from_utf16(&units[clash.at..clash.at + clash.len])
            .expect("a conflict splits a surrogate pair");
        assert_eq!(
            region, clash.remote,
            "a conflict does not name its own region"
        );
    }

    // When one side did not move, the other side is simply right.
    let unmoved = merge(base, local, base);
    assert_eq!(
        unmoved.text, local,
        "the session did not move, the file is right"
    );
    assert!(unmoved.conflicts.is_empty());
    let untouched = merge(base, base, remote);
    assert_eq!(
        untouched.text, remote,
        "the file did not move, the session is right"
    );
    assert!(untouched.conflicts.is_empty());
});
