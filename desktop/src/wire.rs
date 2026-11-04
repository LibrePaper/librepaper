//! The helper's wire version as the shell publishes it to the page. The
//! tests keep these equal to the helper's own copy in
//! `crates/librepaper-companion/wire`.

/// The full version of the helper's wire, as the page global reports it.
pub const VERSION: &str = "3.1.0";
/// The major version of `VERSION`. The shell asks the helper for this major.
pub const MAJOR: u32 = 3;
/// The oldest major the page is told this shell supports.
pub const OLDEST_MAJOR: u32 = 3;

#[cfg(test)]
mod tests {
    use super::{MAJOR, OLDEST_MAJOR, VERSION};

    // Reads a file of the helper's wire, relative to the desktop crate.
    fn read(path: &str) -> String {
        let full = format!("{}/{path}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(&full)
            .unwrap_or_else(|err| panic!("could not read {full}: {err}"))
            .trim()
            .to_string()
    }

    #[test]
    fn the_shell_matches_the_helper_wire() {
        let oldest: u32 = read("../crates/librepaper-companion/wire/OLDEST_MAJOR")
            .parse()
            .expect("OLDEST_MAJOR is a number");
        assert_eq!(oldest, OLDEST_MAJOR);
        let version = read(&format!(
            "../crates/librepaper-companion/wire/v{MAJOR}/VERSION"
        ));
        assert_eq!(version, VERSION);
        assert!(VERSION.starts_with(&format!("{MAJOR}.")));
    }
}
