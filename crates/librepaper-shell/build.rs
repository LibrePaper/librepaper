// The shell is compiled in from web/dist, so a file added to it has to trigger
// a rebuild even though no Rust source changed. The directory reaches the code
// through LIBREPAPER_SHELL_DIST, so the source does not depend on the layout.
//
// The wasm renderers are not in it: their pinned digests are compiled in from
// assets.lock, so a change to that file is a change to the binary.

use std::path::{Path, PathBuf};

fn main() {
    let root = manifest_dir().join("../..");
    let dist = root.join("web/dist");
    let dist = dist.canonicalize().unwrap_or(dist);
    println!("cargo:rustc-env=LIBREPAPER_SHELL_DIST={}", dist.display());
    println!("cargo:rerun-if-changed={}", dist.display());
    // Every file, not just the directory: cargo compares the timestamp of what
    // it is told to watch, and editing a file inside a directory does not
    // change the directory. Naming the directory alone means a changed page or
    // bundle is compiled in only when something else happens to force a
    // rebuild -- which is a stale binary that looks like a working one.
    watch(&dist);

    let lock = root.join("assets.lock");
    println!("cargo:rerun-if-changed={}", lock.display());
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    std::fs::copy(&lock, out.join("assets.lock")).expect("assets.lock is at the repository root");
}

/// Tells cargo to rebuild when any file under this directory changes.
fn watch(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        println!("cargo:rerun-if-changed={}", path.display());
        if path.is_dir() {
            watch(&path);
        }
    }
}

/// Read at run time, not with `env!`: a build-script binary compiled in one
/// worktree is reused by another that shares the target directory, and the
/// compiled-in path then points at a checkout that may no longer exist.
fn manifest_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"))
}
