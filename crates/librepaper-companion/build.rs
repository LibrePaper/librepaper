fn main() {
    println!("cargo:rerun-if-env-changed=LIBREPAPER_VERSION");
    let version = std::env::var("CARGO_PKG_VERSION").expect("cargo sets the package version");
    println!("cargo:rustc-env=LIBREPAPER_BUILD_VERSION=v{version}");
}
