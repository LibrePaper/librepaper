// A packaged crate is built outside this repository's `.cargo/config.toml`.
// Keep SQLx query macros offline there, while allowing an explicit caller
// setting such as `SQLX_OFFLINE=false` to take precedence.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=SQLX_OFFLINE");
    if std::env::var_os("SQLX_OFFLINE").is_none() {
        println!("cargo:rustc-env=SQLX_OFFLINE=true");
    }
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir()
            .join(".sqlx")
            .display()
    );
}

/// Read at run time, not with `env!`: a build-script binary compiled in one
/// worktree is reused by another that shares the target directory, and the
/// compiled-in path then points at a checkout that may no longer exist.
fn manifest_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"))
}
