// The protocol docs and tutorials are compiled in with include_str!. They
// live outside this crate, so the path comes from here; the flat crate's
// build.rs sets the same variable to its own docs/. rustc tracks the
// included files itself.

use std::path::Path;

fn main() {
    let docs = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs")
        .canonicalize()
        .expect("docs/ is at the repository root");
    println!("cargo:rustc-env=LIBREPAPER_DOCS={}", docs.display());
    // The skills bundle, which routes.rs includes by path. The companion
    // compiles the whole directory in and watches it itself.
    let skills_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../skills")
        .canonicalize()
        .expect("skills/ is at the repository root");
    println!("cargo:rustc-env=LIBREPAPER_SKILLS={}", skills_dir.display());
    // The bare package version for the metrics build_info label. A sub-crate
    // may not read CARGO_PKG_VERSION itself (flatten lint rule 4); its build
    // script may, and the flat build.rs emits the same variable.
    let version = std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
    println!("cargo:rustc-env=LIBREPAPER_PKG_VERSION={version}");
}
