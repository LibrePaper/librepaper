//! The user's edited tool records.
//!
//! Only a record the user changed is written, to
//! `<state_home>/librepaper/local/renderers.json`, keyed `<format>.<output>`.
//! Every other key reads as the built-in default, so a later release can
//! change a default without a migration. The old `integrations.json` is not
//! read.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use super::tool::{default_record, Key, Record};
use crate::local::store::{read_json, write_private_json};

/// The record in use for a key, and whether it is the user's own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub record: Record,
    pub edited: bool,
}

/// The store file and what it holds. `None` until the service calls [`init`].
struct State {
    path: PathBuf,
    entries: BTreeMap<String, Record>,
}

static STATE: RwLock<Option<State>> = RwLock::new(None);

/// Loads `<state_home>/librepaper/local/renderers.json`. A missing or
/// unreadable file, or an entry that no longer validates, reads as the
/// default. Called when the service starts; a later call replaces the state.
pub fn init(state_home: &Path) {
    let path = state_home
        .join("librepaper")
        .join("local")
        .join("renderers.json");
    let entries = load_or_default(&path);
    *STATE.write().unwrap_or_else(|poison| poison.into_inner()) = Some(State { path, entries });
}

/// The record in use for `key`: the user's edit, else the built-in default.
pub fn get(key: Key) -> Resolved {
    let edited = STATE
        .read()
        .unwrap_or_else(|poison| poison.into_inner())
        .as_ref()
        .and_then(|state| state.entries.get(&key.to_string()).cloned());
    match edited {
        Some(record) => Resolved {
            record,
            edited: true,
        },
        None => Resolved {
            record: default_record(key).expect("a Key always has a default record"),
            edited: false,
        },
    }
}

/// Every key's record in use, defaults included.
pub fn all() -> BTreeMap<Key, Resolved> {
    Key::all().into_iter().map(|key| (key, get(key))).collect()
}

/// Validates `record`, writes the store, then updates memory. A record equal
/// to the built-in default removes the entry, so it is not written.
pub fn set(key: Key, record: Record) -> Result<(), String> {
    record.validate()?;
    let is_default = default_record(key).as_ref() == Some(&record);
    update(key, (!is_default).then_some(record))
}

/// Returns `key` to its built-in default.
pub fn reset(key: Key) -> Result<(), String> {
    update(key, None)
}

fn update(key: Key, record: Option<Record>) -> Result<(), String> {
    let mut guard = STATE.write().unwrap_or_else(|poison| poison.into_inner());
    let state = guard.as_mut().ok_or("renderers store is not initialised")?;
    let mut entries = state.entries.clone();
    match record {
        Some(record) => entries.insert(key.to_string(), record),
        None => entries.remove(&key.to_string()),
    };
    if let Some(parent) = state.path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    write_private_json(&state.path, &entries).map_err(|e| e.to_string())?;
    state.entries = entries;
    Ok(())
}

fn load_or_default(path: &Path) -> BTreeMap<String, Record> {
    read_json::<BTreeMap<String, Record>>(path)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(name, record)| {
            let key = Key::parse(&name).ok()?;
            record.validate().ok()?;
            Some((key.to_string(), record))
        })
        .collect()
}

/// The store is process-wide, so every test that touches it takes turns.
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    fn with_tempdir<F: Fn(&Path)>(f: F) {
        let _turn = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        f(dir.path());
    }

    fn edited(command: &[&str]) -> Record {
        Record {
            command: command.iter().map(|argument| (*argument).into()).collect(),
            result: "{stem}.pdf".into(),
        }
    }

    fn key(name: &str) -> Key {
        Key::parse(name).unwrap()
    }

    #[test]
    fn an_unedited_key_reads_as_its_default_and_is_not_edited() {
        with_tempdir(|dir| {
            init(dir);
            let resolved = get(key("latex.pdf"));
            assert!(!resolved.edited);
            assert_eq!(Some(resolved.record), default_record(key("latex.pdf")));
            assert_eq!(all().len(), Key::all().len());
        });
    }

    #[test]
    fn set_round_trips_and_marks_the_record_edited() {
        with_tempdir(|dir| {
            init(dir);
            let record = edited(&["latexmk", "-xelatex", "{input}"]);
            set(key("latex.pdf"), record.clone()).expect("set");
            let resolved = get(key("latex.pdf"));
            assert!(resolved.edited);
            assert_eq!(resolved.record, record);
            assert!(!get(key("typst.pdf")).edited);
        });
    }

    #[test]
    fn an_edit_survives_a_restart_and_nothing_else_is_written() {
        with_tempdir(|dir| {
            init(dir);
            let record = edited(&["tectonic", "{input}"]);
            set(key("latex.pdf"), record.clone()).expect("set");
            init(dir);
            assert_eq!(get(key("latex.pdf")).record, record);
            let text = std::fs::read_to_string(
                dir.join("librepaper").join("local").join("renderers.json"),
            )
            .expect("file");
            let written: BTreeMap<String, Record> = serde_json::from_str(&text).expect("json");
            assert_eq!(written.keys().collect::<Vec<_>>(), ["latex.pdf"]);
        });
    }

    #[test]
    fn reset_and_saving_the_default_both_leave_the_file_without_the_key() {
        with_tempdir(|dir| {
            init(dir);
            set(key("latex.pdf"), edited(&["tectonic", "{input}"])).expect("set");
            reset(key("latex.pdf")).expect("reset");
            assert!(!get(key("latex.pdf")).edited);

            set(key("typst.pdf"), edited(&["tectonic", "{input}"])).expect("set");
            set(key("typst.pdf"), default_record(key("typst.pdf")).unwrap()).expect("default");
            assert!(!get(key("typst.pdf")).edited);
            init(dir);
            assert!(!get(key("typst.pdf")).edited);
        });
    }

    #[test]
    fn an_invalid_record_is_refused_and_changes_nothing() {
        with_tempdir(|dir| {
            init(dir);
            assert!(set(key("latex.pdf"), edited(&["latexmk", "{path}"])).is_err());
            assert!(set(key("latex.pdf"), edited(&[])).is_err());
            assert!(!get(key("latex.pdf")).edited);
        });
    }

    #[test]
    fn the_old_integrations_file_is_ignored_and_bad_entries_read_as_default() {
        with_tempdir(|dir| {
            let local = dir.join("librepaper").join("local");
            std::fs::create_dir_all(&local).unwrap();
            std::fs::write(
                local.join("integrations.json"),
                r#"{"quarto":{"path":"/opt/quarto","args":["--quiet"]}}"#,
            )
            .unwrap();
            std::fs::write(
                local.join("renderers.json"),
                r#"{"latex.html":{"command":["x"],"result":"a.html"},
                    "typst.pdf":{"command":["typst","{nope}"],"result":"{stem}.pdf"},
                    "quarto.pdf":{"command":["quarto","render","{input}"],"result":"{stem}.pdf"}}"#,
            )
            .unwrap();
            init(dir);
            assert!(!get(key("typst.pdf")).edited);
            assert!(get(key("quarto.pdf")).edited);
            assert!(!get(key("quarto.html")).edited);
        });
    }
}
