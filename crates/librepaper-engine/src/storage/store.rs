//! Catalogue-backed document metadata, source reads, and mutations.
//!
//! The log is the document: there is no
//! archive here any more, so "creating" a document, forking it into a new
//! slug and publishing a whole directory over an existing "open" one are one
//! act -- write a file set into the document at head -- expressed as the
//! `ReplaceProject` semantic command. What differs between the three is
//! only what the document held before the command ran: nothing, for a fresh
//! slug; another document's files, for a fork, which reads them with
//! `project_files` first; or whatever an "open" document already had, which
//! the command's own preconditionless removal-and-insert makes a true
//! replacement rather than a merge.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use librepaper_base::digest::Digest;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::log::{Command, CommandError, Evidence, Head, PreparedSource, Registry};
use crate::storage::blob::BlobStore;
use crate::storage::postgres::{
    built_in_refusal, Authority, Error as CatalogError, NewAsset, NewLabel,
    PostgresCatalog as Catalog, TagRecord, FAVORITES_COLOUR, FAVORITES_NAME,
};
use librepaper_base::config::Configuration;
use librepaper_base::paths::{admit, Kind};
use librepaper_base::role::{Ceiling, Role};
use librepaper_base::util::parse_timestamp;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct IndexEntry {
    pub slug: String,
    /// Immutable catalogue identity, separate from the mutable public slug.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub storage_id: String,
    pub title: String,
    pub sha: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(skip)]
    pub unowned: bool,
    /// The lowercased GitHub login that uploaded this version, and the only
    /// account that may replace or delete it. Empty on anything published
    /// before ownership was recorded or on a deployment where publishing
    /// needs no account at all.
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
    /// The links that carry a role, and the agent tokens beside them. A
    /// document may hold several links of one role. Each row is addressed by
    /// `LinkGrant::id`.
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
    /// Explicit account grants which are not derived from a share-link visit.
    #[serde(skip)]
    pub account_grants: std::collections::HashMap<String, Role>,
    /// The live share-link hash resolved from the request.
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
/// link at all. `key` exists only on the response which mints the link: the
/// catalogue stores its digest and cannot recover the credential later.
/// `label` is the owner's memo.
/// `budget` is the number of comment actions this link may make in one clock
/// hour; absent uses the deployment's ordinary numeric comment limit.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LinkGrant {
    pub hash: String,
    pub role: String,
    /// The key itself, sealed with the deployment's session key, or empty for
    /// a link minted before the catalogue kept one. The digest above is what
    /// admits a caller; this is only what lets the owner be told the URL
    /// again. See `server::sharing::seal_link_key`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sealed: Vec<u8>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<i64>,
    #[serde(default)]
    pub since: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub until: String,
    /// Whether this row admits a browser (`Link`) or an agent bearer (`Agent`).
    /// An agent row is invisible to every lookup a link key reaches.
    #[serde(default, skip_serializing_if = "LinkKind::is_link")]
    pub kind: LinkKind,
    /// For an agent row, the owner's session generation at mint. Empty for a
    /// link.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner_generation: String,
}

impl std::fmt::Debug for LinkGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkGrant")
            .field("hash", &self.hash)
            .field("role", &self.role)
            .field("sealed", &"[redacted]")
            .field("label", &self.label)
            .field("budget", &self.budget)
            .field("since", &self.since)
            .field("until", &self.until)
            .field("kind", &self.kind)
            .field("owner_generation", &self.owner_generation)
            .finish()
    }
}

/// What a share row admits.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LinkKind {
    #[default]
    Link,
    Agent,
}

impl LinkKind {
    pub fn is_link(&self) -> bool {
        *self == LinkKind::Link
    }

    /// The value the `kind` column holds.
    pub fn as_str(self) -> &'static str {
        match self {
            LinkKind::Link => "link",
            LinkKind::Agent => "agent",
        }
    }

    /// The kind a stored column value names. Anything else reads as a link,
    /// which the column's check constraint keeps from happening.
    pub fn from_column(value: &str) -> Self {
        if value == "agent" {
            LinkKind::Agent
        } else {
            LinkKind::Link
        }
    }
}

impl LinkGrant {
    /// The id the owner addresses this row by: the first 16 hex characters of
    /// its digest. Replacing the credential gives the row a new id.
    pub fn id(&self) -> &str {
        &self.hash[..self.hash.len().min(16)]
    }

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
        self.role.parse().unwrap_or(Role::Reader)
    }
}

impl IndexEntry {
    /// The owner's display name, with the account handle as a fallback.
    pub fn owner_name(&self) -> &str {
        if self.publisher_name.is_empty() {
            &self.publisher
        } else {
            &self.publisher_name
        }
    }

    /// Ownership compares the exact immutable account identity from the
    /// catalogue, and nothing else. It used to take an owner key beside it,
    /// ignored since the catalogue became authoritative: a parameter every
    /// caller filled in and nothing read.
    pub fn owned_by(&self, caller_id: &str) -> bool {
        !self.publisher_id.is_empty() && !caller_id.is_empty() && caller_id == self.publisher_id
    }

    /// The highest role a caller holds on this document. One function, asked
    /// by every route, so that rights are decided in one place rather than in
    /// each handler that happens to need them: `owned_by` answers whether
    /// somebody is the owner, and this answers what they may do, which is the
    /// question the routes were really asking.
    ///
    /// A caller is named by their account id; `link_hash` is the digest of
    /// the key their request carried,
    /// or "" for none. `ceiling` is the deployment's own switches, which a
    /// document may be stricter than and never wider: a grant the switch does
    /// not allow this caller is still recorded, and simply is not honoured.
    pub fn role_of(&self, caller_id: &str, link_hash: &str, ceiling: Ceiling, now: i64) -> Role {
        if self.owned_by(caller_id) {
            return Role::Owner;
        }
        let mut role = Role::Reader;
        if let Some(granted) = self.account_grants.get(caller_id).copied() {
            role = role.max(ceiling.clamp(granted));
        }
        // A link asks the deployment ceiling because it is a caller with no
        // name: an
        // editor link edits under exactly the switch that lets an anonymous
        // caller edit at all, and a reader link never has to ask, since
        // reading is the rung everybody who reaches the document already
        // holds.
        if let Some(by_link) = self.link_role(link_hash, now) {
            role = role.max(ceiling.clamp(by_link));
        }
        // A read link is read-only: the switch is a ceiling on what a link may
        // carry, not a grant to whoever reaches the document.
        role
    }

    /// The role a link carries, when the document knows its hash and it has
    /// not expired. Revoking is deleting the row; an expired link answers the
    /// same way a revoked one does, as no link at all.
    pub fn link_role(&self, link_hash: &str, now: i64) -> Option<Role> {
        self.live_link(link_hash, now).map(LinkGrant::granted)
    }

    /// The live browser link matching a presented digest. Keeping this lookup
    /// beside `link_role` makes link metadata such as its rate budget come from
    /// the same row that grants access. Agent rows never match.
    pub fn live_link(&self, link_hash: &str, now: i64) -> Option<&LinkGrant> {
        if link_hash.is_empty() {
            return None;
        }
        self.links
            .iter()
            .find(|link| link.kind == LinkKind::Link && link.hash == link_hash && link.live_at(now))
    }

    /// The live agent token row matching a presented digest.
    pub fn live_agent(&self, token_hash: &str, now: i64) -> Option<&LinkGrant> {
        if token_hash.is_empty() {
            return None;
        }
        self.links.iter().find(|link| {
            link.kind == LinkKind::Agent && link.hash == token_hash && link.live_at(now)
        })
    }

    /// The row with this id, live or not and of either kind. The dead one
    /// still matters because it lets the dialog report that it expired.
    pub fn link_by_id(&self, id: &str) -> Option<&LinkGrant> {
        if id.is_empty() {
            return None;
        }
        self.links.iter().find(|link| link.id() == id)
    }

    /// Adds a row. Creating never replaces another row, so two links of one
    /// role live side by side.
    pub fn add_link(&mut self, link: LinkGrant) {
        self.links.push(link);
    }

    /// Swaps the row with this id for `link`, keeping its place in the list.
    /// The old key stops meaning anything the moment this returns, because
    /// its hash is no longer on the document. Returns whether a row matched.
    pub fn replace_link(&mut self, id: &str, link: LinkGrant) -> bool {
        match self.links.iter_mut().find(|existing| existing.id() == id) {
            Some(slot) => {
                *slot = link;
                true
            }
            None => false,
        }
    }

    /// Removes the row with this id, live or expired. Returns whether a row
    /// actually went so a revoke that matched nothing can say so.
    pub fn drop_link(&mut self, id: &str) -> bool {
        let before = self.links.len();
        self.links.retain(|link| link.id() != id);
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
            .filter(|link| link.kind == LinkKind::Link && link.live_at(now))
            .map(|link| link.hash.clone())
            .collect();
        self.guests.retain(|guest| live.contains(&guest.link));
    }

    /// Whether a caller is on this document at all: its owner or the holder of
    /// a live link.
    pub fn names(&self, caller_id: &str, link_hash: &str, now: i64) -> bool {
        self.owned_by(caller_id)
            || self.account_grants.contains_key(caller_id)
            || self.link_role(link_hash, now).is_some()
    }

    /// Whether a caller may read this document at all: its owner and whoever
    /// holds a live link, since a read link
    /// is the least a link carries. The URL alone opens nothing for anybody
    /// else -- a link is the whole of sharing, and the bare slug is not one.
    pub fn readable_by(&self, caller_id: &str, link_hash: &str, now: i64) -> bool {
        self.unowned || self.names(caller_id, link_hash, now)
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

pub struct Store {
    /// Where the bytes are. The store does not care what holds them.
    pub blobs: Arc<dyn BlobStore>,
    pub config: Arc<Configuration>,
    /// The authoritative catalogue.
    pub catalog: Arc<Catalog>,
    /// Where a document's log lives while it is open. A whole-directory
    /// write is a semantic command, and a semantic command runs through a
    /// document's sequencer (§7), so producing one needs this even outside a
    /// live room.
    pub registry: Arc<Registry>,
}

/// A document as it is created. There is one version of it from here on, and
/// what is stored is its source: nothing derived is kept, because every
/// browser that shows the document renders it. So there is no HTML here, and
/// no digest of any.
#[derive(Clone, Debug, Default)]
pub struct DocumentInput {
    pub slug: String,
    pub title: String,
    pub source_format: String,
}

/// What `put` returns when a storage rule refuses an upload: the HTTP status
/// and message the rule names, so the handler answers with exactly what the
/// rule decided.
#[derive(Debug, thiserror::Error)]
pub enum PutError {
    #[error("{message}")]
    Quota { status: u16, message: &'static str },
    #[error("{message}")]
    Authorization { status: u16, message: &'static str },
    /// The upload itself was refused; the string is a sentence for the uploader.
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
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

/// The public id of the built-in Favorites tag. Every other tag is named by
/// its UUID.
pub const FAVORITES_TAG: &str = "favorites";

/// A tag as the API shows it.
#[derive(Clone, Debug, Serialize)]
pub struct TagView {
    pub id: String,
    pub name: String,
    pub colour: String,
    pub builtin: bool,
}

impl TagView {
    fn from_record(record: TagRecord) -> Self {
        if record.builtin {
            return Self::favorites();
        }
        Self {
            id: record.id.to_string(),
            name: record.name,
            colour: record.colour,
            builtin: false,
        }
    }

    fn favorites() -> Self {
        Self {
            id: FAVORITES_TAG.into(),
            name: FAVORITES_NAME.into(),
            colour: FAVORITES_COLOUR.into(),
            builtin: true,
        }
    }
}

fn source_put_error(error: CatalogError) -> PutError {
    match error {
        CatalogError::Conflict(message) if message.contains("quota") => PutError::Quota {
            status: 507,
            message: "your storage quota is used up; delete a document first",
        },
        CatalogError::NotFound => PutError::Authorization {
            status: 404,
            message: "not found",
        },
        CatalogError::Conflict(message) => PutError::Authorization {
            status: 409,
            message: if message
                == "A project with this name already exists. Choose a different name."
            {
                "A project with this name already exists. Choose a different name."
            } else {
                "the document changed; reload it before retrying"
            },
        },
        other => PutError::Storage(other.to_string()),
    }
}

/// What a failed `ReplaceProject` becomes for a caller that only knows about
/// an HTTP upload. A conflict here means the sequencer's own precondition,
/// evaluated at head, found the buffer or the flush had moved under it --
/// which for a plain overwrite is exactly what "reload it and try again"
/// means, so the two conflict variants collapse into the one message the
/// old archive-conflict path already used.
fn command_put_error(error: CommandError) -> PutError {
    match error {
        CommandError::Conflict(_) | CommandError::StaleSelection { .. } => {
            PutError::Authorization {
                status: 409,
                message: "the document changed; reload it before retrying",
            }
        }
        CommandError::Storage(error) => source_put_error(error),
        CommandError::Sequencer(error) => PutError::Storage(error.to_string()),
    }
}

/// A command's request id, cast to the number a batch's `client_seq` column
/// holds (§7.3). Nothing compares this to another peer's own sequence, so the
/// exact mapping does not matter -- only that it is stable across a retry
/// that reuses the same request id.
fn client_seq_of(id: Uuid) -> i64 {
    i64::from_be_bytes(id.as_bytes()[..8].try_into().expect("a uuid is 16 bytes"))
}

/// Writes (or replaces) a document's whole file set as one semantic command.
/// Preparing the batch removes every existing text and asset not named by
/// the new set and then writes the new set, so the result is a true
/// replacement rather than a merge -- and for a document that held nothing
/// yet, removing nothing and writing everything is exactly a creation.
struct ReplaceProject {
    request_id: Uuid,
    template_operation_id: Option<Uuid>,
    document_id: Uuid,
    catalog: Arc<Catalog>,
    texts: Vec<(String, String)>,
    assets: Vec<(String, String)>,
    staged_assets: Vec<NewAsset>,
    author_account_id: Option<Uuid>,
    author_label: String,
}

impl Command for ReplaceProject {
    type Output = ();

    fn name(&self) -> &'static str {
        "whole-project replacement"
    }

    // §7.2: a retry with the same `request_id` returns the label the first
    // attempt's transaction wrote rather than replacing the project twice.
    fn replay(&mut self) -> BoxFuture<'_, std::result::Result<Option<Self::Output>, CommandError>> {
        Box::pin(async move {
            self.catalog
                .label_by_request(self.document_id, self.request_id)
                .await
                .map(|found| found.map(|_| ()))
                .map_err(CommandError::from)
        })
    }

    fn evaluate(
        &mut self,
        head: &Head<'_>,
    ) -> std::result::Result<Option<PreparedSource>, CommandError> {
        let texts = &self.texts;
        let assets = &self.assets;
        let client_seq = client_seq_of(self.request_id);
        head.prepare(client_seq, move |draft| {
            let wanted_texts: std::collections::HashSet<&str> =
                texts.iter().map(|(path, _)| path.as_str()).collect();
            let wanted_assets: std::collections::HashSet<&str> =
                assets.iter().map(|(path, _)| path.as_str()).collect();
            for path in librepaper_document::document::session::text_ids_of(draft).into_keys() {
                if !wanted_texts.contains(path.as_str()) {
                    librepaper_document::document::session::remove_path(draft, &path);
                }
            }
            for path in librepaper_document::document::session::assets_of(draft).into_keys() {
                if !wanted_assets.contains(path.as_str()) {
                    librepaper_document::document::session::remove_asset(draft, &path);
                }
            }
            for (path, body) in texts {
                librepaper_document::document::session::put_text(draft, path, body);
            }
            for (path, digest) in assets {
                librepaper_document::document::session::put_asset(draft, path, digest);
            }
            Ok(())
        })
        .map_err(CommandError::Conflict)
    }

    fn transact<'a>(
        &'a mut self,
        tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        evidence: &'a Evidence,
    ) -> BoxFuture<'a, std::result::Result<Self::Output, CommandError>> {
        Box::pin(async move {
            // A no-op replacement (the new set exactly matches the head)
            // produces no batch, so there is nothing after `prepare` beyond
            // what was already true; the label still records that state,
            // named by the state itself rather than by a change that did not
            // happen.
            let frontier = evidence
                .after_frontier
                .clone()
                .unwrap_or_else(|| evidence.before_frontier.clone());
            let digest_hex = evidence
                .after_digest
                .as_deref()
                .unwrap_or(&evidence.before_digest);
            let tree_digest = Some(
                *digest_hex
                    .parse::<Digest>()
                    .map_err(|_| CommandError::Conflict("invalid tree digest".into()))?
                    .as_bytes(),
            );
            if !self.staged_assets.is_empty() {
                self.catalog
                    .complete_assets_in_transaction(tx, &self.staged_assets)
                    .await?;
            }
            // Rows reused at staging may have been deleted by a trim since.
            // A retry stages those bytes again because their rows are gone.
            let digests = self
                .assets
                .iter()
                .map(|(_, digest_hex)| {
                    digest_hex
                        .parse::<Digest>()
                        .map(|digest| digest.as_bytes().to_vec())
                        .map_err(|_| CommandError::Conflict("invalid asset digest".into()))
                })
                .collect::<std::result::Result<Vec<Vec<u8>>, CommandError>>()?;
            if !digests.is_empty()
                && self
                    .catalog
                    .missing_assets_in_transaction(tx, self.document_id, &digests)
                    .await?
                    > 0
            {
                return Err(CommandError::Conflict(
                    "a figure this project names was removed while it was being saved; try again"
                        .to_string(),
                ));
            }
            self.catalog
                .insert_label(
                    tx,
                    &NewLabel {
                        id: Uuid::new_v4(),
                        document_id: self.document_id,
                        source_sequence: evidence.source_sequence,
                        vector: evidence.vector.clone(),
                        frontier,
                        tree_digest,
                        label: None,
                        reason: self.name().to_string(),
                        request_id: Some(self.request_id),
                        author_account_id: self.author_account_id,
                        author_label: self.author_label.clone(),
                    },
                )
                .await?;
            if let Some(operation_id) = self.template_operation_id {
                self.catalog
                    .complete_template_operation(
                        tx,
                        self.document_id,
                        operation_id,
                        evidence.source_sequence,
                        tree_digest,
                    )
                    .await?;
            }
            Ok(())
        })
    }
}

impl Store {
    /// Assembles a store from collaborators the caller already holds.
    ///
    /// Neither asynchronous nor fallible, and never was: it awaits nothing
    /// and the only value it can produce is `Ok`. Every caller wrote
    /// `.await.unwrap()` or `.await.expect(..)` after it, which reads as
    /// though opening a store could fail.
    pub fn open_with_catalog(
        blobs: Arc<dyn BlobStore>,
        config: Arc<Configuration>,
        catalog: Arc<Catalog>,
        registry: Arc<Registry>,
    ) -> Self {
        Self {
            blobs,
            config,
            catalog,
            registry,
        }
    }

    /// Marks a document for deletion. That mark -- `documents.status =
    /// 'deleting'` plus the `deleted_at` it already carries -- is the whole
    /// of the durable state a purge needs: there is no job queue any more,
    /// and the background worker's own startup scan finds every document in
    /// this state without anything having to enqueue a task for it.
    pub async fn begin_delete(&self, slug: &str) -> Result<Option<String>, String> {
        let catalog = &self.catalog;
        let Some(document) = catalog
            .document_by_slug(slug)
            .await
            .map_err(|e| e.to_string())?
        else {
            return Ok(None);
        };
        catalog
            .mark_document_deleting(document.id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(Some(document.id.to_string()))
    }

    pub async fn get_result(&self, slug: &str) -> Result<Option<IndexEntry>, CatalogError> {
        let catalog = &self.catalog;
        let Some(document) = catalog.document_by_slug(slug).await? else {
            return Ok(None);
        };
        // A trashed or purging document keeps its catalogue row until the
        // sweep removes it, but no route may resolve it in the meantime.
        if document.status != "active" {
            return Ok(None);
        }
        // Hidden projects remain active catalogue rows so restoration is
        // lossless, but no public or authenticated route may resolve them.
        if catalog.project_is_hidden(document.id).await? {
            return Ok(None);
        }
        Ok(Some(entry_from_document(catalog, &document).await?))
    }

    /// Reads a bounded active-document maintenance page, including moderated
    /// projects that users must not see but retention still processes.
    /// Returns the next cursor only when another page may exist.
    pub async fn maintenance_page(
        &self,
        before: Option<(time::OffsetDateTime, Uuid)>,
        limit: u32,
    ) -> Result<(Vec<IndexEntry>, Option<(time::OffsetDateTime, Uuid)>), CatalogError> {
        let catalog = &self.catalog;
        let documents = catalog
            .list_documents_for_maintenance(before, i64::from(limit))
            .await?;
        let next = if documents.len() == limit as usize {
            documents
                .last()
                .map(|document| (document.updated_at, document.id))
        } else {
            None
        };
        let entries = entries_from_documents(catalog, &documents).await?;
        Ok((entries, next))
    }

    pub async fn visible_page_with_options(
        &self,
        account_id: Option<&str>,
        cursor: Option<(&str, &str)>,
        limit: u32,
        include_templates: bool,
    ) -> Result<Vec<IndexEntry>, CatalogError> {
        let catalog = &self.catalog;
        let account_id = account_id
            .map(uuid::Uuid::parse_str)
            .transpose()
            .map_err(|_| CatalogError::Invalid("invalid listing account".into()))?;
        let before = match cursor {
            Some((updated, slug)) => {
                if librepaper_base::util::parse_timestamp(updated).is_none() {
                    return Err(CatalogError::Invalid("invalid listing cursor".into()));
                }
                let document = catalog
                    .document_by_slug(slug)
                    .await?
                    .ok_or_else(|| CatalogError::Invalid("invalid listing cursor".into()))?;
                // A cursor may only name a project the caller could already
                // list, so a private slug cannot be probed through it.
                let entry = entry_from_document(catalog, &document).await?;
                let caller = account_id.map(|id| id.to_string()).unwrap_or_default();
                // The listing also shows projects a signed-in guest was pinned
                // to by a link, so that pin counts as the link they present.
                let pinned = entry
                    .guests
                    .iter()
                    .find(|guest| guest.id == caller)
                    .map(|guest| guest.link.as_str())
                    .unwrap_or_default();
                if !entry.readable_by(&caller, pinned, librepaper_base::util::now_unix()) {
                    return Err(CatalogError::Invalid("invalid listing cursor".into()));
                }
                // The wire timestamp is second-precision. Use the row's exact
                // value so documents updated within the same second are not
                // skipped between pages.
                Some((document.updated_at, document.id))
            }
            None => None,
        };
        let documents = catalog
            .visible_documents(account_id, before, i64::from(limit), include_templates)
            .await?;
        entries_from_documents(catalog, &documents).await
    }

    /// One keyset page of this account's completed custom templates.
    pub async fn templates_page(
        &self,
        account_id: &str,
        cursor: Option<(&str, &str)>,
        limit: u32,
    ) -> Result<(Vec<IndexEntry>, Option<(String, String)>), CatalogError> {
        let catalog = &self.catalog;
        let account_id = uuid::Uuid::parse_str(account_id)
            .map_err(|_| CatalogError::Invalid("invalid listing account".into()))?;
        if !(1..=200).contains(&limit) {
            return Err(CatalogError::Invalid(
                "template page limit must be 1..=200".into(),
            ));
        }
        let before = match cursor {
            Some((updated, id)) => {
                let updated = time::OffsetDateTime::parse(
                    updated,
                    &time::format_description::well_known::Rfc3339,
                )
                .map_err(|_| CatalogError::Invalid("invalid template cursor".into()))?;
                let id = uuid::Uuid::parse_str(id)
                    .map_err(|_| CatalogError::Invalid("invalid template cursor".into()))?;
                Some((updated, id))
            }
            None => None,
        };
        let documents = catalog
            .templates_by_owner(account_id, before, i64::from(limit))
            .await?;
        let next = if documents.len() == limit as usize {
            match documents.last() {
                Some(document) => Some((
                    document
                        .updated_at
                        .format(&time::format_description::well_known::Rfc3339)
                        .map_err(|error| CatalogError::Invalid(error.to_string()))?,
                    document.id.to_string(),
                )),
                None => None,
            }
        } else {
            None
        };
        Ok((entries_from_documents(catalog, &documents).await?, next))
    }

    /// Every document this account has deleted and can still get back, newest
    /// deletion first, each paired with the moment its purge is due.
    pub async fn trashed_page(
        &self,
        account_id: &str,
        limit: u32,
    ) -> Result<Vec<(IndexEntry, Option<String>)>, CatalogError> {
        let catalog = &self.catalog;
        let account_id = uuid::Uuid::parse_str(account_id)
            .map_err(|_| CatalogError::Invalid("invalid listing account".into()))?;
        let documents = catalog
            .trashed_documents(account_id, i64::from(limit))
            .await?;
        let entries = entries_from_documents(catalog, &documents).await?;
        let mut rows = Vec::with_capacity(entries.len());
        for (entry, document) in entries.into_iter().zip(documents.iter()) {
            // `trashed_documents` already returned the deletion timestamp.
            // Derive the fixed purge grace from that row instead of issuing
            // one `deletion_due` query per document.
            let due = document.deleted_at.map(|at| {
                librepaper_base::util::format_unix(
                    (at + crate::storage::maintenance::DELETION_GRACE).unix_timestamp(),
                )
            });
            rows.push((entry, due));
        }
        Ok(rows)
    }

    /// Put a deleted document back. Refuses once its purge has started, which
    /// is the only moment at which the answer can be no for a document that
    /// was in the trash a second ago.
    pub async fn restore(&self, slug: &str, account_id: &str) -> Result<bool, ModifyError> {
        let catalog = &self.catalog;
        let account_id = uuid::Uuid::parse_str(account_id)
            .map_err(|_| ModifyError::Refused("invalid account".into()))?;
        let document = catalog
            .document_by_slug(slug)
            .await
            .map_err(|error| ModifyError::Storage(error.to_string()))?
            .ok_or(ModifyError::NotFound)?;
        // Only the owner has a trash: a document shared with you leaves your
        // listing when its owner deletes it, and getting it back is theirs to
        // do, not yours.
        if document.owner_id != account_id {
            return Err(ModifyError::Refused("not yours to restore".into()));
        }
        catalog
            .restore_document(document.id)
            .await
            .map_err(|error| ModifyError::Storage(error.to_string()))
    }

    /// Bring a deleted document's purge forward to now.
    pub async fn purge_now(
        &self,
        slug: &str,
        account_id: &str,
    ) -> Result<Option<uuid::Uuid>, ModifyError> {
        let catalog = &self.catalog;
        let account_id = uuid::Uuid::parse_str(account_id)
            .map_err(|_| ModifyError::Refused("invalid account".into()))?;
        let document = catalog
            .document_by_slug(slug)
            .await
            .map_err(|error| ModifyError::Storage(error.to_string()))?
            .ok_or(ModifyError::NotFound)?;
        if document.owner_id != account_id {
            return Err(ModifyError::Refused("not yours to delete".into()));
        }
        let changed = catalog
            .hasten_deletion(document.id)
            .await
            .map_err(|error| ModifyError::Storage(error.to_string()))?;
        Ok(changed.then_some(document.id))
    }

    /// Every tag this account holds, Favorites first. Favorites is listed
    /// even before the account has starred anything, because it always exists.
    pub async fn list_tags(&self, account_id: &str) -> Result<Vec<TagView>, CatalogError> {
        let records = self
            .catalog
            .list_tags(parse_account_id(account_id)?)
            .await?;
        let mut tags: Vec<TagView> = records.into_iter().map(TagView::from_record).collect();
        if !tags.iter().any(|tag| tag.builtin) {
            tags.insert(0, TagView::favorites());
        }
        Ok(tags)
    }

    /// Make a tag of this account's own.
    pub async fn create_tag(
        &self,
        account_id: &str,
        name: &str,
        colour: &str,
    ) -> Result<TagView, CatalogError> {
        let record = self
            .catalog
            .create_tag(parse_account_id(account_id)?, name, colour)
            .await?;
        Ok(TagView::from_record(record))
    }

    /// Rename or recolour one of this account's own tags. The built-in tag
    /// refuses either, whatever the request asks for.
    pub async fn update_tag(
        &self,
        account_id: &str,
        tag: &str,
        name: Option<&str>,
        colour: Option<&str>,
    ) -> Result<TagView, CatalogError> {
        let record = self
            .catalog
            .update_tag(
                parse_account_id(account_id)?,
                custom_tag_id(tag)?,
                name,
                colour,
            )
            .await?;
        Ok(TagView::from_record(record))
    }

    /// Delete one of this account's own tags. Projects keep everything else
    /// they hold.
    pub async fn delete_tag(&self, account_id: &str, tag: &str) -> Result<(), CatalogError> {
        self.catalog
            .delete_tag(parse_account_id(account_id)?, custom_tag_id(tag)?)
            .await
    }

    /// Put a tag on a document this account can already see, or take it off.
    /// Favorites is created the first time it is put on anything.
    pub async fn set_document_tag(
        &self,
        slug: &str,
        account_id: &str,
        tag: &str,
        on: bool,
    ) -> Result<(), ModifyError> {
        let (document_id, account_id) = self.mark_target(slug, account_id).await?;
        let tag_id = if tag != FAVORITES_TAG {
            Uuid::parse_str(tag).map_err(|_| ModifyError::NotFound)?
        } else if on {
            self.catalog
                .ensure_favorites(account_id)
                .await
                .map_err(tag_modify_error)?
        } else {
            // An account that never starred has nothing to take off.
            match self
                .catalog
                .favorites_tag(account_id)
                .await
                .map_err(tag_modify_error)?
            {
                Some(id) => id,
                None => return Ok(()),
            }
        };
        self.catalog
            .set_document_tag(account_id, document_id, tag_id, on)
            .await
            .map_err(tag_modify_error)
    }

    /// Hide or restore a shared document in this account's shared-project view.
    pub async fn set_shared_hidden(
        &self,
        slug: &str,
        account_id: &str,
        hidden: bool,
    ) -> Result<bool, ModifyError> {
        let (document_id, account_id) = self.mark_target(slug, account_id).await?;
        self.catalog
            .set_shared_hidden(account_id, document_id, hidden)
            .await
            .map_err(|error| ModifyError::Storage(error.to_string()))
    }

    /// Note that this account has just opened this document.
    pub async fn mark_opened(&self, slug: &str, account_id: &str) -> Result<(), ModifyError> {
        let (document_id, account_id) = self.mark_target(slug, account_id).await?;
        self.catalog
            .mark_opened(account_id, document_id)
            .await
            .map_err(|error| ModifyError::Storage(error.to_string()))
    }

    /// Every file in a project, by path, as it currently stands.
    ///
    /// The log is the document (§1): this reads the head through the
    /// document's own sequencer -- admitting it from the base and the rows if
    /// it is not already resident -- rather than replaying the log a second
    /// way here. A document created but never opened reads the same way,
    /// because creation itself is now a row in the log rather than a version
    /// kept beside it.
    ///
    /// The paths are the projection's, not the raw maps'. Those two disagree
    /// about a path collision, an invalid name and a text that is not one,
    /// and this is a copy of the project: writing it out under the raw names
    /// produces a directory that is not the document anybody reads, and on a
    /// collision writes one of the two files over the other.
    pub async fn project_files(
        &self,
        slug: &str,
    ) -> Result<Option<Vec<(String, Vec<u8>)>>, String> {
        let Some(document) = self
            .catalog
            .document_by_slug(slug)
            .await
            .map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };
        let sequencer = self
            .registry
            .get(document.id, slug)
            .await
            .map_err(|error| error.to_string())?;
        let projected = sequencer
            .projection()
            .await
            .map_err(|error| error.to_string())?;
        let mut files: Vec<(String, Vec<u8>)> = Vec::new();
        let mut named_assets: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        for (path, entry) in &projected.projection.files {
            match projected.texts.get(path) {
                Some(text) => files.push((path.clone(), text.clone().into_bytes())),
                // No text at a projected path is an asset: its bytes are in
                // the store under the digest the entry names.
                None => {
                    named_assets.insert(path.clone(), entry.digest.clone());
                }
            }
        }
        // Figures live in the store under their digest, and the document
        // carries only the name somebody gave them. One whose bytes have gone
        // is skipped rather than failing the copy: a project missing a figure
        // is still worth having.
        let digests: Vec<_> = named_assets.values().cloned().collect();
        if !digests.is_empty() {
            let records = self
                .catalog
                .assets_by_digests(document.id, &digests)
                .await
                .map_err(|error| error.to_string())?;
            let keys: std::collections::HashMap<String, String> = records
                .into_iter()
                .map(|record| (hex::encode(&record.digest), record.storage_key))
                .collect();
            for (path, digest) in named_assets {
                let Some(key) = keys.get(&digest) else {
                    continue;
                };
                if let Ok(bytes) = self.blobs.get(key).await {
                    files.push((path, bytes));
                }
            }
        }
        Ok(Some(files))
    }

    /// The comment and file counts for a page of the listing, by slug.
    ///
    /// This is what replaced a request per project. It asks the catalogue two
    /// aggregate questions about the whole page, rather than opening every
    /// project's room to count what is in it -- which is what the landing
    /// page was doing from the browser, and what doing it from the server
    /// instead would only have moved rather than fixed.
    pub async fn counts_for(
        &self,
        entries: &[IndexEntry],
    ) -> Result<std::collections::HashMap<String, (i64, i64, Option<i32>)>, CatalogError> {
        let (by_id, ids) = slugs_by_id(entries);
        let counts = self.catalog.listing_counts(&ids).await?;
        Ok(counts
            .into_iter()
            .filter_map(|row| {
                let slug = by_id.get(&row.document_id)?.clone();
                Some((slug, (row.comments, row.open, row.file_count)))
            })
            .collect())
    }

    /// The marks this account holds on the documents listed, by slug. The
    /// listing decorates its rows with these; a document the caller has never
    /// touched simply has no entry.
    pub async fn marks_for(
        &self,
        account_id: &str,
        entries: &[IndexEntry],
    ) -> Result<std::collections::HashMap<String, (String, bool)>, CatalogError> {
        let account_id = uuid::Uuid::parse_str(account_id)
            .map_err(|_| CatalogError::Invalid("invalid listing account".into()))?;
        let (by_id, ids) = slugs_by_id(entries);
        let marks = self.catalog.marks_for_documents(account_id, &ids).await?;
        Ok(marks
            .into_iter()
            .filter_map(|mark| {
                let slug = by_id.get(&mark.document_id)?.clone();
                let opened = mark
                    .opened_at
                    .map(|at| librepaper_base::util::format_unix(at.unix_timestamp()))
                    .unwrap_or_default();
                Some((slug, (opened, mark.shared_hidden)))
            })
            .collect())
    }

    /// The tags this account has put on each document listed, by slug. The
    /// built-in tag is named "favorites" and every other tag by its UUID.
    pub async fn tags_for(
        &self,
        account_id: &str,
        entries: &[IndexEntry],
    ) -> Result<std::collections::HashMap<String, Vec<String>>, CatalogError> {
        let account_id = parse_account_id(account_id)?;
        let (by_id, ids) = slugs_by_id(entries);
        let links = self.catalog.tags_for_documents(account_id, &ids).await?;
        let mut by_slug: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for link in links {
            let Some(slug) = by_id.get(&link.document_id) else {
                continue;
            };
            let tag = if link.builtin {
                FAVORITES_TAG.to_string()
            } else {
                link.tag_id.to_string()
            };
            by_slug.entry(slug.clone()).or_default().push(tag);
        }
        Ok(by_slug)
    }

    /// A mark is written against a document the caller can already see, and
    /// against nothing else: resolving the slug here is what stops a mark from
    /// being a way to ask whether a private document exists.
    async fn mark_target(
        &self,
        slug: &str,
        account_id: &str,
    ) -> Result<(uuid::Uuid, uuid::Uuid), ModifyError> {
        let account_id = uuid::Uuid::parse_str(account_id)
            .map_err(|_| ModifyError::Refused("invalid account".into()))?;
        let document = self
            .catalog
            .document_by_slug(slug)
            .await
            .map_err(|error| ModifyError::Storage(error.to_string()))?
            .ok_or(ModifyError::NotFound)?;
        Ok((document.id, account_id))
    }

    /// Writes a whole directory of files into a document as one semantic
    /// command (§7): a brand-new slug, a fork into a new slug, and a
    /// publish over an existing "open" document all call this, and from the
    /// log's point of view they are the same act -- what differs is only
    /// what the document held before the command ran.
    pub async fn put_directory_as_actor(
        &self,
        value: DocumentInput,
        files: Vec<(String, Vec<u8>)>,
        actor: Authority,
    ) -> Result<IndexEntry, PutError> {
        self.put_directory_with_operation(value, files, actor, None)
            .await
    }

    /// Writes a template's first source command against the atomically
    /// reserved target row. Completion is part of `ReplaceProject`'s durable
    /// transaction, so the row stays hidden from the template shelf until
    /// its copied source is committed.
    pub async fn put_template_directory_as_actor(
        &self,
        value: DocumentInput,
        files: Vec<(String, Vec<u8>)>,
        actor: Authority,
        request_id: Uuid,
        target_document_id: Uuid,
    ) -> Result<IndexEntry, PutError> {
        self.put_directory_with_operation(
            value,
            files,
            actor,
            Some((request_id, target_document_id)),
        )
        .await
    }

    async fn put_directory_with_operation(
        &self,
        value: DocumentInput,
        files: Vec<(String, Vec<u8>)>,
        actor: Authority,
        template_operation: Option<(Uuid, Uuid)>,
    ) -> Result<IndexEntry, PutError> {
        let catalog = &self.catalog;
        let account_id = actor.account_id.ok_or(PutError::Authorization {
            status: 401,
            message: "bundle actor account is invalid",
        })?;
        let session_generation = actor.session_generation.ok_or(PutError::Authorization {
            status: 401,
            message: "bundle actor session is invalid",
        })?;
        let (document, created_here) = match catalog
            .document_by_slug(&value.slug)
            .await
            .map_err(source_put_error)?
        {
            Some(document) => {
                if template_operation.is_some_and(|(_, expected_id)| document.id != expected_id) {
                    return Err(PutError::Authorization {
                        status: 409,
                        message: "the template save target changed",
                    });
                }
                (document, false)
            }
            None if template_operation.is_some() => {
                return Err(PutError::Authorization {
                    status: 409,
                    message: "the template save target disappeared",
                });
            }
            None => (
                catalog
                    .create_document(crate::storage::postgres::NewDocument {
                        slug: value.slug.clone(),
                        owner_id: account_id,
                        owner_session_generation: Some(session_generation),
                        ownership_mode: "owned".into(),
                        title: value.title.clone(),
                        source_format: value.source_format.clone(),
                    })
                    .await
                    .map_err(source_put_error)?,
                true,
            ),
        };
        if document.owner_id != account_id && !actor.policy_edit {
            return Err(PutError::Authorization {
                status: 403,
                message: "edit access changed",
            });
        }

        let write_project = async {
            // Texts and assets are sorted and staged by
            // `sort_and_stage_assets`; a project created empty has neither.
            let (mut texts, assets, staged_assets) =
                self.sort_and_stage_assets(document.id, files).await?;
            texts.sort_by(|a, b| a.0.cmp(&b.0));

            // The name, not the id. `actor.account_id` is the catalogue uuid,
            // and a timeline signed with it can only say "Unknown editor".
            let author_label = match catalog.account_display_name(account_id).await {
                name if name.is_empty() => actor.principal_key.clone(),
                name => name,
            };
            let sequencer = self
                .registry
                .get(document.id, &value.slug)
                .await
                .map_err(|error| PutError::Storage(error.to_string()))?;
            let mut command = ReplaceProject {
                request_id: template_operation
                    .map(|(request_id, _)| request_id)
                    .unwrap_or_else(Uuid::new_v4),
                template_operation_id: template_operation.map(|(request_id, _)| request_id),
                document_id: document.id,
                catalog: catalog.clone(),
                texts,
                assets,
                staged_assets,
                author_account_id: Some(account_id),
                author_label,
            };
            sequencer
                .command(&actor, &mut command)
                .await
                .map_err(command_put_error)
        }
        .await;
        if let Err(error) = write_project {
            if created_here {
                match catalog
                    .discard_unwritten_document(document.id, account_id)
                    .await
                {
                    Ok(true) => {
                        self.registry.retire_empty(document.id).await;
                    }
                    Ok(false) => {}
                    Err(cleanup_error) => {
                        return Err(PutError::Storage(format!(
                            "{error}; could not remove the refused new project: {cleanup_error}"
                        )));
                    }
                }
            }
            return Err(error);
        }

        self.get_result(&value.slug)
            .await
            .map_err(source_put_error)?
            .ok_or_else(|| PutError::Storage("document disappeared after commit".into()))
    }

    /// Sorts a directory's non-entrypoint files by the deployment's path
    /// rules and stages figure bytes under immutable blob keys without
    /// cataloguing them. Returns the texts, path-digest pairs ready for
    /// `put_text`/`put_asset`, and asset rows to commit with the authorized
    /// semantic command. Shared by a plain publish and `seed::activity`,
    /// which both start from the same kind of directory and differ only in
    /// how many log rows the result becomes. A path the
    /// rules refuse was already refused before the upload reached here
    /// (`server::documents::preflight_directory`), so it is dropped rather
    /// than failing the whole directory, the same way a figure whose bytes
    /// never resolved is dropped when a project is read back.
    pub async fn sort_and_stage_assets(
        &self,
        document_id: uuid::Uuid,
        files: Vec<(String, Vec<u8>)>,
    ) -> Result<(Vec<(String, String)>, Vec<(String, String)>, Vec<NewAsset>), PutError> {
        let rules = librepaper_document::document::paths::rules(&self.config);
        let mut texts = Vec::new();
        let mut asset_files = Vec::new();
        for (path, bytes) in files {
            match admit(&rules, &path) {
                Ok(Kind::Text) => texts.push((path, String::from_utf8_lossy(&bytes).into_owned())),
                Ok(Kind::Asset) => {
                    if let Err(error) = librepaper_document::document::formats::check_asset(&bytes)
                    {
                        return Err(PutError::Invalid(format!("{path}: {error}")));
                    }
                    asset_files.push(crate::storage::source::ProjectFile {
                        path,
                        bytes,
                        media_type: "application/octet-stream".into(),
                    });
                }
                Err(_) => {}
            }
        }
        let (assets, staged_assets) = if asset_files.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            let (_, staged_assets) = crate::storage::source::SourceStorage::new(
                self.catalog.clone(),
                self.blobs.clone(),
            )
            .stage_assets(document_id, asset_files.iter())
            .await
            .map_err(|error| PutError::Storage(error.to_string()))?;
            let assets = asset_files
                .iter()
                .map(|file| (file.path.clone(), Digest::of(&file.bytes).to_hex()))
                .collect();
            (assets, staged_assets)
        };
        Ok((texts, assets, staged_assets))
    }

    pub async fn remove(&self, slug: &str) -> Result<usize, String> {
        Ok(usize::from(self.begin_delete(slug).await?.is_some()))
    }

    pub async fn rename(&self, slug: &str, title: &str, actor: &Authority) -> Result<(), String> {
        if title.is_empty() {
            return Ok(());
        }
        let catalog = &self.catalog;
        let Some(document) = catalog
            .document_by_slug(slug)
            .await
            .map_err(|e| e.to_string())?
        else {
            return Err("document not found".into());
        };
        let changed = catalog
            .update_document_title(document.id, actor, title)
            .await
            .map_err(|error| match error {
                CatalogError::Conflict(_) => "ownership changed".to_string(),
                CatalogError::NotFound => "document not found".to_string(),
                other => other.to_string(),
            })?;
        if !changed {
            return Err("ownership changed".into());
        }
        Ok(())
    }

    pub async fn pin_link_guest(
        &self,
        slug: &str,
        account_id: &str,
        link_hash: &str,
        role: Role,
    ) -> Result<(), ModifyError> {
        let catalog = &self.catalog;
        let document = catalog
            .document_by_slug(slug)
            .await
            .map_err(|error| ModifyError::Storage(error.to_string()))?
            .ok_or(ModifyError::NotFound)?;
        let account_id = uuid::Uuid::parse_str(account_id)
            .map_err(|_| ModifyError::Refused("invalid guest account".into()))?;
        let hash = *link_hash
            .parse::<Digest>()
            .map_err(|_| ModifyError::Refused("invalid link digest".into()))?
            .as_bytes();
        if role == Role::Owner {
            return Err(ModifyError::Refused("invalid guest role".into()));
        }
        catalog
            .pin_link_guest(document.id, account_id, role, hash)
            .await
            .map_err(|error| ModifyError::Storage(error.to_string()))
    }

    pub async fn modify_as_owner<F>(
        &self,
        slug: &str,
        actor: &Authority,
        change: F,
    ) -> Result<IndexEntry, ModifyError>
    where
        F: Fn(&mut IndexEntry) -> Result<(), String>,
    {
        let catalog = &self.catalog;
        let document = catalog
            .document_by_slug(slug)
            .await
            .map_err(|e| ModifyError::Storage(e.to_string()))?
            .ok_or(ModifyError::NotFound)?;
        if actor.account_id != Some(document.owner_id) {
            return Err(ModifyError::Refused("ownership changed".into()));
        }
        let mut entry = entry_from_document(catalog, &document)
            .await
            .map_err(|e| ModifyError::Storage(e.to_string()))?;
        change(&mut entry).map_err(ModifyError::Refused)?;
        let links = entry
            .links
            .iter()
            .map(|link| {
                let hash = *link
                    .hash
                    .parse::<Digest>()
                    .map_err(|_| ModifyError::Refused("invalid link digest".into()))?
                    .as_bytes();
                let expiry = if link.until.is_empty() {
                    None
                } else {
                    librepaper_base::util::parse_timestamp(&link.until)
                        .and_then(|seconds| time::OffsetDateTime::from_unix_timestamp(seconds).ok())
                };
                Ok((
                    link.role.clone(),
                    hash,
                    link.sealed.clone(),
                    link.label.clone(),
                    expiry,
                    link.budget,
                    link.kind.as_str().to_string(),
                    link.owner_generation.clone(),
                ))
            })
            .collect::<Result<Vec<_>, ModifyError>>()?;
        let live_hashes = entry
            .links
            .iter()
            .filter(|link| link.live_at(librepaper_base::util::now_unix()))
            .map(|link| {
                link.hash
                    .parse::<Digest>()
                    .map(|digest| digest.as_bytes().to_vec())
                    .map_err(|_| ModifyError::Refused("invalid link digest".into()))
            })
            .collect::<Result<Vec<_>, ModifyError>>()?;
        catalog
            .save_document_sharing(document.id, actor, &links, &live_hashes)
            .await
            .map_err(|error| match error {
                CatalogError::Conflict(_) => ModifyError::Refused("ownership changed".into()),
                CatalogError::NotFound => ModifyError::NotFound,
                other => ModifyError::Storage(other.to_string()),
            })?;
        self.get_result(slug)
            .await
            .map_err(|e| ModifyError::Storage(e.to_string()))?
            .ok_or(ModifyError::NotFound)
    }
}

#[cfg(test)]
#[path = "store_import_tests.rs"]
mod import_tests;

#[cfg(test)]
#[path = "store_link_tests.rs"]
mod link_tests;

async fn entry_from_document(
    catalog: &Catalog,
    document: &crate::storage::postgres::DocumentRecord,
) -> Result<IndexEntry, CatalogError> {
    Ok(
        entries_from_documents(catalog, std::slice::from_ref(document))
            .await?
            .into_iter()
            .next()
            .expect("one document yields one entry"),
    )
}

/// Builds the listing entry for a whole page of documents in one pass.
///
/// An entry needs the owner account, the document's live share links, and its
/// grants -- plus the account behind each link-sourced grant, for the guest's
/// display name. Asked per document that was four queries a row and one more
/// per guest, so rendering a 200-document page cost upwards of a thousand
/// round trips. Each of the three collections is read once for the whole page
/// and indexed in memory here instead.
async fn entries_from_documents(
    catalog: &Catalog,
    documents: &[crate::storage::postgres::DocumentRecord],
) -> Result<Vec<IndexEntry>, CatalogError> {
    use std::collections::HashMap;

    if documents.is_empty() {
        return Ok(Vec::new());
    }
    let document_ids: Vec<uuid::Uuid> = documents.iter().map(|document| document.id).collect();
    let links = catalog.share_links_for_documents(&document_ids).await?;
    let grants = catalog.grants_for_documents(&document_ids).await?;

    // Owners, plus the guests named by a link-sourced grant: the only accounts
    // any entry on this page can mention.
    let mut account_ids: Vec<uuid::Uuid> =
        documents.iter().map(|document| document.owner_id).collect();
    account_ids.extend(
        grants
            .iter()
            .filter(|grant| grant.source_link_hash.is_some())
            .map(|grant| grant.account_id),
    );
    account_ids.sort_unstable();
    account_ids.dedup();
    let accounts: HashMap<uuid::Uuid, _> = catalog
        .accounts_by_ids(&account_ids)
        .await?
        .into_iter()
        .map(|account| (account.id, account))
        .collect();

    let mut links_by_document: HashMap<uuid::Uuid, Vec<_>> = HashMap::new();
    for link in links {
        links_by_document
            .entry(link.document_id)
            .or_default()
            .push(link);
    }
    let mut grants_by_document: HashMap<uuid::Uuid, Vec<_>> = HashMap::new();
    for grant in grants {
        grants_by_document
            .entry(grant.document_id)
            .or_default()
            .push(grant);
    }

    Ok(documents
        .iter()
        .map(|document| {
            let owner = accounts.get(&document.owner_id);
            let links = links_by_document
                .remove(&document.id)
                .unwrap_or_default()
                .into_iter()
                .map(|link| LinkGrant {
                    hash: hex::encode(link.token_hash),
                    role: link.role,
                    sealed: link.sealed_token,
                    label: link.label,
                    budget: link.comment_budget,
                    since: librepaper_base::util::format_unix(link.created_at.unix_timestamp()),
                    until: link
                        .expires_at
                        .map(|at| librepaper_base::util::format_unix(at.unix_timestamp()))
                        .unwrap_or_default(),
                    kind: LinkKind::from_column(&link.kind),
                    owner_generation: link.owner_generation,
                })
                .collect();
            let mut guests = Vec::new();
            let mut account_grants = HashMap::new();
            for grant in grants_by_document.remove(&document.id).unwrap_or_default() {
                let Some(hash) = grant.source_link_hash else {
                    if let Some(role) = grant
                        .role
                        .as_deref()
                        .and_then(|word| word.parse::<Role>().ok())
                    {
                        account_grants.insert(grant.account_id.to_string(), role);
                    }
                    continue;
                };
                // A grant whose account has since been erased names nobody, and
                // is skipped exactly as the per-row lookup skipped a miss.
                let Some(account) = accounts.get(&grant.account_id) else {
                    continue;
                };
                guests.push(Guest {
                    id: grant.account_id.to_string(),
                    name: account.display_name.clone(),
                    since: librepaper_base::util::format_unix(grant.created_at.unix_timestamp()),
                    link: hex::encode(hash),
                });
            }
            IndexEntry {
                slug: document.slug.clone(),
                storage_id: document.id.to_string(),
                title: document.title.clone(),
                // There is no version row any more to name a moment by; the
                // log's own sequence is the cheapest thing that changes
                // exactly when the document does, so it is what a listing
                // page uses as a change token.
                sha: document.update_sequence.to_string(),
                created_at: librepaper_base::util::format_unix(
                    document.created_at.unix_timestamp(),
                ),
                updated_at: librepaper_base::util::format_unix(
                    document.updated_at.unix_timestamp(),
                ),
                unowned: document.ownership_mode == "open",
                publisher: owner.map(|a| a.handle.clone()).unwrap_or_default(),
                publisher_id: document.owner_id.to_string(),
                publisher_name: owner.map(|a| a.display_name.clone()).unwrap_or_default(),
                size: 0,
                source_format: document.source_format.clone(),
                links,
                guests,
                account_grants,
                bookmark_link_hash: None,
            }
        })
        .collect())
}

pub fn random_suffix(config: &Configuration) -> String {
    let alphabet = config.suffix_alphabet.as_bytes();
    librepaper_base::util::random_bytes(config.suffix_length)
        .into_iter()
        .map(|byte| alphabet[byte as usize % alphabet.len()] as char)
        .collect()
}

pub fn slugify(value: &str, config: &Configuration) -> String {
    let mut out = String::new();
    let mut dash = false;
    for character in value.to_lowercase().chars() {
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            out.push(character);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    let slug: String = out
        .trim_matches('-')
        .chars()
        .take(config.slug_max)
        .collect();
    slug.trim_matches('-').to_string()
}

fn parse_account_id(account_id: &str) -> Result<uuid::Uuid, CatalogError> {
    uuid::Uuid::parse_str(account_id).map_err(|_| CatalogError::Invalid("invalid account".into()))
}

/// The UUID of one of the account's own tags. A tag id that is not a UUID can
/// name no tag of anybody's, so it is simply not found; the built-in tag is
/// refused before that, because it is a tag but never a writable one.
fn custom_tag_id(tag: &str) -> Result<uuid::Uuid, CatalogError> {
    if tag == FAVORITES_TAG {
        return Err(built_in_refusal());
    }
    uuid::Uuid::parse_str(tag).map_err(|_| CatalogError::NotFound)
}

fn tag_modify_error(error: CatalogError) -> ModifyError {
    match error {
        CatalogError::NotFound => ModifyError::NotFound,
        CatalogError::Invalid(message) | CatalogError::Conflict(message) => {
            ModifyError::Refused(message)
        }
        other => ModifyError::Storage(other.to_string()),
    }
}

/// The listed documents' slugs by storage id, and those ids.
fn slugs_by_id(
    entries: &[IndexEntry],
) -> (
    std::collections::HashMap<uuid::Uuid, String>,
    Vec<uuid::Uuid>,
) {
    let mut by_id = std::collections::HashMap::new();
    for entry in entries {
        if let Ok(id) = uuid::Uuid::parse_str(&entry.storage_id) {
            by_id.insert(id, entry.slug.clone());
        }
    }
    let ids = by_id.keys().copied().collect();
    (by_id, ids)
}
