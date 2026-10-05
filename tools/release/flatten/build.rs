// The build script of the generated single crate (target/flat/), copied to its
// root by tools/release/flatten/flatten. It is the UNION of the sub-crates' build
// scripts: every sub-crate build.rs must have its counterpart here, and every
// `cargo:rustc-env=NAME` a sub-crate emits must be emitted here too.
// `tools/release/flatten/flatten lint` (rule 8) fails when one is missing.
//
// Inputs are read from the flat package's own staged directories, so every
// path is relative to this crate's root:
//
// - dist/       the compiled-in shell (staged from the shell crate)
// - assets.lock pinned wasm digests, copied into OUT_DIR for include_str!
// - .sqlx/      the offline query cache
// - docs/       protocol docs and tutorials, compiled in through LIBREPAPER_DOCS

use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    println!("cargo:rerun-if-env-changed=LIBREPAPER_VERSION");
    println!("cargo:rerun-if-env-changed=SQLX_OFFLINE");
    // The companion crate's build.rs derives the same value.
    let version = std::env::var("CARGO_PKG_VERSION").expect("cargo sets the package version");
    println!("cargo:rustc-env=LIBREPAPER_BUILD_VERSION=v{version}");
    // The server crate's metrics label: the bare package version.
    println!("cargo:rustc-env=LIBREPAPER_PKG_VERSION={}", std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION"));
    // A packaged crate is built outside this repository's `.cargo/config.toml`.
    // Keep SQLx query macros offline there, while allowing an explicit caller
    // setting such as `SQLX_OFFLINE=false` to take precedence.
    if std::env::var_os("SQLX_OFFLINE").is_none() {
        println!("cargo:rustc-env=SQLX_OFFLINE=true");
    }

    // The shell crate's build.rs sets the same variable to its own dist/.
    let dist = root.join("dist");
    println!("cargo:rustc-env=LIBREPAPER_SHELL_DIST={}", dist.display());

    // The facade's build.rs sets the same variable to the repository's docs/.
    let docs = root.join("docs");
    println!("cargo:rustc-env=LIBREPAPER_DOCS={}", docs.display());

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
    let sqlx_dir = root.join(".sqlx");
    if sqlx_dir.exists() {
        println!("cargo:rerun-if-changed={}", sqlx_dir.display());
        watch(&sqlx_dir);
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
