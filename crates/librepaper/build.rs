// The built browser app and the pinned wasm digests are compiled into
// librepaper-shell, which watches them itself. The docs path is set by
// librepaper-server's build script.

fn main() {
    println!("cargo:rerun-if-env-changed=LIBREPAPER_VERSION");
}
