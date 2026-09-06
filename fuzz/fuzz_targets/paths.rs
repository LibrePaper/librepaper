//! The path rules, over any string a peer could put in the shared document's
//! `paths` map or send to a route. Nothing here may panic, normalising is
//! idempotent, and a name that passes keeps passing once it is stored.
#![no_main]

use komodoc::paths::{check, collision_key, kind_of, normalise, placeholder, suffixed};
use komodoc_fuzz::{configuration, rules};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: (&str, u8)| {
    let (path, nth) = input;
    let config = configuration();
    let rules = rules(&config);

    let once = normalise(path);
    assert_eq!(
        normalise(&once),
        once,
        "normalising twice is not normalising once"
    );
    assert_eq!(collision_key(path), collision_key(&once));

    // None of these may panic, whatever the bytes.
    let _ = kind_of(&rules, path);
    let _ = placeholder(path);
    let _ = suffixed(path, usize::from(nth));

    if let Ok(kind) = check(&rules, path) {
        // What is stored is the normalised form, and it is checked again by
        // every route and by the repair pass: it has to pass the same way.
        assert_eq!(
            check(&rules, &once),
            Ok(kind),
            "a stored path stops passing"
        );
        assert_eq!(kind_of(&rules, &once), Ok(kind));
        // Moving a colliding file aside keeps its kind, and the result is a
        // path the rules accept unless it grew past the length limit.
        let aside = suffixed(&once, usize::from(nth));
        assert_eq!(
            kind_of(&rules, &aside),
            Ok(kind),
            "a suffix changed the kind"
        );
        assert!(
            check(&rules, &aside).is_ok() || aside.len() > rules.max_path,
            "{aside:?}: moving a file aside made it a name the rules refuse"
        );
    }
});
