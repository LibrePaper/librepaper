//! The font library a deployment serves to typst documents.
//!
//! The compiler embeds typst's own default fonts and nothing else, and a
//! browser has no system fonts worth relying on -- and reading the reader's
//! would make one document render two ways. So a document that names another
//! family gets it from here: `--typst-fonts DIR` is a directory of font files, read
//! once at startup for the families each carries, and served by family. The
//! editor's compile loop asks for a family the compiler warned about, and
//! `publish` asks the same deployment for the same files, so the preview and
//! the stored PDF are set in the same faces.
//!
//! Which fonts a deployment offers is the operator's decision, and the
//! licence question is theirs too: the files are served as they are.

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{HeaderMap, Response};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// The most files a library is read for. A directory with more than this is
/// probably not a font library, and every file is opened at startup.
const MOST_FILES: usize = 4096;

/// One font file: where it is, what it hashes to, and the families it holds.
#[derive(Clone, Debug)]
pub struct FontFile {
    /// Its path under the library, with `/` separators.
    pub name: String,
    pub sha: String,
    pub families: Vec<String>,
    pub len: u64,
    modified: Option<std::time::SystemTime>,
}

#[derive(Clone, Debug)]
pub struct Library {
    root: PathBuf,
    files: Vec<FontFile>,
}

impl Library {
    /// Reads the directory the flag names. A directory that is not there is
    /// refused at startup, where the operator is; one with no fonts in it is
    /// allowed and warned about, since it may be about to be filled.
    pub fn open(flag: &str) -> Result<Library, String> {
        let root = PathBuf::from(flag.trim());
        if flag.trim().is_empty() || !root.is_dir() {
            return Err(format!(
                "--typst-fonts {flag} is not a directory on this machine.\n\n  \
                 Point it at a directory of .ttf, .otf, .ttc or .otc files; every family they\n  \
                 carry is served to typst documents that name it."
            ));
        }
        let mut files = Vec::new();
        let mut pending = vec![root.clone()];
        while let Some(dir) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut paths: Vec<PathBuf> =
                entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
            paths.sort();
            for path in paths {
                let leaf = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                if leaf.starts_with('.') {
                    continue;
                }
                if path.is_dir() {
                    pending.push(path);
                    continue;
                }
                if !wasm_typst::typst::is_font(&leaf) {
                    continue;
                }
                if files.len() >= MOST_FILES {
                    return Err(format!(
                        "--typst-fonts {flag} holds more than {MOST_FILES} font files, which is more than a library"
                    ));
                }
                let Ok(bytes) = std::fs::read(&path) else {
                    continue;
                };
                let families = families_in(&bytes);
                if families.is_empty() {
                    eprintln!(
                        "warning: --typst-fonts: no font could be read from {}",
                        path.display()
                    );
                    continue;
                }
                let name = path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().to_string())
                    .collect::<Vec<_>>()
                    .join("/");
                files.push(FontFile {
                    name,
                    sha: hex::encode(Sha256::digest(&bytes)),
                    families,
                    len: bytes.len() as u64,
                    modified: std::fs::metadata(&path)
                        .ok()
                        .and_then(|m| m.modified().ok()),
                });
            }
        }
        if files.is_empty() {
            eprintln!("warning: --typst-fonts {flag} holds no font files yet");
        }
        Ok(Library { root, files })
    }

    /// For the startup line.
    pub fn describe(&self) -> String {
        let mut families: Vec<&str> = self
            .files
            .iter()
            .flat_map(|file| file.families.iter().map(String::as_str))
            .collect();
        families.sort_unstable();
        families.dedup();
        format!(
            "{} ({} files, {} families)",
            self.root.display(),
            self.files.len(),
            families.len()
        )
    }

    /// The families this library has, each with the files that carry it, as
    /// `<sha>/<name>` paths under `/api/fonts/`. Keyed by the lowercase family
    /// name, which is what typst's warning names and the compiler matches by.
    pub fn index(&self) -> Value {
        let mut families: std::collections::BTreeMap<&str, Vec<String>> =
            std::collections::BTreeMap::new();
        for file in &self.files {
            for family in &file.families {
                families
                    .entry(family.as_str())
                    .or_default()
                    .push(format!("{}/{}", file.sha, file.name));
            }
        }
        json!({ "families": families })
    }

    /// The file served as `<sha>/<name>`, or nothing: a name the index does
    /// not have, or one whose bytes no longer hash to the sha in the URL,
    /// is not served. The URL is immutable for the life of those bytes, which
    /// is what lets a browser cache it for a year.
    pub fn file(&self, sha: &str, name: &str) -> Option<&FontFile> {
        self.files
            .iter()
            .find(|file| file.sha == sha && file.name == name)
    }

    pub fn path_of(&self, file: &FontFile) -> PathBuf {
        let mut path = self.root.clone();
        for part in file.name.split('/') {
            path.push(part);
        }
        path
    }

    /// Answers `/api/fonts/<rest>`: the index, or one file.
    pub async fn response(
        &self,
        rest: &str,
        head: bool,
        headers: &HeaderMap,
        meter: &Arc<super::cost::CostMeter>,
    ) -> Response<Body> {
        if rest == "index.json" {
            let mut response = super::write_json(200, &self.index());
            // The library changes when the deployment restarts with another
            // directory, so the index is revalidated; the files it names are
            // addressed by digest and never change.
            super::set(&mut response, "cache-control", "no-cache");
            if head {
                *response.body_mut() = Body::empty();
            }
            return response;
        }
        let Some((sha, encoded)) = rest.split_once('/') else {
            return super::plain(404, "not found");
        };
        let Ok(name) = percent_encoding::percent_decode_str(encoded).decode_utf8() else {
            return super::plain(404, "not found");
        };
        let Some(file) = self.file(sha, &name) else {
            return super::plain(404, "not found");
        };
        // Startup verified the immutable font. Refuse changed files until the
        // library is reopened; admission and range selection precede file reads.
        let _metadata = match tokio::fs::metadata(self.path_of(file)).await {
            Ok(metadata)
                if metadata.len() == file.len && metadata.modified().ok() == file.modified =>
            {
                metadata
            }
            _ => return super::plain(404, "font changed; restart to refresh the library"),
        };
        let blobs = Arc::new(crate::storage::blob::FsStore::new(&self.root, false));
        let mut response =
            super::cost::blob_response(meter, blobs, file.name.clone(), &file.sha, headers, head)
                .await;
        if !response.status().is_success() {
            return response;
        }
        super::set(&mut response, "content-type", content_type(&file.name));
        super::set(
            &mut response,
            "cache-control",
            "public, max-age=31536000, immutable",
        );
        super::set(&mut response, "x-content-type-options", "nosniff");
        response
    }
}

fn content_type(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    if lower.ends_with(".ttf") {
        "font/ttf"
    } else if lower.ends_with(".otf") {
        "font/otf"
    } else if lower.ends_with(".ttc") || lower.ends_with(".otc") {
        "font/collection"
    } else {
        "application/octet-stream"
    }
}

/// The families a font file carries, lowercased as typst matches them, with
/// duplicates removed. Empty for a file that is not a font.
///
/// The engine crate answers this, because the compiler that reads the faces
/// here has to be the one that will set the text: two typst versions in one
/// binary would name the same file's families twice, and differently.
pub use wasm_typst::typst::families_in;
