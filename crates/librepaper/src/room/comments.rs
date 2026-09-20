//! The comments: what one is, how a submission from a reader becomes one, and
//! the anchor that ties it to a passage.
//!
//! Under SPEC-server-is-a-log §7 every mutation here is a [`SequencerCommand`]:
//! `evaluate` runs synchronously against the sequencer's cached document and
//! decides what the write means (where a quoted passage is, whether a
//! suggestion's branch still applies), and `transact` runs inside the fenced
//! transaction that also wrote the log row naming the state `evaluate` saw.
//! The comment and the source it quotes are one transaction because they are
//! written by the same call (§7 step 4).
//!
//! `evaluate` has no database access -- it runs with the sequencer's lock
//! held and nothing else may run meanwhile, which is precisely what makes it
//! safe to trust. So a command that needs to know something about an
//! *existing* row (a suggestion's current proposal, its stored branch) is
//! constructed with that already read by its caller, before `room.command`
//! is called, and re-validates what it needs of it against `tx` inside
//! `transact`, which does have a connection.
//!
//! Every write to `annotations` and `replies` goes through
//! `storage::postgres::annotations`'s `*_authorized` methods rather than SQL
//! written here: that module is the one place `authorize_annotation_mutation`
//! is called from, and it is what turns a rung the sequencer admitted
//! (`Command::authority`, checked against the writer epoch and the document
//! lock) into a check of the actor's actual grant and session. A command
//! that also has to enforce "the suggestion's own author, not just any
//! commenter" -- `RefineSuggestion` -- reads the existing row itself first
//! and refuses before it writes, the same way `DeleteComment` does; that is
//! narrower than what the catalogue checks and the catalogue has no way to
//! express it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use futures_util::future::BoxFuture;
use loro::{Frontiers, LoroDoc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Digest;
use sqlx::{Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::config::Configuration;
use crate::document::session;
use crate::log::sequencer::{
    Command as SequencerCommand, CommandError, Evidence, Head, PreparedSource, Rung,
};
use crate::storage::postgres::{
    self, AnnotationRecord, MutationAuthorization, NewAnnotation, NewLabel, NewProposal, NewReply,
    PostgresCatalog, ReplyRecord, ANNOTATION_PAGE_MAX, REPLY_PAGE_MAX,
};
use crate::util::clean;

use super::annotation::{
    CommentTarget, DerivedAttachment, FileId, OriginalAnchor, PresentationContext, SourceTextTarget,
};
use super::locate::{self, Quote};
use super::{Room, WriteError};

// -- the model ---------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Reply {
    pub id: String,
    pub body: String,
    pub creator: String,
    pub created: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    /// What this comment is about, in the document as it stood when it was
    /// made. Written once by the server, from the selection a client sent,
    /// and never written again: see [`super::annotation`].
    ///
    /// Every stored comment has one -- the persistence layer refuses a
    /// comment that does not. It is optional here because the view served to
    /// a reader has it taken out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_anchor: Option<OriginalAnchor>,
    /// Where that passage is in the projection this comment was last served
    /// against. A cache, keyed by the projection digest, held only in this
    /// room's memory and rebuilt whenever a cold process needs it (§8.3):
    /// never a second opinion about what the comment is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment: Option<DerivedAttachment>,
    /// The page the comment was made on: the words as the render had them.
    /// Display evidence only.
    #[serde(default, skip_serializing_if = "PresentationContext::is_empty")]
    pub presentation: PresentationContext,
    #[serde(default)]
    pub motivation: String,
    /// The highlight colour a reader chose, as `#rrggbb`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// The projection digest the rendered page had when this remark was made,
    /// as hex. Empty for a comment made in the editor, against the source
    /// directly. Evidence only: the staleness check it feeds happens once, in
    /// `AddComment::evaluate`, and nothing here re-checks it later.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub render_digest: String,

    /// The proposal this comment offers, when its motivation is `editing`.
    /// A suggestion is a proposed change with a remark attached, and the
    /// change is a branch like any other.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub proposal: String,
    /// What that proposal wants in place of the passage, and whether it was
    /// taken. **Both are projections and neither is stored**: they are
    /// filled in when a comment is served, from the branch and from
    /// `document_proposal_hunks`, never read back out of a column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub outcome: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub creator: String,
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub resolved: bool,
    #[serde(default)]
    pub resolved_at: Option<String>,
    #[serde(default)]
    pub replies: Vec<Reply>,
    /// Who actually posted this comment: "github:<login>" for a signed-in
    /// caller, "visitor:<sha256 of the visitor token>" for a verified
    /// anonymous browser, or "" for neither. Kept out of every client-bound
    /// shape -- it is checked server-side, never shown.
    #[serde(default, skip_serializing)]
    pub author: String,
}

impl Comment {
    /// A stable token for "the state this comment's quoted text was written
    /// against": the log row that made it durable (§8.2). Not the frontier --
    /// a frontier is a byte blob with no business on the wire as an opaque
    /// revision token, and the row number is what a client actually needs to
    /// echo back for a staleness check to mean something across a reload.
    /// Empty on the redacted view served to a reader, which has no anchor.
    pub fn revision(&self) -> String {
        self.original_anchor
            .as_ref()
            .map(|anchor| anchor.source_sequence.to_string())
            .unwrap_or_default()
    }

    /// The passage this comment is about, if it is about a passage rather
    /// than about the document as a whole.
    pub fn source(&self) -> Option<&SourceTextTarget> {
        self.original_anchor
            .as_ref()
            .and_then(|a| a.target.source())
    }
}

/// Keeps a highlight colour only if it is one: six hexadecimal digits behind
/// a hash, lowercased so two spellings of the same colour are one colour.
pub fn valid_color(value: Option<&str>) -> Option<String> {
    let raw = value?.trim();
    let digits = raw.strip_prefix('#')?;
    (digits.len() == 6 && digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| format!("#{}", digits.to_ascii_lowercase()))
}

/// What a caller is shown: every comment field a client ever sees, plus
/// whether this particular caller may delete it.
#[derive(Serialize)]
pub struct CommentView {
    #[serde(flatten)]
    pub comment: Comment,
    pub mine: bool,
    pub deletable: bool,
}

impl CommentView {
    pub fn for_viewer(comment: &Comment, author: &str, is_owner: bool) -> CommentView {
        let mine = !author.is_empty() && comment.author == author;
        let deletable = deletable(comment, author, is_owner);
        let mut comment = comment.clone();
        if !is_owner {
            comment.original_anchor = None;
            comment.attachment = None;
            if comment.render_digest.is_empty() {
                comment.presentation = PresentationContext::default();
            }
            comment.proposal.clear();
            comment.proposed = None;
            comment.outcome.clear();
        }
        CommentView {
            comment,
            mine,
            deletable,
        }
    }
}

/// A suggestion is an edit, so only an editor may receive one. Every other
/// remark reaches the reader.
pub fn visible_to_reader(comment: &Comment) -> bool {
    comment.motivation != "editing"
}

/// Rule H's authorization test: the document's owner may delete anything on
/// it, and everyone else only their own.
pub fn deletable(item: &Comment, author: &str, is_owner: bool) -> bool {
    is_owner || (!author.is_empty() && item.author == author)
}

fn kind_of(motivation: &str) -> &'static str {
    match motivation {
        "highlighting" => "highlight",
        "editing" => "suggestion",
        _ => "comment",
    }
}

fn motivation_of(kind: &str) -> String {
    match kind {
        "highlight" => "highlighting",
        "suggestion" => "editing",
        _ => "commenting",
    }
    .to_string()
}

fn format_time(value: time::OffsetDateTime) -> String {
    crate::util::format_unix(value.unix_timestamp())
}

pub(super) fn path_for_file_id(doc: &LoroDoc, file_id: &FileId) -> Option<String> {
    session::paths_of(doc)
        .into_iter()
        .find_map(|(id, path)| (id == file_id.0).then_some(path))
}

/// The files a passage could have come from, main first.
fn source_files(doc: &LoroDoc) -> Vec<(String, String, String)> {
    let main = session::main_path(doc);
    let paths = session::paths_of(doc);
    let texts = session::texts_of(doc);
    let mut files: Vec<(String, String, String)> = paths
        .into_iter()
        .filter_map(|(id, path)| texts.get(&path).map(|text| (id, path, text.clone())))
        .collect();
    files.sort_by(|a, b| {
        (a.1 != main, a.1.as_str())
            .partial_cmp(&(b.1 != main, b.1.as_str()))
            .expect("total order")
    });
    files
}

/// Works out what a selection is about, in the document at `doc`.
///
/// This is the one place a rendered selection becomes a source range, and it
/// happens against the sequencer's own head because that is the one copy of
/// the document whose text the server can vouch for. A browser sends words,
/// never an offset: an offset from a client is a claim about a file the
/// client may not even be allowed to read.
fn locate_anchor(
    doc: &LoroDoc,
    quote: &Quote<'_>,
    whole_document: bool,
    cap_exact: usize,
) -> Result<CommentTarget, String> {
    if whole_document || quote.exact.trim().is_empty() {
        return Ok(CommentTarget::Document);
    }
    if quote.exact.chars().count() > cap_exact {
        return Err("that selection is too long to comment on".into());
    }
    let files = source_files(doc);
    let candidates: Vec<locate::Candidate<'_>> = files
        .iter()
        .map(|(id, path, text)| locate::Candidate {
            file_id: id,
            path,
            text,
        })
        .collect();
    locate::locate(&candidates, quote)
        .map(CommentTarget::SourceText)
        .map_err(|failure| failure.message().to_string())
}

/// Strips the control characters a suggestion's proposed text must not carry,
/// the same filter every other free-text field here is put through.
fn strip_control(raw: &str) -> String {
    raw.chars()
        .filter(|&c| {
            let code = c as u32;
            !(code < 0x09
                || (0x0b..=0x0c).contains(&code)
                || (0x0e..=0x1f).contains(&code)
                || code == 0x7f)
        })
        .collect()
}

/// What [`build_suggestion_branch`] works out ahead of the transaction:
/// where the comment lands and the proposal branch that carries its edit.
struct PreparedBranch {
    target: CommentTarget,
    base: Vec<u8>,
    tip: Vec<u8>,
    bytes: Vec<u8>,
    peer: u64,
}

/// Builds a one-hunk branch putting `proposed` where a passage found at `doc`
/// is, and locates that passage fresh -- a refine or a suggestion never
/// trusts an old offset, because the point of locating at head is that
/// offsets move.
fn build_suggestion_branch(
    doc: &LoroDoc,
    quote: &Quote<'_>,
    cap_exact: usize,
    proposed: &str,
) -> Result<PreparedBranch, String> {
    let target = locate_anchor(doc, quote, false, cap_exact)?;
    let source = target
        .source()
        .cloned()
        .ok_or_else(|| "a suggestion has to be about a passage".to_string())?;
    let path = path_for_file_id(doc, &source.file_id)
        .ok_or_else(|| "that source file is not part of this document".to_string())?;
    let (branch_doc, base, tip, peer) = crate::room::proposals::from_suggestion(
        doc,
        &path,
        source.start_utf16 as usize,
        &source.exact,
        proposed,
    )
    .map_err(|error| error.to_string())?;
    let branch_bytes = session::encode_diff(&branch_doc, &session::encode_vector(doc))?;
    Ok(PreparedBranch {
        target,
        base: base.encode(),
        tip: tip.encode(),
        bytes: branch_bytes,
        peer,
    })
}

// -- reading -------------------------------------------------------------

fn annotation_row_to_comment(
    row: &AnnotationRecord,
    replies: Vec<Reply>,
) -> Result<Comment, WriteError> {
    let original_anchor = postgres::original_anchor_from_record(row)?;
    let presentation = postgres::presentation_from_record(row);
    Ok(Comment {
        id: row.id.to_string(),
        original_anchor: Some(original_anchor),
        attachment: None,
        presentation,
        motivation: motivation_of(&row.kind),
        color: row.color.clone(),
        render_digest: row
            .render_digest
            .as_ref()
            .map(hex::encode)
            .unwrap_or_default(),
        proposal: row.proposal_id.map(|id| id.to_string()).unwrap_or_default(),
        proposed: None,
        outcome: String::new(),
        body: row.body.clone(),
        creator: row.author_label.clone(),
        created: format_time(row.created_at),
        resolved: row.resolved_at.is_some(),
        resolved_at: row.resolved_at.map(format_time),
        replies,
        author: row.author_key.clone(),
    })
}

fn reply_row_to_reply(row: ReplyRecord) -> Reply {
    Reply {
        id: row.id.to_string(),
        body: row.body,
        creator: row.author_label,
        created: format_time(row.created_at),
    }
}

/// This document's comments, from the catalogue. `room/mod.rs` calls this to
/// fill its resident comment cache.
/// A digest for "the annotation state changed": a hash of the current
/// comment list, since there is no room-held sequence counter any more.
/// Shared by `agent_query_snapshot`'s pagination cursors and
/// `broadcast_comment_snapshot`'s wire payload, so both agree on what
/// "changed" means.
pub(crate) fn comment_digest_of(comments: &[Comment]) -> i64 {
    let bytes = serde_json::to_vec(comments).unwrap_or_default();
    let digest = sha2::Sha256::digest(&bytes);
    i64::from_le_bytes(digest[..8].try_into().expect("8 bytes"))
}

/// Memory estimate ceiling for the existing whole-snapshot protocol. This
/// is a read resource guard, not a limit on accepted comments. Larger
/// collections need end-to-end transport pagination, not a larger SQL page.
pub(crate) const COMMENT_SNAPSHOT_BYTES_MAX: usize = 16 * 1024 * 1024;

struct ReadBudget(usize);

impl ReadBudget {
    fn charge<T: Serialize>(&mut self, value: &T) -> Result<(), WriteError> {
        // Count without allocating another serialized copy. Account for Rust
        // collection/string overhead as well as the encoded content.
        struct Counter(usize);
        impl std::io::Write for Counter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0 = self.0.saturating_add(bytes.len());
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut count = Counter(0);
        serde_json::to_writer(&mut count, value)
            .map_err(|error| WriteError::Storage(error.to_string()))?;
        let cost = count
            .0
            .saturating_mul(2)
            .saturating_add(std::mem::size_of::<T>() * 2);
        self.0 = self.0.checked_sub(cost).ok_or_else(|| {
            WriteError::Unreadable(
                "comment snapshot exceeds the read memory budget; comments remain stored".into(),
            )
        })?;
        Ok(())
    }
}

/// Page through rows, with a bound on the aggregate resident result as well
/// as each SQL response. A failed load is never cached as an empty list.
pub async fn load(
    catalog: &PostgresCatalog,
    document_id: Uuid,
) -> Result<Vec<Comment>, WriteError> {
    load_with_budget(catalog, document_id, COMMENT_SNAPSHOT_BYTES_MAX).await
}

pub(crate) async fn load_with_budget(
    catalog: &PostgresCatalog,
    document_id: Uuid,
    bytes: usize,
) -> Result<Vec<Comment>, WriteError> {
    let mut budget = ReadBudget(bytes);
    let mut out = Vec::new();
    let mut after: Option<(OffsetDateTime, Uuid)> = None;
    loop {
        let rows = catalog
            .annotations(document_id, after, ANNOTATION_PAGE_MAX)
            .await?;
        let Some(last) = rows.last() else { break };
        after = Some((last.created_at, last.id));
        let reached_end = (rows.len() as i64) < ANNOTATION_PAGE_MAX;
        let mut page = Vec::with_capacity(rows.len());
        let mut indices = HashMap::new();
        for row in &rows {
            let comment = annotation_row_to_comment(row, Vec::new())?;
            budget.charge(&comment)?;
            // The private author key is deliberately not serialized.
            budget.charge(&comment.author)?;
            indices.insert(row.id, page.len());
            page.push(comment);
        }
        let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
        let mut reply_after = None;
        loop {
            let replies = catalog.replies(&ids, reply_after, REPLY_PAGE_MAX).await?;
            let Some(last) = replies.last() else { break };
            reply_after = Some((last.created_at, last.id));
            let replies_done = (replies.len() as i64) < REPLY_PAGE_MAX;
            for row in replies {
                let index = indices[&row.annotation_id];
                let reply = reply_row_to_reply(row);
                budget.charge(&reply)?;
                page[index].replies.push(reply);
            }
            if replies_done {
                break;
            }
        }
        out.extend(page);
        if reached_end {
            break;
        }
    }
    Ok(out)
}

/// The one row a command needs to read back before it writes -- to resolve,
/// delete or refine a comment against what it currently says.
///
/// This is an indexed lookup by `(id, document_id)`, not a scan of the
/// document's first page: the scan it replaces could not see a comment past
/// the page size and refused it as unknown, and the document is in the
/// predicate so an id from a request body can still only name a row of the
/// document the caller was authorized for.
///
/// It reads through the command's own transaction. That is what makes a read
/// *after* a write in the same `transact` correct -- `ResolveComment` writes
/// `resolved_at` and then reports it, and on the pool it reported the value
/// from before its own uncommitted update -- and it keeps a command holding
/// a transaction from taking a second connection out of the pool.
async fn find_annotation(
    catalog: &PostgresCatalog,
    tx: &mut Transaction<'static, Postgres>,
    document_id: Uuid,
    id: Uuid,
) -> Result<AnnotationRecord, CommandError> {
    catalog
        .annotation_in_transaction(tx, document_id, id)
        .await?
        .ok_or_else(|| CommandError::Conflict("unknown comment".into()))
}

/// [`find_annotation`] for the one path that has no transaction open: §7.2's
/// replay, which answers a retry before a transaction is begun.
async fn find_annotation_committed(
    catalog: &PostgresCatalog,
    document_id: Uuid,
    id: Uuid,
) -> Result<AnnotationRecord, CommandError> {
    catalog
        .annotation(document_id, id)
        .await?
        .ok_or_else(|| CommandError::Conflict("unknown comment".into()))
}

/// Rebuilds the `NewAnnotation` a row already holds, for the one field that
/// is actually changing (`resolved`) to go through `replace_annotation_authorized`
/// -- which replaces the whole row, immutable anchor included, and refuses
/// when what it is handed does not match what is stored (the check that
/// keeps an anchor from drifting).
fn replay_of(row: &AnnotationRecord) -> Result<NewAnnotation, CommandError> {
    let original_anchor =
        postgres::original_anchor_from_record(row).map_err(CommandError::Storage)?;
    let presentation = postgres::presentation_from_record(row);
    Ok(NewAnnotation {
        document_id: row.document_id,
        kind: row.kind.clone(),
        body: row.body.clone(),
        author_account_id: row.author_account_id,
        author_key: row.author_key.clone(),
        author_label: row.author_label.clone(),
        color: row.color.clone(),
        proposal_id: row.proposal_id,
        original_anchor,
        presentation,
        render_digest: row.render_digest.clone(),
        attachment: None,
    })
}

// -- semantic commands ---------------------------------------------------

/// The proposal branch half of [`PreparedBranch`], for a caller that already
/// keeps the target (here, `AddComment::resolved_target`) in a field of its
/// own and would otherwise carry it twice.
struct SuggestionBranch {
    base: Vec<u8>,
    tip: Vec<u8>,
    bytes: Vec<u8>,
    peer: u64,
}

/// What a new comment, highlight or suggestion needs before it reaches the
/// sequencer: everything cleaned against the deployment's caps, with nothing
/// left for `evaluate` to validate except where the words actually are.
pub struct AddComment {
    catalog: Arc<PostgresCatalog>,
    id: Uuid,
    document_id: Uuid,
    motivation: String,
    body: String,
    creator: String,
    color: Option<String>,
    author_account_id: Option<Uuid>,
    author_key: String,
    actor: MutationAuthorization,
    exact: String,
    prefix: String,
    suffix: String,
    position: Option<u32>,
    whole_document: bool,
    proposed: Option<String>,
    /// Set only for a comment made against a rendered page. `evaluate`
    /// refuses with `StaleSelection` when this no longer matches the head
    /// digest (§7.1) rather than anchoring against text the reader never saw.
    expected_render_digest: Option<[u8; 32]>,
    cap_exact: usize,
    // filled in by evaluate, consumed by transact.
    resolved_target: Option<CommentTarget>,
    branch: Option<SuggestionBranch>,
}

impl AddComment {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        catalog: Arc<PostgresCatalog>,
        document_id: Uuid,
        id: Uuid,
        config: &Configuration,
        motivation: &str,
        body: &str,
        creator: &str,
        author_account_id: Option<Uuid>,
        author_key: String,
        actor: MutationAuthorization,
        exact: &str,
        prefix: &str,
        suffix: &str,
        position: Option<i64>,
        whole_document: bool,
        color: Option<&str>,
        proposed: Option<&str>,
        expected_render_digest: Option<[u8; 32]>,
    ) -> Result<Self, String> {
        let body = clean(body, config.caps.body).trim().to_string();
        let motivation = config.allowed_motivation(motivation);
        // A highlight is the passage itself and needs no words; a suggestion
        // is its proposal. Everything else is a remark, and a remark with no
        // words is nothing.
        if body.is_empty() && !matches!(motivation.as_str(), "highlighting" | "editing") {
            return Err("comment body is required".into());
        }
        let mut creator = clean(creator, config.caps.creator).trim().to_string();
        if creator.is_empty() {
            creator = "Anonymous".to_string();
        }
        let color = match color {
            Some(raw) if !raw.trim().is_empty() => {
                Some(valid_color(Some(raw)).ok_or("color must be a #RRGGBB value")?)
            }
            _ => None,
        };
        if motivation == "editing" && proposed.is_none() {
            return Err("a suggestion needs a proposal".into());
        }
        let proposed = match motivation.as_str() {
            "editing" => {
                let cleaned = strip_control(proposed.unwrap_or_default());
                if cleaned.chars().count() > config.caps.exact {
                    return Err("the suggestion is too long".into());
                }
                Some(cleaned)
            }
            _ => None,
        };
        Ok(Self {
            catalog,
            id,
            document_id,
            motivation,
            body,
            creator,
            color,
            author_account_id,
            author_key,
            actor,
            exact: clean(exact, config.caps.exact),
            prefix: clean(prefix, config.caps.context),
            suffix: clean(suffix, config.caps.context),
            position: position
                .filter(|at| *at >= 0)
                .map(|at| at.min(u32::MAX as i64) as u32),
            whole_document,
            proposed,
            expected_render_digest,
            cap_exact: config.caps.exact,
            resolved_target: None,
            branch: None,
        })
    }
}

impl SequencerCommand for AddComment {
    type Output = Comment;

    fn name(&self) -> &'static str {
        "comment"
    }

    // A comment, highlight or suggestion needs only commenter access: it is
    // what a commenter link or an `open` document exists to admit, and it
    // lands in exactly the same `annotations` row an editor's would (§7.1).
    fn authority(&self) -> Rung {
        Rung::Commenter
    }

    fn evaluate(&mut self, head: &Head<'_>) -> Result<Option<PreparedSource>, CommandError> {
        if let Some(expected) = &self.expected_render_digest {
            let current = head.digest();
            if hex::encode(expected) != current {
                return Err(CommandError::StaleSelection { digest: current });
            }
        }
        let quote = Quote {
            exact: &self.exact,
            prefix: &self.prefix,
            suffix: &self.suffix,
        };
        let target = locate_anchor(head.doc(), &quote, self.whole_document, self.cap_exact)
            .map_err(CommandError::Conflict)?;
        let source = target.source().cloned();
        if source.is_none() && matches!(self.motivation.as_str(), "editing" | "highlighting") {
            return Err(CommandError::Conflict(
                "a suggestion or highlight has to be about a passage".into(),
            ));
        }
        if self.motivation == "editing" {
            let source = source.expect("checked above");
            let path = path_for_file_id(head.doc(), &source.file_id).ok_or_else(|| {
                CommandError::Conflict("that source file is not part of this document".into())
            })?;
            let proposed = self.proposed.as_deref().unwrap_or_default();
            let (branch_doc, base, tip, peer) = crate::room::proposals::from_suggestion(
                head.doc(),
                &path,
                source.start_utf16 as usize,
                &source.exact,
                proposed,
            )
            .map_err(|error| CommandError::Conflict(error.to_string()))?;
            let branch_bytes =
                session::encode_diff(&branch_doc, &session::encode_vector(head.doc()))
                    .map_err(CommandError::Conflict)?;
            self.branch = Some(SuggestionBranch {
                base: base.encode(),
                tip: tip.encode(),
                bytes: branch_bytes,
                peer,
            });
        }
        self.resolved_target = Some(target);
        Ok(None)
    }

    fn transact<'a>(
        &'a mut self,
        tx: &'a mut Transaction<'static, Postgres>,
        evidence: &'a Evidence,
    ) -> BoxFuture<'a, Result<Self::Output, CommandError>> {
        Box::pin(async move {
            let target = self
                .resolved_target
                .take()
                .expect("evaluate ran before transact");
            let anchor = OriginalAnchor {
                source_sequence: evidence.source_sequence,
                frontier: evidence.before_frontier.clone(),
                target,
            };
            let presentation = PresentationContext {
                rendered_exact: self.exact.clone(),
                rendered_prefix: self.prefix.clone(),
                rendered_suffix: self.suffix.clone(),
                rendered_position_utf16: self.position,
            };
            let row = if let Some(SuggestionBranch {
                base: base_frontiers,
                tip: tip_frontiers,
                bytes: branch_bytes,
                peer,
            }) = self.branch.take()
            {
                let proposal_id = Uuid::new_v4();
                self.catalog
                    .put_suggestion_authorized(
                        tx,
                        self.id,
                        NewAnnotation {
                            document_id: self.document_id,
                            kind: kind_of(&self.motivation).to_string(),
                            body: self.body.clone(),
                            author_account_id: self.author_account_id,
                            author_key: self.author_key.clone(),
                            author_label: self.creator.clone(),
                            color: self.color.clone(),
                            proposal_id: Some(proposal_id),
                            original_anchor: anchor,
                            presentation,
                            render_digest: self
                                .expected_render_digest
                                .map(|digest| digest.to_vec()),
                            attachment: None,
                        },
                        NewProposal {
                            document_id: self.document_id,
                            id: proposal_id,
                            author: self.creator.clone(),
                            author_peer: peer as i64,
                            base_frontiers,
                            tip_frontiers,
                            branch_bytes,
                        },
                        &self.actor,
                    )
                    .await?
            } else {
                self.catalog
                    .put_annotation_authorized(
                        tx,
                        self.id,
                        NewAnnotation {
                            document_id: self.document_id,
                            kind: kind_of(&self.motivation).to_string(),
                            body: self.body.clone(),
                            author_account_id: self.author_account_id,
                            author_key: self.author_key.clone(),
                            author_label: self.creator.clone(),
                            color: self.color.clone(),
                            proposal_id: None,
                            original_anchor: anchor,
                            presentation,
                            render_digest: self
                                .expected_render_digest
                                .map(|digest| digest.to_vec()),
                            attachment: None,
                        },
                        &self.actor,
                        false,
                    )
                    .await?
            };
            annotation_row_to_comment(&row, Vec::new())
                .map_err(|error| CommandError::Storage(postgres::Error::Invalid(error.to_string())))
        })
    }
}

/// A reply to an existing comment. It never touches the source, so it needs
/// nothing of the head; it exists as a command only because every write that
/// reaches the comment cache goes through the sequencer's one lock (§7).
pub struct AddReply {
    catalog: Arc<PostgresCatalog>,
    id: Uuid,
    document_id: Uuid,
    comment_id: Uuid,
    body: String,
    creator: String,
    author_account_id: Option<Uuid>,
    author_key: String,
    actor: MutationAuthorization,
}

impl AddReply {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        catalog: Arc<PostgresCatalog>,
        document_id: Uuid,
        id: Uuid,
        comment_id: Uuid,
        config: &Configuration,
        body: &str,
        creator: &str,
        author_account_id: Option<Uuid>,
        author_key: String,
        actor: MutationAuthorization,
    ) -> Result<Self, String> {
        let body = clean(body, config.caps.body).trim().to_string();
        if body.is_empty() {
            return Err("reply body is required".into());
        }
        let mut creator = clean(creator, config.caps.creator).trim().to_string();
        if creator.is_empty() {
            creator = "Anonymous".to_string();
        }
        Ok(Self {
            catalog,
            id,
            document_id,
            comment_id,
            body,
            creator,
            author_account_id,
            author_key,
            actor,
        })
    }
}

pub struct ReplyOutcome {
    pub comment_id: Uuid,
    pub reply: Reply,
}

impl SequencerCommand for AddReply {
    type Output = ReplyOutcome;

    fn name(&self) -> &'static str {
        "reply"
    }

    fn authority(&self) -> Rung {
        Rung::Commenter
    }

    fn evaluate(&mut self, _head: &Head<'_>) -> Result<Option<PreparedSource>, CommandError> {
        Ok(None)
    }

    fn transact<'a>(
        &'a mut self,
        tx: &'a mut Transaction<'static, Postgres>,
        _evidence: &'a Evidence,
    ) -> BoxFuture<'a, Result<Self::Output, CommandError>> {
        Box::pin(async move {
            let row = self
                .catalog
                .create_reply_authorized(
                    tx,
                    self.document_id,
                    NewReply {
                        id: self.id,
                        annotation_id: self.comment_id,
                        author_account_id: self.author_account_id,
                        author_key: self.author_key.clone(),
                        author_label: self.creator.clone(),
                        body: self.body.clone(),
                    },
                    &self.actor,
                )
                .await?;
            Ok(ReplyOutcome {
                comment_id: row.annotation_id,
                reply: reply_row_to_reply(row),
            })
        })
    }
}

/// Resolves or reopens an ordinary comment (never a suggestion -- deciding
/// one of those is `AcceptSuggestion` or `RejectSuggestion`). There is no
/// per-annotation version column (§8.2 gives one only to
/// `document_proposals`, where a race actually matters): resolving is
/// idempotent by construction, so the last write standing is a correct
/// answer either way.
pub struct ResolveComment {
    catalog: Arc<PostgresCatalog>,
    document_id: Uuid,
    comment_id: Uuid,
    resolved: bool,
    actor: MutationAuthorization,
}

impl ResolveComment {
    pub fn new(
        catalog: Arc<PostgresCatalog>,
        document_id: Uuid,
        comment_id: Uuid,
        resolved: bool,
        actor: MutationAuthorization,
    ) -> Self {
        Self {
            catalog,
            document_id,
            comment_id,
            resolved,
            actor,
        }
    }
}

pub struct ResolveOutcome {
    pub comment_id: Uuid,
    pub resolved: bool,
    pub resolved_at: Option<String>,
}

impl SequencerCommand for ResolveComment {
    type Output = ResolveOutcome;

    fn name(&self) -> &'static str {
        "resolve"
    }

    fn authority(&self) -> Rung {
        Rung::Commenter
    }

    fn evaluate(&mut self, _head: &Head<'_>) -> Result<Option<PreparedSource>, CommandError> {
        Ok(None)
    }

    fn transact<'a>(
        &'a mut self,
        tx: &'a mut Transaction<'static, Postgres>,
        _evidence: &'a Evidence,
    ) -> BoxFuture<'a, Result<Self::Output, CommandError>> {
        Box::pin(async move {
            let existing =
                find_annotation(&self.catalog, tx, self.document_id, self.comment_id).await?;
            let input = replay_of(&existing)?;
            self.catalog
                .replace_annotation_authorized(
                    tx,
                    self.comment_id,
                    input,
                    self.resolved,
                    &self.actor,
                )
                .await?;
            let updated =
                find_annotation(&self.catalog, tx, self.document_id, self.comment_id).await?;
            Ok(ResolveOutcome {
                comment_id: self.comment_id,
                resolved: updated.resolved_at.is_some(),
                resolved_at: updated.resolved_at.map(format_time),
            })
        })
    }
}

/// Deletes a comment. `is_owner` and `author_key` decide whether the caller
/// may, exactly as [`deletable`] would; the check is made against the row's
/// own author read here rather than a copy read earlier, because that is the
/// only copy nothing can have raced. It is narrower than what
/// `delete_annotation_authorized` itself checks (rung and session only, not
/// authorship), so both run: this refuses a commenter deleting someone
/// else's remark, and the catalogue refuses an actor whose access changed.
pub struct DeleteComment {
    catalog: Arc<PostgresCatalog>,
    document_id: Uuid,
    comment_id: Uuid,
    author_key: String,
    is_owner: bool,
    actor: MutationAuthorization,
}

impl DeleteComment {
    pub fn new(
        catalog: Arc<PostgresCatalog>,
        document_id: Uuid,
        comment_id: Uuid,
        author_key: String,
        is_owner: bool,
        actor: MutationAuthorization,
    ) -> Self {
        Self {
            catalog,
            document_id,
            comment_id,
            author_key,
            is_owner,
            actor,
        }
    }
}

impl SequencerCommand for DeleteComment {
    type Output = ();

    fn name(&self) -> &'static str {
        "delete"
    }

    fn authority(&self) -> Rung {
        Rung::Commenter
    }

    fn evaluate(&mut self, _head: &Head<'_>) -> Result<Option<PreparedSource>, CommandError> {
        Ok(None)
    }

    fn transact<'a>(
        &'a mut self,
        tx: &'a mut Transaction<'static, Postgres>,
        _evidence: &'a Evidence,
    ) -> BoxFuture<'a, Result<Self::Output, CommandError>> {
        Box::pin(async move {
            let existing =
                find_annotation(&self.catalog, tx, self.document_id, self.comment_id).await?;
            if !(self.is_owner
                || (!self.author_key.is_empty() && existing.author_key == self.author_key))
            {
                return Err(CommandError::Conflict(
                    "you may only delete your own comments".into(),
                ));
            }
            self.catalog
                .delete_annotation_authorized(tx, self.document_id, self.comment_id, &self.actor)
                .await?;
            Ok(())
        })
    }
}

/// Replaces a pending suggestion's branch with a fresh one, re-locating the
/// same passage at head rather than trusting the offsets it had before: the
/// point of locating at head is that offsets move.
///
/// The branch is updated in place, on the proposal's existing id, through
/// `proposals::update_proposal_branch` (version-conditional: a refine made
/// against a tip somebody else already moved is refused with "read it again"
/// rather than silently discarded or left to open a second, orphaned
/// proposal beside the first). Only the annotation's `body` changes on the
/// `annotations` row, through `replace_annotation_authorized`.
///
/// `authority()` is `Commenter`, not `Editor`, because the product has
/// always let a suggestion's own author refine it without being an editor --
/// only an editor deciding someone *else's* suggestion needed the higher
/// rung. `replace_annotation_authorized` itself checks only commenter access
/// here (`resolved` stays `false`, so its own editor-elevation branch does
/// not fire), so the narrower "author or editor" rule is enforced here,
/// reading the row first, the same way `DeleteComment` enforces its own
/// narrower rule.
/// What a refine carries from `evaluate` to `transact`. `build_suggestion_branch`
/// also works out the target and the base frontiers, but a refine writes
/// onto the proposal's existing branch and leaves its anchor exactly as
/// `existing` already has it (see `RefineSuggestion::transact`), so neither
/// is kept here -- only the tip the branch now sits at and the branch bytes
/// themselves.
struct RefineBranch {
    tip: Vec<u8>,
    bytes: Vec<u8>,
}

pub struct RefineSuggestion {
    catalog: Arc<PostgresCatalog>,
    document_id: Uuid,
    comment_id: Uuid,
    proposal_id: Uuid,
    expected_version: i64,
    author_key: String,
    is_owner: bool,
    body: String,
    exact: String,
    prefix: String,
    suffix: String,
    proposed: String,
    actor: MutationAuthorization,
    cap_exact: usize,
    prepared: Option<RefineBranch>,
}

impl RefineSuggestion {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        catalog: Arc<PostgresCatalog>,
        document_id: Uuid,
        comment_id: Uuid,
        proposal_id: Uuid,
        expected_version: i64,
        author_key: String,
        is_owner: bool,
        config: &Configuration,
        body: &str,
        exact: &str,
        prefix: &str,
        suffix: &str,
        proposed: &str,
        actor: MutationAuthorization,
    ) -> Result<Self, String> {
        let proposed = strip_control(proposed);
        if proposed.chars().count() > config.caps.exact || body.chars().count() > config.caps.body {
            return Err("refinement exceeds the suggestion size limit".into());
        }
        Ok(Self {
            catalog,
            document_id,
            comment_id,
            proposal_id,
            expected_version,
            author_key,
            is_owner,
            body: clean(body, config.caps.body).trim().to_string(),
            exact: clean(exact, config.caps.exact),
            prefix: clean(prefix, config.caps.context),
            suffix: clean(suffix, config.caps.context),
            proposed,
            actor,
            cap_exact: config.caps.exact,
            prepared: None,
        })
    }
}

impl SequencerCommand for RefineSuggestion {
    type Output = Comment;

    fn name(&self) -> &'static str {
        "refine"
    }

    fn authority(&self) -> Rung {
        Rung::Commenter
    }

    fn evaluate(&mut self, head: &Head<'_>) -> Result<Option<PreparedSource>, CommandError> {
        let quote = Quote {
            exact: &self.exact,
            prefix: &self.prefix,
            suffix: &self.suffix,
        };
        let PreparedBranch { tip, bytes, .. } =
            build_suggestion_branch(head.doc(), &quote, self.cap_exact, &self.proposed)
                .map_err(CommandError::Conflict)?;
        self.prepared = Some(RefineBranch { tip, bytes });
        Ok(None)
    }

    fn transact<'a>(
        &'a mut self,
        tx: &'a mut Transaction<'static, Postgres>,
        _evidence: &'a Evidence,
    ) -> BoxFuture<'a, Result<Self::Output, CommandError>> {
        Box::pin(async move {
            let existing =
                find_annotation(&self.catalog, tx, self.document_id, self.comment_id).await?;
            if !(self.is_owner
                || (!self.author_key.is_empty() && existing.author_key == self.author_key))
            {
                return Err(CommandError::Conflict(
                    "only the suggestion author or an editor may refine it".into(),
                ));
            }
            // `target` is only where the branch's replacement text is rooted
            // at head; the suggestion's own anchor -- the passage it
            // discusses -- is immutable and stays whatever `existing`
            // already carries. It is not written back over that anchor.
            let RefineBranch {
                tip: tip_frontiers,
                bytes: branch_bytes,
            } = self.prepared.take().expect("evaluate ran before transact");
            let stored = self
                .catalog
                .update_proposal_branch(
                    tx,
                    self.document_id,
                    self.proposal_id,
                    self.expected_version,
                    tip_frontiers,
                    branch_bytes,
                )
                .await?;
            // `update_proposal_branch` never errors on a version mismatch --
            // per §7.2 it just hands back the row as it stands, for the
            // caller to compare with what it asked for. A version this call
            // did not itself produce means somebody else moved the proposal
            // first.
            if stored.version != self.expected_version + 1 {
                return Err(CommandError::Conflict(
                    "suggestion changed; read it again before refining".into(),
                ));
            }
            let mut input = replay_of(&existing)?;
            input.body = self.body.clone();
            self.catalog
                .replace_annotation_authorized(tx, self.comment_id, input, false, &self.actor)
                .await?;
            let row = find_annotation(&self.catalog, tx, self.document_id, self.comment_id).await?;
            annotation_row_to_comment(&row, Vec::new())
                .map_err(|error| CommandError::Storage(postgres::Error::Invalid(error.to_string())))
        })
    }
}

/// Takes a suggestion. The one command here that produces source (§7.3): the
/// branch is merged into a fork of head and the fork's export is what the
/// row carries. A retry of the same `request_id` finds the `document_labels`
/// row this writes and returns without merging twice (§7.2).
///
/// An editor decision about someone else's suggestion, never reachable by a
/// commenter in the product this replaces -- `authority()` is `Editor`, and
/// `replace_annotation_authorized` enforces the same rung a second time
/// itself, because `resolved=true` on a `kind="suggestion"` row is exactly
/// the case it raises its own check for.
pub struct AcceptSuggestion {
    catalog: Arc<PostgresCatalog>,
    document_id: Uuid,
    comment_id: Uuid,
    proposal_id: Uuid,
    base_frontiers: Vec<u8>,
    tip_frontiers: Vec<u8>,
    branch_bytes: Vec<u8>,
    decided_by: String,
    actor: MutationAuthorization,
    request_id: Uuid,
}

impl AcceptSuggestion {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        catalog: Arc<PostgresCatalog>,
        document_id: Uuid,
        comment_id: Uuid,
        proposal_id: Uuid,
        base_frontiers: Vec<u8>,
        tip_frontiers: Vec<u8>,
        branch_bytes: Vec<u8>,
        decided_by: String,
        actor: MutationAuthorization,
        request_id: Uuid,
    ) -> Self {
        Self {
            catalog,
            document_id,
            comment_id,
            proposal_id,
            base_frontiers,
            tip_frontiers,
            branch_bytes,
            decided_by,
            actor,
            request_id,
        }
    }
}

/// What accepting a suggestion answers with. `resolved_in` is
/// `docs/protocol/room-v2.md`'s "the `source_sequence` of that row, which is
/// the only durable name the state has now that there are no version ids" --
/// the row §7 step 4 wrote alongside the merge, not a field on `Comment`
/// itself, because nothing about a comment's own row carries the log
/// position its own *acceptance* landed in.
pub struct Accepted {
    pub comment: Comment,
    pub resolved_in: i64,
}

impl SequencerCommand for AcceptSuggestion {
    type Output = Accepted;

    fn name(&self) -> &'static str {
        "accept"
    }

    fn authority(&self) -> Rung {
        Rung::Editor
    }

    // §7.2: a retry with the same `request_id` finds the `document_labels`
    // row `transact` wrote for the merge and returns without merging twice.
    fn replay(&mut self) -> BoxFuture<'_, Result<Option<Self::Output>, CommandError>> {
        Box::pin(async move {
            let Some(label) = self
                .catalog
                .label_by_request(self.document_id, self.request_id)
                .await?
            else {
                return Ok(None);
            };
            let row =
                find_annotation_committed(&self.catalog, self.document_id, self.comment_id).await?;
            let comment = annotation_row_to_comment(&row, Vec::new()).map_err(|error| {
                CommandError::Storage(postgres::Error::Invalid(error.to_string()))
            })?;
            Ok(Some(Accepted {
                comment,
                resolved_in: label.source_sequence,
            }))
        })
    }

    fn evaluate(&mut self, head: &Head<'_>) -> Result<Option<PreparedSource>, CommandError> {
        let base = Frontiers::decode(&self.base_frontiers)
            .map_err(|error| CommandError::Conflict(format!("invalid proposal base: {error}")))?;
        let tip = Frontiers::decode(&self.tip_frontiers)
            .map_err(|error| CommandError::Conflict(format!("invalid proposal tip: {error}")))?;
        let proposal = crate::room::proposals::Proposal {
            base,
            tip: tip.clone(),
        };
        let reviewer = fresh_peer();
        head.prepare(self.request_id.as_u128() as i64, |draft| {
            crate::room::proposals::resolve(
                draft,
                &proposal,
                &self.branch_bytes,
                &HashSet::new(),
                &tip,
                reviewer,
            )
            .map(|_diff| ())
            .map_err(|error| error.to_string())
        })
        .map_err(CommandError::Conflict)
    }

    fn transact<'a>(
        &'a mut self,
        tx: &'a mut Transaction<'static, Postgres>,
        evidence: &'a Evidence,
    ) -> BoxFuture<'a, Result<Self::Output, CommandError>> {
        Box::pin(async move {
            let after_frontier = evidence
                .after_frontier
                .clone()
                .ok_or_else(|| CommandError::Conflict("accept produced no source".into()))?;
            self.catalog
                .decide_proposal_hunk(
                    tx,
                    self.document_id,
                    self.proposal_id,
                    0,
                    true,
                    &self.decided_by,
                    &self.tip_frontiers,
                    None,
                    1,
                )
                .await
                .map_err(CommandError::from)?;
            self.catalog
                .resolve_proposal(tx, self.proposal_id, &self.decided_by)
                .await
                .map_err(CommandError::from)?;
            let existing =
                find_annotation(&self.catalog, tx, self.document_id, self.comment_id).await?;
            let input = replay_of(&existing)?;
            self.catalog
                .replace_annotation_authorized(tx, self.comment_id, input, true, &self.actor)
                .await?;
            let tree_digest = evidence
                .after_digest
                .as_deref()
                .and_then(|digest| hex::decode(digest).ok())
                .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok());
            self.catalog
                .insert_label(
                    tx,
                    &NewLabel {
                        id: Uuid::new_v4(),
                        document_id: self.document_id,
                        source_sequence: evidence.source_sequence,
                        vector: evidence.vector.clone(),
                        frontier: after_frontier,
                        tree_digest,
                        label: None,
                        reason: "accept".into(),
                        request_id: Some(self.request_id),
                        author_account_id: self.actor.account_id,
                        author_label: self.decided_by.clone(),
                    },
                )
                .await
                .map_err(CommandError::from)?;
            let row = find_annotation(&self.catalog, tx, self.document_id, self.comment_id).await?;
            let comment = annotation_row_to_comment(&row, Vec::new()).map_err(|error| {
                CommandError::Storage(postgres::Error::Invalid(error.to_string()))
            })?;
            Ok(Accepted {
                comment,
                resolved_in: evidence.source_sequence,
            })
        })
    }
}

/// Declines a suggestion. Unlike accept, this produces no source: the branch
/// is simply never merged, so there is nothing to prepare in `evaluate`. An
/// editor decision, for the same reason `AcceptSuggestion` is.
pub struct RejectSuggestion {
    catalog: Arc<PostgresCatalog>,
    document_id: Uuid,
    comment_id: Uuid,
    proposal_id: Uuid,
    tip_frontiers: Vec<u8>,
    decided_by: String,
    actor: MutationAuthorization,
}

impl RejectSuggestion {
    pub fn new(
        catalog: Arc<PostgresCatalog>,
        document_id: Uuid,
        comment_id: Uuid,
        proposal_id: Uuid,
        tip_frontiers: Vec<u8>,
        decided_by: String,
        actor: MutationAuthorization,
    ) -> Self {
        Self {
            catalog,
            document_id,
            comment_id,
            proposal_id,
            tip_frontiers,
            decided_by,
            actor,
        }
    }
}

impl SequencerCommand for RejectSuggestion {
    type Output = Comment;

    fn name(&self) -> &'static str {
        "reject"
    }

    fn authority(&self) -> Rung {
        Rung::Editor
    }

    fn evaluate(&mut self, _head: &Head<'_>) -> Result<Option<PreparedSource>, CommandError> {
        Ok(None)
    }

    fn transact<'a>(
        &'a mut self,
        tx: &'a mut Transaction<'static, Postgres>,
        _evidence: &'a Evidence,
    ) -> BoxFuture<'a, Result<Self::Output, CommandError>> {
        Box::pin(async move {
            self.catalog
                .decide_proposal_hunk(
                    tx,
                    self.document_id,
                    self.proposal_id,
                    0,
                    false,
                    &self.decided_by,
                    &self.tip_frontiers,
                    None,
                    1,
                )
                .await
                .map_err(CommandError::from)?;
            self.catalog
                .resolve_proposal(tx, self.proposal_id, &self.decided_by)
                .await
                .map_err(CommandError::from)?;
            let existing =
                find_annotation(&self.catalog, tx, self.document_id, self.comment_id).await?;
            let input = replay_of(&existing)?;
            self.catalog
                .replace_annotation_authorized(tx, self.comment_id, input, true, &self.actor)
                .await?;
            let row = find_annotation(&self.catalog, tx, self.document_id, self.comment_id).await?;
            annotation_row_to_comment(&row, Vec::new())
                .map_err(|error| CommandError::Storage(postgres::Error::Invalid(error.to_string())))
        })
    }
}

/// One suggestion offered as part of an assistant's batch, before it is
/// checked against the head.
///
/// `id` is the caller's, not minted here, because a create's idempotency is
/// its client-chosen primary key (§7.2): a batch retried after a lost
/// response inserts nothing a second time, it just reads back what is
/// already there. It also names the suggestion's proposal -- see
/// `AgentSuggestionBatch::transact` -- so the two rows a suggestion is made
/// of share one identity rather than two that a partial retry could let
/// drift apart.
#[derive(Clone, Debug)]
pub struct BatchSuggestion {
    pub id: Uuid,
    pub exact: String,
    pub prefix: String,
    pub suffix: String,
    pub proposed: String,
    pub body: String,
}

/// Adds an assistant's suggestions against one head, each independently: a
/// bad or stale anchor is that item's own refusal rather than the whole
/// batch's. Every item that does succeed is anchored to the very same head,
/// because the batch is one command and evaluates under one lock (§7 step 2).
pub struct AgentSuggestionBatch {
    catalog: Arc<PostgresCatalog>,
    document_id: Uuid,
    items: Vec<BatchSuggestion>,
    creator: String,
    author_account_id: Option<Uuid>,
    author_key: String,
    actor: MutationAuthorization,
    cap_exact: usize,
    prepared: Vec<Option<PreparedBranch>>,
}

impl AgentSuggestionBatch {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        catalog: Arc<PostgresCatalog>,
        document_id: Uuid,
        items: Vec<BatchSuggestion>,
        config: &Configuration,
        creator: String,
        author_account_id: Option<Uuid>,
        author_key: String,
        actor: MutationAuthorization,
    ) -> Result<Self, String> {
        if items.is_empty() {
            return Err("a suggestion batch cannot be empty".into());
        }
        if items.len() > 100 {
            return Err("a suggestion batch may contain at most 100 items".into());
        }
        Ok(Self {
            catalog,
            document_id,
            items,
            creator,
            author_account_id,
            author_key,
            actor,
            cap_exact: config.caps.exact,
            prepared: Vec::new(),
        })
    }
}

/// One item's outcome: what got made, or why it did not.
pub enum BatchItemResult {
    Created(Box<Comment>),
    Refused(String),
}

impl SequencerCommand for AgentSuggestionBatch {
    type Output = Vec<BatchItemResult>;

    fn name(&self) -> &'static str {
        "agent-suggestion-batch"
    }

    fn authority(&self) -> Rung {
        Rung::Commenter
    }

    fn evaluate(&mut self, head: &Head<'_>) -> Result<Option<PreparedSource>, CommandError> {
        self.prepared = self
            .items
            .iter()
            .map(|item| {
                let quote = Quote {
                    exact: &item.exact,
                    prefix: &item.prefix,
                    suffix: &item.suffix,
                };
                build_suggestion_branch(head.doc(), &quote, self.cap_exact, &item.proposed).ok()
            })
            .collect();
        Ok(None)
    }

    fn transact<'a>(
        &'a mut self,
        tx: &'a mut Transaction<'static, Postgres>,
        evidence: &'a Evidence,
    ) -> BoxFuture<'a, Result<Self::Output, CommandError>> {
        Box::pin(async move {
            let prepared = std::mem::take(&mut self.prepared);
            let mut results = Vec::with_capacity(prepared.len());
            for (item, slot) in self.items.iter().zip(prepared) {
                let Some(PreparedBranch {
                    target,
                    base,
                    tip,
                    bytes: branch_bytes,
                    peer,
                }) = slot
                else {
                    results.push(BatchItemResult::Refused(
                        "that passage is not in this document's source".into(),
                    ));
                    continue;
                };
                let anchor = OriginalAnchor {
                    source_sequence: evidence.source_sequence,
                    frontier: evidence.before_frontier.clone(),
                    target,
                };
                // The proposal and the annotation that discusses it share one
                // id, the caller's own: a replay finds both already there by
                // that id and opens no second branch beside the first (§7.2).
                let row = self
                    .catalog
                    .put_suggestion_authorized(
                        tx,
                        item.id,
                        NewAnnotation {
                            document_id: self.document_id,
                            kind: "suggestion".into(),
                            body: item.body.clone(),
                            author_account_id: self.author_account_id,
                            author_key: self.author_key.clone(),
                            author_label: self.creator.clone(),
                            color: None,
                            proposal_id: Some(item.id),
                            original_anchor: anchor,
                            presentation: PresentationContext::default(),
                            render_digest: None,
                            attachment: None,
                        },
                        NewProposal {
                            document_id: self.document_id,
                            id: item.id,
                            author: self.creator.clone(),
                            author_peer: peer as i64,
                            base_frontiers: base,
                            tip_frontiers: tip,
                            branch_bytes,
                        },
                        &self.actor,
                    )
                    .await;
                match row {
                    Ok(row) => match annotation_row_to_comment(&row, Vec::new()) {
                        Ok(comment) => results.push(BatchItemResult::Created(Box::new(comment))),
                        Err(error) => results.push(BatchItemResult::Refused(error.to_string())),
                    },
                    Err(error) => results.push(BatchItemResult::Refused(error.to_string())),
                }
            }
            Ok(results)
        })
    }
}

/// A peer id for a branch that nothing else will use, minted the same way
/// `room::proposals::fresh_peer` does but for a command a reviewer's decision
/// authors as the deployment rather than as any editor's own socket.
fn fresh_peer() -> loro::PeerID {
    let bytes = crate::auth::random_bytes(8);
    let mut id = [0u8; 8];
    id.copy_from_slice(&bytes);
    loro::PeerID::from_le_bytes(id) | 1
}

// -- broadcasting ---------------------------------------------------------

impl Room {
    /// Adds the same caller-specific controls to a newly-created comment
    /// event that a hello or REST snapshot carries.
    pub async fn comment_event_for(&self, payload: &Value, author: &str, is_owner: bool) -> Value {
        let kind = payload.get("type").and_then(Value::as_str);
        if !matches!(kind, Some("comment" | "refine" | "reply")) {
            return payload.clone();
        }
        let id = match kind {
            Some("comment" | "refine") => payload
                .get("comment")
                .and_then(|comment| comment.get("id"))
                .and_then(Value::as_str),
            Some("reply") => payload.get("comment_id").and_then(Value::as_str),
            _ => None,
        };
        let Some(id) = id else {
            return if is_owner {
                payload.clone()
            } else {
                json!({"type": "annotation-redacted"})
            };
        };
        let comments = self.comments().await.unwrap_or_default();
        let Some(comment) = comments.iter().find(|comment| comment.id == id) else {
            return if is_owner {
                payload.clone()
            } else {
                json!({"type": "annotation-redacted"})
            };
        };
        if !is_owner && !visible_to_reader(comment) {
            return json!({"type": "annotation-redacted"});
        }
        let view = CommentView::for_viewer(comment, author, is_owner);
        let mut event = payload.clone();
        if let Ok(value) = serde_json::to_value(view) {
            event["comment"] = value;
        }
        event
    }

    /// Relays a comment event to everyone else in the room, in the view each
    /// peer is entitled to. An editing suggestion is part of the project an
    /// editor peer has open; a reader peer sees only what the public
    /// annotation channel may carry, which for that comment is a redaction.
    pub async fn broadcast_comment_event(&self, skip: Option<u64>, payload: &Value) {
        let editors = self.comment_event_for(payload, "", true).await;
        self.broadcast_editors_except(skip, &editors).await;
        let readers = self.comment_event_for(payload, "", false).await;
        self.broadcast_readers_except(skip, &readers).await;
    }

    /// The whole list, for a change no single event describes: an agent's
    /// batch adds, edits and deletes in one act.
    pub async fn broadcast_comment_snapshot(&self, digest: i64) {
        let editor_comments = match self.snapshot_for("", true).await {
            Ok(comments) => comments,
            Err(error) => {
                self.broadcast(&json!({"type": "error", "message": error.to_string(), "retryable": error.is_temporary()})).await;
                return;
            }
        };
        let editors = json!({"type": "comments", "comment_digest": digest,
            "comments": editor_comments});
        self.broadcast_editors_except(None, &editors).await;
        let reader_comments = match self.snapshot_for("", false).await {
            Ok(comments) => comments,
            Err(error) => {
                self.broadcast_readers_except(None, &json!({"type": "error", "message": error.to_string(), "retryable": error.is_temporary()})).await;
                return;
            }
        };
        let readers = json!({"type": "comments", "comment_digest": digest,
            "comments": reader_comments});
        self.broadcast_readers_except(None, &readers).await;
    }
}
