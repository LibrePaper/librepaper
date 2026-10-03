// The skills are compiled in from a directory, so a file added to it has to
// trigger a rebuild even though no Rust source changed.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-env-changed=LIBREPAPER_VERSION");
    // The skills bundle lives outside this crate, so the path comes from here;
    // the flat crate's build.rs sets the same variable to its own skills/.
    let skills_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../skills")
        .canonicalize()
        .expect("skills/ is at the repository root");
    println!("cargo:rustc-env=LIBREPAPER_SKILLS={}", skills_dir.display());
    let skills = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills");
    println!("cargo:rerun-if-changed={}", skills.display());
    watch(&skills);
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
