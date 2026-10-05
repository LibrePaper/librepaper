// The protocol docs and tutorials are compiled in with include_str!. They
// live outside this crate, so the path comes from here. rustc tracks the
// included files itself.

use std::path::Path;

fn main() {
    let docs = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs")
        .canonicalize()
        .expect("docs/ is at the repository root");
    println!("cargo:rustc-env=LIBREPAPER_DOCS={}", docs.display());
}
