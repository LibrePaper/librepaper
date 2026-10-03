// The shell is compiled in from a directory, so a file added to it has to
// trigger a rebuild even though no Rust source changed.
//
// The wasm renderers are not in it: their pinned digests are compiled in from
// assets.lock, so a change to that file is a change to the binary.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-env-changed=LIBREPAPER_VERSION");
    println!("cargo:rerun-if-env-changed=SQLX_OFFLINE");
    // A packaged crate is built outside this repository's `.cargo/config.toml`.
    // Keep SQLx query macros offline there, while allowing an explicit caller
    // setting such as `SQLX_OFFLINE=false` to take precedence.
    if std::env::var_os("SQLX_OFFLINE").is_none() {
        println!("cargo:rustc-env=SQLX_OFFLINE=true");
    }
    println!(
        "cargo:rerun-if-changed={}",
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets.lock")
            .display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.sqlx")
            .display()
    );
    let skills = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills");
    println!("cargo:rerun-if-changed={}", skills.display());
    watch(&skills);

    let shell = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/dist");
    // Every file, not just the directory: cargo compares the timestamp of what
    // it is told to watch, and editing a file inside a directory does not
    // change the directory. Naming the directory alone means a changed page or
    // bundle is compiled in only when something else happens to force a
    // rebuild -- which is a stale binary that looks like a working one.
    watch(&shell);
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
