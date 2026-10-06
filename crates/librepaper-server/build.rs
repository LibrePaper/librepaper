// The protocol docs and tutorials are compiled in with include_str!. They
// live outside this crate, so the path comes from here. rustc tracks the
// included files itself.

use std::path::PathBuf;

fn main() {
    let docs = manifest_dir()
        .join("../../docs")
        .canonicalize()
        .expect("docs/ is at the repository root");
    println!("cargo:rustc-env=LIBREPAPER_DOCS={}", docs.display());
}

/// Read at run time, not with `env!`: a build-script binary compiled in one
/// worktree is reused by another that shares the target directory, and the
/// compiled-in path then points at a checkout that may no longer exist.
fn manifest_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"))
}
