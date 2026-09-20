//! Platform-neutral document domain boundary.
//!
//! This crate deliberately has no I/O, clock, database, HTTP, filesystem or
//! async-runtime dependency. Native server owners supply
//! identity and policy explicitly. The Loro graph is never regenerated.

use loro::{LoroDoc, ValueOrContainer};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

pub const SCHEMA_VERSION: u32 = 1;
pub const ROOTS: [&str; 4] = ["files", "paths", "assets", "meta"];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct CommitSequence(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct SourceRevision(pub u64);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ValidationError {
    InvalidSchemaVersion,
    UnsupportedSchemaVersion {
        found: u64,
        supported: u32,
    },
    UnsupportedRoot(String),
    InvalidContainer {
        root: String,
        expected: &'static str,
    },
    InvalidPath(String),
    PathCollision(String),
    TooManyFiles {
        files: usize,
        ceiling: usize,
    },
    SourceTooLarge {
        bytes: usize,
        ceiling: usize,
    },
    HistoryTooLarge {
        bytes: usize,
        ceiling: usize,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub source_bytes: usize,
    pub encoded_history_bytes: usize,
    pub files: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Measurement {
    pub source_bytes: usize,
    pub encoded_history_bytes: usize,
    pub files: usize,
}

/// Read the embedded domain schema. Documents created before explicit
/// versioning are version 1; malformed or future markers are never coerced.
pub fn schema_version(doc: &LoroDoc) -> Result<u32, ValidationError> {
    let Some(version) = doc.get_map("meta").get("schema.version") else {
        return Ok(1);
    };
    let ValueOrContainer::Value(loro::LoroValue::I64(version)) = version else {
        return Err(ValidationError::InvalidSchemaVersion);
    };
    if version < 1 {
        return Err(ValidationError::InvalidSchemaVersion);
    }
    if version as u64 > u64::from(SCHEMA_VERSION) {
        return Err(ValidationError::UnsupportedSchemaVersion {
            found: version as u64,
            supported: SCHEMA_VERSION,
        });
    }
    Ok(version as u32)
}

/// Exact final-candidate measurements. Encoded history is an export, not an
/// accumulated delta estimate, and source bytes are UTF-8 projection bytes.
pub fn measure(doc: &LoroDoc) -> Measurement {
    let files = doc.get_map("files");
    let paths = doc.get_map("paths");
    let assets = doc.get_map("assets");
    let mut source_bytes = 0usize;
    let mut file_count = assets.len();
    for id in paths.keys() {
        let id = id.to_string();
        let Some(ValueOrContainer::Value(loro::LoroValue::String(path))) = paths.get(&id) else {
            continue;
        };
        let Some(ValueOrContainer::Container(loro::Container::Text(text))) = files.get(&id) else {
            continue;
        };
        source_bytes = source_bytes
            .saturating_add(path.len())
            .saturating_add(text.to_string().len());
        file_count += 1;
    }
    Measurement {
        source_bytes,
        encoded_history_bytes: doc
            .export(loro::ExportMode::all_updates())
            .map_or(usize::MAX, |v| v.len()),
        files: file_count,
    }
}

pub fn validate(doc: &LoroDoc, limits: Limits) -> Result<Measurement, ValidationError> {
    let deep = doc.get_deep_value();
    if let loro::LoroValue::Map(map) = &deep {
        for root in map.keys() {
            if !ROOTS.contains(&root.as_str()) {
                return Err(ValidationError::UnsupportedRoot(root.to_string()));
            }
        }
    }
    for root in ROOTS {
        if let Some(value) = doc.get_deep_value().as_map().and_then(|map| map.get(root)) {
            if value.as_map().is_none() {
                return Err(ValidationError::InvalidContainer {
                    root: root.into(),
                    expected: "map",
                });
            }
        }
    }
    schema_version(doc)?;
    let mut collision_keys = std::collections::HashSet::new();
    let paths = doc.get_map("paths");
    for id in paths.keys() {
        let Some(ValueOrContainer::Value(loro::LoroValue::String(path))) = paths.get(&id) else {
            return Err(ValidationError::InvalidPath(format!(
                "path for file {id} is not a string"
            )));
        };
        let key = collision_key(&path)?;
        if !collision_keys.insert(key) {
            return Err(ValidationError::PathCollision(path.to_string()));
        }
    }
    // Text and assets share the user-visible path namespace. An asset is
    // keyed by its path and has no stable move identity yet, so normalization
    // must reject collisions rather than deleting or silently renaming it.
    let assets = doc.get_map("assets");
    for path in assets.keys() {
        let key = collision_key(&path)?;
        if !collision_keys.insert(key) {
            return Err(ValidationError::PathCollision(path.to_string()));
        }
    }
    let measured = measure(doc);
    if measured.files > limits.files {
        return Err(ValidationError::TooManyFiles {
            files: measured.files,
            ceiling: limits.files,
        });
    }
    if measured.source_bytes > limits.source_bytes {
        return Err(ValidationError::SourceTooLarge {
            bytes: measured.source_bytes,
            ceiling: limits.source_bytes,
        });
    }
    if measured.encoded_history_bytes > limits.encoded_history_bytes {
        return Err(ValidationError::HistoryTooLarge {
            bytes: measured.encoded_history_bytes,
            ceiling: limits.encoded_history_bytes,
        });
    }
    Ok(measured)
}

fn collision_key(path: &str) -> Result<String, ValidationError> {
    let normalized: String = path.nfc().collect();
    if normalized.is_empty()
        || normalized.starts_with('/')
        || normalized
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(ValidationError::InvalidPath(path.to_string()));
    }
    Ok(normalized.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_project_is_valid() {
        let doc = LoroDoc::new();
        for root in ROOTS {
            let _ = doc.get_map(root);
        }
        assert!(validate(
            &doc,
            Limits {
                source_bytes: 1,
                encoded_history_bytes: 1024,
                files: 1
            }
        )
        .is_ok());
    }

    #[test]
    fn revision_numbers_are_not_frontiers() {
        assert_ne!(
            std::mem::size_of::<SourceRevision>(),
            std::mem::size_of::<loro::Frontiers>()
        );
    }

    #[test]
    fn text_and_assets_share_one_normalized_path_namespace() {
        let doc = LoroDoc::new();
        let files = doc.get_map("files");
        let text = files
            .insert_container("file-1", loro::LoroText::new())
            .unwrap();
        text.insert(0, "body").unwrap();
        doc.get_map("paths").insert("file-1", "Résumé.md").unwrap();
        doc.get_map("assets")
            .insert("RE\u{301}SUMÉ.MD", "digest")
            .unwrap();
        let error = validate(
            &doc,
            Limits {
                source_bytes: 1024,
                encoded_history_bytes: 16 * 1024,
                files: 10,
            },
        )
        .unwrap_err();
        assert!(matches!(error, ValidationError::PathCollision(_)));
    }

    #[test]
    fn future_schema_requires_an_upgrade_and_is_not_repaired() {
        let doc = LoroDoc::new();
        doc.get_map("meta")
            .insert("schema.version", i64::from(SCHEMA_VERSION) + 1)
            .unwrap();
        let error = validate(
            &doc,
            Limits {
                source_bytes: 1024,
                encoded_history_bytes: 16 * 1024,
                files: 10,
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ValidationError::UnsupportedSchemaVersion {
                found: 2,
                supported: SCHEMA_VERSION
            }
        ));
        assert!(matches!(
            doc.get_map("meta").get("schema.version"),
            Some(ValueOrContainer::Value(loro::LoroValue::I64(2)))
        ));
    }
}
