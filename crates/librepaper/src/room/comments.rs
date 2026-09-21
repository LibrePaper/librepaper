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
    self, AnnotationRecord, AnnotationState, MutationAuthorization, NewAnnotation, NewLabel,
    NewProposal, NewReply, PostgresCatalog, ReplyRecord,
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
    /// This row's own place in its thread's traversal, so a consumer
    /// that stops part-way through a page can continue from exactly the
    /// row it stopped at. Never on the wire: a cursor is, and this is what
    /// one is built from.
    #[serde(skip)]
    pub at: Option<Position>,
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
    /// A bounded prefix of this thread, never the whole of it: at most
    /// [`REPLY_PREVIEW`] on an annotation page, and whatever the byte
    /// budget of that page afforded. The rest is its own traversal.
    #[serde(default)]
    pub replies: Vec<Reply>,
    /// This comment's own place in the document's traversal, for the
    /// same reason as [`Reply::at`].
    #[serde(skip)]
    pub at: Option<Position>,
    /// How many replies this thread actually has, from the catalogue. The
    /// authoritative figure: a client shows this rather than counting
    /// `replies`.
    #[serde(default)]
    pub reply_total: i64,
    /// Where to continue this thread, present exactly when `replies` is a
    /// proper prefix of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_cursor: Option<String>,
    /// The newest reply `updated_at` in this thread, in microseconds. Kept
    /// off the wire and used only by
    /// [`super::agent_comments::comment_version`], so that token describes
    /// the thread rather than however much of it one page carried.
    #[serde(default, skip_serializing)]
    pub thread_changed_micros: i64,
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
        at: Some(Position {
            created: row.created_at,
            id: row.id,
        }),
        reply_total: 0,
        reply_cursor: None,
        thread_changed_micros: 0,
        author: row.author_key.clone(),
    })
}

fn reply_row_to_reply(row: ReplyRecord) -> Reply {
    Reply {
        id: row.id.to_string(),
        body: row.body,
        creator: row.author_label,
        created: format_time(row.created_at),
        at: Some(Position {
            created: row.created_at,
            id: row.id,
        }),
    }
}

// -- bounded paging ------------------------------------------------------
//
// There is no whole-document read here any more, and no room-held list for
// one to fill. Every consumer -- a browser, a socket `hello`, an export, an
// agent -- walks the same keyset traversal a page at a time. See
// `docs/protocol/comments-v1.md` for the contract this implements.

/// How many comments one page carries when a caller does not say, and the
/// most it may ask for. These are transport page sizes, not admission
/// caps: nothing here limits how many comments a document may hold.
pub const COMMENT_PAGE_DEFAULT: usize = 50;
pub const COMMENT_PAGE_MAX: usize = 200;

/// How many replies of each thread an annotation page carries. The rest of
/// a thread is its own traversal, so one comment with a hundred thousand
/// replies cannot defeat the annotation page limit.
pub const REPLY_PREVIEW: usize = 10;

/// The same pair for one thread's own reply traversal.
pub const THREAD_PAGE_DEFAULT: usize = 50;
pub const THREAD_PAGE_MAX: usize = 200;

/// The encoded content one page may carry, spent over the annotations first
/// and then over their reply previews.
///
/// Rows are never read and then measured against this: a sizing query
/// reports each candidate row's stored width first (`SizedRow`), and only
/// the prefix that fits is read in full. A page that cannot fit even its
/// first row returns that row anyway and says `oversize`, because nothing
/// ever refused an oversized comment at write time and a page that returned
/// nothing would strand everything behind it.
pub const PAGE_BYTES_MAX: usize = 512 * 1024;

/// What a row costs beyond its stored bytes: the JSON field names, the
/// formatted timestamps, the UUID spellings and the `Comment`/`Reply`
/// structs themselves. A flat estimate, deliberately generous.
const ROW_OVERHEAD_BYTES: usize = 1024;
const REPLY_OVERHEAD_BYTES: usize = 256;

/// A place in a `(created_at, id)` traversal.
///
/// Both columns are written once and never rewritten, so a row cannot move
/// within this order: that is what makes "strictly after" a traversal that
/// neither repeats nor skips. `id` is the tie-breaker and it is
/// load-bearing -- comments written in one transaction share `now()` to the
/// microsecond.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub created: OffsetDateTime,
    pub id: Uuid,
}

impl Position {
    /// Before every real row, which is what "start at the beginning"
    /// encodes to when a thread's replies have to be asked for from the
    /// front.
    pub fn start() -> Position {
        Position {
            created: OffsetDateTime::UNIX_EPOCH,
            id: Uuid::nil(),
        }
    }

    fn pair(self) -> (OffsetDateTime, Uuid) {
        (self.created, self.id)
    }

    fn micros(self) -> i64 {
        (self.created.unix_timestamp_nanos() / 1_000) as i64
    }

    fn from_micros(micros: i64, id: Uuid) -> Option<Position> {
        OffsetDateTime::from_unix_timestamp_nanos(micros as i128 * 1_000)
            .ok()
            .map(|created| Position { created, id })
    }
}

/// A cursor on the wire. Every field but `at`/`id` is a binding rather than
/// a position: a cursor presented to a different document, a different
/// thread or a different set of query options is refused outright.
///
/// It is not a capability. Authorization runs in full on every page
/// request, before this is even decoded, so holding one buys no access.
#[derive(Serialize, Deserialize)]
struct WireCursor {
    v: u8,
    d: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    t: Option<Uuid>,
    o: String,
    at: i64,
    id: Uuid,
}

/// The query options a cursor is bound to. Only one thing changes which
/// rows are eligible today: whether the caller may see suggestions.
pub fn options_key(suggestions: bool) -> &'static str {
    if suggestions {
        "editor"
    } else {
        "reader"
    }
}

pub fn encode_cursor(
    document_id: Uuid,
    thread: Option<Uuid>,
    suggestions: bool,
    at: Position,
) -> String {
    use base64::Engine;
    let wire = WireCursor {
        v: 1,
        d: document_id,
        t: thread,
        o: options_key(suggestions).to_string(),
        at: at.micros(),
        id: at.id,
    };
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&wire).unwrap_or_default())
}

/// Decodes a cursor, or says why it does not belong to this request.
pub fn decode_cursor(
    raw: &str,
    document_id: Uuid,
    thread: Option<Uuid>,
    suggestions: bool,
) -> Result<Position, WriteError> {
    use base64::Engine;
    let refuse = || WriteError::Invalid("that pagination cursor is not for this request".into());
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw)
        .map_err(|_| refuse())?;
    let wire: WireCursor = serde_json::from_slice(&bytes).map_err(|_| refuse())?;
    if wire.v != 1
        || wire.d != document_id
        || wire.t != thread
        || wire.o != options_key(suggestions)
    {
        return Err(refuse());
    }
    Position::from_micros(wire.at, wire.id).ok_or_else(refuse)
}

/// One page of a document's comments.
pub struct CommentPage {
    pub comments: Vec<Comment>,
    /// Where the next page starts, and `None` when this one ended the
    /// traversal. Never inferred from a short page: the sizing query is
    /// asked for one row more than the page, so `complete` is proof.
    pub next: Option<Position>,
    pub complete: bool,
    /// This page held one row that did not fit the byte budget on its own
    /// and was served regardless, so the collection stays traversable.
    pub oversize: bool,
}

/// One thread's replies, past whatever the annotation page previewed.
pub struct ReplyPage {
    pub replies: Vec<Reply>,
    pub next: Option<Position>,
    pub complete: bool,
    pub total: i64,
    pub oversize: bool,
}

/// Spends a byte budget over sized rows, always admitting at least one.
///
/// Returns how many rows fit and whether the first one had to be admitted
/// over budget.
fn admit(sizes: &[i64], overhead: usize, budget: usize) -> (usize, bool) {
    let mut left = budget;
    let mut taken = 0usize;
    let mut oversize = false;
    for size in sizes {
        let cost = (*size).max(0) as usize + overhead;
        if cost > left {
            if taken == 0 {
                // Nothing ever refused a comment this wide at write time,
                // and a page of nothing would make every row behind it
                // unreachable. So it goes, and the page says so.
                oversize = true;
                taken = 1;
            }
            break;
        }
        left -= cost;
        taken += 1;
    }
    (taken, oversize)
}

/// One page of comments, from `after` forward.
///
/// Three bounded statements: the annotations' widths, the annotations that
/// fit, and their reply previews. They share one short repeatable-read
/// transaction, which ends before the page is returned.
pub async fn page(
    catalog: &PostgresCatalog,
    document_id: Uuid,
    after: Option<Position>,
    limit: usize,
    suggestions: bool,
) -> Result<CommentPage, WriteError> {
    let limit = limit.clamp(1, COMMENT_PAGE_MAX);
    let cursor = after.map(Position::pair);
    let mut tx = catalog
        .pool()
        .begin()
        .await
        .map_err(|e| WriteError::Storage(e.to_string()))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await
        .map_err(|e| WriteError::Storage(e.to_string()))?;
    // One more than the page, so "there is nothing after this" is something
    // the traversal proved rather than inferred from a short read.
    let sizes = catalog
        .annotation_sizes_in_transaction(
            &mut tx,
            document_id,
            cursor,
            limit as i64 + 1,
            suggestions,
        )
        .await?;
    let more_rows = sizes.len() > limit;
    let sizes = &sizes[..sizes.len().min(limit)];
    if sizes.is_empty() {
        return Ok(CommentPage {
            comments: Vec::new(),
            next: None,
            complete: true,
            oversize: false,
        });
    }
    let widths: Vec<i64> = sizes.iter().map(|row| row.bytes).collect();
    let (take, oversize) = admit(&widths, ROW_OVERHEAD_BYTES, PAGE_BYTES_MAX);
    let spent: usize = widths[..take]
        .iter()
        .map(|bytes| (*bytes).max(0) as usize + ROW_OVERHEAD_BYTES)
        .sum();
    let left = PAGE_BYTES_MAX.saturating_sub(spent);

    let ids: Vec<Uuid> = sizes[..take].iter().map(|row| row.id).collect();
    let rows = catalog
        .annotations_in_transaction(&mut tx, document_id, &ids, suggestions)
        .await?;
    let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
    let summaries: HashMap<Uuid, (i64, i64)> = catalog
        .reply_summaries_in_transaction(&mut tx, &ids)
        .await?
        .into_iter()
        .map(|row| (row.annotation_id, (row.replies, row.changed_micros)))
        .collect();

    // The reply previews are chosen the same way the comments were: widths
    // first, then whatever the remaining budget affords, in page order.
    let mut previews = catalog
        .reply_preview_sizes_in_transaction(&mut tx, &ids, REPLY_PREVIEW as i64)
        .await?;
    previews.sort_by_key(|row| (row.created_at, row.id));
    let mut by_thread: HashMap<Uuid, Vec<&postgres::SizedReply>> = HashMap::new();
    for row in &previews {
        by_thread.entry(row.annotation_id).or_default().push(row);
    }
    let mut wanted: Vec<Uuid> = Vec::new();
    let mut reply_budget = left;
    for id in &ids {
        for row in by_thread.get(id).into_iter().flatten() {
            let cost = row.bytes.max(0) as usize + REPLY_OVERHEAD_BYTES;
            if cost > reply_budget {
                reply_budget = 0;
                break;
            }
            reply_budget -= cost;
            wanted.push(row.id);
        }
        if reply_budget == 0 {
            break;
        }
    }
    let mut loaded: HashMap<Uuid, Vec<Reply>> = HashMap::new();
    let mut positions: HashMap<Uuid, Position> = HashMap::new();
    for row in catalog
        .replies_by_id_in_transaction(&mut tx, &wanted)
        .await?
    {
        positions.insert(
            row.annotation_id,
            Position {
                created: row.created_at,
                id: row.id,
            },
        );
        let thread = row.annotation_id;
        loaded
            .entry(thread)
            .or_default()
            .push(reply_row_to_reply(row));
    }

    let mut comments = Vec::with_capacity(rows.len());
    for row in &rows {
        let replies = loaded.remove(&row.id).unwrap_or_default();
        let (total, changed) = summaries.get(&row.id).copied().unwrap_or((0, 0));
        let mut comment = annotation_row_to_comment(row, replies)?;
        comment.reply_total = total;
        comment.thread_changed_micros = changed;
        comment.reply_cursor = ((comment.replies.len() as i64) < total).then(|| {
            let at = positions
                .get(&row.id)
                .copied()
                .unwrap_or_else(Position::start);
            encode_cursor(document_id, Some(row.id), suggestions, at)
        });
        comments.push(comment);
    }

    // Where to continue: after the last row actually returned. When every
    // row the sizing pass chose was deleted between the two statements
    // there is no such row, and the sizing pass's own last position stands
    // in -- an incomplete page must always say where to continue, or the
    // rows behind it are unreachable.
    let last = rows
        .last()
        .map(|row| Position {
            created: row.created_at,
            id: row.id,
        })
        .or_else(|| {
            sizes[..take].last().map(|row| Position {
                created: row.created_at,
                id: row.id,
            })
        });
    // A page cut short by the byte budget has more to give even when the
    // sizing query saw the end of the collection.
    let complete = !more_rows && take == sizes.len();
    Ok(CommentPage {
        comments,
        next: (!complete).then_some(last).flatten(),
        complete,
        oversize,
    })
}

/// One page of one thread's replies, from `after` forward.
pub async fn thread_page(
    catalog: &PostgresCatalog,
    comment_id: Uuid,
    after: Option<Position>,
    limit: usize,
) -> Result<ReplyPage, WriteError> {
    let limit = limit.clamp(1, THREAD_PAGE_MAX);
    let cursor = after.map(Position::pair);
    let mut tx = catalog
        .pool()
        .begin()
        .await
        .map_err(|e| WriteError::Storage(e.to_string()))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await
        .map_err(|e| WriteError::Storage(e.to_string()))?;
    let total = catalog
        .reply_summaries_in_transaction(&mut tx, &[comment_id])
        .await?
        .first()
        .map(|row| row.replies)
        .unwrap_or(0);
    let sizes = catalog
        .thread_reply_sizes_in_transaction(&mut tx, comment_id, cursor, limit as i64 + 1)
        .await?;
    let more_rows = sizes.len() > limit;
    let sizes = &sizes[..sizes.len().min(limit)];
    if sizes.is_empty() {
        return Ok(ReplyPage {
            replies: Vec::new(),
            next: None,
            complete: true,
            total,
            oversize: false,
        });
    }
    let widths: Vec<i64> = sizes.iter().map(|row| row.bytes).collect();
    let (take, oversize) = admit(&widths, REPLY_OVERHEAD_BYTES, PAGE_BYTES_MAX);
    let wanted: Vec<Uuid> = sizes[..take].iter().map(|row| row.id).collect();
    let rows = catalog
        .replies_by_id_in_transaction(&mut tx, &wanted)
        .await?;
    let last = sizes[take - 1];
    let complete = !more_rows && take == sizes.len();
    Ok(ReplyPage {
        replies: rows.into_iter().map(reply_row_to_reply).collect(),
        next: (!complete).then_some(Position {
            created: last.created_at,
            id: last.id,
        }),
        complete,
        total,
        oversize,
    })
}

/// Every comment of a document, by walking every page and every thread.
///
/// Test-only, deliberately: this is the shape that was removed from the
/// server, and a helper that collects would be exactly that shape again if
/// production code could reach it. What it is for is checking that the
/// traversal loses nothing and repeats nothing.
#[cfg(test)]
pub(crate) async fn walk_all(
    catalog: &PostgresCatalog,
    document_id: Uuid,
    suggestions: bool,
) -> Result<Vec<Comment>, WriteError> {
    let mut out: Vec<Comment> = Vec::new();
    let mut after: Option<Position> = None;
    loop {
        let page = page(catalog, document_id, after, COMMENT_PAGE_MAX, suggestions).await?;
        let complete = page.complete;
        let next = page.next;
        for mut comment in page.comments {
            let id = Uuid::parse_str(&comment.id).expect("comment id is a uuid");
            let mut cursor = comment.replies.last().and_then(|reply| reply.at);
            while (comment.replies.len() as i64) < comment.reply_total {
                let more = thread_page(catalog, id, cursor, THREAD_PAGE_MAX).await?;
                if more.replies.is_empty() {
                    break;
                }
                cursor = more.replies.last().and_then(|reply| reply.at);
                comment.replies.extend(more.replies);
            }
            out.push(comment);
        }
        if complete {
            break;
        }
        match next {
            Some(position) => after = Some(position),
            None => break,
        }
    }
    Ok(out)
}

/// One comment by id, with the same bounded reply preview a page gives it.
///
/// This is what every single-comment consumer reads: the event a mutation
/// broadcasts, an agent's version check, a suggestion decision looking up
/// its proposal. None of them ever needed the document's other comments,
/// and the read that used to give them all of it is gone.
pub async fn one(
    catalog: &PostgresCatalog,
    document_id: Uuid,
    id: Uuid,
    suggestions: bool,
) -> Result<Option<Comment>, WriteError> {
    let Some(row) = catalog.annotation(document_id, id).await? else {
        return Ok(None);
    };
    let (total, changed) = catalog
        .reply_summaries(&[id])
        .await?
        .first()
        .map(|row| (row.replies, row.changed_micros))
        .unwrap_or((0, 0));
    let preview = thread_page(catalog, id, None, REPLY_PREVIEW).await?;
    let mut comment = annotation_row_to_comment(&row, preview.replies)?;
    comment.reply_total = total;
    comment.thread_changed_micros = changed;
    // Bound to the caller's own options, like every other cursor: one
    // issued to a reader has to decode on a reader's next request.
    comment.reply_cursor = preview
        .next
        .map(|at| encode_cursor(document_id, Some(id), suggestions, at));
    Ok(Some(comment))
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
    tx: &mut Transaction<'_, Postgres>,
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
        tx: &'a mut Transaction<'_, Postgres>,
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
        tx: &'a mut Transaction<'_, Postgres>,
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
        tx: &'a mut Transaction<'_, Postgres>,
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
        tx: &'a mut Transaction<'_, Postgres>,
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
        tx: &'a mut Transaction<'_, Postgres>,
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
        tx: &'a mut Transaction<'_, Postgres>,
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
        tx: &'a mut Transaction<'_, Postgres>,
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
        tx: &'a mut Transaction<'_, Postgres>,
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
    /// event that a page carries.
    ///
    /// The one comment this event is about is read by id. It used to be
    /// found by scanning the room's resident copy of every comment on the
    /// document, which is the whole-collection dependency this path had no
    /// need of: a `reply` event names its thread and a `comment` event
    /// names itself.
    pub async fn comment_event_for(&self, payload: &Value, author: &str, is_owner: bool) -> Value {
        let kind = payload.get("type").and_then(Value::as_str);
        if !matches!(
            kind,
            Some("comment" | "refine" | "reply" | "delete" | "resolve" | "accept" | "reject")
        ) {
            return payload.clone();
        }
        if kind == Some("delete") {
            if let Some(id) = payload.get("comment_id").and_then(Value::as_str) {
                self.attachments.lock().await.remove(id);
            }
        }
        let mut event = self.comment_event_view(payload, author, is_owner).await;
        // Counts describe the viewer's entire collection, including rows
        // outside their loaded prefix. Never derive them from a client delta.
        match self.comment_state(is_owner).await {
            Ok(state) => event["state"] = comment_state_json(&state),
            Err(error) => log::warn!("could not read comment counts for {}: {error}", self.slug),
        }
        event
    }

    async fn comment_event_view(&self, payload: &Value, author: &str, is_owner: bool) -> Value {
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
        let redacted = || {
            if is_owner {
                payload.clone()
            } else {
                json!({"type": "annotation-redacted"})
            }
        };
        let comment = match self.comment_by_id(id.unwrap_or_default(), is_owner).await {
            Ok(Some(comment)) => comment,
            Ok(None) => return redacted(),
            Err(error) => {
                // A read failure is not "there is no such comment". A
                // broadcast has nowhere to put a reason, so the peer gets
                // what it always got and the reason goes to the log.
                log::warn!(
                    "could not read a comment of {} for its event: {error}",
                    self.slug
                );
                return redacted();
            }
        };
        if !is_owner && !visible_to_reader(&comment) {
            return json!({"type": "annotation-redacted"});
        }
        let view = CommentView::for_viewer(&comment, author, is_owner);
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

    /// Says the collection moved in a way no single event describes -- an
    /// agent's batch adds, edits and deletes in one act -- and how large it
    /// now is.
    ///
    /// It carries no rows. What used to go out here was every comment on
    /// the document, twice (once redacted for readers); a client now
    /// re-reads the pages it is actually holding.
    pub async fn broadcast_comments_changed(&self) {
        for (editors, suggestions) in [(true, true), (false, false)] {
            let state = match self.comment_state(suggestions).await {
                Ok(state) => state,
                Err(error) => {
                    let payload = json!({"type": "error", "message": error.to_string(),
                        "retryable": error.is_temporary()});
                    if editors {
                        self.broadcast_editors_except(None, &payload).await;
                    } else {
                        self.broadcast_readers_except(None, &payload).await;
                    }
                    continue;
                }
            };
            let payload = json!({"type": "comments-changed", "state": comment_state_json(&state)});
            if editors {
                self.broadcast_editors_except(None, &payload).await;
            } else {
                self.broadcast_readers_except(None, &payload).await;
            }
        }
    }
}

/// The `state` block every paged answer carries: authoritative counts, and
/// a token that changes whenever the collection does.
pub fn comment_state_json(state: &AnnotationState) -> Value {
    json!({
        "total": state.total,
        "open": state.open,
        "replies": state.replies,
        "revision": comment_revision(state),
    })
}

/// A short opaque token over the collection's shape and its newest
/// `updated_at`. Two reads that produce the same token saw the same
/// collection; a different token means something was created, deleted,
/// resolved, replied to or edited in between.
pub fn comment_revision(state: &AnnotationState) -> String {
    let mut hash = sha2::Sha256::new();
    for value in [state.total, state.open, state.replies, state.changed_micros] {
        hash.update(value.to_le_bytes());
    }
    hex::encode(&hash.finalize()[..8])
}
