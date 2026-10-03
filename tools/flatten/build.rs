// The build script of the generated single crate (target/flat/), copied to its
// root by tools/flatten/flatten. It is the UNION of the sub-crates' build
// scripts: every sub-crate build.rs must have its counterpart here, and every
// `cargo:rustc-env=NAME` a sub-crate emits must be emitted here too.
// `tools/flatten/flatten lint` (rule 8) fails when one is missing.
//
// Inputs are read from the flat package's own staged directories, so every
// path is relative to this crate's root:
//
// - dist/       the compiled-in shell (staged from the shell crate)
// - assets.lock pinned wasm digests, copied into OUT_DIR for include_str!
// - .sqlx/      the offline query cache
// - skills/     the compiled-in skills bundle

use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    println!("cargo:rerun-if-env-changed=LIBREPAPER_VERSION");
    println!("cargo:rerun-if-env-changed=SQLX_OFFLINE");
    // A packaged crate is built outside this repository's `.cargo/config.toml`.
    // Keep SQLx query macros offline there, while allowing an explicit caller
    // setting such as `SQLX_OFFLINE=false` to take precedence.
    if std::env::var_os("SQLX_OFFLINE").is_none() {
        println!("cargo:rustc-env=SQLX_OFFLINE=true");
    }

    // The shell crate's build.rs sets the same variable to its own dist/.
    let dist = root.join("dist");
    println!("cargo:rustc-env=LIBREPAPER_SHELL_DIST={}", dist.display());

    // The shell crate's build.rs does the same copy: the pinned digests are
    // compiled in, so a change to the file is a change to the binary.
    let lock = root.join("assets.lock");
    println!("cargo:rerun-if-changed={}", lock.display());
    let out = std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR");
    std::fs::copy(&lock, Path::new(&out).join("assets.lock"))
        .expect("assets.lock is staged next to Cargo.toml");

    // A directory is watched file by file: cargo compares the timestamp of
    // what it is told to watch, and editing a file inside a directory does not
    // change the directory.
    for name in [".sqlx", "skills"] {
        let dir = root.join(name);
        if dir.exists() {
            println!("cargo:rerun-if-changed={}", dir.display());
            watch(&dir);
        }
    }
    watch(&dist);
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
