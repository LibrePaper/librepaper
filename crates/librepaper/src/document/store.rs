//! Catalogue-backed document metadata, source reads, and mutations.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::Configuration;
use crate::storage::blob::BlobStore;
use crate::storage::postgres::{Error as CatalogError, PostgresCatalog as Catalog};
use crate::util::parse_timestamp;

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
    /// The owner's display name, with the account handle as a fallback.
    pub fn owner_name(&self) -> &str {
        if self.publisher_name.is_empty() {
            &self.publisher
        } else {
            &self.publisher_name
        }
    }

    /// Ownership compares the exact immutable account identity from the catalog.
    pub fn owned_by(&self, _owner_key: &str, caller_id: &str) -> bool {
        !self.publisher_id.is_empty() && !caller_id.is_empty() && caller_id == self.publisher_id
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
        if let Some(granted) = self.account_grants.get(caller_id).copied() {
            if granted == Role::Editor && ceiling.edit {
                role = role.max(Role::Editor);
            } else if granted.at_least(Role::Commenter) && ceiling.comment {
                role = role.max(Role::Commenter);
            }
        }
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
        self.owned_by(owner_key, caller_id)
            || self.account_grants.contains_key(caller_id)
            || self.link_role(link_hash, now).is_some()
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
    /// The authoritative catalogue.
    pub catalog: Arc<Catalog>,
}

/// A document as it is created. There is one version of it from here on, and
/// what is stored is its source: nothing derived is kept, because every
/// browser that shows the document renders it. So there is no HTML here, and
/// no digest of any.
#[derive(Clone, Debug, Default)]
pub struct DocumentInput {
    pub slug: String,
    pub title: String,
    /// The document itself. A document written as HTML has HTML for its
    /// source and the identity for its renderer.
    pub source: String,
    pub source_format: String,
    /// The path of the main file. A document of one file is a directory of
    /// one file, and this is what it is called in it.
    pub main: String,
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

impl Store {
    pub async fn open_with_catalog(
        blobs: Arc<dyn BlobStore>,
        config: Arc<Configuration>,
        catalog: Arc<Catalog>,
    ) -> Result<Self, String> {
        Ok(Self {
            blobs,
            config,
            catalog,
        })
    }

    pub async fn begin_delete(&self, slug: &str) -> Result<Option<String>, String> {
        let catalog = &self.catalog;
        let Some(document) = catalog
            .document_by_slug(slug)
            .await
            .map_err(|e| e.to_string())?
        else {
            return Ok(None);
        };
        let changed = catalog
            .mark_document_deleting(document.id)
            .await
            .map_err(|e| e.to_string())?;
        if changed {
            catalog
                .enqueue_job(crate::storage::postgres::NewJob {
                    kind: "document_deletion".into(),
                    document_id: Some(document.id),
                    account_id: Some(document.owner_id),
                    scope_key: format!("document:{}", document.id),
                    dedupe_key: Some("delete".into()),
                    payload: serde_json::json!({
                        "document_id": document.id,
                    }),
                    priority: 0,
                    max_attempts: 100,
                    run_after: time::OffsetDateTime::now_utc() + time::Duration::days(7),
                })
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(Some(document.id.to_string()))
    }

    pub async fn get_result(&self, slug: &str) -> Result<Option<IndexEntry>, CatalogError> {
        let catalog = &self.catalog;
        let Some(document) = catalog.document_by_slug(slug).await? else {
            return Ok(None);
        };
        Ok(Some(entry_from_document(catalog, &document).await?))
    }

    pub async fn get_checked(&self, slug: &str) -> Result<Option<IndexEntry>, CatalogError> {
        self.get_result(slug).await
    }

    pub async fn list_result(&self) -> Result<Vec<IndexEntry>, CatalogError> {
        let catalog = &self.catalog;
        let documents = catalog.list_documents(None, 200).await?;
        entries_from_documents(catalog, &documents).await
    }

    pub async fn visible_page_with_options(
        &self,
        account_id: Option<&str>,
        _owner_key: Option<&str>,
        cursor: Option<(&str, &str)>,
        limit: u32,
        include_examples: bool,
    ) -> Result<Vec<IndexEntry>, CatalogError> {
        let catalog = &self.catalog;
        let account_id = account_id
            .map(uuid::Uuid::parse_str)
            .transpose()
            .map_err(|_| CatalogError::Invalid("invalid listing account".into()))?;
        let before = match cursor {
            Some((updated, slug)) => {
                if crate::util::parse_timestamp(updated).is_none() {
                    return Err(CatalogError::Invalid("invalid listing cursor".into()));
                }
                let document = catalog
                    .document_by_slug(slug)
                    .await?
                    .ok_or_else(|| CatalogError::Invalid("invalid listing cursor".into()))?;
                // The wire timestamp is second-precision. Use the row's exact
                // value so documents updated within the same second are not
                // skipped between pages.
                Some((document.updated_at, document.id))
            }
            None => None,
        };
        let documents = catalog
            .visible_documents(account_id, before, i64::from(limit), include_examples)
            .await?;
        entries_from_documents(catalog, &documents).await
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
            let due = catalog
                .deletion_due(document.id)
                .await?
                .map(|at| crate::util::format_unix(at.unix_timestamp()));
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
    pub async fn purge_now(&self, slug: &str, account_id: &str) -> Result<bool, ModifyError> {
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
        catalog
            .hasten_deletion(document.id)
            .await
            .map_err(|error| ModifyError::Storage(error.to_string()))
    }

    /// Star a document for this account, or take the star off it.
    pub async fn set_favorite(
        &self,
        slug: &str,
        account_id: &str,
        on: bool,
    ) -> Result<bool, ModifyError> {
        let (document_id, account_id) = self.mark_target(slug, account_id).await?;
        self.catalog
            .set_favorite(account_id, document_id, on)
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

    /// The marks this account holds on the documents listed, by slug. The
    /// listing decorates its rows with these; a document the caller has never
    /// touched simply has no entry.
    /// Every file in a project, by path, as it currently stands.
    ///
    /// Read from the durable operation history rather than from a version or
    /// from the live room. Versions are moments somebody named, so the newest
    /// one can be hours behind the text; the operation log is never behind,
    /// and replaying it needs no room to be resident. A document that has no
    /// operation history yet -- one created from a template and never opened
    /// -- is read from the version it was created with, which is the only
    /// thing that exists to read.
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
        let collaboration = crate::storage::collaboration::CollaborationStorage::new(
            self.catalog.clone(),
            self.blobs.clone(),
        );
        let recovered = collaboration
            .recover(document.id)
            .await
            .map_err(|error| error.to_string())?;
        if recovered.base.is_none() && recovered.updates.is_empty() {
            return self.seeded_project_files(document.id).await;
        }
        let doc = crate::document::session::new_doc();
        if let Some(base) = recovered.base {
            crate::document::session::apply_update(&doc, &base)
                .map_err(|error| error.to_string())?;
        }
        for update in recovered.updates {
            crate::document::session::apply_update(&doc, &update.update_bytes)
                .map_err(|error| error.to_string())?;
        }
        let mut files: Vec<(String, Vec<u8>)> = crate::document::session::texts_of(&doc)
            .into_iter()
            .map(|(path, text)| (path, text.into_bytes()))
            .collect();
        // Figures live in the store under their digest, and the document
        // carries only the name somebody gave them. One whose bytes have gone
        // is skipped rather than failing the copy: a project missing a figure
        // is still worth having.
        let named = crate::document::session::assets_of(&doc);
        let digests: Vec<_> = named.values().cloned().collect();
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
            for (path, digest) in named {
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

    /// The files of a document that has never been edited, read from the
    /// version it was created with. Only `project_files` needs this, and only
    /// for the window between a document being created and first opened.
    async fn seeded_project_files(
        &self,
        document_id: uuid::Uuid,
    ) -> Result<Option<Vec<(String, Vec<u8>)>>, String> {
        let source = crate::storage::source::SourceStorage::new(
            self.catalog.clone(),
            self.blobs.clone(),
            Default::default(),
        );
        let Some(project) = source
            .read_current(document_id)
            .await
            .map_err(|error| format!("{error:?}"))?
        else {
            return Ok(None);
        };
        let mut files = Vec::with_capacity(project.archive.files.len());
        for file in &project.archive.files {
            match file {
                crate::storage::source_archive::SourceFile::Inline { path, bytes } => {
                    files.push((path.clone(), bytes.clone()));
                }
                crate::storage::source_archive::SourceFile::Asset { path, asset_id, .. } => {
                    if let Some(bytes) = project.assets.get(asset_id) {
                        files.push((path.clone(), bytes.clone()));
                    }
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
        let mut by_id = std::collections::HashMap::new();
        for entry in entries {
            if let Ok(id) = uuid::Uuid::parse_str(&entry.storage_id) {
                by_id.insert(id, entry.slug.clone());
            }
        }
        let ids: Vec<_> = by_id.keys().copied().collect();
        let counts = self.catalog.listing_counts(&ids).await?;
        Ok(counts
            .into_iter()
            .filter_map(|row| {
                let slug = by_id.get(&row.document_id)?.clone();
                Some((slug, (row.comments, row.open, row.file_count)))
            })
            .collect())
    }

    pub async fn marks_for(
        &self,
        account_id: &str,
        entries: &[IndexEntry],
    ) -> Result<std::collections::HashMap<String, (bool, String)>, CatalogError> {
        let account_id = uuid::Uuid::parse_str(account_id)
            .map_err(|_| CatalogError::Invalid("invalid listing account".into()))?;
        let mut by_id = std::collections::HashMap::new();
        for entry in entries {
            if let Ok(id) = uuid::Uuid::parse_str(&entry.storage_id) {
                by_id.insert(id, entry.slug.clone());
            }
        }
        let ids: Vec<_> = by_id.keys().copied().collect();
        let marks = self.catalog.marks_for_documents(account_id, &ids).await?;
        Ok(marks
            .into_iter()
            .filter_map(|mark| {
                let slug = by_id.get(&mark.document_id)?.clone();
                let opened = mark
                    .opened_at
                    .map(|at| crate::util::format_unix(at.unix_timestamp()))
                    .unwrap_or_default();
                Some((slug, (mark.favorited_at.is_some(), opened)))
            })
            .collect())
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

    pub async fn put_directory_as_actor(
        &self,
        value: DocumentInput,
        files: Vec<(String, Vec<u8>)>,
        actor: MutationActor,
    ) -> Result<IndexEntry, PutError> {
        let catalog = &self.catalog;
        let account_id =
            uuid::Uuid::parse_str(&actor.account_id).map_err(|_| PutError::Authorization {
                status: 401,
                message: "bundle actor account is invalid",
            })?;
        let document = match catalog
            .document_by_slug(&value.slug)
            .await
            .map_err(source_put_error)?
        {
            Some(document) => document,
            None => catalog
                .create_document(crate::storage::postgres::NewDocument {
                    slug: value.slug.clone(),
                    owner_id: account_id,
                    ownership_mode: if actor.unowned_publisher {
                        "open".into()
                    } else {
                        "owned".into()
                    },
                    title: value.title.clone(),
                    source_format: value.source_format.clone(),
                    main_path: value.main.clone(),
                    settings: serde_json::json!({"version":1}),
                })
                .await
                .map_err(source_put_error)?,
        };
        if document.owner_id != account_id && !actor.policy_editor {
            return Err(PutError::Authorization {
                status: 403,
                message: "edit access changed",
            });
        }
        let mut project_files = vec![crate::storage::source::ProjectFile {
            path: value.main.clone(),
            bytes: value.source.into_bytes(),
            media_type: "text/plain; charset=utf-8".into(),
        }];
        project_files.extend(files.into_iter().map(|(path, bytes)| {
            crate::storage::source::ProjectFile {
                path,
                bytes,
                media_type: "application/octet-stream".into(),
            }
        }));
        crate::storage::source::SourceStorage::new(
            catalog.clone(),
            self.blobs.clone(),
            Default::default(),
        )
        .with_retained_asset_limit(self.config.max_assets)
        .commit_project(crate::storage::source::CommitProject {
            document_id: document.id,
            files: project_files,
            through_update_sequence: document.update_sequence,
            project_generation: document.project_generation,
            // What this is: a whole document written at once, from outside
            // any room -- an upload, a fork, a starter, a seeded example. It
            // used to be called "publish", from when publishing was something
            // a person did; nothing here publishes anything.
            reason: "created".into(),
            label: None,
            author_account_id: Some(account_id),
            // The name, not the id. `actor.account_id` is the catalogue uuid,
            // and a timeline signed with it can only say "Unknown editor".
            author_label: match catalog.account_display_name(account_id).await {
                name if name.is_empty() => actor.owner_key.clone(),
                name => name,
            },
            make_current: true,
        })
        .await
        .map_err(|e| PutError::Storage(e.to_string()))?;
        self.get_result(&value.slug)
            .await
            .map_err(source_put_error)?
            .ok_or_else(|| PutError::Storage("document disappeared after commit".into()))
    }

    pub async fn remove(&self, slug: &str) -> Result<usize, String> {
        Ok(usize::from(self.begin_delete(slug).await?.is_some()))
    }

    pub async fn check_project_title(&self, _slug: &str, title: &str) -> Result<(), String> {
        if title.trim().is_empty() {
            Err("a project title is required".into())
        } else {
            Ok(())
        }
    }

    pub async fn rename(&self, slug: &str, title: &str) -> Result<(), String> {
        if title.is_empty() {
            return Ok(());
        }
        let catalog = &self.catalog;
        let Some(document) = catalog
            .document_by_slug(slug)
            .await
            .map_err(|e| e.to_string())?
        else {
            return Ok(());
        };
        catalog
            .update_document_identity(
                document.id,
                title,
                document.owner_id,
                &document.ownership_mode,
            )
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn modify<F>(&self, slug: &str, change: F) -> Result<IndexEntry, ModifyError>
    where
        F: Fn(&mut IndexEntry) -> Result<(), String>,
    {
        self.modify_inner(slug, None, change).await
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
        let raw = hex::decode(link_hash)
            .map_err(|_| ModifyError::Refused("invalid link digest".into()))?;
        let hash: [u8; 32] = raw
            .try_into()
            .map_err(|_| ModifyError::Refused("invalid link digest".into()))?;
        let role = match role {
            Role::Reader => crate::storage::postgres::AccessRole::Reader,
            Role::Commenter => crate::storage::postgres::AccessRole::Commenter,
            Role::Editor => crate::storage::postgres::AccessRole::Editor,
            Role::Owner => return Err(ModifyError::Refused("invalid guest role".into())),
        };
        catalog
            .pin_link_guest(document.id, account_id, role, hash)
            .await
            .map_err(|error| ModifyError::Storage(error.to_string()))
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
        self.modify_inner(slug, Some(actor), change).await
    }

    async fn modify_inner<F>(
        &self,
        slug: &str,
        actor: Option<&MutationActor>,
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
        if let Some(actor) = actor {
            if uuid::Uuid::parse_str(&actor.account_id).ok() != Some(document.owner_id) {
                return Err(ModifyError::Refused("ownership changed".into()));
            }
        }
        let mut entry = entry_from_document(catalog, &document)
            .await
            .map_err(|e| ModifyError::Storage(e.to_string()))?;
        change(&mut entry).map_err(ModifyError::Refused)?;
        let owner = uuid::Uuid::parse_str(&entry.publisher_id).unwrap_or(document.owner_id);
        let mode = if entry.example {
            "example"
        } else if entry.unowned {
            "open"
        } else {
            "owned"
        };
        catalog
            .update_document_identity(document.id, &entry.title, owner, mode)
            .await
            .map_err(|e| ModifyError::Storage(e.to_string()))?;
        let links = entry
            .links
            .iter()
            .map(|link| {
                let raw = hex::decode(&link.hash)
                    .map_err(|_| ModifyError::Refused("invalid link digest".into()))?;
                let hash: [u8; 32] = raw
                    .try_into()
                    .map_err(|_| ModifyError::Refused("invalid link digest".into()))?;
                let expiry = if link.until.is_empty() {
                    None
                } else {
                    crate::util::parse_timestamp(&link.until)
                        .and_then(|seconds| time::OffsetDateTime::from_unix_timestamp(seconds).ok())
                };
                Ok((
                    link.role.clone(),
                    hash,
                    link.label.clone(),
                    expiry,
                    link.budget,
                ))
            })
            .collect::<Result<Vec<_>, ModifyError>>()?;
        catalog
            .replace_share_links(document.id, &links)
            .await
            .map_err(|e| ModifyError::Storage(e.to_string()))?;
        let live_hashes = entry
            .links
            .iter()
            .filter(|link| link.live_at(crate::util::now_unix()))
            .filter_map(|link| hex::decode(&link.hash).ok())
            .collect::<Vec<_>>();
        catalog
            .prune_link_grants(document.id, &live_hashes)
            .await
            .map_err(|e| ModifyError::Storage(e.to_string()))?;
        self.get_result(slug)
            .await
            .map_err(|e| ModifyError::Storage(e.to_string()))?
            .ok_or(ModifyError::NotFound)
    }
}

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
                    key: String::new(),
                    label: link.label,
                    budget: link.comment_budget,
                    since: crate::util::format_unix(link.created_at.unix_timestamp()),
                    until: link
                        .expires_at
                        .map(|at| crate::util::format_unix(at.unix_timestamp()))
                        .unwrap_or_default(),
                })
                .collect();
            let mut guests = Vec::new();
            let mut account_grants = HashMap::new();
            for grant in grants_by_document.remove(&document.id).unwrap_or_default() {
                let Some(hash) = grant.source_link_hash else {
                    if let Some(role) = Role::parse(&grant.role) {
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
                    since: crate::util::format_unix(grant.created_at.unix_timestamp()),
                    link: hex::encode(hash),
                });
            }
            IndexEntry {
                slug: document.slug.clone(),
                storage_id: document.id.to_string(),
                title: document.title.clone(),
                sha: document
                    .current_version_id
                    .map(|id| id.to_string())
                    .unwrap_or_default(),
                created_at: crate::util::format_unix(document.created_at.unix_timestamp()),
                updated_at: crate::util::format_unix(document.updated_at.unix_timestamp()),
                example: document.ownership_mode == "example",
                unowned: document.ownership_mode == "open",
                publisher: owner.map(|a| a.handle.clone()).unwrap_or_default(),
                publisher_id: document.owner_id.to_string(),
                publisher_name: owner.map(|a| a.display_name.clone()).unwrap_or_default(),
                size: 0,
                source_format: document.source_format.clone(),
                main: document.main_path.clone(),
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
    crate::auth::random_bytes(config.suffix_length)
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

pub fn example_suffix(base: &str, config: &Configuration) -> String {
    let digest = Sha256::digest(format!("librepaper example {base}").as_bytes());
    let alphabet = config.suffix_alphabet.as_bytes();
    digest
        .iter()
        .take(config.suffix_length)
        .map(|byte| alphabet[*byte as usize % alphabet.len()] as char)
        .collect()
}

pub fn digest_of(text: &str) -> String {
    digest_of_bytes(text.as_bytes())
}
pub fn digest_of_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
