//! The helper's wire version: the semver that the page and the desktop app
//! negotiate over. It is separate from the release version. See
//! `docs/dev/protocol/local.md` and `SPEC-wire.md`.

/// Breaking changes go up here.
pub const MAJOR: u32 = 3;
/// Additions go up here, and reset with the major.
pub const MINOR: u32 = 1;
/// Fixes that a page must rely on go up here, and reset with the major or minor.
pub const PATCH: u32 = 0;
/// The oldest major this binary still answers.
pub const OLDEST_MAJOR: u32 = 3;

/// The helper's wire version as `MAJOR.MINOR.PATCH`.
pub fn version() -> String {
    format!("{MAJOR}.{MINOR}.{PATCH}")
}

/// The majors this binary answers, oldest first.
pub fn protocol_versions() -> Vec<u32> {
    (OLDEST_MAJOR..=MAJOR).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_files_agree_with_the_constants() {
        let oldest =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/wire/OLDEST_MAJOR"))
                .expect("wire/OLDEST_MAJOR");
        assert_eq!(oldest.trim(), OLDEST_MAJOR.to_string());

        let path = format!("{}/wire/v{}/VERSION", env!("CARGO_MANIFEST_DIR"), MAJOR);
        let current = std::fs::read_to_string(&path).expect("wire/v{MAJOR}/VERSION");
        assert_eq!(current.trim(), version());
    }
}
