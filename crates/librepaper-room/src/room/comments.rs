//! The comments: what one is, how a submission from a reader becomes one, and
//! the anchor that ties it to a passage.
//!
//! Every mutation here is a [`SequencerCommand`]:
//! `evaluate` runs synchronously against the sequencer's cached document and
//! decides what the write means (where a quoted passage is), and `transact` runs inside the fenced
//! transaction that also wrote the log row naming the state `evaluate` saw.
//! The comment and the source it quotes are one transaction because they are
//! written by the same call.
//!
//! `evaluate` has no database access -- it runs with the sequencer's lock
//! held and nothing else may run meanwhile, which is precisely what makes it
//! safe to trust. So a command that needs to know something about an
//! *existing* row (a comment being resolved, edited or deleted) is
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
//! that also has to enforce "the comment's own author, not just any
//! commenter" reads the existing row itself first and refuses before it
//! writes, the same way `DeleteComment` does; that is narrower than what
//! the catalogue checks and the catalogue has no way to express it.

use std::collections::HashMap;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use loro::LoroDoc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Digest;
use sqlx::{Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use librepaper_base::config::Configuration;
use librepaper_base::util::clean;
use librepaper_document::document::{anchors, session};
use librepaper_engine::log::sequencer::{
    Command as SequencerCommand, CommandError, Evidence, Head, PreparedSource, Rung,
};
use librepaper_engine::storage::postgres::{
    self, AnnotationRecord, AnnotationState, Authority, NewAnnotation, NewReply, PostgresCatalog,
    ReplyRecord,
};

use super::locate::{self, Quote};
use super::source_change::client_seq_of;
use super::{Room, WriteError};
use librepaper_engine::storage::annotation::{
    CommentTarget, DerivedAttachment, OriginalAnchor, PresentationContext, SourceTextTarget,
};

// -- the model ---------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Reply {
    pub id: String,
    pub body: String,
    pub creator: String,
    pub created: String,
    /// When the author last changed the body; absent until the first edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited: Option<String>,
    /// Who wrote this reply, checked server-side and never shown. Only
    /// what [`Reply::mine`] is derived from.
    #[serde(default, skip_serializing)]
    pub author: String,
    /// Whether the viewer wrote this reply. Set when a view is made for a
    /// caller; false on a row not yet shown to anyone.
    #[serde(default)]
    pub mine: bool,
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
    /// and never written again: see [`librepaper_engine::storage::annotation`].
    ///
    /// Every stored comment has one -- the persistence layer refuses a
    /// comment that does not. It is optional here because the view served to
    /// a reader has it taken out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_anchor: Option<OriginalAnchor>,
    /// Where that passage is in the projection this comment was served
    /// against. Derived from the comment's anchor record each time it is
    /// read, keyed by the projection digest, and never stored: never a second
    /// opinion about what the comment is about.
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

    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub creator: String,
    #[serde(default)]
    pub created: String,
    /// When the author last changed the body; absent until the first edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited: Option<String>,
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

    /// The optimistic-concurrency token an agent echoes back as
    /// `expected_version`.
    ///
    /// Deliberately not a digest of the whole serialized `Comment`, which is
    /// what it used to be: a `Comment` now carries a *page* of its thread, so
    /// that digest would have depended on how many replies the caller's page
    /// happened to load and a `reply` would have raced its own version check.
    /// This names the row and the thread instead -- the row's own mutable
    /// fields, how many replies it has, and when the newest of them last
    /// changed -- so the same comment produces the same token from a page, a
    /// single-row read or an agent window, and any create, edit, resolve,
    /// delete or reply changes it.
    pub fn version(&self) -> String {
        super::catalog::request_digest(&json!({
            "id": self.id,
            "body": self.body,
            "motivation": self.motivation,
            "color": self.color,
            "resolved": self.resolved,
            "resolved_at": self.resolved_at,
            "created": self.created,
            "creator": self.creator,
            "render_digest": self.render_digest,
            "presentation": self.presentation,
            "replies": self.reply_total,
            "replies_changed": self.thread_changed_micros,
        }))
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

/// One comment mutation, with everything every audience has to be told
/// already read.
///
/// Typed rather than three independent passes over the same JSON: which
/// audience gets a row, which gets a redaction, and which counts go with
/// it are decisions about the mutation, and making them once is what keeps
/// the query count from growing with the number of audiences.
pub struct CommentEvent {
    payload: Value,
    /// Whether this kind of event carries a comment row at all. A `delete`
    /// or a `resolve` names a comment and sends no row; `carries_comment`
    /// tells that apart from "there was a row and this audience may not see
    /// it", which is a redaction.
    carries_comment: bool,
    editor: Option<Comment>,
    reader: Option<Comment>,
    state: Option<librepaper_engine::storage::postgres::AnnotationState>,
    /// The attribution key of the reply a `reply` event announces, taken
    /// off the payload so it is never sent. Only the sender's own view
    /// uses it, to mark that reply `mine`.
    reply_author: String,
}

impl CommentEvent {
    fn bare(payload: Value) -> Self {
        Self {
            reply_author: String::new(),
            payload,
            carries_comment: false,
            editor: None,
            reader: None,
            state: None,
        }
    }

    /// This event as one audience is entitled to see it.
    ///
    /// `author` decides only `mine` and `deletable`, which are derived from
    /// the row already read: a second caller asking for their own view costs
    /// no query.
    pub fn view_for(&self, author: &str, is_owner: bool) -> Value {
        let comment = if is_owner { &self.editor } else { &self.reader };
        let mut event = match (self.carries_comment, comment) {
            (true, Some(comment)) => {
                let mut event = self.payload.clone();
                if let Ok(value) =
                    serde_json::to_value(CommentView::for_viewer(comment, author, is_owner))
                {
                    event["comment"] = value;
                }
                event
            }
            // A row this audience may not see, or one that is no longer
            // there. An editor still gets the event; a reader is told only
            // that the annotation channel moved.
            (true, None) if !is_owner => json!({"type": "annotation-redacted"}),
            _ => self.payload.clone(),
        };
        if !author.is_empty() && self.reply_author == author {
            if let Some(reply) = event.get_mut("reply") {
                reply["mine"] = json!(true);
            }
        }
        if let Some(state) = &self.state {
            event["state"] = comment_state_json(state);
        }
        event
    }
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
        let deletable = Moderation::may(
            CommentAction::Delete,
            comment,
            &Moderator {
                key: author,
                editor: is_owner,
            },
        );
        let mut comment = comment.clone();
        for reply in &mut comment.replies {
            reply.mine = !author.is_empty() && reply.author == author;
        }
        if !is_owner {
            comment.original_anchor = None;
            comment.attachment = None;
            if comment.render_digest.is_empty() {
                comment.presentation = PresentationContext::default();
            }
        }
        CommentView {
            comment,
            mine,
            deletable,
        }
    }
}

/// A moderation action on an existing comment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommentAction {
    Delete,
    Resolve,
}

/// Who is asking to moderate: their attribution key (empty when they have
/// none) and whether they hold the editor rung on the document.
#[derive(Clone, Copy, Debug)]
pub struct Moderator<'a> {
    pub key: &'a str,
    pub editor: bool,
}

impl<'a> From<&'a CommentAuthor> for Moderator<'a> {
    fn from(author: &'a CommentAuthor) -> Self {
        Self {
            key: &author.key,
            editor: author.is_owner,
        }
    }
}

/// The one moderation rule: an editor may delete or resolve any comment, and
/// an author may delete or resolve their own. Every path that decides this
/// asks here, and the identity compared is the attribution key.
pub struct Moderation;

impl Moderation {
    pub fn may(action: CommentAction, comment: &Comment, who: &Moderator<'_>) -> bool {
        Self::may_by(action, &comment.author, who)
    }

    /// The same test against a stored row's author key, for a command that
    /// holds the row and not a built `Comment`. `Comment::author` is copied
    /// from that column and nowhere else, so the two never differ.
    pub fn may_by(action: CommentAction, author: &str, who: &Moderator<'_>) -> bool {
        match action {
            CommentAction::Delete | CommentAction::Resolve => {
                who.editor || (!who.key.is_empty() && author == who.key)
            }
        }
    }
}

/// Why an action on an existing comment was refused: a stable
/// machine-readable code and a sentence for the caller. The transport that
/// asked decides how to present it.
#[derive(Debug)]
pub struct ActionRefusal {
    pub code: &'static str,
    pub message: String,
}

impl ActionRefusal {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

pub fn validate_existing_action(
    action: &str,
    comment_id: &str,
    comment: Option<&Comment>,
    supplied_version: &str,
    current_version: Option<&str>,
    who: &Moderator<'_>,
) -> Result<(), ActionRefusal> {
    if comment_id.is_empty() {
        return Err(ActionRefusal::new(
            "invalid_params",
            "comment_id is required for this action",
        ));
    }
    let comment =
        comment.ok_or_else(|| ActionRefusal::new("not_found", "comment does not exist"))?;
    if supplied_version.is_empty() {
        return Err(ActionRefusal::new(
            "invalid_params",
            "expected_version is required for this action",
        ));
    }
    if current_version != Some(supplied_version) {
        return Err(ActionRefusal::new("conflict", "comment version changed"));
    }
    let action = match action {
        "delete" => Some(CommentAction::Delete),
        "resolve" => Some(CommentAction::Resolve),
        _ => None,
    };
    match action {
        Some(action) if !Moderation::may(action, comment, who) => Err(ActionRefusal::new(
            "permission_changed",
            "only the comment author or an editor may change this comment",
        )),
        _ => Ok(()),
    }
}

fn kind_of(motivation: &str) -> &'static str {
    match motivation {
        "highlighting" => "highlight",
        _ => "comment",
    }
}

fn motivation_of(kind: &str) -> String {
    match kind {
        "highlight" => "highlighting",
        _ => "commenting",
    }
    .to_string()
}

fn format_time(value: time::OffsetDateTime) -> String {
    librepaper_base::util::format_unix(value.unix_timestamp())
}

/// The text paired with a stable file id. Paths label files, but two supported
/// texts may share a raw path until projection gives them distinct names.
pub(super) fn text_for_file_id(doc: &LoroDoc, file_id: &str) -> Option<String> {
    let paths = doc.get_map(session::PATHS);
    if !matches!(
        paths.get(file_id),
        Some(loro::ValueOrContainer::Value(loro::LoroValue::String(_)))
    ) {
        return None;
    }
    match doc.get_map(session::FILES).get(file_id)? {
        loro::ValueOrContainer::Container(loro::Container::Text(text)) => Some(text.to_string()),
        _ => None,
    }
}

/// The files a passage could have come from, in path order.
fn source_files(doc: &LoroDoc) -> Vec<(String, String, String)> {
    let paths = session::paths_of(doc);
    let mut files: Vec<(String, String, String)> = paths
        .into_iter()
        .filter_map(|(id, path)| text_for_file_id(doc, &id).map(|text| (id, path, text)))
        .collect();
    files.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
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
        body: row.body.clone(),
        creator: row.author_label.clone(),
        created: format_time(row.created_at),
        edited: row.edited_at.map(format_time),
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
        edited: row.edited_at.map(format_time),
        author: row.author_key,
        mine: false,
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
// `docs/dev/protocol/comments-v1.md` for the contract this implements.

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
/// a position: a cursor presented to a different document or a different
/// thread is refused outright.
///
/// It is not a capability. Authorization runs in full on every page
/// request, before this is even decoded, so holding one buys no access.
#[derive(Serialize, Deserialize)]
struct WireCursor {
    v: u8,
    d: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    t: Option<Uuid>,
    at: i64,
    id: Uuid,
}

pub fn encode_cursor(document_id: Uuid, thread: Option<Uuid>, at: Position) -> String {
    use base64::Engine;
    let wire = WireCursor {
        v: 1,
        d: document_id,
        t: thread,
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
) -> Result<Position, WriteError> {
    use base64::Engine;
    let refuse = || WriteError::Invalid("that pagination cursor is not for this request".into());
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw)
        .map_err(|_| refuse())?;
    let wire: WireCursor = serde_json::from_slice(&bytes).map_err(|_| refuse())?;
    if wire.v != 1 || wire.d != document_id || wire.t != thread {
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
        .annotation_sizes_in_transaction(&mut tx, document_id, cursor, limit as i64 + 1)
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
        .annotations_in_transaction(&mut tx, document_id, &ids)
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
            encode_cursor(document_id, Some(row.id), at)
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
// Not under `cfg(test)`: `Room::all_comments`, which the server tests use, calls it.
pub(crate) async fn walk_all(
    catalog: &PostgresCatalog,
    document_id: Uuid,
) -> Result<Vec<Comment>, WriteError> {
    let mut out: Vec<Comment> = Vec::new();
    let mut after: Option<Position> = None;
    loop {
        let page = page(catalog, document_id, after, COMMENT_PAGE_MAX).await?;
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
/// broadcasts, an agent's version check. None of them ever needed the document's other comments,
/// and the read that used to give them all of it is gone.
pub async fn one(
    catalog: &PostgresCatalog,
    document_id: Uuid,
    id: Uuid,
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
    comment.reply_cursor = preview
        .next
        .map(|at| encode_cursor(document_id, Some(id), at));
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

// -- semantic commands ---------------------------------------------------

/// Who is writing, as every annotation command needs them.
///
/// Not another principal model, and not a widening of one: these are exactly
/// the answers a `server::Viewer` already gives, kept distinct
/// because they *are* distinct, and grouped only so they stop travelling as
/// four positional arguments in an order nothing but the compiler enforced.
///
/// - `creator` is the display name a comment is signed with, which the
///   server computes and never takes from a client.
/// - `account_id` is the account a signed-in comment belongs to.
/// - `key` is the attribution key, checked server-side and never shown.
/// - `authorization` carries the session generation, the deployment's policy
///   ceiling and the link that bounds it, and is re-checked by the catalogue
///   at commit time. That check is the one that matters and this changes
///   nothing about it.
/// - `is_owner` is this caller's rung *on this document*, which is a
///   different question from the deployment ceiling in `authorization`: it
///   is what decides moderation -- deleting somebody else's comment.
#[derive(Clone, Debug)]
pub struct CommentAuthor {
    pub creator: String,
    pub account_id: Option<Uuid>,
    pub key: String,
    pub authorization: Authority,
    pub is_owner: bool,
}

impl CommentAuthor {
    pub fn new(
        creator: impl Into<String>,
        account_id: Option<Uuid>,
        key: impl Into<String>,
        authorization: Authority,
        is_owner: bool,
    ) -> Self {
        Self {
            creator: creator.into(),
            account_id,
            key: key.into(),
            authorization,
            is_owner,
        }
    }
}

/// What a new comment or highlight needs before it reaches the
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
    actor: Authority,
    exact: String,
    prefix: String,
    suffix: String,
    position: Option<u32>,
    whole_document: bool,
    /// Set only for a comment made against a rendered page. `evaluate`
    /// refuses with `StaleSelection` when this no longer matches the head
    /// digest (§7.1) rather than anchoring against text the reader never saw.
    expected_render_digest: Option<[u8; 32]>,
    cap_exact: usize,
    // filled in by evaluate, consumed by transact.
    resolved_target: Option<CommentTarget>,
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
        author: &CommentAuthor,
        exact: &str,
        prefix: &str,
        suffix: &str,
        position: Option<i64>,
        whole_document: bool,
        color: Option<&str>,
        expected_render_digest: Option<[u8; 32]>,
    ) -> Result<Self, String> {
        let body = clean(body, config.caps.body).trim().to_string();
        let motivation = config.allowed_motivation(motivation);
        // A highlight is the passage itself and needs no words. Everything
        // else is a remark, and a remark with no words is nothing.
        if body.is_empty() && motivation != "highlighting" {
            return Err("comment body is required".into());
        }
        let mut creator = clean(&author.creator, config.caps.creator)
            .trim()
            .to_string();
        if creator.is_empty() {
            creator = "Anonymous".to_string();
        }
        let color = match color {
            Some(raw) if !raw.trim().is_empty() => {
                Some(valid_color(Some(raw)).ok_or("color must be a #RRGGBB value")?)
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
            author_account_id: author.account_id,
            author_key: author.key.clone(),
            actor: author.authorization.clone(),
            exact: clean(exact, config.caps.exact),
            prefix: clean(prefix, config.caps.context),
            suffix: clean(suffix, config.caps.context),
            position: position
                .filter(|at| *at >= 0)
                .map(|at| at.min(u32::MAX as i64) as u32),
            whole_document,
            expected_render_digest,
            cap_exact: config.caps.exact,
            resolved_target: None,
        })
    }
}

/// A passage comment answers a retry from its row, in `replay`.
///
/// The passage's anchor record is a source batch, so `evaluate` moves the
/// frontier and the source sequence. `put_annotation_authorized` makes a
/// same-id create idempotent by checking the stored evidence, and that
/// evidence now differs by the first attempt's own batch, so without this
/// hook the retry would be refused as a conflict. A whole-document comment
/// prepares nothing and still replays through that check.
///
/// The retry is answered here, before a second record is prepared. It
/// compares the row's content and leaves the evidence out, because a replay
/// cannot reproduce the evidence once the record is in the document. The
/// target kind is compared too, so a whole-document request is never answered
/// by a passage comment that reused its id.
impl SequencerCommand for AddComment {
    type Output = Comment;

    fn name(&self) -> &'static str {
        "comment"
    }

    // A comment or highlight needs only commenter access: it is
    // what a commenter link or an `open` document exists to admit, and it
    // lands in exactly the same `annotations` row an editor's would (§7.1).
    fn authority(&self) -> Rung {
        Rung::Commenter
    }

    fn replay(&mut self) -> BoxFuture<'_, Result<Option<Self::Output>, CommandError>> {
        Box::pin(async move {
            let Some(row) = self.catalog.annotation(self.document_id, self.id).await? else {
                return Ok(None);
            };
            // The kind this request would create. It follows `locate_anchor`,
            // where a whole-document request and a blank quote are both a
            // document.
            let target_kind = if self.whole_document || self.exact.trim().is_empty() {
                "document"
            } else {
                "source_text"
            };
            let same = row.document_id == self.document_id
                && row.kind == kind_of(&self.motivation)
                && row.target_kind == target_kind
                && row.body == self.body
                && row.author_account_id == self.author_account_id
                && row.author_key == self.author_key
                && row.author_label == self.creator
                && row.color == self.color
                && row.render_digest == self.expected_render_digest.map(|digest| digest.to_vec())
                && row.rendered_exact == self.exact
                && row.rendered_prefix == self.prefix
                && row.rendered_suffix == self.suffix
                && row.rendered_position_utf16 == self.position.map(|at| at as i32);
            if !same {
                return Err(CommandError::Conflict(
                    "annotation id was reused with different content".into(),
                ));
            }
            annotation_row_to_comment(&row, Vec::new())
                .map(Some)
                .map_err(|error| CommandError::Storage(postgres::Error::Invalid(error.to_string())))
        })
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
        if source.is_none() && self.motivation == "highlighting" {
            return Err(CommandError::Conflict(
                "a highlight has to be about a passage".into(),
            ));
        }
        // The passage's anchor record is written with the comment, in the same
        // transaction. Its cursors are made on the head before this write,
        // which is the state the range was measured against.
        let prepared = match &source {
            Some(source) => {
                let id = self.id.to_string();
                head.prepare(client_seq_of(self.id), |draft| {
                    if anchors::create_anchor(
                        draft,
                        &source.file_id.0,
                        &id,
                        source.start_utf16 as usize,
                        source.end_utf16 as usize,
                        &source.exact,
                        &source.prefix,
                        &source.suffix,
                    ) {
                        Ok(())
                    } else {
                        Err("the passage could not be anchored".into())
                    }
                })
                .map_err(CommandError::Conflict)?
            }
            None => None,
        };
        self.resolved_target = Some(target);
        Ok(prepared)
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
            // The frontier is the state before this comment's record was
            // written, so the range is measured against exactly this state.
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
            let row = self
                .catalog
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
                        original_anchor: anchor,
                        presentation,
                        render_digest: self.expected_render_digest.map(|digest| digest.to_vec()),
                        attachment: None,
                    },
                    &self.actor,
                    false,
                )
                .await?;
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
    actor: Authority,
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
        author: &CommentAuthor,
    ) -> Result<Self, String> {
        let body = clean(body, config.caps.body).trim().to_string();
        if body.is_empty() {
            return Err("reply body is required".into());
        }
        let mut creator = clean(&author.creator, config.caps.creator)
            .trim()
            .to_string();
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
            author_account_id: author.account_id,
            author_key: author.key.clone(),
            actor: author.authorization.clone(),
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

/// Resolves or reopens a comment. There is no per-annotation version column:
/// resolving is idempotent by construction, so the last write standing is a
/// correct answer either way.
pub struct ResolveComment {
    catalog: Arc<PostgresCatalog>,
    document_id: Uuid,
    comment_id: Uuid,
    resolved: bool,
    author_key: String,
    is_owner: bool,
    actor: Authority,
}

impl ResolveComment {
    pub fn new(
        catalog: Arc<PostgresCatalog>,
        document_id: Uuid,
        comment_id: Uuid,
        resolved: bool,
        author: &CommentAuthor,
    ) -> Self {
        Self {
            catalog,
            document_id,
            comment_id,
            resolved,
            author_key: author.key.clone(),
            is_owner: author.is_owner,
            actor: author.authorization.clone(),
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
            if !Moderation::may_by(
                CommentAction::Resolve,
                &existing.author_key,
                &Moderator {
                    key: &self.author_key,
                    editor: self.is_owner,
                },
            ) {
                return Err(CommandError::Conflict(
                    "you may only resolve your own comments".into(),
                ));
            }
            // One column, changed by one authorized update that answers with
            // what it wrote. Nothing else about the annotation is touched,
            // so nothing else about it can be lost on the way round.
            //
            // `document_id` is in the predicate, not checked afterwards, so
            // naming another document's comment finds nothing -- which is
            // the caller's precondition failing, not a storage failure.
            let resolved_at = self
                .catalog
                .set_annotation_resolved_authorized(
                    tx,
                    self.document_id,
                    self.comment_id,
                    self.resolved,
                    &self.actor,
                )
                .await
                .map_err(|error| match error {
                    postgres::Error::NotFound => CommandError::Conflict("unknown comment".into()),
                    other => CommandError::Storage(other),
                })?;
            Ok(ResolveOutcome {
                comment_id: self.comment_id,
                resolved: resolved_at.is_some(),
                resolved_at: resolved_at.map(format_time),
            })
        })
    }
}

/// Deletes a comment. `is_owner` and `author_key` decide whether the caller
/// may, by [`Moderation`]; the check is made against the row's
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
    actor: Authority,
}

impl DeleteComment {
    pub fn new(
        catalog: Arc<PostgresCatalog>,
        document_id: Uuid,
        comment_id: Uuid,
        author: &CommentAuthor,
    ) -> Self {
        Self {
            catalog,
            document_id,
            comment_id,
            author_key: author.key.clone(),
            is_owner: author.is_owner,
            actor: author.authorization.clone(),
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

    // A comment that has no record (a whole-document remark, or one from
    // before records existed) has nothing to delete here. Otherwise the record
    // goes in the same transaction as the comment, and if the authorship check
    // in `transact` refuses, the prepared source is discarded with it.
    fn evaluate(&mut self, head: &Head<'_>) -> Result<Option<PreparedSource>, CommandError> {
        let id = self.comment_id.to_string();
        if head.doc().get_map(anchors::ANCHORS).get(&id).is_none() {
            return Ok(None);
        }
        head.prepare(client_seq_of(self.comment_id), |draft| {
            draft
                .get_map(anchors::ANCHORS)
                .delete(&id)
                .map_err(|error| error.to_string())
        })
        .map_err(CommandError::Conflict)
    }

    fn transact<'a>(
        &'a mut self,
        tx: &'a mut Transaction<'_, Postgres>,
        _evidence: &'a Evidence,
    ) -> BoxFuture<'a, Result<Self::Output, CommandError>> {
        Box::pin(async move {
            let existing =
                find_annotation(&self.catalog, tx, self.document_id, self.comment_id).await?;
            if !Moderation::may_by(
                CommentAction::Delete,
                &existing.author_key,
                &Moderator {
                    key: &self.author_key,
                    editor: self.is_owner,
                },
            ) {
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

/// Changes the body of a comment or a reply. Only its author may: the
/// document's owner may delete someone else's words but not rewrite them.
/// No history is kept; the row records only when it was last edited.
pub struct EditComment {
    catalog: Arc<PostgresCatalog>,
    document_id: Uuid,
    comment_id: Uuid,
    reply_id: Option<Uuid>,
    body: String,
    author_key: String,
    actor: Authority,
}

impl EditComment {
    pub fn new(
        catalog: Arc<PostgresCatalog>,
        document_id: Uuid,
        comment_id: Uuid,
        reply_id: Option<Uuid>,
        config: &Configuration,
        body: &str,
        author: &CommentAuthor,
    ) -> Result<Self, String> {
        let body = clean(body, config.caps.body).trim().to_string();
        if body.is_empty() {
            return Err("comment body is required".into());
        }
        Ok(Self {
            catalog,
            document_id,
            comment_id,
            reply_id,
            body,
            author_key: author.key.clone(),
            actor: author.authorization.clone(),
        })
    }
}

pub struct EditOutcome {
    pub comment_id: Uuid,
    pub reply_id: Option<Uuid>,
    pub body: String,
    pub edited: String,
}

impl SequencerCommand for EditComment {
    type Output = EditOutcome;

    fn name(&self) -> &'static str {
        "edit"
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
            let refused = || CommandError::Conflict("you may only edit your own comments".into());
            let edited = match self.reply_id {
                None => {
                    let existing =
                        find_annotation(&self.catalog, tx, self.document_id, self.comment_id)
                            .await?;
                    if self.author_key.is_empty() || existing.author_key != self.author_key {
                        return Err(refused());
                    }
                    self.catalog
                        .edit_annotation_body_authorized(
                            tx,
                            self.document_id,
                            self.comment_id,
                            &self.body,
                            &self.actor,
                        )
                        .await?
                }
                Some(reply_id) => {
                    let writer = self
                        .catalog
                        .reply_author_key_in_transaction(
                            tx,
                            self.document_id,
                            self.comment_id,
                            reply_id,
                        )
                        .await?
                        .ok_or_else(|| CommandError::Conflict("unknown reply".into()))?;
                    if self.author_key.is_empty() || writer != self.author_key {
                        return Err(refused());
                    }
                    self.catalog
                        .edit_reply_body_authorized(
                            tx,
                            self.document_id,
                            self.comment_id,
                            reply_id,
                            &self.body,
                            &self.actor,
                        )
                        .await?
                }
            };
            Ok(EditOutcome {
                comment_id: self.comment_id,
                reply_id: self.reply_id,
                body: self.body.clone(),
                edited: format_time(edited),
            })
        })
    }
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
        self.prepare_comment_event(payload)
            .await
            .view_for(author, is_owner)
    }

    /// Everything one comment mutation has to be told with, read once.
    ///
    /// A mutation is announced three times -- to the editors, to the
    /// readers, and back to whoever made it -- and each announcement used to
    /// re-read the comment and the collection counts for itself. Six queries
    /// for two distinct answers, and the number grew with the number of
    /// audiences rather than with what they need to be told apart by.
    ///
    /// Read the editor's row once; a reader view is derived from it with the
    /// same anchor redactions as a page. The collection counts are one read
    /// shared by both audiences. `author` only decides `mine` and
    /// `deletable`, which are derived from the row already in hand.
    pub async fn prepare_comment_event(&self, payload: &Value) -> CommentEvent {
        let kind = payload.get("type").and_then(Value::as_str);
        if !matches!(
            kind,
            Some("comment" | "reply" | "delete" | "resolve" | "edit")
        ) {
            return CommentEvent::bare(payload.clone());
        }
        // Only the three kinds that carry a comment row need it at all.
        let id = match kind {
            Some("comment") => payload
                .get("comment")
                .and_then(|comment| comment.get("id"))
                .and_then(Value::as_str),
            Some("reply") => payload.get("comment_id").and_then(Value::as_str),
            _ => None,
        };
        let mut event = CommentEvent::bare(payload.clone());
        if let Some(fields) = event.payload.as_object_mut() {
            if let Some(Value::String(key)) = fields.remove("reply_author") {
                event.reply_author = key;
            }
        }
        event.carries_comment = id.is_some();
        if let Some(id) = id {
            event.editor = self.event_comment(id).await;
            event.reader = event
                .editor
                .as_ref()
                .map(|comment| CommentView::for_viewer(comment, "", false).comment);
        }
        // Counts describe the whole collection, including rows outside a
        // client's loaded prefix. Never derived from a client delta, and
        // never from the one row above: they are about the collection.
        match self.comment_state().await {
            Ok(state) => event.state = Some(state),
            Err(error) => {
                tracing::warn!("could not read comment counts for {}: {error}", self.slug)
            }
        }
        event
    }

    /// The editor's fully enriched row, or nothing when it is missing.
    /// The reader view is derived from this value above, so every reply and
    /// attachment read is shared between both audience views.
    async fn event_comment(&self, id: &str) -> Option<Comment> {
        #[cfg(test)]
        self.event_comment_reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        match self.comment_by_id(id).await {
            Ok(comment) => comment,
            Err(error) => {
                // A read failure is not "there is no such comment". A
                // broadcast has nowhere to put a reason, so the peer gets
                // what it always got and the reason goes to the log.
                tracing::warn!(
                    "could not read a comment of {} for its event: {error}",
                    self.slug
                );
                None
            }
        }
    }

    /// Relays a comment event to everyone else in the room, in the view each
    /// peer is entitled to. A reader peer sees only what the public
    /// annotation channel may carry.
    pub async fn broadcast_comment_event(&self, skip: Option<u64>, payload: &Value) {
        let event = self.prepare_comment_event(payload).await;
        self.broadcast_prepared(skip, &event).await;
    }

    /// The same, for a caller that has already prepared the event because it
    /// also wants a view of its own.
    pub async fn broadcast_prepared(&self, skip: Option<u64>, event: &CommentEvent) {
        self.broadcast_editors_except(skip, &event.view_for("", true))
            .await;
        self.broadcast_readers_except(skip, &event.view_for("", false))
            .await;
    }

    /// Says the collection moved in a way no single event describes -- an
    /// agent's batch adds, edits and deletes in one act -- and how large it
    /// now is.
    ///
    /// It carries no rows. What used to go out here was every comment on
    /// the document, twice (once redacted for readers); a client now
    /// re-reads the pages it is actually holding.
    pub async fn broadcast_comments_changed(&self) {
        let payload = match self.comment_state().await {
            Ok(state) => json!({"type": "comments-changed", "state": comment_state_json(&state)}),
            Err(error) => json!({"type": "error", "message": error.to_string(),
                "retryable": error.is_temporary()}),
        };
        self.broadcast_editors_except(None, &payload).await;
        self.broadcast_readers_except(None, &payload).await;
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

#[cfg(test)]
mod moderation_tests {
    use super::*;

    fn comment_by(author: &str) -> Comment {
        Comment {
            author: author.into(),
            ..Comment::default()
        }
    }

    fn may(action: CommentAction, author: &str, key: &str, editor: bool) -> bool {
        Moderation::may(action, &comment_by(author), &Moderator { key, editor })
    }

    const BOTH: [CommentAction; 2] = [CommentAction::Delete, CommentAction::Resolve];

    #[test]
    fn an_editor_deletes_and_resolves_anyone_elses_comment() {
        for action in BOTH {
            assert!(may(action, "account:a", "account:editor", true));
        }
    }

    #[test]
    fn an_author_deletes_and_resolves_their_own_comment() {
        for action in BOTH {
            assert!(may(action, "account:a", "account:a", false));
        }
    }

    #[test]
    fn a_commenter_cannot_touch_someone_elses_comment() {
        for action in BOTH {
            assert!(!may(action, "account:a", "account:b", false));
        }
    }

    #[test]
    fn a_reader_with_no_key_can_do_neither() {
        for action in BOTH {
            assert!(!may(action, "account:a", "", false));
            assert!(!may(action, "", "", false));
        }
    }
}
