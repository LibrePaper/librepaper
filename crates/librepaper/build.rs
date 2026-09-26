// The shell is compiled in from a directory, so a file added to it -- a
// freshly built WebAssembly module -- has to trigger a rebuild even though no
// Rust source changed.
//
// Those files are build outputs rather than sources, and a binary
// without them starts and then fails on its first request. Better to say so
// here, where the fix is one command away, than to ship a binary that dies at
// startup.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-env-changed=LIBREPAPER_VERSION");
    let skills = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills");
    println!("cargo:rerun-if-changed={}", skills.display());
    watch(&skills);

    let shell = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/dist");
    // Every file, not just the directory: cargo compares the timestamp of what
    // it is told to watch, and editing a file inside a directory does not
    // change the directory. Naming the directory alone means a changed page or
    // renderer is compiled in only when something else happens to force a
    // rebuild -- which is a stale binary that looks like a working one.
    watch(&shell);
    for (file, how) in [
        ("wasm/markdown.wasm", "make wasm"),
        ("wasm/bibliography.wasm", "make wasm"),
        ("wasm/citations.wasm", "make wasm"),
        ("wasm/typst.wasm", "make wasm"),
    ] {
        if !shell.join(file).exists() {
            println!(
                "cargo:warning=web/dist/{file} is missing, and the binary needs it. Run: {how}"
            );
            std::process::exit(1);
        }
    }
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
