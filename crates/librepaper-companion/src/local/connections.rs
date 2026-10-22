//! Runner records: the indirection between a sidebar runner's document
//! bridge and the document it serves.
//!
//! An agent is used only from the browser sidebar. The sidebar assistant's
//! own MCP subprocess names a runner record (`librepaper mcp
//! --connection runner-<hash>`), never a link. This machine holds the
//! mapping from that name to the document URL, in
//! `<state_home>/librepaper/local/connections.json` at mode 0600.
//!
//! The record holds no secret. The URL carries no key, and the chat token is
//! the sidebar's own. The only credential the bridge uses is the scoped grant
//! file beside the record, and that lives for one run. The runner removes
//! both when it ends, so an agent that reads this directory later finds
//! nothing that opens a document.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::store::write_private_json;
use librepaper_base::util::now_unix;

/// How long a connection survives without being resolved. A runner removes
/// its record when it ends, so this only sweeps what a crash left behind.
pub const IDLE_TTL_SECONDS: i64 = 24 * 3600;

/// The variable that names a sidebar runner's grant file (see
/// [`ConnectionStore::grant_path`]) in its document bridge's environment, and
/// only there. Its presence is what tells `librepaper mcp` it is serving a
/// sidebar runner and must use that grant rather than this computer's login.
pub const GRANT_FILE_VARIABLE: &str = "LIBREPAPER_RUNNER_GRANT_FILE";

/// The variable that names the runner record `librepaper mcp` serves when no
/// `--connection` is given.
pub const CONNECTION_VARIABLE: &str = "LIBREPAPER_MCP_CONNECTION";

/// The variable that carries an agent token (`lpt_...`) for
/// `librepaper mcp --document`. It is read from the environment only, never
/// from a flag, so the token stays out of process listings.
pub const AGENT_TOKEN_VARIABLE: &str = "LIBREPAPER_AGENT_TOKEN";

/// The variable that points `librepaper mcp` at the companion's state home,
/// where connections.json lives. It takes precedence over the XDG default so
/// the agent's own XDG_STATE_HOME is never touched.
pub const STATE_HOME_VARIABLE: &str = "LIBREPAPER_BRIDGE_STATE_HOME";

/// One document an agent on this computer may reach.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Connection {
    /// The document URL, with no key. Opening the document takes the runner's
    /// scoped grant, which is never stored here.
    pub link: String,
    /// The browser origin that registered it.
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
/// conversation) pair. Deterministic in both the key-free document URL and
/// the conversation, so the same runner rewrites the same record on every
/// start instead of leaving old ones behind.
pub fn runner_connection_name(document_url: &str, conversation: &str) -> String {
    use sha2::Digest as _;
    let digest = hex::encode(sha2::Sha256::digest(
        format!("{document_url}\0{conversation}").as_bytes(),
    ));
    format!("runner-{}", &digest[..48])
}

impl ConnectionStore {
    /// `state_home` is an XDG state base, matching `ServiceFile::new`.
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

    /// Where a sidebar runner publishes the scoped grant its document bridge
    /// authenticates with. Beside the connection record rather than in the
    /// runner's own directory, because that directory is the agent's working
    /// directory and an agent reads its working directory freely. The runner
    /// removes the grant and its record together when it ends.
    pub fn grant_path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.grant"))
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
        lock.lock().map_err(|error| error.to_string())?;
        let mut all = self.load_checked()?;
        let result = change(&mut all)?;
        self.save(&all).map_err(|error| error.to_string())?;
        Ok(result)
    }

    /// Park the sidebar assistant's chat credential under
    /// [`runner_connection_name`], recorded with the requesting origin.
    /// Replaced on every runner start. A key fragment on the URL is dropped
    /// before anything is hashed or stored.
    pub fn put_runner(
        &self,
        document_url: &str,
        origin: &str,
        conversation: &str,
        chat_token: &str,
    ) -> Result<String, String> {
        let mut url = url::Url::parse(document_url)
            .map_err(|error| format!("invalid document URL: {error}"))?;
        url.set_fragment(None);
        let document_url = url.to_string();
        let name = runner_connection_name(&document_url, conversation);
        let now = now_unix();
        self.update(|all| {
            all.insert(
                name.clone(),
                Connection {
                    link: document_url,
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

    #[cfg(test)]
    pub(crate) fn replace_for_test(&self, name: &str, entry: Connection) {
        let mut all = self.load();
        all.insert(name.to_string(), entry);
        self.save(&all).unwrap();
    }

    /// Delete a record only if it is still the one `seen` describes. A
    /// restarted runner replaces its record before the old runner's teardown
    /// runs, and that teardown must not delete the new runner's record.
    pub fn remove_if_unchanged(&self, name: &str, seen: &Connection) -> Result<(), String> {
        self.update(|all| {
            if all
                .get(name)
                .is_some_and(|now| now.created == seen.created && now.chat_token == seen.chat_token)
            {
                all.remove(name);
            }
            Ok(())
        })
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
    fn a_runner_record_resolves_to_its_key_free_link() {
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
            runner_connection_name("https://d.example/docs/a", "conversation-1")
        );
        let resolved = store.resolve(&name).unwrap();
        assert_eq!(resolved.link, "https://d.example/docs/a");
        let on_disk = std::fs::read_to_string(store.path()).unwrap();
        assert!(!on_disk.contains("secret"), "{on_disk}");
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
    fn remove_if_unchanged_leaves_a_replaced_record() {
        let (_dir, store) = store();
        let name = store
            .put_runner("https://d.example/docs/a", "o", "c1", "t1")
            .unwrap();
        let seen = store.get(&name).unwrap();
        let mut all = store.load();
        all.get_mut(&name).unwrap().created += 1;
        store.save(&all).unwrap();
        store.remove_if_unchanged(&name, &seen).unwrap();
        assert!(store.get(&name).is_some());

        let current = store.get(&name).unwrap();
        store.remove_if_unchanged(&name, &current).unwrap();
        assert!(store.get(&name).is_none());
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
