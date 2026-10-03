// A packaged crate is built outside this repository's `.cargo/config.toml`.
// Keep SQLx query macros offline there, while allowing an explicit caller
// setting such as `SQLX_OFFLINE=false` to take precedence.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-env-changed=SQLX_OFFLINE");
    if std::env::var_os("SQLX_OFFLINE").is_none() {
        println!("cargo:rustc-env=SQLX_OFFLINE=true");
    }
    println!(
        "cargo:rerun-if-changed={}",
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(".sqlx")
            .display()
    );
}
