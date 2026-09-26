//! A catalog v3 source archive: the zstd/tar/JSON container a version's bytes
//! travel in. A peer never writes one by hand, but the bytes come back off
//! object storage and out of the database, and `decode` is the only thing
//! standing between those bytes and a directory written to disk. So it is
//! read here the way the update target reads a v1 update: an honest archive
//! built from whatever the mutator shaped, then damaged, then decoded.
//!
//! What is asserted:
//!
//! * nothing panics, whatever the bytes;
//! * an archive that encodes decodes back to the same archive;
//! * encoding is deterministic, so a version is named by what it says --
//!   re-encoding a decoded archive gives the same bytes and the same digest;
//! * every path that survives a decode is relative and free of `..`, so
//!   joining one under a project root stays under it;
//! * a decode honours the ceilings it was given.
#![no_main]

use std::path::{Component, Path};

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use librepaper::source_archive::{decode, encode, ArchiveLimits, SourceArchive, SourceFile};

#[derive(Arbitrary, Debug)]
enum File {
    Inline {
        path: String,
        body: String,
    },
    Asset {
        path: String,
        asset: u128,
        digest: [u8; 32],
        bytes: u64,
        media_type: String,
    },
}

#[derive(Arbitrary, Debug)]
enum Damage {
    Truncate(u16),
    Flip { at: u16, bits: u8 },
    Drop(u16),
    Add { at: u16, byte: u8 },
}

#[derive(Arbitrary, Debug)]
struct Input {
    /// Every format, so that the four the rules refuse are reached too.
    format: String,
    main_path: String,
    files: Vec<File>,
    damage: Vec<Damage>,
    /// The ceilings, small so that the limit paths are reached often.
    inline_file_bytes: u16,
    source_bytes: u16,
    archive_bytes: u16,
    max_files: u8,
}

fuzz_target!(|input: Input| {
    let limits = ArchiveLimits {
        inline_file_bytes: usize::from(input.inline_file_bytes),
        source_bytes: usize::from(input.source_bytes),
        archive_bytes: usize::from(input.archive_bytes).max(1024),
        files: usize::from(input.max_files),
    };

    let archive = SourceArchive {
        source_format: input.format.clone(),
        main_path: input.main_path.clone(),
        files: input
            .files
            .iter()
            .take(16)
            .map(|file| match file {
                File::Inline { path, body } => SourceFile::Inline {
                    path: path.clone(),
                    bytes: body.clone().into_bytes(),
                },
                File::Asset {
                    path,
                    asset,
                    digest,
                    bytes,
                    media_type,
                } => SourceFile::Asset {
                    path: path.clone(),
                    asset_id: uuid::Uuid::from_u128(*asset),
                    digest: *digest,
                    bytes: *bytes,
                    media_type: media_type.clone(),
                },
            })
            .collect(),
    };

    let Ok(encoded) = encode(archive, limits) else {
        // Refused: nothing was written, and there is nothing to decode.
        return;
    };
    assert!(
        encoded.bytes.len() <= limits.archive_bytes,
        "an encoding came back over the ceiling it was given"
    );

    // What was encoded decodes, and to the same archive.
    let decoded = decode(&encoded.bytes, limits).expect("what encodes decodes");
    check(&decoded, limits);

    // And it encodes again to the same bytes: a version is named by its
    // digest, so two servers holding the same files have to agree on them.
    let again = encode(
        SourceArchive {
            source_format: decoded.source_format.clone(),
            main_path: decoded.main_path.clone(),
            files: decoded.files.clone(),
        },
        limits,
    )
    .expect("a decoded archive re-encodes");
    assert_eq!(again.bytes, encoded.bytes, "encoding is not deterministic");
    assert_eq!(again.digest, encoded.digest, "the digest moved");
    assert_eq!(again.logical_bytes, encoded.logical_bytes);

    // Now damaged the way a truncated upload or a flipped bit damages it.
    let mut bytes = encoded.bytes;
    for damage in input.damage.iter().take(16) {
        if bytes.is_empty() {
            break;
        }
        match *damage {
            Damage::Truncate(at) => bytes.truncate(usize::from(at) % bytes.len()),
            Damage::Flip { at, bits } => {
                let at = usize::from(at) % bytes.len();
                bytes[at] ^= bits;
            }
            Damage::Drop(at) => {
                bytes.remove(usize::from(at) % bytes.len());
            }
            Damage::Add { at, byte } => bytes.insert(usize::from(at) % (bytes.len() + 1), byte),
        }
    }
    // Damage is refused or it is not, but what comes back is an archive that
    // holds to every rule an undamaged one holds to.
    if let Ok(damaged) = decode(&bytes, limits) {
        check(&damaged, limits);
    }
});

/// What is true of any archive a decode hands back, however the bytes arrived.
fn check(archive: &SourceArchive, limits: ArchiveLimits) {
    assert!(
        !archive.files.is_empty() && archive.files.len() <= limits.files,
        "a decoded archive is outside the file-count ceiling"
    );

    let mut seen = std::collections::HashSet::new();
    let mut inline_total = 0_u64;
    for file in &archive.files {
        let path = file.path();
        assert!(seen.insert(path), "a decoded archive names {path:?} twice");
        // The one that matters: this path is about to be joined under a
        // project root and written.
        let parsed = Path::new(path);
        assert!(
            !parsed.is_absolute()
                && parsed
                    .components()
                    .all(|part| matches!(part, Component::Normal(_))),
            "a decoded archive holds an escaping path {path:?}"
        );
        assert!(!path.is_empty() && path.len() <= 4096);
        if let SourceFile::Inline { bytes, .. } = file {
            std::str::from_utf8(bytes).expect("a decoded inline file is UTF-8");
            inline_total = inline_total.saturating_add(bytes.len() as u64);
        }
    }
    assert!(
        inline_total <= limits.source_bytes as u64,
        "a decoded archive carries more inline source than the ceiling allows"
    );
    assert!(
        seen.contains(archive.main_path.as_str()),
        "a decoded archive's main file is not in it"
    );
    assert!(
        matches!(
            archive.source_format.as_str(),
            "markdown" | "html" | "typst" | "latex" | "quarto"
        ),
        "a decoded archive names an unknown format {:?}",
        archive.source_format
    );
}
