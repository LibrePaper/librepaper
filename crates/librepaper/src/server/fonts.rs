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

#[path = "font_exceptions.rs"]
mod font_exceptions;

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
                if !is_font(&leaf) {
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
    pub async fn response(&self, rest: &str, head: bool, headers: &HeaderMap) -> Response<Body> {
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
            super::cost::blob_response(blobs, file.name.clone(), &file.sha, headers, head).await;
        if !response.status().is_success() {
            return response;
        }
        super::set(&mut response, "content-type", content_type(&file.name));
        super::set(
            &mut response,
            "cache-control",
            "public, max-age=31536000, immutable",
        );
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

/// True when the filename is a font (.ttf, .otf, .ttc, .otc), false otherwise.
fn is_font(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".ttf")
        || lower.ends_with(".otf")
        || lower.ends_with(".ttc")
        || lower.ends_with(".otc")
}

/// The families a font file carries, lowercased as typst matches them, with
/// duplicates removed. Empty for a file that is not a font. Uses typst 0.15.1's
/// logic: the exception table overrides the family name extracted from each face,
/// otherwise read the family name from the FAMILY name ID and trim style suffixes.
pub fn families_in(bytes: &[u8]) -> Vec<String> {
    let count = ttf_parser::fonts_in_collection(bytes).unwrap_or(1);
    let mut families = Vec::new();
    for index in 0..count {
        if let Ok(face) = ttf_parser::Face::parse(bytes, index) {
            let ps_name = find_name(&face, ttf_parser::name_id::POST_SCRIPT_NAME);
            let exception = ps_name.as_deref().and_then(font_exceptions::find_exception);
            let family = exception.map(|s| s.to_string()).or_else(|| {
                let family = find_name(&face, ttf_parser::name_id::FAMILY)?;
                Some(typographic_family(&family).to_string())
            });
            if let Some(family_name) = family {
                let family_lower = family_name.to_lowercase();
                if !family_lower.is_empty() && !families.contains(&family_lower) {
                    families.push(family_lower);
                }
            }
        }
    }
    families.sort();
    families.dedup();
    families
}

/// Try to find and decode the name with the given id, following typst's logic
/// for finding and decoding font names.
fn find_name(face: &ttf_parser::Face, name_id: u16) -> Option<String> {
    use ttf_parser::PlatformId;

    for entry in face.names() {
        if entry.name_id == name_id {
            if let Some(string) = entry.to_string() {
                return Some(string);
            }

            if entry.platform_id == PlatformId::Macintosh && entry.encoding_id == 0 {
                return Some(decode_mac_roman(entry.name));
            }
        }
    }

    None
}

/// Decode mac roman encoded bytes into a string, following typst's logic.
fn decode_mac_roman(coded: &[u8]) -> String {
    #[rustfmt::skip]
    const TABLE: [char; 128] = [
        'Ä', 'Å', 'Ç', 'É', 'Ñ', 'Ö', 'Ü', 'á', 'à', 'â', 'ä', 'ã', 'å', 'ç', 'é', 'è',
        'ê', 'ë', 'í', 'ì', 'î', 'ï', 'ñ', 'ó', 'ò', 'ô', 'ö', 'õ', 'ú', 'ù', 'û', 'ü',
        '†', '°', '¢', '£', '§', '•', '¶', 'ß', '®', '©', '™', '´', '¨', '≠', 'Æ', 'Ø',
        '∞', '±', '≤', '≥', '¥', 'µ', '∂', '∑', '∏', 'π', '∫', 'ª', 'º', 'Ω', 'æ', 'ø',
        '¿', '¡', '¬', '√', 'ƒ', '≈', '∆', '«', '»', '…', '\u{a0}', 'À', 'Ã', 'Õ', 'Œ', 'œ',
        '\u{2013}', '\u{2014}', '\u{201c}', '\u{201d}', '\u{2018}', '\u{2019}', '÷', '◊', 'ÿ', 'Ÿ', '⁄', '€', '‹', '›', 'ﬁ', 'ﬂ',
        '‡', '·', '‚', '„', '‰', 'Â', 'Ê', 'Á', 'Ë', 'È', 'Í', 'Î', 'Ï', 'Ì', 'Ó', 'Ô',
        '\u{f8ff}', 'Ò', 'Ú', 'Û', 'Ù', 'ı', 'ˆ', '˜', '¯', '˘', '˙', '˚', '¸', '˝', '˛', 'ˇ',
    ];

    fn char_from_mac_roman(code: u8) -> char {
        if code < 128 {
            code as char
        } else {
            TABLE[(code - 128) as usize]
        }
    }

    coded.iter().copied().map(char_from_mac_roman).collect()
}

/// Trim style naming from a family name, following typst 0.15.1's logic.
fn typographic_family(mut family: &str) -> &str {
    // Separators between names, modifiers and styles.
    const SEPARATORS: [char; 3] = [' ', '-', '_'];

    // Modifiers that can appear in combination with suffixes.
    const MODIFIERS: &[&str] = &[
        "extra", "ext", "ex", "x", "semi", "sem", "sm", "demi", "dem", "ultra",
    ];

    // Style suffixes.
    #[rustfmt::skip]
    const SUFFIXES: &[&str] = &[
        "normal", "italic", "oblique", "slanted",
        "thin", "th", "hairline", "light", "lt", "regular", "medium", "med",
        "md", "bold", "bd", "demi", "extb", "black", "blk", "bk", "heavy",
        "narrow", "condensed", "cond", "cn", "cd", "compressed", "expanded", "exp",
        "vf", "var", "variable",
    ];

    // Trim spacing and weird leading dots in Apple fonts.
    family = family.trim().trim_start_matches('.');

    // Lowercase the string so that the suffixes match case-insensitively.
    let lower = family.to_ascii_lowercase();
    let mut len = usize::MAX;
    let mut trimmed = lower.as_str();

    // Trim style suffixes repeatedly.
    while trimmed.len() < len {
        len = trimmed.len();

        // Find style suffix.
        let mut t = trimmed;
        let mut shortened = false;
        while let Some(s) = SUFFIXES.iter().find_map(|s| t.strip_suffix(s)) {
            shortened = true;
            t = s;
        }

        if !shortened {
            break;
        }

        // Strip optional separator.
        if let Some(s) = t.strip_suffix(SEPARATORS) {
            trimmed = s;
            t = s;
        }

        // Also allow an extra modifier, but apply it only if it is separated
        // from the text before it (to prevent false positives).
        if let Some(t) = MODIFIERS.iter().find_map(|s| t.strip_suffix(s)) {
            if let Some(stripped) = t.strip_suffix(SEPARATORS) {
                trimmed = stripped;
            }
        }
    }

    // Apply style suffix trimming.
    family = &family[..len];

    family
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_in_matches_typst_output() {
        // Every font typst bundles is named here exactly as the compiler names it.
        for (index, font) in typst_assets::fonts().enumerate() {
            let our_families = families_in(font);
            let typst_families: Vec<String> =
                typst::text::Font::iter(typst::foundations::Bytes::new(font.to_vec()))
                    .map(|f| f.info().family.to_lowercase())
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect();

            assert_eq!(our_families, typst_families, "bundled font {index}");
        }
    }
}
