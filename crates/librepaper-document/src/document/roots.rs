//! Typed access to the roots of the shared Loro document.
//!
//! The document is four maps (`files`, `paths`, `assets`, `meta`) and the
//! `changes` root. Everything that reads a root by key goes through
//! [`Roots`], so a missing entry, an entry of the wrong type and a deleted
//! entry all answer the same way wherever they are asked.

use loro::{Container, LoroDoc, LoroMap, LoroText, LoroValue, ValueOrContainer};

use crate::document::projection::{ASSETS, CHANGES, FILES, META, PATHS};

/// The roots of one document.
pub struct Roots<'doc> {
    doc: &'doc LoroDoc,
}

impl<'doc> Roots<'doc> {
    pub fn new(doc: &'doc LoroDoc) -> Self {
        Self { doc }
    }

    /// The `files` root: an id to the text at it.
    pub fn files(&self) -> LoroMap {
        self.doc.get_map(FILES)
    }

    /// The `paths` root: an id to the path that text is known by.
    pub fn paths(&self) -> LoroMap {
        self.doc.get_map(PATHS)
    }

    /// The `assets` root: a path to the digest of the bytes at it.
    pub fn assets(&self) -> LoroMap {
        self.doc.get_map(ASSETS)
    }

    /// The `meta` root: document metadata.
    pub fn meta(&self) -> LoroMap {
        self.doc.get_map(META)
    }

    /// The `changes` root: change and fragment records, and the plain
    /// records of the earlier model.
    pub fn changes(&self) -> LoroMap {
        self.doc.get_map(CHANGES)
    }

    /// The text filed under `id`. `None` when there is no entry, when it was
    /// deleted, or when the entry is not a text.
    pub fn text_by_id(&self, id: &str) -> Option<LoroText> {
        match self.files().get(id)? {
            ValueOrContainer::Container(Container::Text(t)) => Some(t),
            _ => None,
        }
    }

    /// The id of the first entry of `paths` whose value is the string `path`,
    /// in key order. Entries whose value is not a string are skipped. It is a
    /// scan, so a caller that asks about many paths should read `paths()`
    /// once instead.
    pub fn id_by_path(&self, path: &str) -> Option<String> {
        let paths = self.paths();
        for key in paths.keys() {
            let key = key.to_string();
            if Self::string_at(&paths, &key).as_deref() == Some(path) {
                return Some(key);
            }
        }
        None
    }

    /// The id of the last text, in `files` key order, that is named by
    /// `path`. This differs from `id_by_path` only when two entries claim one
    /// path (the larger id wins here, the smaller there) or when the first
    /// claimant is not a text.
    pub fn text_id_by_path_last(&self, path: &str) -> Option<String> {
        let files = self.files();
        let paths = self.paths();
        let mut found = None;
        for key in files.keys() {
            let id = key.to_string();
            if !matches!(
                files.get(&id),
                Some(ValueOrContainer::Container(Container::Text(_)))
            ) {
                continue;
            }
            if Self::string_at(&paths, &id).as_deref() == Some(path) {
                found = Some(id);
            }
        }
        found
    }

    /// The path filed under `id`, when that entry is a string.
    pub fn path_of(&self, id: &str) -> Option<String> {
        Self::string_at(&self.paths(), id)
    }

    /// The record at `id` in `root`, when it is a map container. Plain values
    /// of the earlier model are not records.
    pub fn record_in(root: &LoroMap, id: &str) -> Option<LoroMap> {
        match root.get(id)? {
            ValueOrContainer::Container(Container::Map(m)) => Some(m),
            _ => None,
        }
    }

    /// The plain map value at `id` of the `changes` root: the shape the
    /// earlier model wrote. A map container is not one.
    pub fn plain_record(&self, id: &str) -> Option<loro::LoroMapValue> {
        match self.changes().get(id)? {
            ValueOrContainer::Value(LoroValue::Map(m)) => Some(m),
            _ => None,
        }
    }

    /// The string stored at `key`, when the entry is a plain string.
    pub fn string_at(map: &LoroMap, key: &str) -> Option<String> {
        match map.get(key)? {
            ValueOrContainer::Value(LoroValue::String(s)) => Some(s.to_string()),
            _ => None,
        }
    }

    /// The string a plain value holds, for values already read out of a map.
    pub fn string_of(value: &LoroValue) -> Option<String> {
        match value {
            LoroValue::String(s) => Some(s.to_string()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loro::LoroText;

    fn doc_with(files: &[(&str, &str, &str)]) -> LoroDoc {
        let doc = LoroDoc::new();
        for (id, path, body) in files {
            let text = doc.get_map(FILES).insert_container(id, LoroText::new()).unwrap();
            text.insert(0, body).unwrap();
            doc.get_map(PATHS).insert(id, *path).unwrap();
        }
        doc
    }

    #[test]
    fn text_by_id_finds_texts_only() {
        let doc = doc_with(&[("a", "a.md", "hello")]);
        doc.get_map(FILES).insert("n", 7).unwrap();
        let roots = Roots::new(&doc);
        assert_eq!(roots.text_by_id("a").unwrap().to_string(), "hello");
        assert!(roots.text_by_id("n").is_none());
        assert!(roots.text_by_id("missing").is_none());
    }

    #[test]
    fn text_by_id_is_none_for_a_deleted_entry() {
        let doc = doc_with(&[("a", "a.md", "x")]);
        doc.get_map(FILES).delete("a").unwrap();
        assert!(Roots::new(&doc).text_by_id("a").is_none());
    }

    #[test]
    fn id_by_path_and_path_of() {
        let doc = doc_with(&[("a", "a.md", ""), ("b", "b.md", "")]);
        let roots = Roots::new(&doc);
        assert_eq!(roots.id_by_path("b.md").as_deref(), Some("b"));
        assert_eq!(roots.id_by_path("c.md"), None);
        assert_eq!(roots.path_of("a").as_deref(), Some("a.md"));
        assert_eq!(roots.path_of("zzz"), None);
    }

    #[test]
    fn id_by_path_skips_non_strings_and_deleted_entries() {
        let doc = doc_with(&[("a", "a.md", ""), ("b", "b.md", "")]);
        doc.get_map(PATHS).insert("n", 7).unwrap();
        doc.get_map(PATHS).delete("a").unwrap();
        let roots = Roots::new(&doc);
        assert_eq!(roots.id_by_path("a.md"), None);
        assert_eq!(roots.id_by_path("b.md").as_deref(), Some("b"));
        assert_eq!(roots.path_of("n"), None);
        assert_eq!(roots.path_of("a"), None);
    }

    #[test]
    fn two_entries_claiming_one_path() {
        let doc = doc_with(&[("a", "x.md", "first"), ("b", "x.md", "second")]);
        let roots = Roots::new(&doc);
        assert_eq!(roots.id_by_path("x.md").as_deref(), Some("a"));
        assert_eq!(roots.text_id_by_path_last("x.md").as_deref(), Some("b"));
    }

    #[test]
    fn last_text_claimant_ignores_non_texts() {
        let doc = doc_with(&[("a", "x.md", "first")]);
        doc.get_map(FILES).insert("b", 7).unwrap();
        doc.get_map(PATHS).insert("b", "x.md").unwrap();
        let roots = Roots::new(&doc);
        assert_eq!(roots.text_id_by_path_last("x.md").as_deref(), Some("a"));
    }

    #[test]
    fn missing_roots_read_as_empty() {
        let doc = LoroDoc::new();
        let roots = Roots::new(&doc);
        assert!(roots.text_by_id("a").is_none());
        assert!(roots.id_by_path("a.md").is_none());
        assert!(roots.path_of("a").is_none());
        assert!(roots.changes().keys().next().is_none());
        assert!(roots.assets().keys().next().is_none());
        assert!(roots.meta().keys().next().is_none());
    }

    #[test]
    fn string_at_reads_plain_strings_only() {
        let doc = LoroDoc::new();
        let map = doc.get_map("m");
        map.insert("s", "v").unwrap();
        map.insert("n", 3).unwrap();
        map.insert_container("c", LoroText::new()).unwrap();
        assert_eq!(Roots::string_at(&map, "s").as_deref(), Some("v"));
        assert_eq!(Roots::string_at(&map, "n"), None);
        assert_eq!(Roots::string_at(&map, "c"), None);
        assert_eq!(Roots::string_at(&map, "missing"), None);
        map.delete("s").unwrap();
        assert_eq!(Roots::string_at(&map, "s"), None);
        assert_eq!(Roots::string_of(&LoroValue::from("v")).as_deref(), Some("v"));
        assert_eq!(Roots::string_of(&LoroValue::from(1)), None);
    }

    #[test]
    fn records_and_plain_records_are_told_apart() {
        let doc = LoroDoc::new();
        let roots = Roots::new(&doc);
        let changes = roots.changes();
        changes.insert_container("rec", LoroMap::new()).unwrap();
        let mut plain = std::collections::HashMap::new();
        plain.insert("file".to_string(), LoroValue::from("f"));
        changes.insert("old", LoroValue::from(plain)).unwrap();
        changes.insert("num", 1).unwrap();
        assert!(Roots::record_in(&changes, "rec").is_some());
        assert!(Roots::record_in(&changes, "old").is_none());
        assert!(Roots::record_in(&changes, "num").is_none());
        assert!(Roots::record_in(&changes, "missing").is_none());
        assert!(roots.plain_record("old").is_some());
        assert!(roots.plain_record("rec").is_none());
        assert!(roots.plain_record("num").is_none());
        assert!(roots.plain_record("missing").is_none());
    }
}
