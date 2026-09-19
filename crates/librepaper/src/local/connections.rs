//! Named connections: the indirection that keeps a document key out of an
//! agent's configuration file.
//!
//! An agent's MCP entry names a connection (`librepaper agent mcp
//! --connection dissertation`), never a link. This machine holds the mapping
//! from that name to the protected document URL, in
//! `<state_home>/librepaper/local/connections.json` at mode 0600, next to the
//! pairing store that authorizes writing it.
//!
//! Three things follow, and all three are the reason for the indirection:
//! the key never reaches a config file a user might paste or commit; a
//! rotated link is repaired in one place without the user editing any agent's
//! configuration; and revoking an agent's access is deleting one row rather
//! than hunting through the config of every tool that was ever connected.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::pairing::{read_json, write_private_json};
use crate::auth::now_unix;

/// How long a connection survives without being resolved. A connection is
/// renewed on every successful resolve, so an agent in daily use never
/// expires, and one wired up and forgotten does not keep a document key on
/// disk indefinitely.
pub const IDLE_TTL_SECONDS: i64 = 90 * 24 * 3600;

/// One document an agent on this computer may reach.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Connection {
    /// The protected document URL, key fragment included. This is the secret
    /// the whole module exists to keep out of agent configuration.
    pub link: String,
    /// The browser origin that registered it, so a revoke of a pairing can
    /// take its connections with it.
    pub origin: String,
    /// What the link grants: `reader`, `commenter` or `editor`. Recorded for
    /// display only; the link itself is what actually bounds access.
    #[serde(default)]
    pub access: String,
    /// Human label for the sidebar and `librepaper local connections`.
    #[serde(default)]
    pub title: String,
    pub created: i64,
    /// Last time an agent resolved this connection.
    #[serde(default)]
    pub used: i64,
    /// The sidebar conversation and its credential, for the adapter the
    /// sidebar assistant spawns. Present only on internal records: an agent
    /// the user connected themselves has no sidebar channel to dispatch
    /// browser renders through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_token: Option<String>,
    /// Written by the runner for its own adapter rather than by a user
    /// connecting an agent. Hidden from listings so the sidebar shows only
    /// connections a person made.
    #[serde(default)]
    pub internal: bool,
}

#[derive(Clone)]
pub struct ConnectionStore {
    dir: PathBuf,
}

/// A connection name is typed into config files and command lines, so it is
/// restricted to what is safe in both and readable in neither's error
/// messages when it is wrong.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'-' | b'_'))
}

/// A stable, readable name derived from a document title or slug, made unique
/// against what is already stored. Users see this in their agent config, so
/// `thesis-2` beats a hash.
pub fn derive_name(preferred: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let mut base: String = preferred
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while base.contains("--") {
        base = base.replace("--", "-");
    }
    let base = base.trim_matches('-').to_string();
    let base = if base.is_empty() {
        "document".to_string()
    } else {
        base.chars().take(48).collect()
    };
    if !taken(&base) {
        return base;
    }
    for suffix in 2..1000 {
        let candidate = format!("{base}-{suffix}");
        if !taken(&candidate) {
            return candidate;
        }
    }
    format!("{base}-{}", now_unix())
}

impl ConnectionStore {
    /// `state_home` is an XDG state base, matching `PairingStore::new`.
    pub fn new(state_home: &Path) -> Self {
        Self {
            dir: state_home.join("librepaper").join("local"),
        }
    }

    fn path(&self) -> PathBuf {
        self.dir.join("connections.json")
    }

    fn load(&self) -> BTreeMap<String, Connection> {
        let mut all: BTreeMap<String, Connection> = read_json(&self.path()).unwrap_or_default();
        let now = now_unix();
        all.retain(|_, entry| {
            let last = entry.used.max(entry.created);
            now.saturating_sub(last) < IDLE_TTL_SECONDS
        });
        all
    }

    fn save(&self, all: &BTreeMap<String, Connection>) -> std::io::Result<()> {
        write_private_json(&self.path(), all)
    }

    /// Store a connection under `name`, replacing any connection of the same
    /// name. Returns the name actually used.
    pub fn put(&self, name: &str, connection: Connection) -> Result<String, String> {
        if !valid_name(name) {
            return Err("invalid connection name".into());
        }
        let mut all = self.load();
        if all.len() >= 200 && !all.contains_key(name) {
            return Err("too many connections on this computer".into());
        }
        all.insert(name.to_string(), connection);
        self.save(&all).map_err(|err| err.to_string())?;
        Ok(name.to_string())
    }

    /// Register a document under a name derived from `title`, reusing the
    /// existing name when this origin already registered this exact link, so
    /// connecting a second agent to one document does not mint a second name.
    pub fn register(
        &self,
        title: &str,
        link: &str,
        origin: &str,
        access: &str,
    ) -> Result<String, String> {
        let all = self.load();
        if let Some((name, _)) = all
            .iter()
            .find(|(_, entry)| entry.link == link && entry.origin == origin)
        {
            let name = name.clone();
            let now = now_unix();
            let mut all = all;
            if let Some(entry) = all.get_mut(&name) {
                entry.access = access.to_string();
                entry.title = title.to_string();
                entry.used = now;
            }
            self.save(&all).map_err(|err| err.to_string())?;
            return Ok(name);
        }
        let name = derive_name(title, &|candidate| all.contains_key(candidate));
        let now = now_unix();
        self.put(
            &name,
            Connection {
                link: link.to_string(),
                origin: origin.to_string(),
                access: access.to_string(),
                title: title.to_string(),
                created: now,
                used: now,
                conversation: None,
                chat_token: None,
                internal: false,
            },
        )
    }

    /// Park the sidebar assistant's own credentials under a private name, so
    /// the command line the runner hands its agent carries a name rather than
    /// a document key and a channel token. Replaced on every runner start.
    pub fn put_internal(
        &self,
        name: &str,
        link: &str,
        conversation: &str,
        chat_token: &str,
    ) -> Result<String, String> {
        let now = now_unix();
        self.put(
            name,
            Connection {
                link: link.to_string(),
                origin: "local-runner".into(),
                access: String::new(),
                title: String::new(),
                created: now,
                used: now,
                conversation: Some(conversation.to_string()),
                chat_token: Some(chat_token.to_string()),
                internal: true,
            },
        )
    }

    /// Resolve a name to its document link, renewing its idle clock. This is
    /// the call `librepaper agent mcp --connection` makes on startup.
    pub fn resolve(&self, name: &str) -> Result<Connection, String> {
        if !valid_name(name) {
            return Err("invalid connection name".into());
        }
        let mut all = self.load();
        let Some(entry) = all.get_mut(name) else {
            return Err(format!(
                "no connection named '{name}' on this computer; reconnect the document in your browser"
            ));
        };
        entry.used = now_unix();
        let found = entry.clone();
        let _ = self.save(&all);
        Ok(found)
    }

    pub fn get(&self, name: &str) -> Option<Connection> {
        self.load().get(name).cloned()
    }

    /// Every connection a person made, newest first, for the sidebar and the
    /// CLI listing. Internal runner records are bookkeeping, not choices, so
    /// they never appear.
    pub fn list(&self) -> Vec<(String, Connection)> {
        let mut all: Vec<(String, Connection)> = self
            .load()
            .into_iter()
            .filter(|(_, entry)| !entry.internal)
            .collect();
        all.sort_by(|a, b| b.1.created.cmp(&a.1.created).then(a.0.cmp(&b.0)));
        all
    }

    pub fn remove(&self, name: &str) -> bool {
        let mut all = self.load();
        let removed = all.remove(name).is_some();
        if removed {
            let _ = self.save(&all);
        }
        removed
    }

    /// Drop every connection registered by `origin`. Called when a pairing is
    /// revoked, so revoking the browser's access also revokes what it handed
    /// to the agents on this machine.
    pub fn remove_origin(&self, origin: &str) -> usize {
        let mut all = self.load();
        let before = all.len();
        all.retain(|_, entry| entry.origin != origin);
        let removed = before - all.len();
        if removed > 0 {
            let _ = self.save(&all);
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, ConnectionStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ConnectionStore::new(dir.path());
        (dir, store)
    }

    #[test]
    fn names_are_derived_readably_and_made_unique() {
        assert_eq!(derive_name("My Thesis!", &|_| false), "my-thesis");
        assert_eq!(derive_name("", &|_| false), "document");
        let taken = |c: &str| c == "notes";
        assert_eq!(derive_name("Notes", &taken), "notes-2");
    }

    #[test]
    fn a_registered_document_resolves_to_its_protected_link() {
        let (_dir, store) = store();
        let name = store
            .register(
                "My Thesis",
                "https://d.example/docs/a#k=secret",
                "o",
                "editor",
            )
            .unwrap();
        assert_eq!(name, "my-thesis");
        assert_eq!(
            store.resolve(&name).unwrap().link,
            "https://d.example/docs/a#k=secret"
        );
    }

    #[test]
    fn the_same_document_from_one_origin_keeps_one_name() {
        let (_dir, store) = store();
        let first = store
            .register("Thesis", "https://d.example/docs/a#k=s", "o", "commenter")
            .unwrap();
        let second = store
            .register("Thesis", "https://d.example/docs/a#k=s", "o", "editor")
            .unwrap();
        assert_eq!(first, second);
        // The later registration's access wins, so upgrading a connection in
        // the sidebar does not leave the stale role on display.
        assert_eq!(store.get(&first).unwrap().access, "editor");
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn revoking_an_origin_takes_its_connections() {
        let (_dir, store) = store();
        store
            .register("A", "https://d.example/docs/a#k=1", "one", "editor")
            .unwrap();
        store
            .register("B", "https://d.example/docs/b#k=2", "two", "editor")
            .unwrap();
        assert_eq!(store.remove_origin("one"), 1);
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn an_unknown_name_says_what_to_do_about_it() {
        let (_dir, store) = store();
        let error = store.resolve("missing").unwrap_err();
        assert!(error.contains("reconnect the document"), "{error}");
        assert!(store.resolve("Not A Name").unwrap_err().contains("invalid"));
    }

    #[test]
    fn an_idle_connection_expires_rather_than_keeping_a_key_forever() {
        let (_dir, store) = store();
        let name = store
            .register("Old", "https://d.example/docs/a#k=s", "o", "editor")
            .unwrap();
        let mut all: BTreeMap<String, Connection> =
            read_json(&store.path()).expect("stored connections");
        let stale = now_unix() - IDLE_TTL_SECONDS - 1;
        let entry = all.get_mut(&name).unwrap();
        entry.created = stale;
        entry.used = stale;
        write_private_json(&store.path(), &all).unwrap();
        assert!(store.resolve(&name).is_err());
    }
}
