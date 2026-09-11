//! The document store: the index of what exists, and the bytes of each
//! version.
//!
//! Where those bytes live is not its business -- see blob.rs. What is here is
//! the interesting half: who owns a document, what a deployment will hold, and
//! the compare-and-swap that keeps the index honest when two writes race.
//!
//! **Coherence model.** Each `Store` keeps its own copy of the index in
//! memory and consults only that copy on an ordinary read. A single active
//! writer per deployment is what this is built for, and there the in-memory
//! copy is always current, because nothing else ever moves the index out
//! from under it. A second instance sharing the same storage -- two live
//! HTTP servers behind one bucket -- does not see the first instance's
//! writes as they happen; it *converges*, not instantly, at two specific
//! moments: a write of its own that loses the compare-and-swap reloads the
//! winning index and retries once, and a `get` that misses reloads before
//! answering not-found, so a document created elsewhere becomes visible.
//! Between those moments a second instance can still authorize a read or a
//! grant against an index the first has already moved past. That is a
//! deliberate trade -- correctness on every write and on every miss, without
//! putting a storage round trip on every hit -- and it means anything that
//! must observe a revocation the instant it lands belongs on the instance
//! that made it, not on this best-effort convergence.

use std::collections::HashMap;
use std::sync::Arc;

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::auth::stored_id;
use crate::config::Configuration;
use crate::storage::blob::{
    document_key, document_prefix, examples_key, legacy_source_key, room_key, room_lock_key,
    source_key, source_prefix, BlobError, BlobStore, BlobVersion, INDEX_KEY,
};
use crate::storage::catalog::{
    Account, Catalog, CatalogError, CheckpointAssetRef, OperationActor, OperationRequest,
    SourceHistoryRecord,
};
use crate::util::new_id;
use crate::util::{now_unix, parse_timestamp, timestamp};

const MAX_GRANTS_PER_RESULT: i64 = 256;
const MAX_LINKS_PER_RESULT: i64 = 16;
const MAX_GUESTS_PER_RESULT: i64 = 256;

/// Validate a native source-history receipt before startup rolls it forward.
/// Unlike the normal read API this deliberately never falls back to a legacy
/// whole-file object: a receipt that names the new recipe/chunk graph must
/// prove that graph is complete, or remain pending for later reconciliation.
async fn validate_staged_source_graph(
    blobs: &dyn BlobStore,
    storage_id: &str,
    tree_bytes: &[u8],
    tree_sha: &str,
    sources: &[SourceHistoryRecord],
    assets: Option<&[CheckpointAssetRef]>,
) -> bool {
    let tree: crate::document::history::Tree = match serde_json::from_slice(tree_bytes) {
        Ok(tree) => tree,
        Err(_) => return false,
    };
    if tree.digest() != tree_sha {
        return false;
    }
    if tree.files.values().any(|entry| {
        entry.kind == "text"
            && !sources.iter().any(|source| {
                source.file_digest == entry.sha && source.uncompressed_bytes == entry.size
            })
    }) {
        return false;
    }
    let has_tree_assets = tree.files.values().any(|entry| entry.kind == "asset");
    let tree_assets = tree.files.values().filter(|entry| entry.kind == "asset");
    let staged_assets = match assets {
        Some(assets) => assets,
        // A native staged graph with asset entries must carry the immutable
        // asset root set. Treat an absent property as incomplete rather than
        // allowing startup to commit a checkpoint that GC cannot protect.
        None if has_tree_assets => return false,
        None => &[],
    };
    for entry in tree_assets {
        if entry.sha.is_empty() || entry.size < 0 {
            return false;
        }
        let key = crate::storage::blob::asset_key(storage_id, &entry.sha);
        let Some(asset) = staged_assets.iter().find(|asset| asset.object_key == key) else {
            return false;
        };
        if asset.bytes != entry.size {
            return false;
        }
        let bytes = match blobs.get(&key).await {
            Ok(bytes) => bytes,
            Err(_) => return false,
        };
        if bytes.len() as i64 != asset.bytes {
            return false;
        }
        let expected = entry.sha.clone();
        if !tokio::task::spawn_blocking(move || hex::encode(Sha256::digest(&bytes)) == expected)
            .await
            .unwrap_or(false)
        {
            return false;
        }
    }
    if staged_assets.iter().any(|asset| {
        !tree.files.values().any(|entry| {
            entry.kind == "asset"
                && entry.sha
                    == asset
                        .object_key
                        .strip_prefix(&crate::storage::blob::asset_prefix(storage_id))
                        .unwrap_or_default()
                && entry.size == asset.bytes
        })
    }) {
        return false;
    }
    for source in sources {
        let expected_file_digest = match hex::decode(&source.file_digest) {
            Ok(bytes) if bytes.len() == 32 => bytes,
            _ => return false,
        };
        if source.recipe_key
            != crate::storage::blob::content_recipe_key(storage_id, &source.file_digest)
            || source.recipe_digest.is_empty()
        {
            return false;
        }
        if !tree
            .files
            .values()
            .any(|entry| entry.sha == source.file_digest)
        {
            return false;
        }
        let recipe_bytes = match blobs.get(&source.recipe_key).await {
            Ok(bytes) => bytes,
            Err(_) => return false,
        };
        let recipe = match crate::storage::encoding::Recipe::from_bytes(&recipe_bytes) {
            Ok(recipe) => recipe,
            Err(_) => return false,
        };
        if recipe.file_digest.as_slice() != expected_file_digest.as_slice()
            || hex::encode(recipe.digest()) != source.recipe_digest
            || recipe_bytes.len() as i64 != source.recipe_bytes
            || recipe.uncompressed_len != source.uncompressed_bytes as u64
            || recipe.codec as i64 != source.codec
        {
            return false;
        }
        let mut objects = HashMap::with_capacity(recipe.chunks.len());
        for reference in &recipe.chunks {
            let key =
                crate::storage::blob::content_chunk_key(storage_id, &hex::encode(reference.digest));
            let bytes = match blobs.get(&key).await {
                Ok(bytes) => bytes,
                Err(_) => return false,
            };
            objects.insert(reference.digest, bytes);
        }
        // Check every descriptor too, including reused chunks, so a partial
        // graph cannot be rescued by an unrelated legacy source object.
        for object in &source.objects {
            let bytes = match blobs.get(&object.object_key).await {
                Ok(bytes) => bytes,
                Err(_) => return false,
            };
            if object.bytes > 0 && bytes.len() as i64 != object.bytes {
                return false;
            }
        }
        // Reconstruction verifies both chunk and complete-file digests. Keep
        // recovery decompression off the asynchronous runtime as well.
        let verified = tokio::task::spawn_blocking(move || {
            crate::storage::encoding::reconstruct(&recipe, |digest| {
                objects.get(digest).cloned().ok_or_else(|| {
                    crate::storage::encoding::EncodingError::Integrity(
                        "recipe object is missing".into(),
                    )
                })
            })
            .map(|_| ())
        })
        .await;
        if !matches!(verified, Ok(Ok(()))) {
            return false;
        }
    }
    true
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct IndexEntry {
    pub slug: String,
    /// Immutable catalogue identity. Older JSON entries have no identity and
    /// continue to use their slug as the object prefix until republished.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub storage_id: String,
    pub title: String,
    pub sha: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub example: bool,
    #[serde(skip)]
    pub unowned: bool,
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
    /// What other readers see the owner called. The share dialog shows the
    /// owner to everyone named on the document, and for a Google account the
    /// handle in `publisher` is an email address, which is shown to nobody;
    /// so the name is recorded beside it whenever an identity is in hand. An
    /// entry from before names were recorded is a GitHub entry, whose login
    /// is its name, and reads back through `owner_name`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub publisher_name: String,
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
    /// The path of the document's main file: the one a renderer is run on, and
    /// the one `source_format` is derived from. A document is a directory, and
    /// which of its files is the document is declared rather than discovered --
    /// `\include` and `#import` are not parsed, because TeX cannot be resolved
    /// statically and a guess that is wrong is worse than a list. Empty on an
    /// entry written before there were directories, which reads as the one
    /// file such a document has.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub main: String,
    /// The accounts this document names, by role. A grant by name is keyed on
    /// the GitHub numeric id, exactly as ownership is, so it survives a rename
    /// and follows the person across browsers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub editors: Vec<Grant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commenters: Vec<Grant>,
    /// The links that carry a role: at most one per role, since minting a
    /// role's link again rotates it rather than adding a second one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<LinkGrant>,
    /// Accounts that opened this document through a link while signed in. A
    /// link names nobody, so this is how an owner learns who is actually on
    /// the other end of one without either side signing anything more than
    /// being logged in. Pruned whenever the link that let a guest in is
    /// rotated or dropped, since a guest with no live link behind it is not a
    /// guest of anything any more.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guests: Vec<Guest>,
}

/// One account that opened a document through a link while signed in. `link`
/// is the hash of the link that let them in, which is what `prune_guests`
/// matches against when a link is rotated or revoked.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Guest {
    pub id: String,
    pub name: String,
    pub since: String,
    pub link: String,
}

/// One account named on a document. The id is what the grant is matched on;
/// `login` holds the handle -- a GitHub login, or a Google account's verified
/// email -- which is what the owner typed and what a revoke names; `name` is
/// what the dialog shows to everyone else. The field is still called `login`
/// because it is a serialised name in every index already written.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Grant {
    pub id: String,
    pub login: String,
    #[serde(default)]
    pub since: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
}

impl Grant {
    /// What to show for this grant: the recorded name, or the handle for a
    /// grant from before names were recorded, which is a GitHub login.
    pub fn shown(&self) -> &str {
        if self.name.is_empty() {
            &self.login
        } else {
            &self.name
        }
    }
}

/// One link that carries a role. `hash` is the SHA-256 of the key in hex, and
/// `until` is an expiry -- empty for none -- past which the link answers as no
/// link at all. `key` is the key itself, kept so the owner can copy the link
/// again rather than only ever seeing it once; a link written before this
/// field existed has an empty one and cannot be shown again, which is what
/// "legacy link, reset to get a new one" means. `label` is the owner's memo
/// for the link, and still deserialises labels written by older versions.
/// `budget` is the number of comment actions this link may make in one clock
/// hour; absent uses the deployment's ordinary numeric comment limit.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LinkGrant {
    pub hash: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<i64>,
    #[serde(default)]
    pub since: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub until: String,
}

impl LinkGrant {
    /// Whether this link still stands. An `until` that is not a timestamp is
    /// treated as expired rather than as absent: a date nothing here can read
    /// is not a reason to keep answering.
    pub fn live_at(&self, now: i64) -> bool {
        if self.until.is_empty() {
            return true;
        }
        parse_timestamp(&self.until).is_some_and(|until| now < until)
    }

    pub fn granted(&self) -> Role {
        Role::parse(&self.role).unwrap_or(Role::Reader)
    }
}

/// The most a document may open each role to one caller, which is what the
/// deployment's own switches say. `--publishers` and `--commenters` are
/// ceilings: a document may only ever be stricter than its server, so a grant
/// recorded while a switch was wide stops answering when the switch narrows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ceiling {
    /// Whether this caller may comment here at all.
    pub comment: bool,
    /// Whether this caller may publish here, and so be named an editor. A
    /// link names nobody, so it is asked with an empty handle, and that is
    /// true only under `--publishers anyone`: a link cannot edit anywhere
    /// this deployment would otherwise ask for a sign-in first.
    pub edit: bool,
}

impl IndexEntry {
    /// Whether a caller -- named by their owner key (see `Server::owner`) and,
    /// when signed in, their GitHub numeric id -- may replace or delete this
    /// document. An entry with no publisher grants ownership to nobody.
    /// An entry carrying a publisher id compares against the
    /// id instead of the key, since the id survives an account being renamed
    /// and the key would not; a legacy entry, or one owned by a visitor: key,
    /// has no publisher id and falls back to comparing the key.
    ///
    /// A publisher id written before providers existed is a bare number and
    /// means a GitHub account, so it is qualified before the comparison rather
    /// than the index being rewritten. That is also what keeps a Google `sub`
    /// out of a GitHub id's namespace: the two are both decimal strings, and
    /// only the prefix tells them apart.
    /// What to show for the owner: the recorded name, or the handle for an
    /// entry from before names were recorded, which is a GitHub login.
    pub fn owner_name(&self) -> &str {
        if self.publisher_name.is_empty() {
            &self.publisher
        } else {
            &self.publisher_name
        }
    }

    pub fn owned_by(&self, owner_key: &str, caller_id: &str) -> bool {
        if !self.publisher_id.is_empty() {
            !caller_id.is_empty() && caller_id == stored_id(&self.publisher_id)
        } else if self.publisher.is_empty() {
            // An absent credential is not an ownership credential. Seeded
            // examples and pre-identity legacy rows may be readable, but a
            // cookie-less caller must never acquire their mutation rights.
            false
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
    /// A caller is named by their owner key and, when signed in, their GitHub
    /// numeric id; `link_hash` is the digest of the key their request carried,
    /// or "" for none. `ceiling` is the deployment's own switches, which a
    /// document may be stricter than and never wider: a grant the switch does
    /// not allow this caller is still recorded, and simply is not honoured.
    pub fn role_of(
        &self,
        owner_key: &str,
        caller_id: &str,
        link_hash: &str,
        ceiling: Ceiling,
        now: i64,
    ) -> Role {
        if self.owned_by(owner_key, caller_id) {
            return Role::Owner;
        }
        let mut role = Role::Reader;
        // A named editor whom `--publishers` no longer allows keeps whatever
        // `--commenters` still gives them, rather than dropping to a reader.
        // This is legacy: no document is given a new named grant any more,
        // but one recorded before links existed is still honoured exactly
        // this way.
        if let Some(named) = self.named_role(caller_id) {
            if named == Role::Editor && ceiling.edit {
                role = role.max(Role::Editor);
            } else if ceiling.comment {
                role = role.max(Role::Commenter);
            }
        }
        // A link asks the same ceiling a named grant does, because a link is
        // not a different kind of caller, it is a caller with no name: an
        // editor link edits under exactly the switch that lets an anonymous
        // caller edit at all, and a reader link never has to ask, since
        // reading is the rung everybody who reaches the document already
        // holds.
        if let Some(by_link) = self.link_role(link_hash, now) {
            if by_link == Role::Editor && ceiling.edit {
                role = role.max(Role::Editor);
            } else if by_link.at_least(Role::Commenter) && ceiling.comment {
                role = role.max(Role::Commenter);
            }
        }
        // A read link is read-only: the switch is a ceiling on what a link may
        // carry, not a grant to whoever reaches the document. The examples are
        // the one exception when opened without a link, since they exist to
        // be commented on. An explicit Read link still means read-only.
        if ceiling.comment && self.example && link_hash.is_empty() {
            role = role.max(Role::Commenter);
        }
        role
    }

    /// The role this document names an account for, if any. A grant by name
    /// never matches a caller with no account, whose id is empty. A grant
    /// recorded before providers existed holds a bare id and means a GitHub
    /// account, and is qualified before the comparison the same way ownership
    /// is.
    pub fn named_role(&self, caller_id: &str) -> Option<Role> {
        if caller_id.is_empty() {
            return None;
        }
        let names = |grants: &[Grant]| grants.iter().any(|grant| stored_id(&grant.id) == caller_id);
        if names(&self.editors) {
            return Some(Role::Editor);
        }
        if names(&self.commenters) {
            return Some(Role::Commenter);
        }
        None
    }

    /// The role a link carries, when the document knows its hash and it has
    /// not expired. Revoking is deleting the row; an expired link answers the
    /// same way a revoked one does, as no link at all.
    pub fn link_role(&self, link_hash: &str, now: i64) -> Option<Role> {
        self.live_link(link_hash, now).map(LinkGrant::granted)
    }

    /// The live link matching a presented digest. Keeping this lookup beside
    /// `link_role` makes link metadata such as its rate budget come from the
    /// same row that grants access.
    pub fn live_link(&self, link_hash: &str, now: i64) -> Option<&LinkGrant> {
        if link_hash.is_empty() {
            return None;
        }
        self.links
            .iter()
            .find(|link| link.hash == link_hash && link.live_at(now))
    }

    /// The link that carries a role, live or not. The dead one still matters:
    /// it is what lets the dialog say "expired" or "legacy link, reset to get
    /// a new one" instead of just "off".
    pub fn link_for(&self, role: Role) -> Option<&LinkGrant> {
        self.links.iter().find(|link| link.granted() == role)
    }

    /// Replaces whatever link already carries this role. A document holds at
    /// most one link per role, so minting a role that already has one is a
    /// rotation: the old key stops meaning anything the moment this returns,
    /// because its hash is no longer on the document at all.
    pub fn set_link(&mut self, link: LinkGrant) {
        let role = link.granted();
        self.links.retain(|existing| existing.granted() != role);
        self.links.push(link);
    }

    /// Removes the link carrying this role, if there is one. Returns whether
    /// a row actually went, the way `revoke_from` does, so a revoke that
    /// matched nothing can say so.
    pub fn drop_link(&mut self, role: Role) -> bool {
        let before = self.links.len();
        self.links.retain(|link| link.granted() != role);
        before != self.links.len()
    }

    /// Drops every guest whose link no longer lives. Called whenever a link
    /// is rotated or revoked, since rotating replaces the row a guest's hash
    /// pointed at and revoking removes it outright -- either way, a guest
    /// recorded against a hash nothing here still answers to is not a guest
    /// of anything any more.
    pub fn prune_guests(&mut self, now: i64) {
        let live: std::collections::HashSet<String> = self
            .links
            .iter()
            .filter(|link| link.live_at(now))
            .map(|link| link.hash.clone())
            .collect();
        self.guests.retain(|guest| live.contains(&guest.link));
    }

    /// Whether a caller is on this document at all: its owner, named in any
    /// role, or holding a live link.
    pub fn names(&self, owner_key: &str, caller_id: &str, link_hash: &str, now: i64) -> bool {
        self.owned_by(owner_key, caller_id)
            || self.named_role(caller_id).is_some()
            || self.link_role(link_hash, now).is_some()
    }

    /// Whether a caller may read this document at all: its owner, anyone on
    /// it by a legacy grant, and whoever holds a live link, since a read link
    /// is the least a link carries. The URL alone opens nothing for anybody
    /// else -- a link is the whole of sharing, and the bare slug is not one.
    /// The reserved examples are the exception: they are there to be read.
    pub fn readable_by(&self, owner_key: &str, caller_id: &str, link_hash: &str, now: i64) -> bool {
        self.example || self.unowned || self.names(owner_key, caller_id, link_hash, now)
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

    /// The role a grant or a link names, or None for a word that is not one a
    /// document hands out. `reader` parses now too, because a link may carry
    /// it -- on a `private` document that is the only thing a reader link is
    /// for, since reading is otherwise what reaching the document already
    /// gives. Nobody is granted `owner`, since the owner is one account and
    /// `transfer` is how that moves.
    pub fn parse(value: &str) -> Option<Role> {
        match value {
            "reader" => Some(Role::Reader),
            "commenter" => Some(Role::Commenter),
            "editor" => Some(Role::Editor),
            _ => None,
        }
    }

    /// Whether this role includes another: the ladder, asked as a question.
    pub fn at_least(self, wanted: Role) -> bool {
        self >= wanted
    }
}

pub struct Store {
    /// Where the bytes are. The store does not care what holds them.
    pub blobs: Arc<dyn BlobStore>,
    pub config: Arc<Configuration>,
    pub state: Mutex<StoreState>,
    /// The authoritative catalogue for new deployments. `None` is retained
    /// only for the small compatibility surface used by legacy fixtures.
    pub catalog: Option<Arc<Catalog>>,
}

pub struct StoreState {
    pub entries: HashMap<String, IndexEntry>,
    /// The version of index.json these entries were read from, so a write can
    /// say what it expects to be replacing. Empty means "there was no index",
    /// which is how a fresh store starts.
    pub index_version: BlobVersion,
    /// When the index was last re-read from storage because a lookup missed.
    /// A miss is the one read that goes to storage, and a stranger guessing
    /// slugs would otherwise turn every 404 into a download of the whole
    /// index; this is what keeps that to one download per `REFRESH_EVERY`.
    pub refreshed_at: Option<std::time::Instant>,
}

/// The least time between two reloads of the index prompted by misses. Long
/// enough that a scan of made-up slugs costs the bucket one read per window
/// rather than one per request; short enough that a document created on
/// another instance shows up here within the time it takes to paste its link.
pub const REFRESH_EVERY: std::time::Duration = std::time::Duration::from_secs(5);

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
            let mut entries: HashMap<String, IndexEntry> =
                serde_json::from_slice(&raw).map_err(|err| {
                    format!(
                        "the index in {} is not readable ({err}); move it aside to start empty",
                        blobs.describe()
                    )
                })?;
            // `unowned` was introduced after the JSON index format.  It is
            // intentionally not serialized, so infer it for old entries
            // rather than making a public, ownerless document look private
            // after a restart.
            for entry in entries.values_mut() {
                if entry.publisher.is_empty() && !entry.example {
                    entry.unowned = true;
                }
            }
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
    /// The path of the main file. A publish of one file is a directory of one
    /// file, and this is what it is called in it.
    pub main: String,
    pub owner: String,
    pub owner_id: String,
    pub owner_name: String,
    /// A caller that has already parsed a complete directory can provide its
    /// exact initial object peak.  Direct
    /// Store users leave this unset and retain the legacy source-only
    /// admission contract.
    pub peak_bytes: Option<i64>,
}

/// What `put` returns when a storage rule refuses an upload: the HTTP status
/// and message the rule names, so the handler answers with exactly what the
/// rule decided.
#[derive(Debug)]
pub enum PutError {
    Quota { status: u16, message: &'static str },
    Authorization { status: u16, message: &'static str },
    Storage(String),
}

/// Why `modify` did not: the document is not there, the change refused itself
/// with a message meant for the caller, or storage would not take it.
#[derive(Debug)]
pub enum ModifyError {
    NotFound,
    Refused(String),
    Storage(String),
}

#[derive(Clone, Debug)]
pub struct MutationActor {
    pub account_id: String,
    pub owner_key: String,
    pub session_generation: String,
    /// The live link that supplied authority, if any.  Only its digest is
    /// carried into storage; the secret never leaves the request.
    pub link_hash: String,
    /// Whether the current publisher policy still admits this editor.
    pub policy_editor: bool,
    /// Automation requests may use only the supplied link authority.
    pub automation: bool,
    pub unowned_publisher: bool,
}

impl std::fmt::Display for PutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PutError::Quota { message, .. } => write!(f, "{message}"),
            PutError::Authorization { message, .. } => write!(f, "{message}"),
            PutError::Storage(message) => write!(f, "{message}"),
        }
    }
}

/// Declared input for a document-store job.  Every one of them carries a
/// slug, an account id or an object key and no payload; the ones that own a
/// document row add its size below.
const STORE_JOB_BYTES: usize = 512;

impl Store {
    /// Take a document-bytes reservation before an object is written.
    ///
    /// A caller cancelled after dispatch leaves the reservation taken. That is
    /// the conservative outcome the lifecycle rules ask for and it is the
    /// behaviour that already existed: the reservation is refunded by
    /// `release_object_bytes` on the write path's own error handling, and a
    /// reservation no write ever consumes is reconciled by the object ledger,
    /// never by re-running the mutation.
    pub async fn reserve_object_bytes(
        &self,
        slug: &str,
        bytes: i64,
        actor: Option<&MutationActor>,
    ) -> Result<(), PutError> {
        let Some(catalog) = &self.catalog else {
            return Ok(());
        };
        let slug = slug.to_string();
        let actor = actor.cloned();
        let per_owner = self.config.storage.per_owner;
        let total = self.config.storage.total;
        let hard_count =
            (self.config.session.history_max > 0).then(|| self.config.session.history_max as u32);
        catalog
            .execute_catalog(STORE_JOB_BYTES + slug.len(), move |catalog| {
                let result = catalog.reserve_document_bytes_with_authority(
                    &slug,
                    bytes,
                    per_owner,
                    total,
                    actor
                        .as_ref()
                        .map(|actor| crate::storage::catalog::MutationAuthority {
                            account_id: actor.account_id.as_str(),
                            owner_key: actor.owner_key.as_str(),
                            generation: actor.session_generation.as_str(),
                            link_hash: actor.link_hash.as_str(),
                            policy_editor: actor.policy_editor,
                            automation: actor.automation,
                            unowned_publisher: actor.unowned_publisher,
                            execution_epoch: "",
                            agent_checkpoint: None,
                        }),
                );
                // A failed reservation is not permission to evict history
                // synchronously. When the owner is already over its hard
                // quota, enqueue a bounded pressure plan so a retry can make
                // progress after successful GC. A refusal caused only by
                // this prospective write is a no-op while current usage fits.
                if matches!(
                    &result,
                    Err(crate::storage::catalog::CatalogError::Conflict(message))
                        if message.contains("quota")
                ) {
                    let _ = catalog.schedule_hard_pressure_for_slug_for_growth_with_limits(
                        &slug,
                        per_owner,
                        bytes,
                        hard_count,
                        crate::util::now_unix(),
                    );
                }
                result
            })
            .await
            .map_err(crate::storage::catalog::CatalogError::from)
            .map_err(|error| match error {
                crate::storage::catalog::CatalogError::Conflict(message)
                    if message.contains("actor") =>
                {
                    PutError::Authorization {
                        status: 403,
                        message: "edit access changed",
                    }
                }
                crate::storage::catalog::CatalogError::NotFound => PutError::Authorization {
                    status: 404,
                    message: "not found",
                },
                crate::storage::catalog::CatalogError::Conflict(message)
                    if message.contains("quota") || message.contains("deployment") =>
                {
                    PutError::Quota {
                        status: 507,
                        message: if message.contains("deployment") {
                            "this deployment has no room left"
                        } else {
                            "your storage quota is used up; delete a document first"
                        },
                    }
                }
                other => PutError::Storage(other.to_string()),
            })
    }

    pub async fn admit_replacement_upload(&self, slug: &str) -> Result<(), PutError> {
        let Some(catalog) = &self.catalog else {
            return Ok(());
        };
        let slug = slug.to_string();
        let uploads_per_hour = self.config.storage.uploads_per_hour;
        catalog
            .execute_catalog(STORE_JOB_BYTES + slug.len(), move |catalog| {
                catalog.admit_document_upload(&slug, uploads_per_hour)
            })
            .await
            .map_err(crate::storage::catalog::CatalogError::from)
            .map_err(|error| match error {
                crate::storage::catalog::CatalogError::Conflict(message)
                    if message.contains("upload rate") =>
                {
                    PutError::Quota {
                        status: 429,
                        message: "too many uploads this hour; try later",
                    }
                }
                other => PutError::Storage(other.to_string()),
            })
    }

    /// Refund a document-bytes reservation the caller did not use.
    ///
    /// Once dispatched this runs whether or not the caller is still there,
    /// which is the point: the refund is the cleanup for a failed object
    /// write, and losing it to a cancelled request would leave the document
    /// charged for bytes it never stored.
    pub async fn release_object_bytes(&self, slug: &str, bytes: i64) {
        if let Some(catalog) = &self.catalog {
            let slug = slug.to_string();
            let _ = catalog
                .execute_catalog(STORE_JOB_BYTES + slug.len(), move |catalog| {
                    catalog.release_document_bytes(&slug, bytes)
                })
                .await;
        }
    }

    pub async fn begin_delete(&self, slug: &str) -> Result<Option<String>, String> {
        let Some(catalog) = &self.catalog else {
            return Ok(None);
        };
        let slug = slug.to_string();
        catalog
            .execute_catalog(STORE_JOB_BYTES + slug.len(), move |catalog| {
                catalog.begin_delete(&slug)
            })
            .await
            .map(|document| Some(document.storage_id))
            .map_err(|err| err.to_string())
    }

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
                refreshed_at: None,
            }),
            catalog: None,
        })
    }

    /// Open a store backed by the transactional catalogue. The old
    /// `open` constructor intentionally remains available for isolated tests
    /// and old callers, but production startup uses this constructor so no
    /// whole-file JSON index is read or written.
    pub async fn open_with_catalog(
        blobs: Arc<dyn BlobStore>,
        config: Arc<Configuration>,
        catalog: Arc<Catalog>,
    ) -> Result<Store, String> {
        // Publications interrupted after prepare are invisible. On restart,
        // roll forward only when both the staged catalogue checkpoint and its
        // immutable tree object exist; otherwise abort, and discard a never-
        // activated creation so it cannot consume quota forever.
        for pending in catalog
            .execute_catalog(STORE_JOB_BYTES, |catalog| {
                catalog.pending_publications(1000)
            })
            .await
            .map_err(|error| error.to_string())?
        {
            let operation = {
                let storage_id = pending.storage_id.clone();
                let request_id = pending.request_id.clone();
                catalog
                    .execute_catalog(STORE_JOB_BYTES, move |catalog| {
                        catalog.operation(&storage_id, &request_id)
                    })
                    .await
                    .map_err(|error| error.to_string())?
            };
            let staged_sha = operation.as_ref().and_then(|operation| {
                serde_json::from_str::<serde_json::Value>(&operation.intent)
                    .ok()
                    .and_then(|intent| {
                        intent
                            .get("checkpoint")
                            .and_then(|checkpoint| checkpoint.get("sha"))
                            .and_then(|sha| sha.as_str())
                            .map(str::to_owned)
                    })
            });
            let (
                staged_tree_sha,
                staged_sources,
                staged_assets,
                staged_assets_valid,
                native_source_graph,
            ): (
                String,
                Option<Vec<SourceHistoryRecord>>,
                Option<Vec<CheckpointAssetRef>>,
                bool,
                bool,
            ) = operation
                .as_ref()
                .and_then(|operation| {
                    let intent =
                        serde_json::from_str::<serde_json::Value>(&operation.intent).ok()?;
                    let tree_sha = intent
                        .get("checkpoint")
                        .and_then(|checkpoint| checkpoint.get("tree_sha"))
                        .and_then(|sha| sha.as_str())
                        .unwrap_or_default()
                        .to_owned();
                    let asset_property = intent.get("checkpoint_assets");
                    let assets_valid = asset_property.is_none_or(|value| {
                        serde_json::from_value::<Vec<CheckpointAssetRef>>(value.clone()).is_ok()
                    });
                    let assets =
                        asset_property.and_then(|value| serde_json::from_value(value.clone()).ok());
                    let sources = if let Some(value) = intent.get("source_history") {
                        serde_json::from_value(value.clone()).ok()
                    } else if intent.get("checkpoint_assets").is_some() {
                        // Asset-only trees have no source-history records, but
                        // the presence of the asset property still opts the
                        // receipt into strict native graph validation.
                        Some(Vec::new())
                    } else {
                        None
                    };
                    let native = intent.get("source_history").is_some()
                        || intent.get("checkpoint_assets").is_some();
                    Some((tree_sha, sources, assets, assets_valid, native))
                })
                .unwrap_or_default();
            // Agent source operations are coupled to a Yjs marker in the
            // durable session, rather than a staged checkpoint object. The
            // room recovery path must inspect that marker; treating this row
            // as an incomplete publication would abort a successfully saved
            // source effect before its receipt can be reconciled.
            if operation
                .as_ref()
                .is_some_and(|operation| operation.kind == "agent_apply")
            {
                continue;
            }
            let mut staged = if let Some(sha) = staged_sha.as_deref().filter(|sha| !sha.is_empty())
            {
                match blobs
                    .get(&crate::storage::blob::checkpoint_key(
                        &pending.storage_id,
                        sha,
                    ))
                    .await
                {
                    Ok(tree_bytes) => {
                        if native_source_graph {
                            staged_sources.as_deref().is_some_and(|sources| {
                                // This async validation is performed below;
                                // keep the tree read here so legacy receipts
                                // retain their original recovery behavior.
                                !sources.is_empty()
                            })
                        } else if !staged_tree_sha.is_empty() {
                            match serde_json::from_slice::<crate::document::history::Tree>(
                                &tree_bytes,
                            ) {
                                Ok(tree) => tree.digest() == staged_tree_sha,
                                Err(_) => true,
                            }
                        } else {
                            true
                        }
                    }
                    Err(BlobError::NotFound) => false,
                    Err(error) => {
                        // A transient storage error -- a timeout, a hiccup --
                        // is not proof the checkpoint is missing. Treating it
                        // as missing would abort, and for a creation discard,
                        // a publication that may well be complete, the
                        // instant the object store it lives in has a bad
                        // moment. Leave it pending and let a later check of
                        // this same slug decide once storage actually answers.
                        eprintln!(
                            "warning: could not check staged publication for {}: {error}; leaving it pending",
                            pending.slug
                        );
                        continue;
                    }
                }
            } else {
                false
            };
            if native_source_graph {
                if !staged_assets_valid {
                    continue;
                }
                let Some(sha) = staged_sha.as_deref().filter(|sha| !sha.is_empty()) else {
                    continue;
                };
                let tree_bytes = match blobs
                    .get(&crate::storage::blob::checkpoint_key(
                        &pending.storage_id,
                        sha,
                    ))
                    .await
                {
                    Ok(bytes) => bytes,
                    Err(_) => continue,
                };
                let Some(sources) = staged_sources.as_deref() else {
                    continue;
                };
                if !validate_staged_source_graph(
                    blobs.as_ref(),
                    &pending.storage_id,
                    &tree_bytes,
                    &staged_tree_sha,
                    sources,
                    staged_assets.as_deref(),
                )
                .await
                {
                    continue;
                }
                staged = true;
            }
            let storage_id = pending.storage_id.clone();
            let request_id = pending.request_id.clone();
            if staged {
                let sha = staged_sha
                    .as_deref()
                    .expect("staged publication SHA")
                    .to_string();
                let owner_limit = config.storage.per_owner;
                let total_limit = config.storage.total;
                catalog
                    .execute_catalog(STORE_JOB_BYTES, move |catalog| {
                        catalog.commit_operation_with_quota(
                            &storage_id,
                            &request_id,
                            &sha,
                            &sha,
                            Some((owner_limit, total_limit)),
                        )
                    })
                    .await
                    .map_err(|error| error.to_string())?;
            } else {
                let lifecycle_slug = pending.slug.clone();
                let creating = pending.lifecycle == "creating";
                catalog
                    .execute_catalog(STORE_JOB_BYTES, move |catalog| {
                        catalog.abort_operation(
                            &storage_id,
                            &request_id,
                            "startup discarded incomplete publication",
                        )?;
                        if creating {
                            catalog.discard_aborted_creation(&lifecycle_slug)?;
                        }
                        Ok(())
                    })
                    .await
                    .map_err(|error| error.to_string())?;
            }
        }
        Ok(Store {
            blobs,
            config,
            state: Mutex::new(StoreState {
                entries: HashMap::new(),
                index_version: String::new(),
                refreshed_at: None,
            }),
            catalog: Some(catalog),
        })
    }

    /// A document by slug, or nothing if this deployment has none by that
    /// name. A miss is retried once against storage before it is trusted: on
    /// a single-writer deployment the extra check costs one round trip that
    /// always confirms the miss, but on a shared bucket it is what makes a
    /// document another instance just created visible here without every hit
    /// -- the overwhelming majority of calls -- paying for a reload it does
    /// not need.
    #[allow(dead_code)]
    pub async fn get(&self, slug: &str) -> Option<IndexEntry> {
        if let Some(catalog) = &self.catalog {
            return load_catalog_entry(catalog, slug, true).await.ok().flatten();
        }
        {
            let mut state = self.state.lock().await;
            if let Some(entry) = state.entries.get(slug).cloned() {
                return Some(entry);
            }
            // A miss that follows another miss closely is answered from
            // memory: the index was re-read moments ago, and a burst of
            // guessed slugs must not become a burst of downloads.
            let now = std::time::Instant::now();
            if state
                .refreshed_at
                .is_some_and(|at| now.duration_since(at) < REFRESH_EVERY)
            {
                return None;
            }
            state.refreshed_at = Some(now);
        }
        if self.refresh().await.is_err() {
            return None;
        }
        self.state.lock().await.entries.get(slug).cloned()
    }

    /// Authoritative lookup for HTTP paths.  Unlike the compatibility
    /// `get`, catalogue failures are returned to the caller instead of being
    /// flattened into a misleading 404.
    pub async fn get_result(&self, slug: &str) -> Result<Option<IndexEntry>, CatalogError> {
        if let Some(catalog) = &self.catalog {
            return load_catalog_entry(catalog, slug, true).await;
        }
        Ok(self.get(slug).await)
    }

    /// Return a publication still being staged. This is intentionally kept
    /// out of ordinary reads and listings, but lets an exact retry reconcile
    /// an unknown-commit request instead of inventing a new slug.
    #[allow(dead_code)]
    pub async fn pending_publication(&self, slug: &str) -> Option<IndexEntry> {
        self.pending_publication_result(slug).await.ok().flatten()
    }

    pub async fn pending_publication_result(
        &self,
        slug: &str,
    ) -> Result<Option<IndexEntry>, CatalogError> {
        let Some(catalog) = self.catalog.as_ref() else {
            return Ok(None);
        };
        let Some(document) = document_row(catalog, slug).await? else {
            return Ok(None);
        };
        Ok(document
            .pending_publication
            .is_some()
            .then(|| IndexEntry::from_catalog(document)))
    }

    /// Every document, newest first, as the listing endpoint wants.
    pub async fn list(&self) -> Vec<IndexEntry> {
        if let Some(catalog) = &self.catalog {
            let mut entries: Vec<_> = catalog_entries(catalog)
                .await
                .unwrap_or_default()
                .into_values()
                .collect();
            entries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            return entries;
        }
        let state = self.state.lock().await;
        let mut documents: Vec<IndexEntry> = state.entries.values().cloned().collect();
        documents.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        documents
    }

    pub async fn list_result(&self) -> Result<Vec<IndexEntry>, CatalogError> {
        if let Some(catalog) = &self.catalog {
            let mut entries: Vec<_> = catalog_entries(catalog).await?.into_values().collect();
            entries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            return Ok(entries);
        }
        Ok(self.list().await)
    }

    /// One bounded, authorization-aware catalogue page. Unlike the legacy
    /// compatibility helpers this never turns a busy/corrupt catalogue into
    /// an empty successful response.
    #[allow(dead_code)]
    pub async fn visible_page(
        &self,
        account_id: Option<&str>,
        owner_key: Option<&str>,
        cursor: Option<(&str, &str)>,
        limit: u32,
    ) -> Result<Vec<IndexEntry>, CatalogError> {
        self.visible_page_with_options(account_id, owner_key, cursor, limit, true)
            .await
    }

    pub async fn visible_page_with_options(
        &self,
        account_id: Option<&str>,
        owner_key: Option<&str>,
        cursor: Option<(&str, &str)>,
        limit: u32,
        include_examples: bool,
    ) -> Result<Vec<IndexEntry>, CatalogError> {
        let catalog = self.catalog.as_ref().ok_or_else(|| {
            CatalogError::Invalid("bounded listing requires the local catalogue".into())
        })?;
        let account_id = account_id.map(str::to_owned);
        let owner_key = owner_key.map(str::to_owned);
        let cursor = cursor.map(|(a, b)| (a.to_owned(), b.to_owned()));
        // The page and the rows it names are one job.  A job per row would be
        // up to two hundred dispatches for one listing request, and the page
        // is already bounded by `limit`, so there is nothing to gain by
        // letting other work interleave between its rows.
        catalog
            .execute_catalog(STORE_JOB_BYTES, move |catalog| {
                let page = catalog.visible_documents_with_examples(
                    account_id.as_deref(),
                    owner_key.as_deref(),
                    cursor.as_ref().map(|(a, b)| (a.as_str(), b.as_str())),
                    limit,
                    include_examples,
                )?;
                let mut entries = Vec::with_capacity(page.len());
                for document in page {
                    // Listing rows need grants/guest roles, but never need to
                    // decrypt and expose link secrets for every document.
                    entries.push(
                        load_catalog_entry_sql(catalog, &document.slug, false)?
                            .ok_or(CatalogError::NotFound)?,
                    );
                }
                Ok(entries)
            })
            .await
            .map_err(CatalogError::from)
    }

    pub async fn get_checked(&self, slug: &str) -> Result<Option<IndexEntry>, CatalogError> {
        match &self.catalog {
            Some(catalog) => load_catalog_entry(catalog, slug, true).await,
            None => Err(CatalogError::Invalid(
                "checked lookup requires the local catalogue".into(),
            )),
        }
    }

    /// The stored source of a document: the source of the version the index
    /// names, and no other. A document published before sources were versioned
    /// has one unversioned key instead, which is read when there is nothing
    /// under the digest.
    pub async fn read_source(&self, slug: &str) -> Result<Vec<u8>, BlobError> {
        // In catalogue mode `state.entries` is only a lazily filled
        // compatibility cache -- it starts empty and gains a slug only once
        // something else has already touched it -- so a document nobody has
        // looked up since startup would answer not-found here even though
        // the catalogue has it. The catalogue, when there is one, is asked
        // directly instead.
        let (digest, identity) = match &self.catalog {
            Some(catalog) => {
                let document = document_row(catalog, slug)
                    .await
                    .map_err(|error| BlobError::Other(error.to_string()))?
                    .ok_or(BlobError::NotFound)?;
                let identity = if document.storage_id.is_empty() {
                    slug.to_string()
                } else {
                    document.storage_id
                };
                (document.sha, identity)
            }
            None => {
                let state = self.state.lock().await;
                match state.entries.get(slug) {
                    Some(entry) => (entry.sha.clone(), slug.to_string()),
                    None => return Err(BlobError::NotFound),
                }
            }
        };
        match self.blobs.get(&source_key(&identity, &digest)).await {
            Err(BlobError::NotFound) => self.blobs.get(&legacy_source_key(slug)).await,
            other => other,
        }
    }

    pub async fn read(&self, slug: &str, digest: &str) -> Result<Vec<u8>, BlobError> {
        let identity = match &self.catalog {
            Some(catalog) => document_row(catalog, slug)
                .await
                .map_err(|error| BlobError::Other(error.to_string()))?
                .map(|document| document.storage_id)
                .filter(|identity| !identity.is_empty())
                .unwrap_or_else(|| slug.to_string()),
            None => slug.to_string(),
        };
        self.blobs.get(&document_key(&identity, digest)).await
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
        if self.catalog.is_some() {
            return self.put_catalog(v).await;
        }
        let size = v.source.len() as i64;
        let mut state = self.state.lock().await;
        let admission_owner = v.owner.clone();
        let admission_owner_id = v.owner_id.clone();
        self.admit(
            &state,
            &v.slug,
            &admission_owner,
            &admission_owner_id,
            size,
            now_unix(),
        )?;
        let now = timestamp();
        let mut created = now.clone();
        let mut example = false;
        let (mut owner, mut owner_id, mut owner_name) = (v.owner, v.owner_id, v.owner_name);
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
            owner_name = existing.publisher_name.clone();
        }
        if state.entries.values().any(|entry| {
            entry.slug != v.slug
                && entry.publisher_id == owner_id
                && (!owner_id.is_empty() || entry.publisher == owner.to_lowercase())
                && entry.title.trim().to_lowercase() == v.title.trim().to_lowercase()
        }) {
            return Err(PutError::Authorization {
                status: 409,
                message: "A project with this name already exists. Choose a different name.",
            });
        }
        // Who a document is shared with is not changed by its text changing.
        let shared = state.entries.get(&v.slug).cloned().unwrap_or_default();
        let entry = IndexEntry {
            slug: v.slug.clone(),
            storage_id: state
                .entries
                .get(&v.slug)
                .map(|existing| existing.storage_id.clone())
                .filter(|id| !id.is_empty())
                // The JSON/index compatibility path historically addressed
                // every room object by its slug.  Keep that identity stable
                // for the legacy store; catalogue-backed documents get an
                // independently allocated storage_id in put_catalog.
                .unwrap_or_else(|| v.slug.clone()),
            title: v.title,
            // The digest of the source, which is the checkpoint the room is
            // about to write. From here the index's `sha` names the newest
            // checkpoint rather than an HTML object.
            sha: digest_of(&v.source),
            size,
            created_at: created,
            updated_at: now,
            example,
            unowned: owner.is_empty(),
            publisher: owner.to_lowercase(),
            publisher_id: owner_id,
            publisher_name: owner_name,
            source_format: v.source_format,
            main: v.main,
            editors: shared.editors,
            commenters: shared.commenters,
            links: shared.links,
            guests: shared.guests,
        };
        let previous = state.entries.insert(v.slug.clone(), entry.clone());
        // `put` returns through `put_catalog` above whenever a catalogue
        // exists, so everything from here on is the legacy JSON-index path
        // and never touches the catalogue at all.
        let mut written = self.save_locked(&mut state).await;
        // Another process moved the index between our reading it and our
        // writing it. The quota was decided against an index that no longer
        // exists, so it is decided again against the one that does -- which is
        // what stops two deployments sharing a bucket from both admitting the
        // last of a quota. One retry: a second conflict means the index is
        // busier than this write is worth.
        if matches!(written, Err(BlobError::Conflict)) {
            let stored = match self.reload_locked(&mut state, &v.slug).await {
                Ok(stored) => stored,
                Err(err) => {
                    // The reload itself failed, so there is no fresher answer
                    // for what storage actually holds. Fall back to what
                    // this instance had before attempting the replacement,
                    // rather than a bare removal that would delete a
                    // document that was never actually gone.
                    match previous.clone() {
                        Some(previous) => {
                            state.entries.insert(v.slug.clone(), previous);
                        }
                        None => {
                            state.entries.remove(&v.slug);
                        }
                    }
                    return Err(PutError::Storage(err));
                }
            };
            // `admit` must see this exactly as storage does: a replacement
            // when storage still has a row for this slug, a creation when it
            // does not. Removing the slug unconditionally -- as this used to
            // do -- turned every replacement retried here into a brand-new
            // document, asking for a free document-count slot the document
            // already occupied.
            let mut reference = state.entries.clone();
            match &stored {
                Some(stored) => {
                    reference.insert(v.slug.clone(), stored.clone());
                }
                None => {
                    reference.remove(&v.slug);
                }
            }
            let fresh = StoreState {
                entries: reference,
                index_version: state.index_version.clone(),
                refreshed_at: state.refreshed_at,
            };
            if let Err(refused) = self.admit(
                &fresh,
                &v.slug,
                &admission_owner,
                &admission_owner_id,
                size,
                now_unix(),
            ) {
                // Memory must equal the index just reloaded: the entry
                // storage actually has for this slug, or nothing if it never
                // had one. Saving here -- as this used to do -- would
                // overwrite that just-read index with a copy missing this
                // slug, deleting a document the refused write only ever
                // meant to replace.
                match stored {
                    Some(stored) => {
                        state.entries.insert(v.slug.clone(), stored);
                    }
                    None => {
                        state.entries.remove(&v.slug);
                    }
                }
                return Err(refused);
            }
            if fresh.entries.values().any(|other| {
                other.slug != entry.slug
                    && other.publisher_id == entry.publisher_id
                    && (!entry.publisher_id.is_empty() || other.publisher == entry.publisher)
                    && other.title.trim().to_lowercase() == entry.title.trim().to_lowercase()
            }) {
                match stored {
                    Some(stored) => {
                        state.entries.insert(v.slug.clone(), stored);
                    }
                    None => {
                        state.entries.remove(&v.slug);
                    }
                }
                return Err(PutError::Authorization {
                    status: 409,
                    message: "A project with this name already exists. Choose a different name.",
                });
            }
            written = self.save_locked(&mut state).await;
        }
        if let Err(err) = written {
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

    /// Authoritative publication path for local SQLite deployments.  The
    /// compatibility `StoreState` is refreshed only for the single affected
    /// slug after the catalogue transaction commits; it is never used for
    /// quota admission and never rewritten as a whole index.
    pub async fn prepare_publication(
        &self,
        slug: &str,
        request_digest: &str,
        kind: &str,
        actor: Option<&MutationActor>,
    ) -> Result<String, String> {
        let catalog = self
            .catalog
            .as_ref()
            .ok_or_else(|| "publication receipts require the local catalogue".to_string())?;
        let document = document_row(catalog, slug)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "document not found".to_string())?;
        if let Some(request_id) = document.pending_publication {
            let storage_id = document.storage_id.clone();
            let wanted = request_id.clone();
            let operation = catalog
                .execute_catalog(STORE_JOB_BYTES, move |catalog| {
                    catalog.operation(&storage_id, &wanted)
                })
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "publication receipt is missing".to_string())?;
            if operation.request_digest != request_digest {
                return Err("document has a different publication in progress".into());
            }
            return Ok(request_id);
        }
        let request_id = new_id();
        let intent_slug = slug.to_string();
        let kind = kind.to_string();
        let storage_id = document.storage_id.clone();
        let expected_head = document.sha.clone();
        let actor = actor.cloned();
        let submit_request_id = request_id.clone();
        let request_digest = request_digest.to_string();
        catalog
            .execute_catalog(STORE_JOB_BYTES + intent_slug.len(), move |catalog| {
                let slug = intent_slug.as_str();
                let kind = kind.as_str();
                let document_sha = expected_head.as_str();
                let actor = actor.as_ref();
                catalog.prepare_operation(&OperationRequest {
                    storage_id: &storage_id,
                    request_id: &submit_request_id,
                    kind,
                    request_digest: &request_digest,
                    // This is parsed back with serde_json -- by the startup
                    // roll-forward above, and by `stage_publication_measurement`
                    // and `stage_publication_checkpoint` in operations.rs -- so it
                    // must actually be JSON. Rust's `Debug` escaping (the old
                    // `format!("{:?}", ...)` this replaced) is not JSON escaping:
                    // a control character in an owner key, say, becomes a `\u{7f}`
                    // that no JSON parser accepts, and the whole receipt becomes
                    // unreadable from then on.
                    intent: &if let Some(actor) = actor {
                        serde_json::json!({
                            "slug": slug,
                            "kind": kind,
                            "staged_required": true,
                            "expected_head": document_sha,
                            "new_head": null,
                            "durable_coverage": null,
                            "reservations": [],
                            "output_descriptors": [],
                            "actor": {
                                "account_id": actor.account_id,
                                "owner_key": actor.owner_key,
                                "generation": actor.session_generation,
                            },
                        })
                        .to_string()
                    } else {
                        serde_json::json!({
                            "slug": slug,
                            "kind": kind,
                            "staged_required": true,
                            "expected_head": document_sha,
                            "new_head": null,
                            "durable_coverage": null,
                            "reservations": [],
                            "output_descriptors": [],
                        })
                        .to_string()
                    },
                    created_at: now_unix(),
                    actor: actor.map(|actor| OperationActor {
                        account_id: actor.account_id.as_str(),
                        owner_key: actor.owner_key.as_str(),
                        generation: actor.session_generation.as_str(),
                        required_role: "editor",
                    }),
                })
            })
            .await
            .map_err(|error| error.to_string())?;
        Ok(request_id)
    }

    /// Reserve the publication peak before object I/O.
    ///
    /// A caller cancelled after dispatch leaves the reservation taken and the
    /// pending receipt owning it; `commit_publication`/`abort_publication`
    /// settle it by that receipt's request id, so the reservation is never
    /// orphaned and the mutation is never repeated.
    pub async fn reserve_publication_peak(&self, slug: &str, bytes: i64) -> Result<(), String> {
        let Some(catalog) = &self.catalog else {
            return Ok(());
        };
        let slug = slug.to_string();
        catalog
            .execute_catalog(STORE_JOB_BYTES + slug.len(), move |catalog| {
                catalog.reserve_publication_peak(&slug, bytes)
            })
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn commit_publication(&self, slug: &str, result: &str) -> Result<(), String> {
        self.commit_publication_checked(slug, result)
            .await
            .map_err(|error| error.to_string())
    }

    /// Preserve the catalogue refusal type through final publication admission.
    pub async fn commit_publication_checked(
        &self,
        slug: &str,
        result: &str,
    ) -> Result<(), crate::storage::catalog::CatalogExecError> {
        let Some(catalog) = &self.catalog else {
            return Ok(());
        };
        let document = document_row(catalog, slug)
            .await?
            .ok_or(CatalogError::NotFound)?;
        let Some(request_id) = document.pending_publication else {
            return Ok(());
        };
        let storage_id = document.storage_id.clone();
        let result = result.to_string();
        let owner_limit = self.config.storage.per_owner;
        let total_limit = self.config.storage.total;
        catalog
            .execute_catalog(STORE_JOB_BYTES + result.len(), move |catalog| {
                catalog.commit_operation_with_quota(
                    &storage_id,
                    &request_id,
                    &result,
                    &result,
                    Some((owner_limit, total_limit)),
                )
            })
            .await?;
        Ok(())
    }

    pub async fn abort_publication(&self, slug: &str, result: &str) -> Result<(), String> {
        let Some(catalog) = &self.catalog else {
            return Ok(());
        };
        let Some(document) = document_row(catalog, slug)
            .await
            .map_err(|error| error.to_string())?
        else {
            return Ok(());
        };
        let Some(request_id) = document.pending_publication else {
            return Ok(());
        };
        let storage_id = document.storage_id.clone();
        let result = result.to_string();
        catalog
            .execute_catalog(STORE_JOB_BYTES + result.len(), move |catalog| {
                catalog.abort_operation(&storage_id, &request_id, &result)
            })
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    async fn put_catalog(&self, v: Publication) -> Result<IndexEntry, PutError> {
        let catalog = self
            .catalog
            .as_ref()
            .expect("put_catalog requires a catalogue");
        let existing = document_row(catalog, &v.slug)
            .await
            .map_err(|err| PutError::Storage(err.to_string()))?;
        let now = timestamp();
        let (owner_id, owner_key, title, created_at, example, storage_id, source_format, main) =
            if let Some(document) = existing.as_ref() {
                (
                    document.owner_id.clone(),
                    document.owner_key.clone(),
                    if v.title.is_empty() {
                        document.title.clone()
                    } else {
                        v.title.clone()
                    },
                    document.created_at.clone(),
                    document.example,
                    document.storage_id.clone(),
                    if v.source_format.is_empty() {
                        document.source_format.clone()
                    } else {
                        v.source_format.clone()
                    },
                    if v.main.is_empty() {
                        document.main.clone()
                    } else {
                        v.main.clone()
                    },
                )
            } else {
                let owner_id = (!v.owner_id.is_empty()).then(|| v.owner_id.clone());
                let owner_key = if owner_id.is_some() {
                    String::new()
                } else if v.owner.is_empty() {
                    format!("example:{}", v.slug)
                } else {
                    v.owner.clone()
                };
                (
                    owner_id,
                    owner_key,
                    v.title.clone(),
                    now.clone(),
                    false,
                    random_storage_id(),
                    v.source_format.clone(),
                    v.main.clone(),
                )
            };
        if let Some(owner_id) = owner_id.as_deref() {
            let provider = owner_id
                .split_once(':')
                .map(|(provider, _)| provider)
                .unwrap_or("github");
            let account = Account {
                id: owner_id.to_string(),
                provider: provider.to_string(),
                handle: v.owner.clone(),
                name: v.owner_name.clone(),
                email: String::new(),
                first_seen: created_at.clone(),
                last_seen: now.clone(),
                plan: "default".to_string(),
                status: "active".to_string(),
                session_generation: random_storage_id(),
                erasure_cursor: None,
            };
            catalog
                .execute_catalog(STORE_JOB_BYTES + account.id.len(), move |catalog| {
                    catalog.upsert_account(&account)
                })
                .await
                .map_err(|err| PutError::Storage(err.to_string()))?;
        }
        // A complete server upload supplies its exact initial object peak
        // before any object I/O. This is a preflight reservation: a large
        // upload cannot create a catalogue row
        // and discover the ceiling only after writing its first tree/session
        // object.  The direct Store API leaves it unset for compatibility;
        // its object ledger still reserves every later materialization.
        let publication_reservation = v
            .peak_bytes
            .unwrap_or(v.source.len() as i64)
            .max(v.source.len() as i64);
        let document = crate::storage::catalog::NewDocument {
            slug: v.slug.clone(),
            storage_id,
            title,
            sha: digest_of(&v.source),
            created_at: created_at.clone(),
            published_at: created_at,
            updated_at: now,
            example,
            owner_key,
            owner_id,
            // `put` admits the catalogue row; the publication route then
            // installs its pending receipt before any externally visible
            // response. While that receipt is pending, normal reads/listings
            // remain hidden. Direct Store users may round-trip an admitted
            // document without having to manufacture a publication receipt.
            status: if v.peak_bytes.is_some() {
                "creating".to_string()
            } else {
                "active".to_string()
            },
            size: v.source.len() as i64,
            // Reserve the exact initial object peak before any object is
            // written. Reconciliation after checkpoint records the ledger's
            // measured bytes.
            counted_size: publication_reservation,
            maintenance_reserved: 0,
            last_auto_checkpoint_at: now_unix(),
            source_format,
            main,
        };
        let replacing = existing.is_some();
        let limits = self.config.storage;
        let hard_count =
            (self.config.session.history_max > 0).then(|| self.config.session.history_max as u32);
        let pressure_owner = document.owner_id.clone();
        let pressure_growth = document.counted_size;
        let result = catalog
            .execute_catalog(STORE_JOB_BYTES + document.slug.len(), move |catalog| {
                if replacing {
                    catalog.replace_document_admitted(
                        &document,
                        limits.per_owner,
                        limits.total,
                        limits.uploads_per_hour,
                    )
                } else {
                    catalog.create_document_admitted(
                        &document,
                        limits.per_owner,
                        limits.total,
                        limits.documents_per_owner,
                        limits.uploads_per_hour,
                    )
                }
            })
            .await
            .map_err(crate::storage::catalog::CatalogError::from);
        if matches!(
            &result,
            Err(crate::storage::catalog::CatalogError::Conflict(message))
                if message.contains("quota")
        ) {
            if let Some(owner) = pressure_owner {
                let _ = catalog
                    .execute_catalog(STORE_JOB_BYTES + owner.len(), move |catalog| {
                        catalog.schedule_hard_pressure_for_growth_with_limits(
                            &owner,
                            limits.per_owner,
                            // A failed admission has not charged these
                            // prospective bytes yet; pressure must reserve
                            // room for the whole attempted publication.
                            pressure_growth,
                            hard_count,
                            crate::util::now_unix(),
                        )
                    })
                    .await;
            }
        }
        let document = result.map_err(|err| match err {
            crate::storage::catalog::CatalogError::Conflict(message)
                if message.contains("project with this name") =>
            {
                PutError::Authorization {
                    status: 409,
                    message: "A project with this name already exists. Choose a different name.",
                }
            }

            crate::storage::catalog::CatalogError::Conflict(message)
                if message.contains("quota exceeded") || message.contains("upload rate") =>
            {
                PutError::Quota {
                    status: if message.contains("upload rate") {
                        429
                    } else {
                        507
                    },
                    message: if message.contains("document count") {
                        "you have reached the document limit; delete one first"
                    } else if message.contains("upload rate") {
                        "too many uploads this hour; try later"
                    } else if message.contains("owner") {
                        "your storage quota is used up; delete a document first"
                    } else {
                        "this deployment has no room left"
                    },
                }
            }
            other => PutError::Storage(other.to_string()),
        })?;
        let document = document_row(catalog, &document.slug)
            .await
            .map_err(|err| PutError::Storage(err.to_string()))?
            .ok_or_else(|| PutError::Storage("catalogue publication disappeared".into()))?;
        let entry = IndexEntry::from_catalog(document);
        self.state
            .lock()
            .await
            .entries
            .insert(v.slug, entry.clone());
        Ok(entry)
    }

    /// Removes what the old layout kept: the rendered HTML of every version,
    /// and the sources beside them. Called once a document's source is durable
    /// as a checkpoint, and never before, so the copy that goes is a copy and
    /// not the document. A failure leaves objects behind for a later pass and
    /// is not worth reporting: nothing depends on them any more.
    pub async fn drop_derived(&self, slug: &str) {
        // The content-addressed catalogue layout has no disposable derived
        // subtree: trees, blobs, assets, and renderings share one identity
        // prefix and are reclaimed only after a reference scan.  The sole
        // obsolete object this compatibility hook may remove is the old,
        // unversioned source object.
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
        main: &str,
    ) -> Result<(), String> {
        if let Some(catalog) = &self.catalog {
            // Ordinary session/asset persistence is already covered by the
            // exact object ledger.  It must not rewrite the document's
            // measured size or totals while a checkpoint is being prepared;
            // only a checkpoint head (identified by `sha`) reconciles the
            // aggregate row.
            if sha.is_none() {
                return Ok(());
            }
            if document_row(catalog, slug)
                .await
                .ok()
                .flatten()
                .and_then(|document| document.pending_publication)
                .is_some()
            {
                let (slug, sha, format, main) = (
                    slug.to_string(),
                    sha.map(str::to_owned),
                    format.to_string(),
                    main.to_string(),
                );
                catalog
                    .execute_catalog(STORE_JOB_BYTES + slug.len(), move |catalog| {
                        catalog.stage_publication_measurement(
                            &slug,
                            sha.as_deref(),
                            size,
                            &format,
                            &main,
                        )
                    })
                    .await
                    .map_err(|err| err.to_string())?;
                return Ok(());
            }
            let updated_at = sha.map(|_| timestamp());
            let document = {
                let (owned_slug, sha, format, main) = (
                    slug.to_string(),
                    sha.map(str::to_owned),
                    format.to_string(),
                    main.to_string(),
                );
                catalog
                    .execute_catalog(STORE_JOB_BYTES + owned_slug.len(), move |catalog| {
                        catalog.record_document_measurement(
                            &owned_slug,
                            size,
                            sha.as_deref(),
                            updated_at.as_deref(),
                            &format,
                            &main,
                        )
                    })
                    .await
                    .map_err(|err| err.to_string())?
            };
            self.state
                .lock()
                .await
                .entries
                .insert(slug.to_string(), IndexEntry::from_catalog(document));
            return Ok(());
        }
        let mut state = self.state.lock().await;
        let mut retried = false;
        loop {
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
            // Which file is the main one lives in the shared document, where an
            // editor changes it; the index keeps a copy because the landing page
            // and the routes read the entry and never open the session.
            if !main.is_empty() {
                updated.main = main.to_string();
            }
            if let Some(sha) = sha {
                updated.sha = sha.to_string();
                updated.updated_at = timestamp();
            }
            if updated.size == entry.size
                && updated.sha == entry.sha
                && updated.source_format == entry.source_format
                && updated.main == entry.main
            {
                return Ok(());
            }
            state.entries.insert(slug.to_string(), updated);
            match self.save_locked(&mut state).await {
                Ok(()) => return Ok(()),
                Err(BlobError::Conflict) if !retried => {
                    // Another instance moved the index first -- most likely
                    // its own checkpoint for a different document. Reload
                    // what it left and recompute this update against that,
                    // once, rather than dropping a size or a checkpoint sha
                    // that will make the next quota decision, or the next
                    // reader, wrong until someone notices.
                    state.entries.insert(slug.to_string(), entry);
                    self.refresh_locked(&mut state).await?;
                    retried = true;
                }
                Err(err) => {
                    // The index did not move, so neither does the copy of it in
                    // memory: a size recorded here and nowhere else would make the
                    // next quota decision from a number no later run can see.
                    state.entries.insert(slug.to_string(), entry);
                    return Err(err.to_string());
                }
            }
        }
    }

    /// Reject a conflicting name before a replacement upload writes content.
    pub async fn check_project_title(&self, slug: &str, title: &str) -> Result<(), String> {
        if let Some(catalog) = &self.catalog {
            let slug = slug.to_string();
            let title = title.to_string();
            return catalog
                .execute_catalog(STORE_JOB_BYTES + slug.len() + title.len(), move |catalog| {
                    catalog.check_project_title(&slug, &title)
                })
                .await
                .map_err(|err| err.to_string());
        }
        let state = self.state.lock().await;
        let Some(entry) = state.entries.get(slug) else {
            return Ok(());
        };
        if title.is_empty() || title == entry.title {
            return Ok(());
        }
        if state.entries.values().any(|other| {
            other.slug != slug
                && other.publisher_id == entry.publisher_id
                && (!entry.publisher_id.is_empty() || other.publisher == entry.publisher)
                && other.title.trim().to_lowercase() == title.trim().to_lowercase()
        }) {
            return Err("A project with this name already exists. Choose a different name.".into());
        }
        Ok(())
    }

    /// Renames a document. A publish onto an existing slug is an edit into its
    /// session rather than a new version, so the title is the one thing about
    /// the index entry such a publish still changes.
    pub async fn rename(&self, slug: &str, title: &str) -> Result<(), String> {
        if let Some(catalog) = &self.catalog {
            let Some(mut document) = document_row(catalog, slug)
                .await
                .map_err(|err| err.to_string())?
            else {
                return Ok(());
            };
            if title.is_empty() || title == document.title {
                return Ok(());
            }
            document.title = title.to_string();
            document.updated_at = timestamp();
            let document = catalog
                .execute_catalog(STORE_JOB_BYTES + document.slug.len(), move |catalog| {
                    catalog.update_document(&document)
                })
                .await
                .map_err(|err| err.to_string())?;
            self.state
                .lock()
                .await
                .entries
                .insert(slug.to_string(), IndexEntry::from_catalog(document));
            return Ok(());
        }
        let mut state = self.state.lock().await;
        let mut retried = false;
        loop {
            let Some(entry) = state.entries.get(slug).cloned() else {
                return Ok(());
            };
            if entry.title == title || title.is_empty() {
                return Ok(());
            }
            if state.entries.values().any(|other| {
                other.slug != slug
                    && other.publisher_id == entry.publisher_id
                    && (!entry.publisher_id.is_empty() || other.publisher == entry.publisher)
                    && other.title.trim().to_lowercase() == title.trim().to_lowercase()
            }) {
                return Err(
                    "A project with this name already exists. Choose a different name.".into(),
                );
            }
            let mut updated = entry.clone();
            updated.title = title.to_string();
            updated.updated_at = timestamp();
            state.entries.insert(slug.to_string(), updated);
            match self.save_locked(&mut state).await {
                Ok(()) => return Ok(()),
                Err(BlobError::Conflict) if !retried => {
                    state.entries.insert(slug.to_string(), entry);
                    self.refresh_locked(&mut state).await?;
                    retried = true;
                }
                Err(err) => {
                    state.entries.insert(slug.to_string(), entry);
                    return Err(err.to_string());
                }
            }
        }
    }

    /// Rewrites one entry -- whom it is shared with, who owns it -- under the
    /// same lock every other index write takes. The change is applied to a
    /// copy, so a refused write leaves the entry exactly as it was, and the
    /// closure may refuse it itself, which is how a grant the deployment's
    /// switches forbid is turned away without anything being written.
    ///
    /// `change` may run twice: once against the entry this instance had, and
    /// -- only if that save loses the compare-and-swap to a write from
    /// another instance -- once more against the entry the winner left
    /// behind. It must therefore be a plain function of the entry it is
    /// given, not of anything captured that a second run would repeat, which
    /// every caller in this codebase already is.
    pub async fn modify<F>(&self, slug: &str, change: F) -> Result<IndexEntry, ModifyError>
    where
        F: Fn(&mut IndexEntry) -> Result<(), String>,
    {
        if let Some(catalog) = &self.catalog {
            let Some(mut entry) = load_catalog_entry(catalog, slug, true)
                .await
                .map_err(|err| ModifyError::Storage(err.to_string()))?
            else {
                return Err(ModifyError::NotFound);
            };
            change(&mut entry).map_err(ModifyError::Refused)?;
            update_catalog_entry_access(catalog, entry, None)
                .await
                .map_err(|err| ModifyError::Storage(err.to_string()))?;
            let refreshed = load_catalog_entry(catalog, slug, true)
                .await
                .map_err(|err| ModifyError::Storage(err.to_string()))?
                .ok_or(ModifyError::NotFound)?;
            self.state
                .lock()
                .await
                .entries
                .insert(slug.to_string(), refreshed.clone());
            return Ok(refreshed);
        }
        let mut state = self.state.lock().await;
        let mut retried = false;
        loop {
            let Some(entry) = state.entries.get(slug).cloned() else {
                return Err(ModifyError::NotFound);
            };
            let mut updated = entry.clone();
            change(&mut updated).map_err(ModifyError::Refused)?;
            state.entries.insert(slug.to_string(), updated.clone());
            match self.save_locked(&mut state).await {
                Ok(()) => return Ok(updated),
                Err(BlobError::Conflict) if !retried => {
                    // Another instance moved the index first -- maybe with
                    // exactly the revocation or grant this call is racing.
                    // Reload what it left and retry this edit against that,
                    // once, instead of reporting a conflict the caller has no
                    // way to act on and leaving this instance's index stale
                    // for every read after this one too.
                    state.entries.insert(slug.to_string(), entry);
                    if let Err(err) = self.refresh_locked(&mut state).await {
                        return Err(ModifyError::Storage(err));
                    }
                    retried = true;
                }
                Err(err) => {
                    state.entries.insert(slug.to_string(), entry);
                    return Err(ModifyError::Storage(err.to_string()));
                }
            }
        }
    }

    pub async fn modify_as_owner<F>(
        &self,
        slug: &str,
        actor: &MutationActor,
        change: F,
    ) -> Result<IndexEntry, ModifyError>
    where
        F: Fn(&mut IndexEntry) -> Result<(), String>,
    {
        let Some(catalog) = &self.catalog else {
            return self.modify(slug, change).await;
        };
        let Some(mut entry) = load_catalog_entry(catalog, slug, true)
            .await
            .map_err(|err| ModifyError::Storage(err.to_string()))?
        else {
            return Err(ModifyError::NotFound);
        };
        change(&mut entry).map_err(ModifyError::Refused)?;
        update_catalog_entry_access(catalog, entry.clone(), Some(actor.clone()))
            .await
            .map_err(|err| ModifyError::Refused(err.to_string()))?;
        self.state
            .lock()
            .await
            .entries
            .insert(slug.to_string(), entry.clone());
        Ok(entry)
    }

    /// Hands a visitor's documents to the account that has just signed in:
    /// every entry whose publisher is that visitor key is rewritten to the
    /// login and the numeric id, and the quota moves with them, since the
    /// quota is counted by publisher. A document with no publisher at all is
    /// nobody's to adopt and stays as it is. Returns how many moved.
    pub async fn adopt(
        &self,
        visitor_key: &str,
        login: &str,
        id: &str,
        name: &str,
    ) -> Result<usize, String> {
        if visitor_key.is_empty() || login.is_empty() {
            return Ok(0);
        }
        if let Some(catalog) = &self.catalog {
            let provider = id.split_once(':').map(|part| part.0).unwrap_or("github");
            let account = Account {
                id: id.to_string(),
                provider: provider.into(),
                handle: login.to_lowercase(),
                name: name.to_string(),
                email: String::new(),
                first_seen: timestamp(),
                last_seen: timestamp(),
                plan: "default".into(),
                status: "active".into(),
                session_generation: if cfg!(test) {
                    "test-session-generation".into()
                } else {
                    random_storage_id()
                },
                erasure_cursor: None,
            };
            let visitor = visitor_key.to_string();
            let new_owner = id.to_string();
            let per_owner = self.config.storage.per_owner;
            // The account row and every transfer it authorises run in one job.
            // Each transfer is still its own transaction, exactly as before;
            // what the job removes is the runtime worker parked on the
            // connection once per document in the deployment.
            let moved = catalog
                .execute_catalog(STORE_JOB_BYTES + account.id.len(), move |catalog| {
                    catalog.upsert_account(&account)?;
                    let mut moved = 0;
                    for document in catalog.documents()? {
                        if document.owner_id.is_none() && document.owner_key == visitor {
                            catalog.transfer_ownership(&document.slug, &new_owner, per_owner)?;
                            moved += 1;
                        }
                    }
                    Ok(moved)
                })
                .await
                .map_err(|err| err.to_string())?;
            let entries = catalog_entries(catalog)
                .await
                .map_err(|err| err.to_string())?;
            let mut state = self.state.lock().await;
            for (slug, entry) in entries {
                state.entries.insert(slug, entry);
            }
            return Ok(moved);
        }
        let mut state = self.state.lock().await;
        let mut retried = false;
        loop {
            let mine: Vec<String> = state
                .entries
                .values()
                .filter(|entry| !entry.publisher.is_empty() && entry.publisher == visitor_key)
                .map(|entry| entry.slug.clone())
                .collect();
            if mine.is_empty() {
                return Ok(0);
            }
            let before = state.entries.clone();
            for slug in &mine {
                if let Some(entry) = state.entries.get_mut(slug) {
                    entry.publisher = login.to_lowercase();
                    entry.publisher_id = id.to_string();
                    entry.publisher_name = name.to_string();
                }
            }
            match self.save_locked(&mut state).await {
                Ok(()) => return Ok(mine.len()),
                Err(BlobError::Conflict) if !retried => {
                    // Reload and recompute which documents are still the
                    // visitor's to adopt: another instance may have already
                    // moved some of them, or none, and the answer has to come
                    // from what is actually stored, not from this instance's
                    // now-stale guess.
                    state.entries = before;
                    self.refresh_locked(&mut state).await?;
                    retried = true;
                }
                Err(err) => {
                    state.entries = before;
                    return Err(err.to_string());
                }
            }
        }
    }

    /// How many bytes this document may occupy before it carries its owner or
    /// the deployment over a ceiling. `None` for a document with no index
    /// entry, which has no owner to charge and no ceiling to reach.
    pub async fn room_for(&self, slug: &str) -> Option<i64> {
        // `state.entries` is only a compatibility cache in catalogue mode: it
        // starts empty in `open_with_catalog` and gains a slug only once
        // something else has already looked it up. Answering from that cache
        // alone means a document untouched since startup reads as "no
        // ceiling" instead of the ceiling it actually has, and even a cached
        // document's ceiling is computed against whatever fraction of the
        // deployment happens to be cached rather than the whole of it. The
        // catalogue, when there is one, is asked directly instead.
        if let Some(catalog) = &self.catalog {
            let limits = self.config.storage;
            let slug = slug.to_string();
            return catalog
                .execute_catalog(STORE_JOB_BYTES + slug.len(), move |catalog| {
                    catalog.physical_room_for(&slug, limits.per_owner, limits.total)
                })
                .await
                .ok()?;
        }
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
        owner_id: &str,
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
            let same_owner = if !owner_id.is_empty() {
                entry.publisher_id == owner_id
            } else {
                entry.publisher == owner
            };
            if !same_owner {
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
        if let Some(catalog) = &self.catalog {
            let document = document_row(catalog, slug)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| format!("document {slug} was not found"))?;
            {
                let slug = slug.to_string();
                catalog
                    .execute_catalog(STORE_JOB_BYTES + slug.len(), move |catalog| {
                        catalog.begin_delete(&slug).map(|_| ())
                    })
                    .await
                    .map_err(|err| err.to_string())?;
            }
            let mut keys: Vec<(String, i64)> = Vec::new();
            for prefix in
                crate::storage::maintenance::document_object_prefixes(slug, &document.storage_id)
            {
                let found = self
                    .blobs
                    .list(&prefix)
                    .await
                    .map_err(|err| format!("could not enumerate document objects: {err}"))?;
                keys.extend(found.into_iter().map(|object| (object.key, object.size)));
            }
            keys.extend(
                [
                    examples_key(slug),
                    room_key(slug),
                    room_lock_key(slug),
                    crate::storage::blob::session_key(slug),
                    crate::storage::blob::history_index_key(slug),
                    format!("chat/{slug}.json"),
                    legacy_source_key(slug),
                    format!("documents/{slug}"),
                ]
                .into_iter()
                .map(|key| (key, 0)),
            );
            keys.sort_by(|left, right| left.0.cmp(&right.0));
            keys.dedup_by(|left, right| left.0 == right.0);
            // Queue every known object before deleting any of them.  A crash
            // after this point leaves the document deleting and the durable
            // queue is sufficient for the next maintenance pass to resume.
            {
                let queued: Vec<(String, i64)> = keys.clone();
                let slug = slug.to_string();
                catalog
                    .execute_catalog(
                        STORE_JOB_BYTES
                            + queued.iter().map(|(key, _)| key.len() + 16).sum::<usize>(),
                        move |catalog| {
                            for (key, bytes) in &queued {
                                catalog.queue_delete(&crate::storage::catalog::PendingDelete {
                                    slug: slug.clone(),
                                    object_key: key.clone(),
                                    bytes: *bytes,
                                    queued_at: now_unix(),
                                    delete_after: now_unix(),
                                })?;
                            }
                            Ok(())
                        },
                    )
                    .await
                    .map_err(|err| err.to_string())?;
            }
            let mut removed = 0;
            for (key, _) in &keys {
                self.blobs
                    .delete(std::slice::from_ref(key))
                    .await
                    .map_err(|err| format!("could not reclaim document objects: {err}"))?;
                let (slug, key) = (slug.to_string(), key.clone());
                catalog
                    .execute_catalog(STORE_JOB_BYTES + key.len(), move |catalog| {
                        catalog.complete_delete_object(&slug, &key)
                    })
                    .await
                    .map_err(|err| err.to_string())?;
                removed += 1;
            }
            crate::storage::journal::JournalStore::new(catalog.clone())
                .retire_storage_async(document.storage_id.clone(), now_unix())
                .await
                .map_err(|err| format!("could not retire journal objects: {err}"))?;
            let retirement_worker = crate::storage::maintenance::JournalRetirementWorker::new(
                catalog.clone(),
                self.blobs.clone(),
                1_000,
            )
            .map_err(|err| err.to_string())?;
            retirement_worker
                .run_once(now_unix())
                .await
                .map_err(|err| err.to_string())?;
            {
                let slug = slug.to_string();
                catalog
                    .execute_catalog(STORE_JOB_BYTES + slug.len(), move |catalog| {
                        catalog.finish_delete(&slug)
                    })
                    .await
                    .map_err(|err| err.to_string())?;
            }
            self.state.lock().await.entries.remove(slug);
            return Ok(removed);
        }
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
        if let Ok(found) = self
            .blobs
            .list(&crate::storage::blob::history_prefix(slug))
            .await
        {
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
                crate::storage::blob::session_key(slug),
                crate::storage::blob::history_index_key(slug),
                format!("chat/{slug}.json"),
            ])
            .await;

        let mut state = self.state.lock().await;
        let mut retried = false;
        loop {
            state.entries.remove(slug);
            match self.save_locked(&mut state).await {
                Ok(()) => return Ok(removed),
                Err(BlobError::Conflict) if !retried => {
                    // Removing an already-removed slug is still the outcome
                    // asked for, so there is nothing to reapply here beyond
                    // reloading and removing again against what is current.
                    self.refresh_locked(&mut state).await?;
                    retried = true;
                }
                Err(err) => return Err(err.to_string()),
            }
        }
    }

    /// Writes the index, and only over the version these entries were read
    /// from. On a single-writer deployment the mutex already guarantees that,
    /// and the check costs a comparison; on a bucket two processes can reach,
    /// it is what stops one of them overwriting the other's documents. A
    /// conflict is reported rather than retried, because what to do about it
    /// is the caller's decision.
    ///
    /// Only ever called in legacy JSON-index mode: every catalogue-backed
    /// caller returns before reaching this, so there is no catalogue branch
    /// here to keep in sync with the catalogue schema.
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

    /// Re-reads the index into these entries, keeping this process's own
    /// pending change to `slug`. Called when a write loses the
    /// compare-and-swap: another process moved the index, so the quota
    /// decision that was made against the old one has to be made again against
    /// the new one. That is what serializes admission deployment-wide -- a
    /// decision is only ever committed against the index it was made from.
    ///
    /// Returns whatever storage actually had for `slug` before this process's
    /// own in-flight change was laid back over it, so a caller that ends up
    /// refusing that change can restore exactly what was there rather than
    /// guessing -- or, worse, assuming there was nothing.
    async fn reload_locked(
        &self,
        state: &mut StoreState,
        slug: &str,
    ) -> Result<Option<IndexEntry>, String> {
        let (entries, at) = load_index(self.blobs.as_ref()).await?;
        let stored = entries.get(slug).cloned();
        let mine = state.entries.get(slug).cloned();
        state.entries = entries;
        state.index_version = at;
        if let Some(mine) = mine {
            state.entries.insert(slug.to_string(), mine);
        } else {
            state.entries.remove(slug);
        }
        Ok(stored)
    }

    /// Reloads the index from storage into `state`, unconditionally. Unlike
    /// `reload_locked`, which keeps this process's own not-yet-durable value
    /// for the one document it was admitting, this discards everything held
    /// in memory: it is what every other losing write retries against, since
    /// the value to redo the edit on is whatever the winner actually left,
    /// not this instance's guess at it.
    async fn refresh_locked(&self, state: &mut StoreState) -> Result<(), String> {
        let (entries, at) = load_index(self.blobs.as_ref()).await?;
        state.entries = entries;
        state.index_version = at;
        Ok(())
    }

    /// Reloads the index if the copy in storage has moved past the one this
    /// process is holding, and does nothing at all -- not even a parse -- if
    /// it has not. `get` calls this on a miss, which is the read-side half of
    /// catching up with another instance sharing the same storage: a document
    /// published there becomes visible here, at the cost of one read that, on
    /// the single-writer deployment this store is built for, always confirms
    /// there was nothing to catch up on.
    pub async fn refresh(&self) -> Result<(), String> {
        let mut state = self.state.lock().await;
        match self.blobs.get_versioned(INDEX_KEY).await {
            Ok((raw, at)) => {
                if at == state.index_version {
                    return Ok(());
                }
                let entries: HashMap<String, IndexEntry> =
                    serde_json::from_slice(&raw).map_err(|err| {
                        format!(
                            "the index in {} is not readable ({err}); move it aside to start empty",
                            self.blobs.describe()
                        )
                    })?;
                state.entries = entries;
                state.index_version = at;
                Ok(())
            }
            // No index in storage is not news this process should act on:
            // either there never was one, and memory is already empty, or it
            // went missing under a running deployment, and forgetting every
            // document here would only make that worse. What is held stays.
            Err(BlobError::NotFound) => Ok(()),
            Err(err) => Err(format!(
                "could not read the index from {}: {err}",
                self.blobs.describe()
            )),
        }
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
    let sum = Sha256::digest(format!("librepaper example {base}").as_bytes());
    let alphabet = config.suffix_alphabet.as_bytes();
    sum.iter()
        .take(config.suffix_length)
        .map(|b| alphabet[*b as usize % alphabet.len()] as char)
        .collect()
}

/// The content hash a document is addressed by.
pub fn digest_of(html: &str) -> String {
    digest_of_bytes(html.as_bytes())
}

/// The same, for bytes nobody is going to read as text: a figure, a font.
pub fn digest_of_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn random_storage_id() -> String {
    hex::encode(crate::auth::random_bytes(16))
}

impl IndexEntry {
    fn from_catalog(document: crate::storage::catalog::Document) -> Self {
        let unowned = document.owner_id.is_none() && document.owner_key.starts_with("example:");
        let publisher = if unowned {
            String::new()
        } else {
            document.owner_key.clone()
        };
        Self {
            slug: document.slug,
            storage_id: document.storage_id,
            title: document.title,
            sha: document.sha,
            created_at: document.created_at,
            updated_at: document.updated_at,
            example: document.example,
            unowned,
            // Signed-in owners are represented by owner_id in the catalogue;
            // visitors by owner_key. Keep the old authorization model's
            // fields populated so the rest of the HTTP layer remains stable.
            publisher,
            publisher_id: document.owner_id.unwrap_or_default(),
            publisher_name: String::new(),
            size: document.size,
            source_format: document.source_format,
            main: document.main,
            ..Self::default()
        }
    }
}

/// One document row, read on a blocking thread.
async fn document_row(
    catalog: &Arc<Catalog>,
    slug: &str,
) -> Result<Option<crate::storage::catalog::Document>, CatalogError> {
    let slug = slug.to_string();
    catalog
        .execute_catalog(STORE_JOB_BYTES + slug.len(), move |catalog| {
            catalog.document(&slug)
        })
        .await
        .map_err(CatalogError::from)
}

/// Every active document as a compatibility entry.  One job: this is the
/// unbounded whole-table read the inventory flags, and splitting it into a
/// job per document would multiply one already-expensive call into hundreds
/// of dispatches without bounding anything.
async fn catalog_entries(
    catalog: &Arc<Catalog>,
) -> Result<HashMap<String, IndexEntry>, CatalogError> {
    catalog
        .execute_catalog(STORE_JOB_BYTES, |catalog| {
            let mut entries = HashMap::new();
            for document in catalog.documents()? {
                if document.status != "active" {
                    continue;
                }
                let slug = document.slug.clone();
                if let Some(entry) = load_catalog_entry_sql(catalog, &slug, true)? {
                    entries.insert(slug, entry);
                }
            }
            Ok(entries)
        })
        .await
        .map_err(CatalogError::from)
}

/// Load one listing/detail entry as a single job.
///
/// The link secrets are unsealed *after* the connection closure returns rather
/// than inside it.  Unsealing is a catalogue call of its own, and calling one
/// while the connection is held is the re-entrancy this boundary forbids: it
/// works only for as long as `open_link_key` happens not to touch the
/// connection, and it would deadlock the single connection the moment it did.
async fn load_catalog_entry(
    catalog: &Arc<Catalog>,
    slug: &str,
    decrypt_links: bool,
) -> Result<Option<IndexEntry>, CatalogError> {
    let slug = slug.to_string();
    catalog
        .execute_catalog(STORE_JOB_BYTES + slug.len(), move |catalog| {
            load_catalog_entry_sql(catalog, &slug, decrypt_links)
        })
        .await
        .map_err(CatalogError::from)
}

fn load_catalog_entry_sql(
    catalog: &Catalog,
    slug: &str,
    decrypt_links: bool,
) -> Result<Option<IndexEntry>, CatalogError> {
    let Some(document) = catalog.document(slug)? else {
        return Ok(None);
    };
    if document.status != "active" || document.pending_publication.is_some() {
        return Ok(None);
    }
    let mut entry = IndexEntry::from_catalog(document);
    // Sealed envelopes come out of SQL here and are opened below, outside the
    // connection.
    #[allow(clippy::type_complexity)]
    let mut sealed_links: Vec<(String, String, Vec<u8>, String, Option<i64>, String, String)> =
        Vec::new();
    catalog.with_connection(|connection| {
        if let Some(owner_id) = (!entry.publisher_id.is_empty()).then_some(&entry.publisher_id) {
            if let Some((handle, name)) = connection
                .query_row(
                    "SELECT handle,name FROM accounts WHERE id = ?1",
                    [owner_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?
            {
                entry.publisher = handle;
                entry.publisher_name = name;
            }
        }
        let mut grants = connection.prepare(
            "SELECT g.role, g.account_id, a.handle, a.name, g.since
             FROM grants g JOIN accounts a ON a.id = g.account_id
             WHERE g.slug = ?1 ORDER BY g.account_id LIMIT ?2",
        )?;
        let rows = grants.query_map(rusqlite::params![slug, MAX_GRANTS_PER_RESULT], |row| {
            Ok((
                row.get::<_, String>(0)?,
                Grant {
                    id: row.get(1)?,
                    login: row.get(2)?,
                    name: row.get(3)?,
                    since: row.get(4)?,
                },
            ))
        })?;
        for row in rows {
            let (role, grant) = row?;
            match role.as_str() {
                "editor" => entry.editors.push(grant),
                "commenter" => entry.commenters.push(grant),
                _ => {}
            }
        }
        if decrypt_links {
            let mut links = connection.prepare(
                "SELECT role, hash, sealed, label, budget, since, until
                 FROM links WHERE slug = ?1 ORDER BY role LIMIT ?2",
            )?;
            sealed_links = links
                .query_map(rusqlite::params![slug, MAX_LINKS_PER_RESULT], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
        } else {
            let mut links = connection.prepare(
                "SELECT role, hash, label, budget, since, until
                 FROM links WHERE slug = ?1 ORDER BY role LIMIT ?2",
            )?;
            entry.links = links
                .query_map(rusqlite::params![slug, MAX_LINKS_PER_RESULT], |row| {
                    Ok(LinkGrant {
                        role: row.get(0)?,
                        hash: row.get(1)?,
                        key: String::new(),
                        label: row.get(2)?,
                        budget: row.get(3)?,
                        since: row.get(4)?,
                        until: row.get(5)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
        }
        let mut guests = connection.prepare(
            "SELECT ge.account_id, a.name, ge.since, ge.link_hash
             FROM guests ge JOIN accounts a ON a.id = ge.account_id
             WHERE ge.slug = ?1 ORDER BY ge.account_id LIMIT ?2",
        )?;
        entry.guests = guests
            .query_map(rusqlite::params![slug, MAX_GUESTS_PER_RESULT], |row| {
                Ok(Guest {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    since: row.get(2)?,
                    link: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(())
    })?;
    for (role, hash, sealed, label, budget, since, until) in sealed_links {
        let key = catalog.open_link_key(&entry.storage_id, &role, &hash, &sealed)?;
        entry.links.push(LinkGrant {
            role,
            hash,
            key,
            label,
            budget,
            since,
            until,
        });
    }
    Ok(Some(entry))
}

/// Apply a compatibility entry to the authoritative catalogue without
/// treating its cached access lists as a whole-row snapshot.  Metadata and
/// access changes commit together; unchanged access rows retain their sealed
/// values and absent rows alone are removed.
async fn update_catalog_entry_access(
    catalog: &Arc<Catalog>,
    entry: IndexEntry,
    actor: Option<MutationActor>,
) -> Result<(), CatalogError> {
    catalog
        .execute_catalog(STORE_JOB_BYTES + entry.slug.len(), move |catalog| {
            update_catalog_entry_access_sql(catalog, &entry, actor.as_ref())
        })
        .await
        .map_err(CatalogError::from)
}

/// A read-modify-write across five catalogue calls, exactly as before: the
/// final `update_document_access` re-checks the actor's rights and session
/// generation inside its own transaction, which is what makes the sequence
/// safe without being one transaction.  Running it as one job keeps that
/// sequence off a runtime worker without changing its atomicity.
fn update_catalog_entry_access_sql(
    catalog: &Catalog,
    entry: &IndexEntry,
    actor: Option<&MutationActor>,
) -> Result<(), CatalogError> {
    let mut document = catalog
        .document(&entry.slug)?
        .ok_or(CatalogError::NotFound)?;
    // Every caller of this function -- `modify`, `modify_as_owner`, and the
    // link/grant/owner closures in the HTTP layer -- changes only who a
    // document names and whose it is; none of them touch the document's
    // content or its checkpoint. `entry` is loaded, mutated by the caller's
    // closure, and written back here, but the load can race a checkpoint
    // committed by another writer in between: copying `title`, `sha`,
    // `updated_at`, `size`, `counted_size`, `source_format`, or `main` from
    // this possibly-stale snapshot would silently revert that newer
    // checkpoint. So only the fields an access change is actually allowed to
    // touch are written back: `example`, and the owner identity.
    document.example = entry.example;
    document.owner_key = if entry.unowned {
        // Anonymous command-line uploads use an internal sentinel so they
        // remain publicly listable without accidentally becoming owned by a
        // visitor. Preserve it when compatibility metadata is synchronized.
        format!("example:{}", entry.slug)
    } else if entry.publisher_id.is_empty() {
        entry.publisher.clone()
    } else {
        String::new()
    };
    document.owner_id = (!entry.publisher_id.is_empty()).then(|| entry.publisher_id.clone());

    let current_links = catalog.links(&entry.slug)?;
    let mut links = Vec::with_capacity(entry.links.len());
    for link in &entry.links {
        let sealed = current_links
            .iter()
            .find(|current| {
                current.role == link.role
                    && current.hash == link.hash
                    && current.label == link.label
                    && current.budget == link.budget
                    && current.since == link.since
                    && current.until == link.until
            })
            .map(|current| current.sealed.clone())
            .map(Ok)
            .unwrap_or_else(|| {
                catalog.seal_link_key(&entry.storage_id, &link.role, &link.hash, &link.key)
            })?;
        links.push(crate::storage::catalog::Link {
            slug: entry.slug.clone(),
            role: link.role.clone(),
            hash: link.hash.clone(),
            sealed,
            label: link.label.clone(),
            budget: link.budget,
            since: link.since.clone(),
            until: link.until.clone(),
        });
    }
    let mut grants = Vec::with_capacity(entry.editors.len() + entry.commenters.len());
    for (role, rows) in [("editor", &entry.editors), ("commenter", &entry.commenters)] {
        for grant in rows {
            let provider = grant
                .id
                .split_once(':')
                .map(|part| part.0)
                .unwrap_or("github");
            catalog.upsert_account(&Account {
                id: grant.id.clone(),
                provider: provider.into(),
                handle: grant.login.clone(),
                name: grant.shown().to_string(),
                email: String::new(),
                first_seen: grant.since.clone(),
                last_seen: grant.since.clone(),
                plan: "default".into(),
                status: "active".into(),
                session_generation: if cfg!(test) {
                    "test-session-generation".into()
                } else {
                    random_storage_id()
                },
                erasure_cursor: None,
            })?;
            grants.push(crate::storage::catalog::Grant {
                slug: entry.slug.clone(),
                role: role.to_string(),
                account_id: grant.id.clone(),
                since: grant.since.clone(),
            });
        }
    }
    let guests = entry
        .guests
        .iter()
        .map(|guest| crate::storage::catalog::Guest {
            slug: entry.slug.clone(),
            account_id: guest.id.clone(),
            since: guest.since.clone(),
            link_hash: guest.link.clone(),
        })
        .collect::<Vec<_>>();
    catalog
        .update_document_access(
            &document,
            &grants,
            &links,
            &guests,
            actor.map(|actor| {
                (
                    actor.account_id.as_str(),
                    actor.owner_key.as_str(),
                    actor.session_generation.as_str(),
                )
            }),
        )
        .map(|_| ())
}
