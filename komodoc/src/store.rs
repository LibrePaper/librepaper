//! The document store: the index of what exists, and the bytes of each
//! version.
//!
//! Where those bytes live is not its business -- see blob.rs. What is here is
//! the interesting half: who owns a document, what a deployment will hold, and
//! the compare-and-swap that keeps the index honest when two writes race.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::blob::{
    document_key, document_prefix, examples_key, legacy_source_key, room_key, room_lock_key,
    source_key, source_prefix, BlobError, BlobStore, BlobVersion, INDEX_KEY,
};
use crate::clock::{now_unix, parse_timestamp, timestamp};
use crate::config::Configuration;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct IndexEntry {
    pub slug: String,
    pub title: String,
    pub sha: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub example: bool,
    /// The lowercased GitHub login that uploaded this version, and the only
    /// account that may replace or delete it. Empty on a reserved example, and
    /// on anything published before ownership was recorded or on a deployment
    /// where publishing needs no account at all.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub publisher: String,
    /// The GitHub account's numeric id, set alongside `publisher` on every new
    /// upload from a signed-in caller. A login can be renamed; the numeric id
    /// cannot, so `owned_by` prefers it when a document carries one. Never set
    /// for a visitor-owned or unowned document, since neither has a GitHub
    /// account behind it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub publisher_id: String,
    /// The bytes of the stored HTML -- plus the source beside it, when one is
    /// kept -- and what the storage quotas are measured against. An entry from
    /// before it was recorded reads back as zero, which `admit` treats as free
    /// rather than refusing every upload until each old document is replaced.
    #[serde(default)]
    pub size: i64,
    /// What the stored source is, when the document was published from one.
    /// Empty means there is no source to reopen.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_format: String,
}

impl IndexEntry {
    /// Whether a caller -- named by their owner key (see `Server::owner`) and,
    /// when signed in, their GitHub numeric id -- may replace or delete this
    /// document. An entry with no publisher belongs to no one in particular
    /// and stays shared. An entry carrying a publisher id compares against the
    /// id instead of the key, since the id survives a GitHub account being
    /// renamed and the key would not; a legacy entry, or one owned by a
    /// visitor: key, has no publisher id and falls back to comparing the key.
    pub fn owned_by(&self, owner_key: &str, caller_id: &str) -> bool {
        if self.publisher.is_empty() {
            true
        } else if !self.publisher_id.is_empty() {
            !caller_id.is_empty() && caller_id == self.publisher_id
        } else {
            self.publisher == owner_key.to_lowercase()
        }
    }

    /// The highest role a caller holds on this document. One function, asked
    /// by every route, so that rights are decided in one place rather than in
    /// each handler that happens to need them: `owned_by` answers whether
    /// somebody is the owner, and this answers what they may do, which is the
    /// question the routes were really asking.
    ///
    /// `may_comment` is the deployment's own switch, which is a ceiling: a
    /// document may be stricter than its server and never wider.
    pub fn role_of(&self, owner_key: &str, caller_id: &str, may_comment: bool) -> Role {
        if self.owned_by(owner_key, caller_id) {
            Role::Owner
        } else if may_comment {
            Role::Commenter
        } else {
            Role::Reader
        }
    }

    /// When this document expires from, as seconds since the epoch.
    pub fn expiry_time(&self, from: &str) -> Option<i64> {
        parse_timestamp(if from == "created" {
            &self.created_at
        } else {
            &self.updated_at
        })
    }
}

/// What a caller may do to a document, as a ladder: each role includes the
/// ones beneath it. Reading has never had a switch, so `reader` is what
/// everybody who can reach a document holds; `editor` sits between commenting
/// and owning and is held today by the owner alone, until a document can name
/// somebody.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
// `Editor` is a rung nobody stands on yet: the owner is the only editor a
// document can have until it can name one, and the rung exists so the gates
// can ask the question they mean ("may this caller edit") rather than the one
// that happens to answer it today ("does this caller own it").
#[allow(dead_code)]
pub enum Role {
    Reader,
    Commenter,
    Editor,
    Owner,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Reader => "reader",
            Role::Commenter => "commenter",
            Role::Editor => "editor",
            Role::Owner => "owner",
        }
    }

    /// Whether this role includes another: the ladder, asked as a question.
    pub fn at_least(self, wanted: Role) -> bool {
        self >= wanted
    }
}

pub struct Store {
    /// Where the bytes are: a directory, or somebody else's S3. The store does
    /// not care which.
    pub blobs: Arc<dyn BlobStore>,
    pub config: Arc<Configuration>,
    pub state: Mutex<StoreState>,
}

pub struct StoreState {
    pub entries: HashMap<String, IndexEntry>,
    /// The version of index.json these entries were read from, so a write can
    /// say what it expects to be replacing. Empty means "there was no index",
    /// which is how a fresh store starts.
    pub index_version: BlobVersion,
}

/// Reads index.json, with the version it was read at. No index yet is an
/// empty store, which is how a fresh one starts. An index that exists but
/// cannot be parsed is a different thing entirely, and an error rather than an
/// empty map: carrying on would present every stored document as gone, and
/// the next publish would overwrite the real index with a near-empty one.
pub async fn load_index(
    blobs: &dyn BlobStore,
) -> Result<(HashMap<String, IndexEntry>, BlobVersion), String> {
    match blobs.get_versioned(INDEX_KEY).await {
        Err(BlobError::NotFound) => Ok((HashMap::new(), String::new())),
        Err(err) => Err(format!(
            "could not read the index from {}: {err}",
            blobs.describe()
        )),
        Ok((raw, at)) => {
            let entries: HashMap<String, IndexEntry> =
                serde_json::from_slice(&raw).map_err(|err| {
                    format!(
                        "the index in {} is not readable ({err}); move it aside to start empty",
                        blobs.describe()
                    )
                })?;
            Ok((entries, at))
        }
    }
}

/// A document as it is created. There is one version of it from here on, and
/// what is stored is its source: nothing derived is kept, because every
/// browser that shows the document renders it. So there is no HTML here, and
/// no digest of any.
#[derive(Clone, Debug, Default)]
pub struct Publication {
    pub slug: String,
    pub title: String,
    /// The document itself. A document published as HTML has HTML for its
    /// source and the identity for its renderer.
    pub source: String,
    pub source_format: String,
    pub owner: String,
    pub owner_id: String,
}

/// What `put` returns when a storage rule refuses an upload: the HTTP status
/// and message the rule names, so the handler answers with exactly what the
/// rule decided.
#[derive(Debug)]
pub enum PutError {
    Quota { status: u16, message: &'static str },
    Storage(String),
}

impl std::fmt::Display for PutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PutError::Quota { message, .. } => write!(f, "{message}"),
            PutError::Storage(message) => write!(f, "{message}"),
        }
    }
}

impl Store {
    pub async fn open(
        blobs: Arc<dyn BlobStore>,
        config: Arc<Configuration>,
    ) -> Result<Store, String> {
        let (entries, index_version) = load_index(blobs.as_ref()).await?;
        Ok(Store {
            blobs,
            config,
            state: Mutex::new(StoreState {
                entries,
                index_version,
            }),
        })
    }

    pub async fn get(&self, slug: &str) -> Option<IndexEntry> {
        self.state.lock().await.entries.get(slug).cloned()
    }

    /// Every document, newest first, as the listing endpoint wants.
    pub async fn list(&self) -> Vec<IndexEntry> {
        let state = self.state.lock().await;
        let mut documents: Vec<IndexEntry> = state.entries.values().cloned().collect();
        documents.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        documents
    }

    /// The stored source of a document: the source of the version the index
    /// names, and no other. A document published before sources were versioned
    /// has one unversioned key instead, which is read when there is nothing
    /// under the digest.
    pub async fn read_source(&self, slug: &str) -> Result<Vec<u8>, BlobError> {
        let digest = {
            let state = self.state.lock().await;
            match state.entries.get(slug) {
                Some(entry) => entry.sha.clone(),
                None => return Err(BlobError::NotFound),
            }
        };
        match self.blobs.get(&source_key(slug, &digest)).await {
            Err(BlobError::NotFound) => self.blobs.get(&legacy_source_key(slug)).await,
            other => other,
        }
    }

    pub async fn read(&self, slug: &str, digest: &str) -> Result<Vec<u8>, BlobError> {
        self.blobs.get(&document_key(slug, digest)).await
    }

    /// Names a document in the index. It writes no bytes of the document
    /// itself: the source becomes the first checkpoint, which the room writes,
    /// and from then on the document is the session. What is decided here is
    /// the half that has always been decided here -- whether this deployment
    /// will hold another document, and whose it is.
    ///
    /// Admission and the index mutation happen under the same lock, so two
    /// uploads racing for the last of a quota cannot both be admitted.
    pub async fn put(&self, v: Publication) -> Result<IndexEntry, PutError> {
        let size = v.source.len() as i64;
        let mut state = self.state.lock().await;
        self.admit(&state, &v.slug, &v.owner, size, now_unix())?;
        let now = timestamp();
        let mut created = now.clone();
        let mut example = false;
        let (mut owner, mut owner_id) = (v.owner, v.owner_id);
        if let Some(existing) = state.entries.get(&v.slug) {
            created = existing.created_at.clone();
            // A replacement keeps what the document already is: an example
            // stays an example, and its publisher -- and publisher id -- do not
            // change hands. A document with no publisher belongs to no one in
            // particular and stays that way: the first person to save it must
            // not become its owner, or everyone else loses the document they
            // were editing.
            example = existing.example;
            owner = existing.publisher.clone();
            owner_id = existing.publisher_id.clone();
        }
        let entry = IndexEntry {
            slug: v.slug.clone(),
            title: v.title,
            // The digest of the source, which is the checkpoint the room is
            // about to write. From here the index's `sha` names the newest
            // checkpoint rather than an HTML object.
            sha: digest_of(&v.source),
            size,
            created_at: created,
            updated_at: now,
            example,
            publisher: owner.to_lowercase(),
            publisher_id: owner_id,
            source_format: v.source_format,
        };
        let previous = state.entries.insert(v.slug.clone(), entry.clone());
        if let Err(err) = self.save_locked(&mut state).await {
            // The index naming the document is not durable, so the document
            // does not exist as far as any later run is concerned. Undo the
            // in-memory half rather than report a success that will vanish.
            match previous {
                Some(previous) => state.entries.insert(v.slug.clone(), previous),
                None => state.entries.remove(&v.slug),
            };
            return Err(PutError::Storage(err.to_string()));
        }
        Ok(entry)
    }

    /// Removes what the old layout kept: the rendered HTML of every version,
    /// and the sources beside them. Called once a document's source is durable
    /// as a checkpoint, and never before, so the copy that goes is a copy and
    /// not the document. A failure leaves objects behind for a later pass and
    /// is not worth reporting: nothing depends on them any more.
    pub async fn drop_derived(&self, slug: &str) {
        for prefix in [document_prefix(slug), source_prefix(slug)] {
            if let Ok(found) = self.blobs.list(&prefix).await {
                let keys: Vec<String> = found.into_iter().map(|object| object.key).collect();
                if !keys.is_empty() {
                    let _ = self.blobs.delete(&keys).await;
                }
            }
        }
        // On its own, for the reason `remove` gives: on a directory store the
        // versioned sources live under this key's own name.
        let _ = self.blobs.delete(&[legacy_source_key(slug)]).await;
    }

    /// Records what a document's session and history now cost, and -- when a
    /// checkpoint has just been taken -- the checkpoint the index names. This
    /// is the third step of a checkpoint, between the state and the manifest.
    ///
    /// A checkpoint is never refused for a quota, because refusing it would
    /// lose work; `room_for` below is what the room sheds against instead. So
    /// this records rather than admits.
    pub async fn record_history(
        &self,
        slug: &str,
        sha: Option<&str>,
        size: i64,
        format: &str,
    ) -> Result<(), String> {
        let mut state = self.state.lock().await;
        let Some(entry) = state.entries.get(slug).cloned() else {
            return Ok(());
        };
        let mut updated = entry.clone();
        updated.size = size;
        // The room is the authority on what the document is written in: a
        // migrated document whose source was gone opens as the page it was
        // published as, and the index has to say so or the browser fetches a
        // renderer for a format the document is not in.
        if !format.is_empty() {
            updated.source_format = format.to_string();
        }
        if let Some(sha) = sha {
            updated.sha = sha.to_string();
            updated.updated_at = timestamp();
        }
        if updated.size == entry.size
            && updated.sha == entry.sha
            && updated.source_format == entry.source_format
        {
            return Ok(());
        }
        state.entries.insert(slug.to_string(), updated);
        if let Err(err) = self.save_locked(&mut state).await {
            // The index did not move, so neither does the copy of it in
            // memory: a size recorded here and nowhere else would make the
            // next quota decision from a number no later run can see.
            state.entries.insert(slug.to_string(), entry);
            return Err(err.to_string());
        }
        Ok(())
    }

    /// Renames a document. A publish onto an existing slug is an edit into its
    /// session rather than a new version, so the title is the one thing about
    /// the index entry such a publish still changes.
    pub async fn rename(&self, slug: &str, title: &str) -> Result<(), String> {
        let mut state = self.state.lock().await;
        let Some(entry) = state.entries.get(slug).cloned() else {
            return Ok(());
        };
        if entry.title == title || title.is_empty() {
            return Ok(());
        }
        let mut updated = entry.clone();
        updated.title = title.to_string();
        updated.updated_at = timestamp();
        state.entries.insert(slug.to_string(), updated);
        if let Err(err) = self.save_locked(&mut state).await {
            state.entries.insert(slug.to_string(), entry);
            return Err(err.to_string());
        }
        Ok(())
    }

    /// How many bytes this document may occupy before it carries its owner or
    /// the deployment over a ceiling. `None` for a document with no index
    /// entry, which has no owner to charge and no ceiling to reach.
    pub async fn room_for(&self, slug: &str) -> Option<i64> {
        let state = self.state.lock().await;
        let entry = state.entries.get(slug)?;
        let owner = entry.publisher.clone();
        let (mut total, mut mine) = (0i64, 0i64);
        for (key, other) in &state.entries {
            if key == slug {
                continue;
            }
            total += other.size;
            if other.publisher == owner {
                mine += other.size;
            }
        }
        let limits = self.config.storage;
        Some((limits.total - total).min(limits.per_owner - mine).max(0))
    }

    /// Enforces the storage ceilings a write must clear, under the lock that
    /// makes its index entry. `owner` is the caller's owner key exactly as
    /// `Server::owner` returns it -- a GitHub login, a visitor key, or "" for
    /// the one bucket every unidentified caller shares.
    fn admit(
        &self,
        state: &StoreState,
        slug: &str,
        owner: &str,
        size: i64,
        now: i64,
    ) -> Result<(), PutError> {
        let existing = state.entries.get(slug);
        let replacing = existing.is_some();
        let previous_size = existing.map(|e| e.size).unwrap_or(0);

        let (mut total_bytes, mut owner_bytes) = (0i64, 0i64);
        let mut owner_documents = 0usize;
        let mut owner_uploads_this_hour = 0usize;
        let cutoff = now - 3600;
        for (key, entry) in &state.entries {
            total_bytes += entry.size;
            if entry.publisher != owner {
                continue;
            }
            owner_bytes += entry.size;
            // The document being replaced is not a new document, and is not
            // counted again against the count it already counts toward.
            if key != slug {
                owner_documents += 1;
            }
            if parse_timestamp(&entry.updated_at).is_some_and(|updated| updated > cutoff) {
                owner_uploads_this_hour += 1;
            }
        }
        total_bytes += size - previous_size;
        owner_bytes += size - previous_size;

        let limits = self.config.storage;
        if total_bytes > limits.total {
            return Err(PutError::Quota {
                status: 507,
                message: "this deployment has no room left",
            });
        }
        if owner_bytes > limits.per_owner {
            return Err(PutError::Quota {
                status: 507,
                message: "your storage quota is used up; delete a document first",
            });
        }
        if !replacing && owner_documents >= limits.documents_per_owner {
            return Err(PutError::Quota {
                status: 507,
                message: "you have reached the document limit; delete one first",
            });
        }
        if owner_uploads_this_hour >= limits.uploads_per_hour {
            return Err(PutError::Quota {
                status: 429,
                message: "too many uploads this hour; try later",
            });
        }
        Ok(())
    }

    /// Deletes every stored version of a document and its index entry,
    /// returning how many versions went. The index entry goes last: until it
    /// does the document is still listed, which is a better half-state than a
    /// listing pointing at nothing.
    pub async fn remove(&self, slug: &str) -> Result<usize, String> {
        let mut removed = 0;
        if let Ok(found) = self.blobs.list(&document_prefix(slug)).await {
            let keys: Vec<String> = found.into_iter().map(|o| o.key).collect();
            if !keys.is_empty() && self.blobs.delete(&keys).await.is_ok() {
                removed = keys.len();
            }
        }
        // The sources are not versions of the document, so they are not
        // counted among them; they go with it all the same.
        if let Ok(found) = self.blobs.list(&source_prefix(slug)).await {
            let sources: Vec<String> = found.into_iter().map(|o| o.key).collect();
            if !sources.is_empty() {
                let _ = self.blobs.delete(&sources).await;
            }
        }
        // Alone: on a directory store the versioned sources live under this
        // key's own name, so removing it is a request to remove a directory,
        // and a store that refuses would take the rest of a batch down with it.
        let _ = self.blobs.delete(&[legacy_source_key(slug)]).await;
        // The history and the live document go with the document, which is
        // what destroy has promised in the README since before there was a
        // history to delete.
        if let Ok(found) = self.blobs.list(&crate::blob::history_prefix(slug)).await {
            let keys: Vec<String> = found.into_iter().map(|o| o.key).collect();
            if !keys.is_empty() {
                let _ = self.blobs.delete(&keys).await;
            }
        }
        let _ = self
            .blobs
            .delete(&[
                examples_key(slug),
                room_key(slug),
                room_lock_key(slug),
                crate::blob::session_key(slug),
            ])
            .await;

        let mut state = self.state.lock().await;
        state.entries.remove(slug);
        self.save_locked(&mut state)
            .await
            .map_err(|err| err.to_string())?;
        Ok(removed)
    }

    /// Writes the index, and only over the version these entries were read
    /// from. On a single-writer deployment the mutex already guarantees that,
    /// and the check costs a comparison; on a bucket two processes can reach,
    /// it is what stops one of them overwriting the other's documents. A
    /// conflict is reported rather than retried, because what to do about it
    /// is the caller's decision.
    pub async fn save_locked(&self, state: &mut StoreState) -> Result<(), BlobError> {
        let raw =
            serde_json::to_vec(&state.entries).map_err(|err| BlobError::Other(err.to_string()))?;
        let at = self
            .blobs
            .swap(INDEX_KEY, raw, &state.index_version)
            .await?;
        state.index_version = at;
        Ok(())
    }
}

/// Makes a new document's link unguessable.
pub fn random_suffix(config: &Configuration) -> String {
    let alphabet = config.suffix_alphabet.as_bytes();
    crate::auth::random_bytes(config.suffix_length)
        .into_iter()
        .map(|b| alphabet[b as usize % alphabet.len()] as char)
        .collect()
}

/// The same title yields the same slug on every backend.
pub fn slugify(value: &str, config: &Configuration) -> String {
    let mut out = String::new();
    let mut previous_dash = false;
    for c in value.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            previous_dash = false;
        } else if !previous_dash {
            out.push('-');
            previous_dash = true;
        }
    }
    let slug: String = out
        .trim_matches('-')
        .chars()
        .take(config.slug_max)
        .collect();
    slug.trim_matches('-').to_string()
}

/// The suffix a seeded example gets instead of a random one. The examples are
/// the documents whose links are written down -- in the README, in a talk, in
/// a bookmark -- and re-seeding used to mint a new random suffix and break
/// every one of those links. Deriving the suffix from the base makes it stable
/// across a reseed, while still looking like the random suffix every other
/// document carries. It is not a secret: an example is public on purpose.
pub fn example_suffix(base: &str, config: &Configuration) -> String {
    let sum = Sha256::digest(format!("komodoc example {base}").as_bytes());
    let alphabet = config.suffix_alphabet.as_bytes();
    sum.iter()
        .take(config.suffix_length)
        .map(|b| alphabet[*b as usize % alphabet.len()] as char)
        .collect()
}

/// The content hash a document is addressed by.
pub fn digest_of(html: &str) -> String {
    hex::encode(Sha256::digest(html.as_bytes()))
}
