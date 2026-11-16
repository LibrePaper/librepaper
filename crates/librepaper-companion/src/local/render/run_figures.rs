//! Figures a document's code leaves in the Run directory.
//!
//! A Quarto or Typst run can write images that are not project files: the
//! document refers to `figures/plot.png`, and the script that made it lives in
//! the person's Run directory. The page names the paths it wants, and after a
//! successful run they are read from the Run directory and sent back inside
//! the job's `results` file. The same checks back `POST run-directory/has`.
//!
//! A path is wanted only when it is relative with plain components. It is
//! resolved under the Run directory following symlinks, must be a regular
//! file, and its first bytes must look like an image or a PDF. Anything else
//! is skipped without a word. There is no size cap of our own; the account
//! quota limits what the browser can keep, and the transport limit is only a
//! safety bound on the results file.

use std::path::{Component, Path, PathBuf};

use base64::Engine as _;
use serde_json::{json, Value};

/// How many leading bytes decide what a file is.
const SNIFF_BYTES: usize = 4096;

/// `path` as a relative path of plain components, or `None`.
fn plain(path: &str) -> Option<PathBuf> {
    if path.is_empty() || path.contains('\0') || path.contains('\\') {
        return None;
    }
    let relative = Path::new(path);
    let ok = relative
        .components()
        .all(|part| matches!(part, Component::Normal(_)));
    // A drive prefix such as `C:` is not a normal component on Windows, but
    // on other systems it would pass, so the shape is refused outright.
    let drive = path.as_bytes().get(1) == Some(&b':');
    (ok && !drive).then(|| relative.to_path_buf())
}

/// Whether `head`, the start of a file, is a PNG, JPEG, GIF, WebP, PDF or SVG.
pub(crate) fn sniff(head: &[u8]) -> bool {
    head.starts_with(b"\x89PNG\r\n\x1a\n")
        || head.starts_with(&[0xff, 0xd8, 0xff])
        || head.starts_with(b"GIF87a")
        || head.starts_with(b"GIF89a")
        || (head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP")
        || head.starts_with(b"%PDF-")
        || is_svg(head)
}

/// Text that opens with optional BOM, whitespace, an XML prolog, comments and
/// a doctype, then `<svg`.
fn is_svg(head: &[u8]) -> bool {
    let mut rest = head.strip_prefix(b"\xef\xbb\xbf").unwrap_or(head);
    loop {
        rest = rest.trim_ascii_start();
        let skip = |rest: &[u8], open: &[u8], close: &[u8]| -> Option<usize> {
            rest.starts_with(open)
                .then(|| rest.windows(close.len()).position(|w| w == close))
                .flatten()
                .map(|at| at + close.len())
        };
        if let Some(n) = skip(rest, b"<?", b"?>") {
            rest = &rest[n..];
        } else if let Some(n) = skip(rest, b"<!--", b"-->") {
            rest = &rest[n..];
        } else if rest.len() >= 9 && rest[..9].eq_ignore_ascii_case(b"<!doctype") {
            match rest.iter().position(|&b| b == b'>') {
                Some(n) => rest = &rest[n + 1..],
                None => return false,
            }
        } else {
            break;
        }
    }
    rest.len() >= 4 && rest[..4].eq_ignore_ascii_case(b"<svg")
}

/// The file `path` names under `run_dir`, when it is a wanted kind of figure.
fn resolve(run_dir: &Path, path: &str) -> Option<PathBuf> {
    let full = run_dir.join(plain(path)?);
    // `metadata` follows symlinks, so a link to an image counts.
    std::fs::metadata(&full).ok()?.is_file().then_some(full)
}

fn head_of(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut head = Vec::with_capacity(SNIFF_BYTES);
    std::fs::File::open(path)
        .ok()?
        .take(SNIFF_BYTES as u64)
        .read_to_end(&mut head)
        .ok()?;
    Some(head)
}

/// Whether `path` is a figure present in `run_dir`. Reads only the first bytes.
pub(crate) fn has(run_dir: &Path, path: &str) -> bool {
    resolve(run_dir, path)
        .and_then(|full| head_of(&full))
        .is_some_and(|head| sniff(&head))
}

/// The figures among `wanted` that `run_dir` holds, as `(path, bytes)` in the
/// order asked, each path once. `budget` bounds the encoded size of what is
/// returned; a figure that would exceed it is dropped and logged.
pub(crate) fn collect(run_dir: &Path, wanted: &[String], budget: usize) -> Vec<(String, Vec<u8>)> {
    let mut found: Vec<(String, Vec<u8>)> = Vec::new();
    let mut used = 0usize;
    for path in wanted {
        if found.iter().any(|(seen, _)| seen == path) {
            continue;
        }
        let Some(full) = resolve(run_dir, path) else {
            continue;
        };
        let Some(head) = head_of(&full) else { continue };
        if !sniff(&head) {
            continue;
        }
        let size = std::fs::metadata(&full).map(|m| m.len()).unwrap_or(0) as usize;
        let encoded = size / 3 * 4 + 4 + path.len() + 40;
        if used + encoded > budget {
            eprintln!("run figure {path} left out: the results would be too large");
            continue;
        }
        let Ok(bytes) = std::fs::read(&full) else {
            continue;
        };
        used += bytes.len() / 3 * 4 + 4 + path.len() + 40;
        found.push((path.clone(), bytes));
    }
    found
}

fn entry(path: &str, bytes: &[u8]) -> Value {
    json!({
        "path": path,
        "base64": base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

/// A results file holding only `figures`: `{ version: 1, files: [...] }`.
pub(crate) fn results(figures: &[(String, Vec<u8>)]) -> Option<Vec<u8>> {
    if figures.is_empty() {
        return None;
    }
    let files: Vec<Value> = figures.iter().map(|(p, b)| entry(p, b)).collect();
    serde_json::to_vec(&json!({ "version": 1, "files": files })).ok()
}

/// `results`, a results file already built, with `figures` appended to its
/// files. The original is returned unchanged when it cannot be extended.
pub(crate) fn append(results: Vec<u8>, figures: &[(String, Vec<u8>)]) -> Vec<u8> {
    if figures.is_empty() {
        return results;
    }
    let Ok(mut value) = serde_json::from_slice::<Value>(&results) else {
        return results;
    };
    let Some(files) = value.get_mut("files").and_then(Value::as_array_mut) else {
        return results;
    };
    files.extend(figures.iter().map(|(p, b)| entry(p, b)));
    serde_json::to_vec(&value).unwrap_or(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n rest";

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn wanted(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn sniffs_every_kind() {
        assert!(sniff(PNG));
        assert!(sniff(&[0xff, 0xd8, 0xff, 0xe0]));
        assert!(sniff(b"GIF89a.."));
        assert!(sniff(b"RIFF\0\0\0\0WEBPVP8 "));
        assert!(sniff(b"%PDF-1.7"));
        assert!(sniff(b"<svg xmlns=\"x\"/>"));
        assert!(sniff(
            b"\n <?xml version=\"1.0\"?>\n<!-- c -->\n<!DOCTYPE svg>\n<svg/>"
        ));
        assert!(!sniff(b"just text"));
        assert!(!sniff(b"<html><svg/></html>"));
        assert!(!sniff(b""));
    }

    #[test]
    fn png_is_collected_and_other_things_are_not() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("figures/plot.png"), PNG);
        write(&dir.path().join("notes.txt"), b"plain text");
        std::fs::create_dir_all(dir.path().join("sub.png")).unwrap();
        let got = collect(
            dir.path(),
            &wanted(&["figures/plot.png", "notes.txt", "sub.png", "missing.png"]),
            usize::MAX,
        );
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "figures/plot.png");
        assert_eq!(got[0].1, PNG);
    }

    #[test]
    fn unsafe_paths_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let outer = dir.path().join("outer.png");
        write(&outer, PNG);
        let run = dir.path().join("run");
        write(&run.join("a.png"), PNG);
        let abs = outer.display().to_string();
        let got = collect(
            &run,
            &wanted(&["../outer.png", "a/../a.png", &abs, "/a.png", "C:a.png", ""]),
            usize::MAX,
        );
        assert!(got.is_empty());
        assert!(!has(&run, "../outer.png"));
        assert!(has(&run, "a.png"));
    }

    #[test]
    #[cfg(unix)]
    fn a_symlink_to_an_image_is_followed() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        write(&elsewhere.path().join("real.png"), PNG);
        std::os::unix::fs::symlink(
            elsewhere.path().join("real.png"),
            dir.path().join("link.png"),
        )
        .unwrap();
        let got = collect(dir.path(), &wanted(&["link.png"]), usize::MAX);
        assert_eq!(got.len(), 1);
        assert!(has(dir.path(), "link.png"));
    }

    #[test]
    fn the_budget_drops_figures_without_failing() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a.png"), PNG);
        assert!(collect(dir.path(), &wanted(&["a.png"]), 10).is_empty());
    }

    #[test]
    fn results_and_append_share_one_shape() {
        let figures = vec![("a.png".to_string(), PNG.to_vec())];
        let made: Value = serde_json::from_slice(&results(&figures).unwrap()).unwrap();
        assert_eq!(made["version"], 1);
        assert_eq!(made["files"][0]["path"], "a.png");
        assert!(made["files"][0]["base64"].is_string());
        assert!(results(&[]).is_none());
        let base =
            serde_json::to_vec(&json!({"version": 1, "files": [{"path": "x", "text": "y"}]}))
                .unwrap();
        let both: Value = serde_json::from_slice(&append(base, &figures)).unwrap();
        assert_eq!(both["files"].as_array().unwrap().len(), 2);
        assert_eq!(both["files"][1]["path"], "a.png");
    }
}
