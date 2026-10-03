// The built browser app and the pinned wasm digests are compiled into
// librepaper-shell, which watches them itself. The docs and skills paths are
// set by librepaper-server's and librepaper-companion's build scripts.

fn main() {
    println!("cargo:rerun-if-env-changed=LIBREPAPER_VERSION");
}
