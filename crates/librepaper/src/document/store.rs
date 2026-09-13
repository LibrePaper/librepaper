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

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::auth::stored_id;
use crate::config::Configuration;
use crate::storage::blob::{
    document_key, document_prefix, examples_key, room_key, room_lock_key, source_key,
    source_prefix, BlobError, BlobStore, BlobVersion, ObjectId as BlobObjectId, INDEX_KEY,
};
use crate::storage::catalog::{
    Account, Catalog, CatalogError, CheckpointCommit, CheckpointId, DocumentId, ObjectId,
    ObjectKind, OperationActor, OperationKind, OperationRequest, SourceFormat, UnixMillis,
    V2AdmissionLimits, V2ObjectAllocation, V2OperationInput, V2SourceAdmissionInput,
};
use crate::util::new_id;
use crate::util::{now_unix, parse_timestamp, timestamp};

const MAX_LINKS_PER_RESULT: i64 = 16;
const MAX_GUESTS_PER_RESULT: i64 = 256;
const CATALOG_PAGE_SIZE: u32 = 200;

static SOURCE_ENCODING_POOL: std::sync::OnceLock<crate::storage::encoding::EncodingPool> =
    std::sync::OnceLock::new();

fn source_encoding_pool() -> &'static crate::storage::encoding::EncodingPool {
    SOURCE_ENCODING_POOL.get_or_init(|| {
        crate::storage::encoding::EncodingPool::new(
            2,
            64 * 1024 * 1024,
            crate::storage::encoding::EncodingProfile::default(),
        )
        .expect("static source encoding profile is valid")
    })
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
    /// The live link hash resolved from the requesting account's v2 bookmark.
    /// This is populated only for a bounded listing page and is never
    /// persisted or used as a reverse guest index.  The server still passes
    /// it through `role_of`, so owner checks and deployment ceilings apply.
    #[serde(skip)]
    pub bookmark_link_hash: Option<String>,
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
        // A link asks the deployment ceiling because it is a caller with no
        // name: an
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

    /// The link that carries a role, live or not. The dead one still matters
    /// because it lets the dialog report that the link expired.
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
    /// a row actually went so a revoke that matched nothing can say so.
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

    /// Whether a caller is on this document at all: its owner or the holder of
    /// a live link.
    pub fn names(&self, owner_key: &str, caller_id: &str, link_hash: &str, now: i64) -> bool {
        self.owned_by(owner_key, caller_id) || self.link_role(link_hash, now).is_some()
    }

    /// Whether a caller may read this document at all: its owner and whoever
    /// holds a live link, since a read link
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

fn source_put_error(error: CatalogError) -> PutError {
    match error {
        CatalogError::Refused(crate::storage::catalog::CatalogRefusal::ActorRights, _) => {
            PutError::Authorization {
                status: 403,
                message: "edit access changed",
            }
        }
        CatalogError::Refused(crate::storage::catalog::CatalogRefusal::RequestExpired, _) => {
            PutError::Authorization {
                status: 410,
                message: "request receipt has expired",
            }
        }
        CatalogError::Refused(crate::storage::catalog::CatalogRefusal::OwnerBytes, _) => PutError::Quota {
            status: 507,
            message: "your storage quota is used up; delete a document first",
        },
        CatalogError::Refused(crate::storage::catalog::CatalogRefusal::DeploymentBytes, _) => PutError::Quota {
            status: 507,
            message: "this deployment has no room left",
        },
        CatalogError::Refused(crate::storage::catalog::CatalogRefusal::OwnerDocuments, _) => PutError::Quota {
            status: 507,
            message: "you have reached the document limit; delete one first",
        },
        CatalogError::Refused(crate::storage::catalog::CatalogRefusal::UploadRate, _) => {
            PutError::Quota {
                status: 429,
                message: "too many uploads this hour; try later",
            }
        }
        CatalogError::NotFound => PutError::Authorization {
            status: 404,
            message: "not found",
        },
        CatalogError::Conflict(message) => PutError::Authorization {
            status: 409,
            message: if message == "A project with this name already exists. Choose a different name." {
                "A project with this name already exists. Choose a different name."
            } else {
                "the document changed; reload it before retrying"
            },
        },
        other => PutError::Storage(other.to_string()),
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
                // quota, recheck the live pressure decision for diagnostics.
                // This never creates a durable pressure plan; the retention
                // worker independently recalculates document eligibility.
                // A refusal caused only by this prospective write is a no-op
                // while current usage fits.
                if matches!(
                    &result,
                    Err(crate::storage::catalog::CatalogError::Refused(
                        crate::storage::catalog::CatalogRefusal::OwnerBytes
                            | crate::storage::catalog::CatalogRefusal::DeploymentBytes,
                        _
                    ))
                ) {
                    let _ = catalog.check_hard_pressure_for_slug_for_growth_with_limits(
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
                crate::storage::catalog::CatalogError::Refused(
                    crate::storage::catalog::CatalogRefusal::ActorRights,
                    _,
                ) => PutError::Authorization {
                    status: 403,
                    message: "edit access changed",
                },
                crate::storage::catalog::CatalogError::NotFound => PutError::Authorization {
                    status: 404,
                    message: "not found",
                },
                crate::storage::catalog::CatalogError::Refused(kind, _)
                    if matches!(
                        kind,
                        crate::storage::catalog::CatalogRefusal::OwnerBytes
                            | crate::storage::catalog::CatalogRefusal::DeploymentBytes
                    ) =>
                {
                    PutError::Quota {
                        status: 507,
                        message: if kind == crate::storage::catalog::CatalogRefusal::DeploymentBytes
                        {
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
                crate::storage::catalog::CatalogError::Refused(
                    crate::storage::catalog::CatalogRefusal::UploadRate,
                    _,
                ) => PutError::Quota {
                    status: 429,
                    message: "too many uploads this hour; try later",
                },
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
        // Startup recovery is owned by the deployment's v2 recovery worker.
        // A prepared user operation never authorizes activation after restart.
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
                    let mut entry = load_catalog_entry_sql(catalog, &document.slug, false)?
                        .ok_or(CatalogError::NotFound)?;
                    if let Some(account_id) = account_id.as_deref() {
                        if let Some(hash) = catalog.bookmark_link(&document.slug, account_id)? {
                            entry.bookmark_link_hash = Some(hash);
                        }
                    }
                    entries.push(entry);
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
        self.blobs.get(&source_key(&identity, &digest)).await
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
            return Err(PutError::Authorization {
                status: 401,
                message: "catalog publication requires an authenticated mutation actor",
            });
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
            links: shared.links,
            guests: shared.guests,
            bookmark_link_hash: None,
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

    /// Publish a catalogue-backed source with authority captured at the
    /// request boundary. Publication metadata never supplies ownership or a
    /// session generation.
    pub async fn put_as_actor(
        &self,
        v: Publication,
        actor: MutationActor,
    ) -> Result<IndexEntry, PutError> {
        if self.catalog.is_some() {
            self.put_catalog(v, actor).await
        } else {
            self.put(v).await
        }
    }

    /// Publish a complete source directory through the catalogue's immutable
    /// source closure. `files` contains the parsed upload's non-main paths;
    /// the main source remains in `v.source` and is inserted exactly once.
    pub async fn put_directory_as_actor(
        &self,
        v: Publication,
        files: Vec<(String, Vec<u8>)>,
        actor: MutationActor,
    ) -> Result<IndexEntry, PutError> {
        if self.catalog.is_some() {
            self.put_catalog_files(v, files, actor).await
        } else {
            self.put(v).await
        }
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
                    catalog.pending_operation(&storage_id, &wanted)
                })
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "publication receipt is missing".to_string())?;
            if operation.request_digest != request_digest {
                return Err("document has a different publication in progress".into());
            }
            return Ok(request_id);
        }
        let request_id = crate::util::new_request_key();
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

    async fn put_catalog(
        &self,
        v: Publication,
        actor: MutationActor,
    ) -> Result<IndexEntry, PutError> {
        self.put_catalog_files(v, Vec::new(), actor).await
    }

    async fn put_catalog_files(
        &self,
        v: Publication,
        files: Vec<(String, Vec<u8>)>,
        actor: MutationActor,
    ) -> Result<IndexEntry, PutError> {
        let catalog = self
            .catalog
            .as_ref()
            .expect("put_catalog requires a catalogue");
        let existing = document_row(catalog, &v.slug)
            .await
            .map_err(|error| PutError::Storage(error.to_string()))?;
        let now_ms = crate::util::now_millis();
        let now = timestamp();
        if actor.account_id.is_empty() && actor.owner_key.is_empty() && actor.link_hash.is_empty() {
            return Err(PutError::Authorization {
                status: 401,
                message: "publication actor has no accountable identity",
            });
        }
        let account = if !actor.account_id.is_empty() {
            let account = catalog
                .execute_catalog(STORE_JOB_BYTES + actor.account_id.len(), {
                    let account_id = actor.account_id.clone();
                    move |catalog| catalog.account(&account_id)
                })
                .await
                .map_err(|error| PutError::Storage(error.to_string()))?
                .ok_or(PutError::Authorization {
                    status: 401,
                    message: "publication actor account is not active",
                })?;
            if account.status != "active" || account.session_generation != actor.session_generation
            {
                return Err(PutError::Authorization {
                    status: 401,
                    message: "publication actor session has changed",
                });
            }
            Some(account)
        } else {
            None
        };
        if actor.account_id.starts_with("system:") {
            if actor.account_id != "system:examples"
                || actor.owner_key != ""
                || actor.link_hash != ""
                || !actor.policy_editor
                || actor.automation
                || !v.owner_id.is_empty()
            {
                return Err(PutError::Authorization {
                    status: 403,
                    message: "system publication requires the seed capability",
                });
            }
            let account_id = actor.account_id.clone();
            let generation = actor.session_generation.clone();
            let kind = catalog
                .execute_catalog(STORE_JOB_BYTES + account_id.len() + generation.len(), move |catalog| {
                    catalog.with_connection(|connection| {
                        connection
                            .query_row(
                                "SELECT kind FROM accounts WHERE id=?1 AND status='active' AND session_generation=?2",
                                rusqlite::params![account_id, generation],
                                |row| row.get::<_, String>(0),
                            )
                            .map_err(CatalogError::from)
                    })
                })
                .await
                .map_err(|error| PutError::Storage(error.to_string()))?;
            if kind != "system" {
                return Err(PutError::Authorization {
                    status: 403,
                    message: "the seed actor is not a system account",
                });
            }
        }
        if existing.is_none() && !actor.policy_editor && !actor.unowned_publisher {
            return Err(PutError::Authorization {
                status: 403,
                message: "publication actor is not permitted to create documents",
            });
        }

        let format = if v.source_format.is_empty() {
            existing
                .as_ref()
                .map(|document| document.source_format.clone())
                .filter(|format| !format.is_empty())
                .unwrap_or_else(|| "markdown".into())
        } else {
            v.source_format.clone()
        };
        let source_format = match format.as_str() {
            "markdown" => SourceFormat::Markdown,
            "html" => SourceFormat::Html,
            "typst" => SourceFormat::Typst,
            "latex" => SourceFormat::Latex,
            "quarto" => SourceFormat::Quarto,
            _ => return Err(PutError::Storage("invalid source format".into())),
        };
        let title = if v.title.is_empty() {
            existing
                .as_ref()
                .map(|document| document.title.clone())
                .unwrap_or_else(|| "Untitled".into())
        } else {
            v.title.clone()
        };
        let main = if v.main.is_empty() {
            existing
                .as_ref()
                .map(|document| document.main.clone())
                .filter(|path| !path.is_empty())
                .unwrap_or_else(|| match format.as_str() {
                    "html" => "index.html".into(),
                    "typst" => "index.typ".into(),
                    "latex" => "index.tex".into(),
                    "quarto" => "index.qmd".into(),
                    _ => "index.md".into(),
                })
        } else {
            v.main.clone()
        };
        let storage_id = existing
            .as_ref()
            .map(|document| document.storage_id.clone())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(random_storage_id);
        let owner_id = existing
            .as_ref()
            .and_then(|document| document.owner_id.clone())
            .or_else(|| (!actor.account_id.is_empty()).then(|| actor.account_id.clone()));
        let anonymous_owner_id = if actor.account_id.is_empty()
            && actor.link_hash.is_empty()
            && !actor.owner_key.is_empty()
        {
            Some(format!(
                "anonymous:{}",
                hex::encode(Sha256::digest(actor.owner_key.as_bytes()))
            ))
        } else {
            None
        };
        let created_at = existing
            .as_ref()
            .map(|document| document.created_at.clone())
            .unwrap_or_else(|| now.clone());

        let source = v.source.as_bytes().to_vec();
        let mut file_inputs = std::collections::BTreeMap::<String, Vec<u8>>::new();
        for (path, bytes) in files {
            if path != main {
                file_inputs.insert(path, bytes);
            }
        }
        file_inputs.insert(main.clone(), source.clone());
        if file_inputs.is_empty() {
            return Err(PutError::Storage("source directory is empty".into()));
        }

        let mut tree_files = std::collections::BTreeMap::new();
        let mut object_ids = Vec::new();
        let mut physical = Vec::new();
        let mut logical_bytes = 0i64;
        let mut main_file_digest = [0u8; 32];
        for (path, bytes) in &file_inputs {
            logical_bytes = logical_bytes
                .checked_add(
                    i64::try_from(bytes.len())
                        .map_err(|_| PutError::Storage("source file is too large".into()))?,
                )
                .ok_or_else(|| PutError::Storage("source size overflow".into()))?;
            match crate::document::paths::check(&self.config.paths(), path)
                .map_err(|error| PutError::Storage(error.to_string()))?
            {
                crate::document::paths::Kind::Asset => {
                    let object_id = ObjectId::new(random_storage_id())
                        .map_err(|error| PutError::Storage(error.to_string()))?;
                    let digest: [u8; 32] = Sha256::digest(bytes).into();
                    physical.push((
                        object_id.clone(),
                        ObjectKind::Asset,
                        hex::encode(digest),
                        Some(hex::encode(digest)),
                        bytes.clone(),
                        "application/octet-stream",
                    ));
                    object_ids.push(object_id.clone());
                    tree_files.insert(
                        path.clone(),
                        crate::storage::encoding::TreeFileLocator {
                            kind: "asset".into(),
                            file_id: String::new(),
                            logical_digest: digest,
                            logical_length: bytes.len() as u64,
                            recipe: None,
                            asset: Some(crate::storage::encoding::PhysicalLocator {
                                object_id: BlobObjectId::parse(object_id.as_str().to_owned())
                                    .map_err(|error| PutError::Storage(error.to_string()))?,
                                object_digest: digest,
                                logical_digest: Some(digest),
                                logical_length: bytes.len() as u64,
                                byte_length: bytes.len() as u64,
                                encoding_version: 1,
                            }),
                        },
                    );
                }
                crate::document::paths::Kind::Text => {
                    let text = std::str::from_utf8(bytes).map_err(|_| {
                        PutError::Storage(format!("text source file {path} is not UTF-8"))
                    })?;
                    let plan = source_encoding_pool()
                        .try_plan(bytes.clone())
                        .await
                        .map_err(|error| {
                            PutError::Storage(format!("source planning failed: {error}"))
                        })?;
                    let encoded = source_encoding_pool()
                        .try_encode_planned(bytes.clone(), plan, std::collections::HashSet::new())
                        .await
                        .map_err(|error| {
                            PutError::Storage(format!("source encoding failed: {error}"))
                        })?;
                    if path == &main {
                        main_file_digest = encoded.file_digest;
                    }
                    let recipe_id = ObjectId::new(random_storage_id())
                        .map_err(|error| PutError::Storage(error.to_string()))?;
                    let mut locators_by_digest =
                        std::collections::HashMap::with_capacity(encoded.objects.len());
                    encoded.objects.iter().try_for_each(|object| {
                        let object_id = ObjectId::new(random_storage_id())
                            .map_err(|error| PutError::Storage(error.to_string()))?;
                        let object_digest = Sha256::digest(&object.encoded).into();
                        physical.push((
                            object_id.clone(),
                            ObjectKind::SourceChunk,
                            hex::encode(object_digest),
                            Some(hex::encode(object.digest)),
                            object.encoded.clone(),
                            "application/vnd.librepaper.source-chunk",
                        ));
                        object_ids.push(object_id.clone());
                        let locator = crate::storage::encoding::PhysicalLocator {
                            object_id: BlobObjectId::parse(object_id.as_str().to_owned())
                                .map_err(|error| PutError::Storage(error.to_string()))?,
                            object_digest,
                            logical_digest: Some(object.digest),
                            logical_length: object.uncompressed_len as u64,
                            byte_length: object.encoded.len() as u64,
                            encoding_version: 1,
                        };
                        locators_by_digest.insert(object.digest, locator);
                        Ok::<(), PutError>(())
                    })?;
                    let chunk_locators = encoded
                        .recipe
                        .chunks
                        .iter()
                        .map(|chunk| {
                            let locator =
                                locators_by_digest.get(&chunk.digest).ok_or_else(|| {
                                    PutError::Storage(
                                        "source recipe references an absent chunk".into(),
                                    )
                                })?;
                            if locator.logical_length != u64::from(chunk.length) {
                                return Err(PutError::Storage(
                                    "source recipe chunk length differs from encoded object".into(),
                                ));
                            }
                            Ok(locator.clone())
                        })
                        .collect::<Result<Vec<_>, PutError>>()?;
                    let recipe_envelope = crate::storage::encoding::SourceRecipeEnvelope {
                        version: crate::storage::encoding::SOURCE_ENVELOPE_VERSION,
                        recipe: encoded.recipe.clone(),
                        chunk_locators,
                    };
                    let recipe_bytes = recipe_envelope
                        .to_bytes()
                        .map_err(|error| PutError::Storage(error.to_string()))?;
                    let recipe_locator = crate::storage::encoding::PhysicalLocator {
                        object_id: BlobObjectId::parse(recipe_id.as_str().to_owned())
                            .map_err(|error| PutError::Storage(error.to_string()))?,
                        object_digest: Sha256::digest(&recipe_bytes).into(),
                        logical_digest: Some(encoded.file_digest),
                        logical_length: bytes.len() as u64,
                        byte_length: recipe_bytes.len() as u64,
                        encoding_version: 1,
                    };
                    physical.push((
                        recipe_id.clone(),
                        ObjectKind::SourceRecipe,
                        hex::encode(recipe_locator.object_digest),
                        Some(hex::encode(encoded.file_digest)),
                        recipe_bytes.clone(),
                        "application/vnd.librepaper.source-recipe",
                    ));
                    object_ids.push(recipe_id.clone());
                    tree_files.insert(
                        path.clone(),
                        crate::storage::encoding::TreeFileLocator {
                            kind: "text".into(),
                            file_id: String::new(),
                            logical_digest: encoded.file_digest,
                            logical_length: text.len() as u64,
                            recipe: Some(recipe_locator),
                            asset: None,
                        },
                    );
                }
            }
        }
        if main_file_digest == [0; 32] {
            return Err(PutError::Storage("main source file is not text".into()));
        }
        let tree_id = ObjectId::new(random_storage_id())
            .map_err(|error| PutError::Storage(error.to_string()))?;
        let mut tree_envelope = crate::storage::encoding::TreeEnvelope {
            version: crate::storage::encoding::TREE_ENVELOPE_VERSION,
            main_path: main.clone(),
            source_format: format.clone(),
            settings_json: "{}".into(),
            logical_digest: [0; 32],
            files: tree_files,
        };
        let logical_tree = tree_envelope
            .logical_bytes()
            .map_err(|error| PutError::Storage(error.to_string()))?;
        tree_envelope.logical_digest = Sha256::digest(&logical_tree).into();
        let tree_bytes = tree_envelope
            .to_bytes()
            .map_err(|error| PutError::Storage(error.to_string()))?;
        let tree_digest = hex::encode(tree_envelope.logical_digest);

        object_ids.insert(0, tree_id.clone());
        physical.insert(
            0,
            (
                tree_id.clone(),
                ObjectKind::SourceTree,
                hex::encode(Sha256::digest(&tree_bytes)),
                None,
                tree_bytes.clone(),
                "application/vnd.librepaper.source-tree",
            ),
        );
        let request_digest = digest_of(
            &serde_json::json!({
                "version": 2,
                "effect": "source_publish",
                "slug": v.slug.clone(),
                "format": format.clone(),
                "main": main.clone(),
                "files": file_inputs
                    .iter()
                    .map(|(path, bytes)| {
                        serde_json::json!({
                            "path": path,
                            "digest": hex::encode(Sha256::digest(bytes)),
                            "length": bytes.len(),
                        })
                    })
                    .collect::<Vec<_>>(),
            })
            .to_string(),
        );
        let request_key = crate::util::new_request_key();
        let authority = serde_json::json!({
            "account_id": if !actor.account_id.is_empty() {
                actor.account_id.clone()
            } else {
                anonymous_owner_id.clone().unwrap_or_default()
            },
            "session_generation": actor.session_generation.clone(),
            "link_hash": actor.link_hash.clone(),
            "policy_editor": actor.policy_editor,
            "automation": actor.automation,
            "unowned_publisher": actor.unowned_publisher,
        });
        let plan_json = serde_json::json!({
            "version": 2,
            "effect": "source_publish",
            "title": title.clone(),
            "source_format": format.clone(),
            "main": main.clone(),
            "closure_digest": source_closure_digest(&object_ids),
            "tree_digest": tree_digest.clone(),
            "tree_physical_digest": hex::encode(Sha256::digest(&tree_bytes)),
            "authority": authority,
        })
        .to_string();
        let document_input = crate::storage::catalog::NewDocument {
            slug: v.slug.clone(),
            storage_id: storage_id.clone(),
            title: title.clone(),
            sha: String::new(),
            created_at: created_at.clone(),
            published_at: String::new(),
            updated_at: now.clone(),
            example: existing
                .as_ref()
                .map(|document| document.example)
                .unwrap_or(actor.account_id == "system:examples"),
            owner_key: if owner_id.is_none() {
                actor.owner_key.clone()
            } else {
                String::new()
            },
            owner_id,
            status: "creating".into(),
            size: 0,
            counted_size: 0,
            maintenance_reserved: 0,
            last_auto_checkpoint_at: now_ms,
            source_format: format.clone(),
            main: main.clone(),
        };
        let limits = self.config.storage;
        let document_id = DocumentId::new(storage_id.clone())
            .map_err(|error| PutError::Storage(error.to_string()))?;
        let observed_generation = if existing.is_some() {
            let generation_document = document_id.clone();
            Some(
                catalog
                    .execute_catalog(STORE_JOB_BYTES, move |catalog| {
                        catalog.v2_document_source_generation(&generation_document)
                    })
                    .await
                    .map_err(|error| PutError::Storage(error.to_string()))?,
            )
        } else {
            None
        };
        let operation_id = crate::storage::catalog::OperationId::new(random_storage_id())
            .map_err(|error| PutError::Storage(error.to_string()))?;
        let operation_expires = now_ms
            .checked_add(3_600_000)
            .ok_or_else(|| PutError::Storage("operation expiry overflow".into()))?;
        let operation_deadline = UnixMillis::new(operation_expires)
            .map_err(|error| PutError::Storage(error.to_string()))?;
        let admission_now =
            UnixMillis::new(now_ms).map_err(|error| PutError::Storage(error.to_string()))?;
        let lease_expires = UnixMillis::new(
            now_ms
                .checked_add(120_000)
                .ok_or_else(|| PutError::Storage("lease expiry overflow".into()))?,
        )
        .map_err(|error| PutError::Storage(error.to_string()))?;
        let allocations = physical
            .iter()
            .map(|(id, kind, digest, logical_digest, bytes, _)| {
                Ok(V2ObjectAllocation {
                    document_id: document_id.clone(),
                    id: id.clone(),
                    storage_key: format!("v2/documents/{}/objects/{}", document_id, id),
                    kind: *kind,
                    digest: digest.clone(),
                    logical_digest: logical_digest.clone(),
                    encoding_version: 1,
                    reserved_bytes: i64::try_from(bytes.len())
                        .map_err(|_| PutError::Storage("source object is too large".into()))?,
                    operation_id: operation_id.clone(),
                    now: UnixMillis::new(now_ms)
                        .map_err(|error| PutError::Storage(error.to_string()))?,
                })
            })
            .collect::<Result<Vec<_>, PutError>>()?;
        let admission = V2AdmissionLimits {
            owner_bytes: limits.per_owner,
            deployment_bytes: limits.total,
            owner_documents: limits.documents_per_owner as i64,
        };
        let holder = format!("source:{}", operation_id.as_str());
        let operation = catalog
            .execute_catalog(STORE_JOB_BYTES + plan_json.len() + physical.len() * 128, {
                let operation_id = operation_id.clone();
                let operation = V2OperationInput {
                    scope: crate::storage::catalog::OperationScope::Document(document_id.clone()),
                    actor_key: if !actor.account_id.is_empty() {
                        format!("account:{}", actor.account_id)
                    } else if !actor.link_hash.is_empty() {
                        format!("link:{}", actor.link_hash)
                    } else if let Some(anonymous_owner_id) = anonymous_owner_id.clone() {
                        format!("account:{anonymous_owner_id}")
                    } else {
                        return Err(PutError::Authorization {
                            status: 401,
                            message: "publication actor has no accountable identity",
                        });
                    },
                    request_key,
                    kind: OperationKind::SourcePublish,
                    request_digest,
                    plan_json,
                    expected_document_generation: observed_generation,
                    conversation_id: None,
                    execution_epoch: None,
                    work_expires_at: Some(operation_deadline),
                };
                let document = document_input.clone();
                let allocations = allocations.clone();
                let holder = holder.clone();
                move |catalog| {
                    catalog.admit_v2_source(V2SourceAdmissionInput {
                        document,
                        owner_credential: (!actor.owner_key.is_empty())
                            .then(|| actor.owner_key.clone()),
                        create_document: existing.is_none(),
                        operation_id,
                        operation,
                        allocations,
                        lease_holder: holder,
                        lease_expires_at: lease_expires,
                        limits: admission,
                        now: admission_now,
                    })
                }
            })
            .await
            .map_err(crate::storage::catalog::CatalogError::from)
            .map_err(source_put_error)?;
        let (operation, writer_generation) = operation;
        let writer = crate::storage::v2_catalog::V2ObjectWriter::new(
            Arc::clone(catalog),
            Arc::clone(&self.blobs),
        );
        let mut last_heartbeat = 0i64;
        for (object_id, _, _, _, bytes, content_type) in &physical {
            let renewal_now = crate::util::now_millis();
            if last_heartbeat == 0 || renewal_now.saturating_sub(last_heartbeat) >= 30_000 {
                let renewal_expiry = UnixMillis::new(
                    renewal_now
                        .checked_add(120_000)
                        .ok_or_else(|| PutError::Storage("lease renewal overflow".into()))?,
                )
                .map_err(|error| PutError::Storage(error.to_string()))?;
                catalog
                    .execute_catalog(STORE_JOB_BYTES, {
                        let document_id = document_id.clone();
                        let object_ids = object_ids.clone();
                        let holder = holder.clone();
                        let operation_id = operation.id.clone();
                        let writer_generation = writer_generation.clone();
                        move |catalog| {
                            catalog.renew_v2_lease_set(
                                &document_id,
                                &object_ids,
                                &holder,
                                &operation_id,
                                &writer_generation,
                                renewal_expiry,
                                UnixMillis::new(renewal_now)?,
                            )
                        }
                    })
                    .await
                    .map_err(|error| PutError::Storage(error.to_string()))?;
                last_heartbeat = renewal_now;
            }
            let blob_id = BlobObjectId::parse(object_id.as_str().to_owned())
                .map_err(|error| PutError::Storage(error.to_string()))?;
            writer
                .write_allocated(document_id.as_str(), blob_id, bytes.clone(), content_type)
                .await
                .map_err(PutError::Storage)?;
        }
        let checkpoint_id = CheckpointId::new(random_storage_id())
            .map_err(|error| PutError::Storage(error.to_string()))?;
        let checkpoint_now = UnixMillis::new(crate::util::now_millis())
            .map_err(|error| PutError::Storage(error.to_string()))?;
        let checkpoint = CheckpointCommit {
            document_id: document_id.clone(),
            id: checkpoint_id.clone(),
            tree_object_id: tree_id,
            tree_digest,
            parent_id: None,
            author_account_id: (!actor.account_id.is_empty()).then(|| actor.account_id.clone()),
            author_label: account
                .as_ref()
                .map(|account| account.name.clone())
                .unwrap_or_else(|| "Anonymous".into()),
            reason: "initial source".into(),
            source_format,
            logical_bytes,
            label: None,
            journal_epoch: 0,
            journal_sequence: 0,
            metadata_json: serde_json::json!({
                "version": 2,
                "sourceDigest": hex::encode(main_file_digest),
                "fileCount": file_inputs.len(),
            })
            .to_string(),
            eligible_after: None,
            object_ids,
            make_current: true,
            now: checkpoint_now,
        };
        let recipe_objects = physical
            .iter()
            .filter(|(_, kind, _, _, _, _)| *kind == ObjectKind::SourceRecipe)
            .map(|(id, _, _, _, bytes, _)| (id.as_str().to_owned(), bytes.clone()))
            .collect::<Vec<_>>();
        let proof_input_bytes = tree_bytes.len().saturating_add(
            recipe_objects
                .iter()
                .map(|(_, bytes)| bytes.len())
                .sum::<usize>(),
        );
        let proof = catalog
            .execute_catalog(STORE_JOB_BYTES.saturating_add(proof_input_bytes), {
                let operation_id = operation.id.clone();
                let checkpoint = checkpoint.clone();
                let tree_bytes = tree_bytes.clone();
                let recipe_objects = recipe_objects.clone();
                move |catalog| {
                    catalog.verify_v2_source_closure_bundle(
                        &operation_id,
                        &checkpoint,
                        &tree_bytes,
                        &recipe_objects,
                    )
                }
            })
            .await
            .map_err(|error| PutError::Storage(error.to_string()))?;
        catalog
            .execute_catalog(STORE_JOB_BYTES, {
                let proof = proof.clone();
                let checkpoint = checkpoint.clone();
                move |catalog| {
                    catalog.commit_v2_checkpoint_verified(
                        &proof,
                        &checkpoint,
                        "{\"version\":2,\"effect\":\"source_publish\"}",
                    )
                }
            })
            .await
            .map_err(|error| PutError::Storage(error.to_string()))?;
        for object_id in &checkpoint.object_ids {
            let _ = catalog
                .execute_catalog(STORE_JOB_BYTES, {
                    let document_id = document_id.clone();
                    let object_id = object_id.clone();
                    let holder = holder.clone();
                    move |catalog| catalog.release_v2_lease(&document_id, &object_id, &holder)
                })
                .await;
        }
        let document = document_row(catalog, &v.slug)
            .await
            .map_err(|error| PutError::Storage(error.to_string()))?
            .ok_or_else(|| PutError::Storage("catalogue document disappeared".into()))?;
        let entry = IndexEntry::from_catalog(document);
        self.state
            .lock()
            .await
            .entries
            .insert(v.slug, entry.clone());
        Ok(entry)
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
        let entry = load_catalog_entry(catalog, slug, true)
            .await
            .map_err(|err| ModifyError::Storage(err.to_string()))?
            .ok_or(ModifyError::NotFound)?;
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
                    let mut cursor: Option<(String, String)> = None;
                    loop {
                        let page = catalog.documents_page(
                            cursor
                                .as_ref()
                                .map(|(updated, slug)| (updated.as_str(), slug.as_str())),
                            CATALOG_PAGE_SIZE,
                        )?;
                        if page.is_empty() {
                            break;
                        }
                        for document in &page {
                            if document.owner_id.is_none() && document.owner_key == visitor {
                                catalog.transfer_ownership(
                                    &document.slug,
                                    &new_owner,
                                    per_owner,
                                )?;
                                moved += 1;
                            }
                        }
                        cursor = page
                            .last()
                            .map(|document| (document.updated_at.clone(), document.slug.clone()));
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
                    format!("documents/{slug}"),
                ]
                .into_iter()
                .map(|key| (key, 0)),
            );
            keys.sort_by(|left, right| left.0.cmp(&right.0));
            keys.dedup_by(|left, right| left.0 == right.0);
            // These are compatibility sidecars, outside the canonical v2
            // object graph. Their names come only from exact document-owned
            // prefixes and fixed keys, so they can be removed directly after
            // the document has entered the deleting state. Canonical v2
            // objects are deliberately absent from this list and are handled
            // by the object-row collector below.
            let mut removed = 0;
            for (key, _) in &keys {
                let outcome = self
                    .blobs
                    .delete_each(std::slice::from_ref(key))
                    .await
                    .map_err(|err| format!("could not reclaim document sidecars: {err}"))?
                    .into_iter()
                    .next()
                    .ok_or_else(|| "storage returned no sidecar deletion result".to_string())?;
                if outcome.confirmed() {
                    removed += 1;
                } else {
                    return Err(format!(
                        "could not confirm document sidecar deletion: {}",
                        outcome.why()
                    ));
                }
            }
            // Canonical v2 journal bases and segments are catalogue objects.
            // They are rooted by the document row until begin_delete clears
            // those roots, then the bounded v2 collector verifies and removes
            // each physical object before settling its charge. The old
            // deployment-wide journal manifest retirement path must never be
            // invoked for a v2 document.
            let gc_worker = crate::storage::maintenance::DeletionWorker::new(
                catalog.clone(),
                self.blobs.clone(),
                crate::storage::maintenance::DeletionLimits::default(),
            )
                .map_err(|err| err.to_string())?;
            gc_worker
                .run_v2_once(crate::util::now_millis())
                .await
                .map_err(|err| format!("could not run v2 document cleanup: {err}"))?;
            {
                let slug = slug.to_string();
                match catalog
                    .execute_catalog(STORE_JOB_BYTES + slug.len(), move |catalog| {
                        catalog.finish_delete(&slug)
                    })
                    .await
                {
                    Ok(()) => {}
                    Err(crate::storage::catalog::CatalogExecError::Catalog(
                        crate::storage::catalog::CatalogError::Conflict(_),
                    )) => {
                        // The v2 grace period or another prepared/leased row
                        // can legitimately keep the document deleting. The
                        // durable lifecycle and next maintenance pass will
                        // finish it after physical confirmation.
                    }
                    Err(err) => return Err(err.to_string()),
                }
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

fn source_closure_digest(ids: &[ObjectId]) -> String {
    let mut digest = Sha256::new();
    for id in ids {
        digest.update(id.as_str().as_bytes());
        digest.update([0]);
    }
    hex::encode(digest.finalize())
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

/// Every active document as a compatibility entry, read in bounded catalogue
/// pages. The compatibility caller still receives the full set, but no query
/// or catalogue job retains an unbounded result or monopolises the connection
/// for the entire deployment.
async fn catalog_entries(
    catalog: &Arc<Catalog>,
) -> Result<HashMap<String, IndexEntry>, CatalogError> {
    let mut entries = HashMap::new();
    let mut cursor: Option<(String, String)> = None;
    loop {
        let page_cursor = cursor.clone();
        let page = catalog
            .execute_catalog(STORE_JOB_BYTES, move |catalog| {
                let documents = catalog.documents_page(
                    page_cursor
                        .as_ref()
                        .map(|(updated, slug)| (updated.as_str(), slug.as_str())),
                    CATALOG_PAGE_SIZE,
                )?;
                let mut page_entries = Vec::with_capacity(documents.len());
                for document in &documents {
                    if let Some(entry) = load_catalog_entry_sql(catalog, &document.slug, true)? {
                        page_entries.push((document.slug.clone(), entry));
                    }
                }
                let next = documents
                    .last()
                    .map(|document| (document.updated_at.clone(), document.slug.clone()));
                Ok((page_entries, next))
            })
            .await
            .map_err(CatalogError::from)?;
        for (slug, entry) in page.0 {
            entries.insert(slug, entry);
        }
        let Some(next) = page.1 else {
            break;
        };
        cursor = Some(next);
    }
    Ok(entries)
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
    if document.status != "active" {
        return Ok(None);
    }
    let mut entry = IndexEntry::from_catalog(document);
    if !entry.publisher_id.is_empty() {
        if let Some(owner) = catalog.account(&entry.publisher_id)? {
            entry.publisher = owner.handle;
            entry.publisher_name = owner.name;
        }
    }
    for link in catalog
        .links(slug)?
        .into_iter()
        .take(MAX_LINKS_PER_RESULT as usize)
    {
        let key = if decrypt_links {
            catalog.open_link_key(&entry.storage_id, &link.role, &link.hash, &link.sealed)?
        } else {
            String::new()
        };
        entry.links.push(LinkGrant {
            role: link.role,
            hash: link.hash,
            key,
            label: link.label,
            budget: link.budget,
            since: link.since,
            until: link.until,
        });
    }
    for guest in catalog.guests(slug, MAX_GUESTS_PER_RESULT as u32)? {
        if let Some(account) = catalog.account(&guest.account_id)? {
            entry.guests.push(Guest {
                id: guest.account_id,
                name: account.name,
                since: guest.since,
                link: guest.link_hash,
            });
        }
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
    // `entry` is the compare-and-swap snapshot loaded before the caller's
    // closure.  Keep that revision on the mutation input: the catalogue
    // transaction must reject a concurrent checkpoint or access write rather
    // than replacing it with this stale compatibility view.
    document.updated_at = entry.updated_at.clone();

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
            &[],
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
