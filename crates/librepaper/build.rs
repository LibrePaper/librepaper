// The built browser app and the pinned wasm digests are compiled into
// librepaper-shell, which watches them itself.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-env-changed=LIBREPAPER_VERSION");
    println!("cargo:rerun-if-env-changed=SQLX_OFFLINE");
    // The protocol docs and tutorials are compiled in with include_str!. They
    // live outside this crate, so the path comes from here; the flat crate's
    // build.rs sets the same variable to its own docs/. rustc tracks the
    // included files itself.
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
    // A packaged crate is built outside this repository's `.cargo/config.toml`.
    // Keep SQLx query macros offline there, while allowing an explicit caller
    // setting such as `SQLX_OFFLINE=false` to take precedence.
    if std::env::var_os("SQLX_OFFLINE").is_none() {
        println!("cargo:rustc-env=SQLX_OFFLINE=true");
    }
    println!(
        "cargo:rerun-if-changed={}",
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.sqlx")
            .display()
    );
}
