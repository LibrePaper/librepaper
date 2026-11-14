//! The stored results Calepin leaves beside a Typst document.
//!
//! `calepin compile` runs the code chunks of a `.typ` file and writes its
//! runtime files, `results.json` and figures into `.calepin/` beside the
//! document. The browser's own Typst compiler draws those stored results, so
//! the folder is read here and sent back as one JSON document. The walk never
//! follows a symlink, because the folder can sit beside links into the
//! person's Run directory. `config.toml` stays on the computer, and the two
//! cache markers are Calepin's own bookkeeping.

use std::collections::BTreeMap;
use std::path::Path;

use base64::Engine as _;
use serde_json::json;

const DIR: &str = ".calepin";
const MAX_DEPTH: usize = 8;
const MAX_FILES: usize = 2000;
const MAX_BYTES: u64 = 16 * 1024 * 1024;
const SKIPPED: [&str; 3] = ["fingerprint.xxh3", "expansion.json", "config.toml"];

/// The `.calepin/` folder beside `entrypoint` as one JSON document, or `None`
/// when there is no run to send: no folder, a folder that is a symlink or not
/// a directory, a limit exceeded, an I/O error, or no `results.json`.
pub(super) fn results(root: &Path, entrypoint: &str) -> Option<Vec<u8>> {
    let entry_dir = Path::new(entrypoint).parent().unwrap_or(Path::new(""));
    let dir = root.join(entry_dir).join(DIR);
    let metadata = std::fs::symlink_metadata(&dir).ok()?;
    if !metadata.is_dir() {
        return None;
    }
    let mut walk = Walk::default();
    collect(&dir, "", 0, &mut walk)?;
    if !walk.files.keys().any(|path| path.ends_with("/results.json")) {
        return None;
    }
    let files: Vec<_> = walk
        .files
        .into_iter()
        .map(|(path, bytes)| match String::from_utf8(bytes) {
            Ok(text) => json!({ "path": path, "text": text }),
            Err(error) => json!({
                "path": path,
                "base64": base64::engine::general_purpose::STANDARD.encode(error.as_bytes()),
            }),
        })
        .collect();
    serde_json::to_vec(&json!({ "version": 1, "files": files })).ok()
}

/// What the walk has gathered so far, keyed by the path sent to the browser.
#[derive(Default)]
struct Walk {
    files: BTreeMap<String, Vec<u8>>,
    bytes: u64,
}

/// Adds the files under `dir`, whose path relative to `.calepin` is `rel`.
/// `None` when a limit is exceeded or any entry cannot be read.
fn collect(dir: &Path, rel: &str, depth: usize, walk: &mut Walk) -> Option<()> {
    if depth > MAX_DEPTH {
        return None;
    }
    for entry in std::fs::read_dir(dir).ok()? {
        let entry = entry.ok()?;
        let name = entry.file_name().into_string().ok()?;
        let relative = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path).ok()?;
        let kind = metadata.file_type();
        if kind.is_dir() {
            collect(&path, &relative, depth + 1, walk)?;
        } else if kind.is_file() && !SKIPPED.contains(&name.as_str()) {
            if walk.files.len() >= MAX_FILES || walk.bytes + metadata.len() > MAX_BYTES {
                return None;
            }
            let bytes = std::fs::read(&path).ok()?;
            walk.bytes += bytes.len() as u64;
            if walk.bytes > MAX_BYTES {
                return None;
            }
            walk.files.insert(format!("{DIR}/{relative}"), bytes);
        }
        // Symlinks and other special files are skipped.
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn parse(bytes: &[u8]) -> serde_json::Value {
        serde_json::from_slice(bytes).unwrap()
    }

    fn paths(value: &serde_json::Value) -> Vec<String> {
        value["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file["path"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn a_normal_tree_is_sorted_skips_markers_and_encodes_binary() {
        let root = tempfile::tempdir().unwrap();
        let calepin = root.path().join(".calepin");
        write(&calepin.join("results.json"), b"{\"chunks\":[]}");
        write(&calepin.join("paper/figure.svg"), b"<svg/>");
        write(&calepin.join("paper/chunk.bin"), &[0xff, 0x00, 0x01]);
        write(&calepin.join("runtime/a/deep.typ"), b"#let x = 1");
        write(&calepin.join("fingerprint.xxh3"), b"hash");
        write(&calepin.join("expansion.json"), b"{}");
        write(&calepin.join("config.toml"), b"secret = 1");
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            root.path().join("paper.typ"),
            calepin.join("paper/link.typ"),
        )
        .unwrap();

        let bytes = results(root.path(), "paper.typ").expect("a run");
        let value = parse(&bytes);
        assert_eq!(value["version"], 1);
        assert_eq!(
            paths(&value),
            [
                ".calepin/paper/chunk.bin",
                ".calepin/paper/figure.svg",
                ".calepin/results.json",
                ".calepin/runtime/a/deep.typ",
            ]
        );
        let files = value["files"].as_array().unwrap();
        assert_eq!(files[0]["base64"], "/wAB");
        assert!(files[0].get("text").is_none());
        assert_eq!(files[1]["text"], "<svg/>");
        assert_eq!(files[3]["text"], "#let x = 1");
    }

    #[test]
    fn a_missing_calepin_folder_gives_none() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("paper.typ"), b"= Title");
        assert_eq!(results(root.path(), "paper.typ"), None);
    }

    #[test]
    #[cfg(unix)]
    fn a_symlinked_calepin_folder_gives_none() {
        let root = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        write(&elsewhere.path().join("results.json"), b"{}");
        std::os::unix::fs::symlink(elsewhere.path(), root.path().join(".calepin")).unwrap();
        assert_eq!(results(root.path(), "paper.typ"), None);
    }

    #[test]
    fn a_subfolder_entrypoint_reads_its_own_calepin_folder() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join(".calepin/results.json"), b"{\"root\":1}");
        write(
            &root.path().join("chapters/.calepin/results.json"),
            b"{\"chapter\":1}",
        );
        let bytes = results(root.path(), "chapters/intro.typ").expect("a run");
        let value = parse(&bytes);
        assert_eq!(paths(&value), [".calepin/results.json"]);
        assert_eq!(value["files"][0]["text"], "{\"chapter\":1}");
    }

    #[test]
    fn a_folder_without_results_json_gives_none() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join(".calepin/paper/figure.svg"), b"<svg/>");
        assert_eq!(results(root.path(), "paper.typ"), None);
    }
}
