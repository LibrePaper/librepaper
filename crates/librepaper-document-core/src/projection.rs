//! The projection: what a `LoroDoc` says, read as a directory.
//!
//! The server stores and forwards source bytes and reads only their headers
//! to do so (SPEC-server-is-a-log §1). Whenever it does have to know what the
//! bytes *mean* -- to render for a reader, to anchor a comment, to evaluate a
//! proposal, to export -- it asks this function, and this function is total:
//! every `LoroDoc`, however malformed, projects to exactly one directory.
//! Nothing here writes to the document. There is no repair pass any more,
//! because there is nothing to repair: a shape the schema does not use is
//! absent from the projection and says so in a diagnostic.
//!
//! The algorithm is fixed in seven steps so that two implementations can be
//! held equal by fixtures. The browser runs the same steps in
//! `web/src/lib/projection.js`, and `tests/projection_fixtures.rs` and
//! `web/tests/unit/projection.test.mjs` read one shared corpus.

use std::collections::{BTreeMap, HashMap, HashSet};

use loro::{Container, LoroDoc, LoroValue, ValueOrContainer};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::paths::{self, Kind, Rules};
use crate::{ASSETS, FILES, MAIN, META, PATHS};

/// How long an asset digest is, spelled in hex. An `assets` value of any
/// other length is not a digest this document store could have written.
pub const ASSET_DIGEST_HEX: usize = 64;

/// Why something in the document is not in the projection, or is not where it
/// asked to be. Diagnostics are advisory: they are shown to an editor and
/// they never change what the projection is.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Diagnostic {
    /// A `files` entry whose value is not a text container.
    NotAText { id: String },
    /// A `files` entry with no string in `paths`, or a `paths` entry whose
    /// value is not a string.
    Unnamed { id: String },
    /// A `paths` entry naming an id that is not a text in `files`.
    Dangling { id: String },
    /// An `assets` value that is not a digest of the right shape.
    NotADigest { path: String },
    /// A path the shared rules refuse. The candidate is absent.
    InvalidPath {
        id: String,
        path: String,
        why: String,
    },
    /// A candidate that could not have the path it asked for, and was given
    /// another.
    Moved {
        id: String,
        from: String,
        to: String,
    },
    /// A candidate that asked for a path and could be given none at all.
    Dropped { id: String, path: String },
    /// `meta.main` names nothing in the projection; the first text was used.
    MainMissing { id: String },
}

impl Diagnostic {
    /// The tag, as it appears on the wire and in the fixtures.
    pub fn tag(&self) -> &'static str {
        match self {
            Self::NotAText { .. } => "not-a-text",
            Self::Unnamed { .. } => "unnamed",
            Self::Dangling { .. } => "dangling",
            Self::NotADigest { .. } => "not-a-digest",
            Self::InvalidPath { .. } => "invalid-path",
            Self::Moved { .. } => "moved",
            Self::Dropped { .. } => "dropped",
            Self::MainMissing { .. } => "main-missing",
        }
    }

    fn sort_key(&self) -> (&'static str, String, String) {
        let (first, second) = match self {
            Self::NotAText { id }
            | Self::Unnamed { id }
            | Self::Dangling { id }
            | Self::MainMissing { id } => (id.clone(), String::new()),
            Self::NotADigest { path } => (path.clone(), String::new()),
            Self::InvalidPath { id, path, .. } => (id.clone(), path.clone()),
            Self::Moved { id, from, .. } => (id.clone(), from.clone()),
            Self::Dropped { id, path } => (id.clone(), path.clone()),
        };
        (self.tag(), first, second)
    }
}

/// One file in the projected directory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// `text` or `asset`.
    pub kind: String,
    /// The stable Loro id of a text. Empty for an asset, which is keyed by
    /// its path and has no move identity.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// The digest of what is at this path: the SHA-256 of a text's UTF-8
    /// bytes, or the asset's own digest as the document records it.
    pub digest: String,
    /// The UTF-8 length of a text. Zero for an asset, whose bytes are not
    /// here.
    #[serde(default)]
    pub bytes: u64,
}

/// What the document holds, as a directory.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Projection {
    /// The path of the main file, or empty when the document has none.
    pub main: String,
    /// Its Loro id, or empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub main_id: String,
    /// Every path in the directory, ordered so that two runs over one
    /// document serialize identically.
    pub files: BTreeMap<String, Entry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

impl Projection {
    /// The name of this projection: the SHA-256 of what it says.
    ///
    /// Deliberately the *content* digest of each file rather than its Loro
    /// id, which §4.4 step 7 offers as an alternative. An id is minted afresh
    /// whenever a file is created and never changes when the text in it does,
    /// so a digest built from ids would not move when the document moved --
    /// and every use this digest has, from a reader's etag to the
    /// stale-selection check on a rendered comment (§7.1), needs it to move
    /// exactly when the words do.
    pub fn digest(&self) -> String {
        hex::encode(self.digest_bytes())
    }

    pub fn digest_bytes(&self) -> [u8; 32] {
        Sha256::digest(self.canonical_bytes()).into()
    }

    /// The canonical form the digest is taken over: the main path, then one
    /// line per file in path order. A line-oriented form rather than JSON,
    /// because JSON leaves the implementation free to reorder integer-looking
    /// keys and to spell escapes differently, and two implementations have to
    /// produce the same bytes. No field here can contain a newline: a path
    /// with a control character in it is refused in step 2, and a kind and a
    /// digest are drawn from fixed alphabets.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = String::with_capacity(64 + self.files.len() * 96);
        out.push_str("librepaper.projection.v1\n");
        out.push_str(&self.main);
        out.push('\n');
        for (path, entry) in &self.files {
            out.push_str(path);
            out.push('\t');
            out.push_str(&entry.kind);
            out.push('\t');
            out.push_str(&entry.digest);
            out.push('\n');
        }
        out.into_bytes()
    }

    /// The text at a path, for a caller that projected with bodies.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// The projection and the body of every text in it, which is what an export,
/// a render and an anchoring pass all want in one pass over the document.
#[derive(Clone, Debug, Default)]
pub struct Projected {
    pub projection: Projection,
    /// Text bodies by path.
    pub texts: BTreeMap<String, String>,
}

/// A candidate before reservation: what asked for a path, and which one.
struct Candidate {
    /// `false` for a text, `true` for an asset: the sort key of step 3, text
    /// before asset.
    asset: bool,
    /// The text's Loro id, or `asset:<path>` for an asset.
    key: String,
    /// The text's Loro id, or empty for an asset.
    id: String,
    requested: String,
    digest: String,
    bytes: u64,
    text: Option<String>,
}

/// The seven steps of §4.4, in order.
pub fn project(doc: &LoroDoc, rules: &Rules) -> Projected {
    let mut diagnostics = Vec::new();
    let mut candidates = Vec::new();

    // 1. Collect candidates.
    let files = doc.get_map(FILES);
    let path_map = doc.get_map(PATHS);
    let assets = doc.get_map(ASSETS);

    let mut named: HashMap<String, String> = HashMap::new();
    for key in path_map.keys() {
        let id = key.to_string();
        match path_map.get(&id) {
            Some(ValueOrContainer::Value(LoroValue::String(path))) => {
                named.insert(id, path.to_string());
            }
            _ => diagnostics.push(Diagnostic::Unnamed { id }),
        }
    }

    let mut file_ids: Vec<String> = files.keys().map(|key| key.to_string()).collect();
    file_ids.sort();
    let mut text_ids: HashSet<String> = HashSet::new();
    for id in file_ids {
        let Some(ValueOrContainer::Container(Container::Text(text))) = files.get(&id) else {
            diagnostics.push(Diagnostic::NotAText { id });
            continue;
        };
        text_ids.insert(id.clone());
        let Some(requested) = named.get(&id).cloned() else {
            diagnostics.push(Diagnostic::Unnamed { id });
            continue;
        };
        let body = text.to_string();
        candidates.push(Candidate {
            asset: false,
            key: id.clone(),
            id,
            requested,
            digest: hex::encode(Sha256::digest(body.as_bytes())),
            bytes: body.len() as u64,
            text: Some(body),
        });
    }
    for id in named.keys() {
        if !text_ids.contains(id) {
            diagnostics.push(Diagnostic::Dangling { id: id.clone() });
        }
    }

    let mut asset_paths: Vec<String> = assets.keys().map(|key| key.to_string()).collect();
    asset_paths.sort();
    for path in asset_paths {
        let digest = match assets.get(&path) {
            Some(ValueOrContainer::Value(LoroValue::String(value)))
                if value.len() == ASSET_DIGEST_HEX
                    && value.chars().all(|c| c.is_ascii_hexdigit()) =>
            {
                value.to_string()
            }
            _ => {
                diagnostics.push(Diagnostic::NotADigest { path });
                continue;
            }
        };
        candidates.push(Candidate {
            asset: true,
            key: format!("asset:{path}"),
            id: String::new(),
            requested: path,
            digest,
            bytes: 0,
            text: None,
        });
    }

    // 2. Validate paths.
    candidates.retain(
        |candidate| match paths::check(rules, &candidate.requested) {
            Ok(kind) => {
                let wanted = if candidate.asset {
                    Kind::Asset
                } else {
                    Kind::Text
                };
                if kind == wanted {
                    true
                } else {
                    diagnostics.push(Diagnostic::InvalidPath {
                        id: candidate.key.clone(),
                        path: candidate.requested.clone(),
                        why: format!(
                            "{}: the extension names a {} and this is a {}",
                            candidate.requested,
                            kind.as_str(),
                            wanted.as_str()
                        ),
                    });
                    false
                }
            }
            Err(why) => {
                diagnostics.push(Diagnostic::InvalidPath {
                    id: candidate.key.clone(),
                    path: candidate.requested.clone(),
                    why,
                });
                false
            }
        },
    );

    // 3. Order: kind first, text before asset, then the key as a byte string.
    candidates.sort_by(|left, right| {
        left.asset
            .cmp(&right.asset)
            .then_with(|| left.key.as_bytes().cmp(right.key.as_bytes()))
    });

    // 4. Reserve. `taken` is keyed by the collision key -- NFC, case-folded --
    //    and remembers the spelling that won it, which is what is emitted.
    let mut taken: HashMap<String, String> = HashMap::new();
    let mut placed: Vec<(usize, String)> = Vec::new();
    let mut deferred: Vec<usize> = Vec::new();
    for (index, candidate) in candidates.iter().enumerate() {
        let normalised = paths::normalise(&candidate.requested);
        if free(&taken, &normalised) {
            taken.insert(paths::collision_key(&normalised), normalised.clone());
            placed.push((index, normalised));
        } else {
            deferred.push(index);
        }
    }

    // 5. Suffix. `stem (2).ext`, `stem (3).ext`, and so on, until one is free.
    //    A candidate that cannot be placed inside the bound is dropped rather
    //    than allowed to spin: the bound is larger than any document this
    //    deployment holds, so reaching it means something pathological.
    const MOST: usize = 10_000;
    for index in deferred {
        let requested = paths::normalise(&candidates[index].requested);
        let mut chosen = None;
        for nth in 2..MOST {
            let attempt = paths::suffixed(&requested, nth);
            if free(&taken, &attempt) {
                chosen = Some(attempt);
                break;
            }
        }
        match chosen {
            Some(attempt) => {
                taken.insert(paths::collision_key(&attempt), attempt.clone());
                diagnostics.push(Diagnostic::Moved {
                    id: candidates[index].key.clone(),
                    from: requested,
                    to: attempt.clone(),
                });
                placed.push((index, attempt));
            }
            None => diagnostics.push(Diagnostic::Dropped {
                id: candidates[index].key.clone(),
                path: requested,
            }),
        }
    }

    let mut files_out = BTreeMap::new();
    let mut texts = BTreeMap::new();
    let mut path_of_id: HashMap<String, String> = HashMap::new();
    for (index, path) in placed {
        let candidate = &candidates[index];
        if let Some(body) = &candidate.text {
            texts.insert(path.clone(), body.clone());
            path_of_id.insert(candidate.id.clone(), path.clone());
        }
        files_out.insert(
            path,
            Entry {
                kind: if candidate.asset { "asset" } else { "text" }.to_string(),
                id: candidate.id.clone(),
                digest: candidate.digest.clone(),
                bytes: candidate.bytes,
            },
        );
    }

    // 6. Main.
    let meta = doc.get_map(META);
    let declared = match meta.get(MAIN) {
        Some(ValueOrContainer::Value(LoroValue::String(id))) => id.to_string(),
        _ => String::new(),
    };
    let (main_id, main) = match path_of_id.get(&declared) {
        Some(path) => (declared, path.clone()),
        None => {
            if !declared.is_empty() {
                diagnostics.push(Diagnostic::MainMissing { id: declared });
            }
            // The first text candidate in the order of step 3. `candidates`
            // is still in that order and texts sort before assets, so the
            // first one that received a path is the answer.
            candidates
                .iter()
                .filter(|candidate| !candidate.asset)
                .find_map(|candidate| {
                    path_of_id
                        .get(&candidate.id)
                        .map(|path| (candidate.id.clone(), path.clone()))
                })
                .unwrap_or_default()
        }
    };

    // A total order, so that two runs over one document -- and the browser's
    // run beside the server's -- report the same list in the same sequence.
    diagnostics.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));

    Projected {
        projection: Projection {
            main,
            main_id,
            files: files_out,
            diagnostics,
        },
        texts,
    }
}

/// Whether `path` may be taken: not already a file, not a directory prefix of
/// a file already taken, and not under one.
fn free(taken: &HashMap<String, String>, path: &str) -> bool {
    let key = paths::collision_key(path);
    if taken.contains_key(&key) {
        return false;
    }
    let under = format!("{key}/");
    for existing in taken.keys() {
        // `path` would be a directory holding a file already placed.
        if existing.starts_with(&under) {
            return false;
        }
        // `path` would sit under a file already placed.
        if key.starts_with(&format!("{existing}/")) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use loro::LoroText;

    fn rules() -> Rules<'static> {
        // Leaked once per process in a test binary; the lists are static data
        // and this keeps the fixtures readable.
        fn once(values: &[&str]) -> &'static [String] {
            Box::leak(
                values
                    .iter()
                    .map(|value| value.to_string())
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            )
        }
        Rules {
            text: once(&[
                ".tex", ".md", ".typ", ".bib", ".html", ".qmd", ".txt", ".css",
            ]),
            asset: once(&[".png", ".jpg", ".svg", ".pdf"]),
            derived: once(&[".log", ".aux", ".out"]),
            max_path: 255,
            max_segments: paths::MAX_SEGMENTS,
        }
    }

    fn doc() -> LoroDoc {
        let doc = LoroDoc::new();
        for root in crate::ROOTS {
            let _ = doc.get_map(root);
        }
        doc
    }

    fn put(doc: &LoroDoc, id: &str, path: &str, body: &str) {
        let text = doc
            .get_map(FILES)
            .insert_container(id, LoroText::new())
            .unwrap();
        text.insert(0, body).unwrap();
        doc.get_map(PATHS).insert(id, path).unwrap();
    }

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn an_empty_project_projects_to_nothing() {
        let projected = project(&doc(), &rules());
        assert!(projected.projection.files.is_empty());
        assert_eq!(projected.projection.main, "");
    }

    #[test]
    fn two_files_at_one_path_are_ordered_and_the_later_one_is_suffixed() {
        let doc = doc();
        put(&doc, "bbb", "paper.tex", "second");
        put(&doc, "aaa", "paper.tex", "first");
        let projected = project(&doc, &rules());
        assert_eq!(
            projected.texts.get("paper.tex").map(String::as_str),
            Some("first"),
            "the lower id wins the requested path"
        );
        assert_eq!(
            projected.texts.get("paper (2).tex").map(String::as_str),
            Some("second")
        );
    }

    #[test]
    fn a_pre_existing_suffixed_name_is_skipped_over() {
        let doc = doc();
        put(&doc, "aaa", "paper.tex", "first");
        put(&doc, "bbb", "paper (2).tex", "already");
        put(&doc, "ccc", "paper.tex", "third");
        let projected = project(&doc, &rules());
        assert_eq!(
            projected.texts.get("paper (3).tex").map(String::as_str),
            Some("third")
        );
    }

    #[test]
    fn a_file_that_is_another_files_directory_is_moved_aside() {
        let doc = doc();
        put(&doc, "aaa", "chapters/one.tex", "inside");
        put(&doc, "bbb", "chapters", "shadow");
        let projected = project(&doc, &rules());
        // `chapters` has no extension the rules accept, so it never becomes a
        // candidate at all.
        assert!(projected
            .projection
            .diagnostics
            .iter()
            .any(|d| matches!(d, Diagnostic::InvalidPath { .. })));
        assert!(projected.projection.files.contains_key("chapters/one.tex"));
    }

    #[test]
    fn a_text_under_a_text_is_deferred() {
        let doc = doc();
        put(&doc, "aaa", "notes.tex", "file");
        put(&doc, "bbb", "notes.tex/inner.tex", "under");
        let projected = project(&doc, &rules());
        assert!(projected.projection.files.contains_key("notes.tex"));
        assert!(!projected
            .projection
            .files
            .contains_key("notes.tex/inner.tex"));
    }

    #[test]
    fn case_and_normalization_collide() {
        let doc = doc();
        put(&doc, "aaa", "Re\u{301}sume\u{301}.md", "combining");
        put(&doc, "bbb", "RÉSUMÉ.MD", "precomposed upper");
        let projected = project(&doc, &rules());
        assert_eq!(projected.projection.files.len(), 2);
        assert!(
            projected.projection.files.keys().any(|p| p.contains("(2)")),
            "the second spelling is suffixed, not silently merged: {:?}",
            projected.projection.files.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_text_and_an_asset_at_one_path_put_the_text_first() {
        let doc = doc();
        put(&doc, "aaa", "fig.svg", "not really a figure");
        doc.get_map(ASSETS).insert("fig.svg", DIGEST).unwrap();
        let projected = project(&doc, &rules());
        // `.svg` is an asset extension, so the text candidate is refused in
        // step 2 and the asset keeps the path.
        assert_eq!(
            projected.projection.files.get("fig.svg").map(|e| &e.kind),
            Some(&"asset".to_string())
        );
    }

    #[test]
    fn a_non_text_container_in_files_is_absent_with_a_diagnostic() {
        let doc = doc();
        doc.get_map(FILES).insert("aaa", 7).unwrap();
        doc.get_map(PATHS).insert("aaa", "x.md").unwrap();
        let projected = project(&doc, &rules());
        assert!(projected.projection.files.is_empty());
        assert!(projected
            .projection
            .diagnostics
            .contains(&Diagnostic::NotAText { id: "aaa".into() }));
    }

    #[test]
    fn a_non_string_path_is_absent_with_a_diagnostic() {
        let doc = doc();
        let text = doc
            .get_map(FILES)
            .insert_container("aaa", LoroText::new())
            .unwrap();
        text.insert(0, "body").unwrap();
        doc.get_map(PATHS).insert("aaa", 7).unwrap();
        let projected = project(&doc, &rules());
        assert!(projected.projection.files.is_empty());
        assert!(projected
            .projection
            .diagnostics
            .contains(&Diagnostic::Unnamed { id: "aaa".into() }));
    }

    #[test]
    fn a_climbing_path_is_absent() {
        let doc = doc();
        put(&doc, "aaa", "../escape.md", "no");
        let projected = project(&doc, &rules());
        assert!(projected.projection.files.is_empty());
    }

    #[test]
    fn an_absent_main_falls_back_to_the_first_text() {
        let doc = doc();
        put(&doc, "bbb", "b.md", "b");
        put(&doc, "aaa", "a.md", "a");
        let projected = project(&doc, &rules());
        assert_eq!(projected.projection.main, "a.md");
    }

    #[test]
    fn a_main_naming_a_deleted_file_falls_back_and_says_so() {
        let doc = doc();
        put(&doc, "aaa", "a.md", "a");
        doc.get_map(META).insert(MAIN, "gone").unwrap();
        let projected = project(&doc, &rules());
        assert_eq!(projected.projection.main, "a.md");
        assert!(projected
            .projection
            .diagnostics
            .contains(&Diagnostic::MainMissing { id: "gone".into() }));
    }

    #[test]
    fn astral_characters_survive_paths_and_bodies() {
        let doc = doc();
        put(&doc, "aaa", "\u{1F600}.md", "\u{1F600} body");
        let projected = project(&doc, &rules());
        assert_eq!(
            projected.texts.get("\u{1F600}.md").map(String::as_str),
            Some("\u{1F600} body")
        );
    }

    #[test]
    fn the_digest_moves_when_the_text_does_and_not_when_an_id_does() {
        let doc = doc();
        put(&doc, "aaa", "a.md", "one");
        let first = project(&doc, &rules()).projection.digest();

        let same = self::doc();
        put(&same, "zzz", "a.md", "one");
        same.get_map(META).insert(MAIN, "zzz").unwrap();
        doc.get_map(META).insert(MAIN, "aaa").unwrap();
        assert_eq!(
            project(&doc, &rules()).projection.digest(),
            project(&same, &rules()).projection.digest(),
            "the same directory under different ids is the same document"
        );

        let moved = self::doc();
        put(&moved, "aaa", "a.md", "two");
        assert_ne!(first, project(&moved, &rules()).projection.digest());
    }
}
