//! Runner records: the indirection that keeps a document key out of an
//! agent's configuration file.
//!
//! An agent is used only from the browser sidebar. The sidebar assistant's
//! own MCP subprocess names a runner record (`librepaper mcp
//! --connection runner-<hash>`), never a link. This machine holds the
//! mapping from that name to the protected document URL, in
//! `<state_home>/librepaper/local/connections.json` at mode 0600, next to the
//! pairing store that authorizes writing it.
//!
//! Three things follow, and all three are the reason for the indirection:
//! the key never reaches a config file a user might paste or commit; a
//! rotated link is repaired in one place without the user editing any agent's
//! configuration; and revoking an agent's access is deleting one row rather
//! than hunting through the config of every tool that was ever connected.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use super::pairing::write_private_json;
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
    pub created: i64,
    /// Last time an agent resolved this connection.
    #[serde(default)]
    pub used: i64,
    /// The sidebar conversation and its credential, for the adapter the
    /// sidebar assistant spawns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_token: Option<String>,
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

/// The private connection name a sidebar runner uses for one (document,
/// conversation) pair. Deterministic in both credential URL and conversation,
/// so the same runner rewrites the same record on every start instead of
/// leaving old ones behind.
pub fn runner_connection_name(credential_url: &str, conversation: &str) -> String {
    use sha2::Digest as _;
    let digest = hex::encode(sha2::Sha256::digest(
        format!("{credential_url}\0{conversation}").as_bytes(),
    ));
    format!("runner-{}", &digest[..48])
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

    fn lock_path(&self) -> PathBuf {
        self.dir.join("connections.lock")
    }

    fn load(&self) -> BTreeMap<String, Connection> {
        self.load_checked().unwrap_or_default()
    }

    fn load_checked(&self) -> Result<BTreeMap<String, Connection>, String> {
        let mut all: BTreeMap<String, Connection> = match std::fs::read(self.path()) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| format!("invalid connection store: {error}"))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(format!("could not read connection store: {error}")),
        };
        let now = now_unix();
        all.retain(|_, entry| {
            let last = entry.used.max(entry.created);
            now.saturating_sub(last) < IDLE_TTL_SECONDS
        });
        Ok(all)
    }

    fn save(&self, all: &BTreeMap<String, Connection>) -> std::io::Result<()> {
        write_private_json(&self.path(), all)
    }

    fn update<T>(
        &self,
        change: impl FnOnce(&mut BTreeMap<String, Connection>) -> Result<T, String>,
    ) -> Result<T, String> {
        std::fs::create_dir_all(&self.dir).map_err(|error| error.to_string())?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.lock_path())
            .map_err(|error| error.to_string())?;
        lock.lock_exclusive().map_err(|error| error.to_string())?;
        let mut all = self.load_checked()?;
        let result = change(&mut all)?;
        self.save(&all).map_err(|error| error.to_string())?;
        Ok(result)
    }

    /// Park the sidebar assistant's own credentials under
    /// [`runner_connection_name`], recorded with the real pairing origin so
    /// revoking that pairing also revokes this record. Replaced on every
    /// runner start.
    pub fn put_runner(
        &self,
        credential_url: &str,
        origin: &str,
        conversation: &str,
        chat_token: &str,
    ) -> Result<String, String> {
        let name = runner_connection_name(credential_url, conversation);
        let now = now_unix();
        self.update(|all| {
            all.insert(
                name.clone(),
                Connection {
                    link: credential_url.to_string(),
                    origin: origin.to_string(),
                    created: now,
                    used: now,
                    conversation: Some(conversation.to_string()),
                    chat_token: Some(chat_token.to_string()),
                },
            );
            Ok(name.clone())
        })
    }

    /// Resolve a name to its document link, renewing its idle clock. This is
    /// the call `librepaper mcp --connection` makes on startup.
    pub fn resolve(&self, name: &str) -> Result<Connection, String> {
        if !valid_name(name) {
            return Err("invalid connection name".into());
        }
        self.update(|all| {
            let Some(entry) = all.get_mut(name) else {
                return Err(format!(
                    "no connection named '{name}' on this computer; reconnect the document in your browser"
                ));
            };
            entry.used = now_unix();
            Ok(entry.clone())
        })
    }

    pub fn get(&self, name: &str) -> Option<Connection> {
        self.load().get(name).cloned()
    }

    /// Drop every connection registered by `origin`. Called when a pairing is
    /// revoked, so revoking the browser's access also revokes what it handed
    /// to the agents on this machine.
    pub fn remove_origin(&self, origin: &str) -> usize {
        self.update(|all| {
            let before = all.len();
            all.retain(|_, entry| entry.origin != origin);
            Ok(before - all.len())
        })
        .unwrap_or(0)
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
    fn a_runner_record_resolves_to_its_protected_link() {
        let (_dir, store) = store();
        let name = store
            .put_runner(
                "https://d.example/docs/a#k=secret",
                "https://origin.example",
                "conversation-1",
                "token-1",
            )
            .unwrap();
        assert_eq!(
            name,
            runner_connection_name("https://d.example/docs/a#k=secret", "conversation-1")
        );
        let resolved = store.resolve(&name).unwrap();
        assert_eq!(resolved.link, "https://d.example/docs/a#k=secret");
        assert_eq!(resolved.conversation.as_deref(), Some("conversation-1"));
        assert_eq!(resolved.chat_token.as_deref(), Some("token-1"));
    }

    #[test]
    fn restarting_the_same_runner_replaces_its_record() {
        let (_dir, store) = store();
        let name = store
            .put_runner(
                "https://d.example/docs/a#k=s",
                "https://origin.example",
                "conversation-1",
                "token-1",
            )
            .unwrap();
        let second = store
            .put_runner(
                "https://d.example/docs/a#k=s",
                "https://origin.example",
                "conversation-1",
                "token-2",
            )
            .unwrap();
        assert_eq!(name, second);
        assert_eq!(
            store.get(&name).unwrap().chat_token.as_deref(),
            Some("token-2")
        );
    }

    #[test]
    fn revoking_an_origin_takes_its_connections() {
        let (_dir, store) = store();
        store
            .put_runner("https://d.example/docs/a#k=1", "one", "c1", "t1")
            .unwrap();
        store
            .put_runner("https://d.example/docs/b#k=2", "two", "c2", "t2")
            .unwrap();
        assert_eq!(store.remove_origin("one"), 1);
        assert!(store
            .get(&runner_connection_name(
                "https://d.example/docs/b#k=2",
                "c2"
            ))
            .is_some());
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
            .put_runner("https://d.example/docs/a#k=s", "o", "c1", "t1")
            .unwrap();
        let mut all: BTreeMap<String, Connection> = {
            let bytes = std::fs::read(store.path()).unwrap();
            serde_json::from_slice(&bytes).unwrap()
        };
        let stale = now_unix() - IDLE_TTL_SECONDS - 1;
        let entry = all.get_mut(&name).unwrap();
        entry.created = stale;
        entry.used = stale;
        write_private_json(&store.path(), &all).unwrap();
        assert!(store.resolve(&name).is_err());
    }
}
