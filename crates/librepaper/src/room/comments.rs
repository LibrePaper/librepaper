//! The comments: what one is, how a submission from a reader becomes one, and
//! the anchor that ties it to a passage.

use super::annotation::{
    AnchorStatus, CheckpointId, FileId, PresentationContext, SourceTextTarget,
};
use super::locate::{self, Quote};
use super::resolve;
use super::*;

// Browser submissions carry a random UUID that remains stable across retries.
// Keeping it as the record ID also lets a reconnect snapshot acknowledge a
// write whose direct response was lost. Older clients may use arbitrary
// temporary labels; those retain server-generated IDs.
pub(super) fn submission_id(value: &str) -> Option<&str> {
    (value.len() == 36
        && value.bytes().enumerate().all(|(at, byte)| {
            if [8, 13, 18, 23].contains(&at) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        }))
    .then_some(value)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Reply {
    pub id: String,
    pub body: String,
    pub creator: String,
    pub created: String,
    /// Who actually posted this reply -- a github: or visitor: key, or "" for a
    /// caller with neither -- so a delete can be restricted to it. Never
    /// serialized: a reply is marshaled directly into broadcasts, snapshots and
    /// REST responses. The catalog stores this private attribution separately.
    #[serde(default, skip_serializing)]
    pub author: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    #[serde(default)]
    pub seq: i64,
    /// What this comment is about, in the document as it stood when it was
    /// made. Written once by the server, from the selection a client sent,
    /// and never written again: see [`super::annotation`].
    ///
    /// Every stored comment has one -- the persistence layer refuses a comment
    /// that does not. It is optional here because the view served to a reader
    /// has it taken out: source identities and source quotations are
    /// editorial, and a rendered reader is not shown them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_anchor: Option<OriginalAnchor>,
    /// Where that passage is in the checkpoint this comment was last served
    /// against. Derived, replaceable, and absent when nothing has resolved it
    /// yet -- never a second opinion about what the comment is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment: Option<DerivedAttachment>,
    /// The page the comment was made on: the words as the render had them,
    /// and which publication that was. Display evidence only.
    #[serde(default, skip_serializing_if = "PresentationContext::is_empty")]
    pub presentation: PresentationContext,
    #[serde(default)]
    pub motivation: String,
    /// The highlight colour a reader chose, as `#rrggbb`. Presentation, but
    /// the reader's own rather than the render's, so it belongs to the
    /// comment and is stored with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Published rendering the reader selected from. Empty only for editor
    /// annotations which are not tied to a rendered publication. Kept beside
    /// the comment rather than inside its presentation because who may see a
    /// comment is decided by it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub publication_id: String,

    /// The proposal this comment offers, when its motivation is `editing`.
    ///
    /// A suggestion is a proposed change with a remark attached, and the change
    /// is a branch like any other (§1.2). This id is the whole of what the
    /// comment knows about it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub proposal: String,

    /// What that proposal wants in place of the passage, and whether it was
    /// taken. **Both are projections and neither is stored.**
    ///
    /// The branch owns the text and `document_proposal_hunks` owns the verdict.
    /// These are filled in when a comment is served, so that a reader, an
    /// export or an agent can see a suggestion without going and assembling it
    /// from the branch itself. Nothing writes them back, and the persistence
    /// layer does not look at them -- the columns that used to hold them are
    /// dropped in migration 0010.
    ///
    /// The distinction is the point of §1.2: one of these being wrong is a
    /// stale display, where before it was a second answer to "was this
    /// accepted" that could disagree with the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub outcome: String,
    /// Batch identifier for assistant suggestions. Empty means this comment
    /// was created independently of a batch.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pass: String,
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
    /// The checkpoint current when it was resolved, beside `resolved_at`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resolved_in: String,
    #[serde(default)]
    pub replies: Vec<Reply>,
    /// Who actually posted this comment: "github:<login>" for a signed-in
    /// caller, "visitor:<sha256 of the visitor token>" for a verified
    /// anonymous browser, or "" for neither (including every seeded example,
    /// which belongs to nobody in particular). Kept out of every client-bound
    /// shape for the same reason as `Reply::author`.
    #[serde(default, skip_serializing)]
    pub author: String,
}

impl Comment {
    /// The checkpoint this comment was made against: what the reviewer was
    /// actually looking at. It lives on the anchor, because that is what it
    /// is a property of, and this is the one way to ask for it -- a second
    /// copy beside it could disagree with it.
    ///
    /// Empty on the redacted view served to a reader, which has no anchor.
    pub fn revision(&self) -> &str {
        self.original_anchor
            .as_ref()
            .map(|anchor| anchor.checkpoint_id.0.as_str())
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

pub(super) fn valid_revision(value: &str) -> bool {
    value.is_empty()
        || (matches!(value.len(), 32 | 64)
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()))
}

/// One proposal in an assistant pass. Validation and placement happen while
/// the room's restore lock and state snapshot are held by
/// [`Room::apply_suggestion_batch`].
#[derive(Clone, Debug)]
pub struct BatchSuggestion {
    pub original_anchor: OriginalAnchor,
    pub proposed: String,
    pub body: String,
}

/// Caller identity and admission metadata for one assistant pass.
pub struct BatchCaller<'a> {
    pub address: &'a str,
    pub author: &'a str,
    pub via: &'a str,
    pub budget: Option<i64>,
    pub creator: &'a str,
}

/// Refusal of the entire assistant pass, before any annotation is written.
#[derive(Clone, Debug)]
pub struct BatchRefusal(pub String);

/// What a caller is shown: every comment field a client ever sees, plus
/// whether this particular caller may delete it.
#[derive(Serialize)]
pub struct CommentView {
    #[serde(flatten)]
    pub comment: Comment,
    /// Whether this comment belongs to the caller. The author key itself
    /// remains private; clients use this to decide which controls to show.
    pub mine: bool,
    pub deletable: bool,
}

impl CommentView {
    /// The one place `mine` and `deletable` are derived from a caller's
    /// identity, so a snapshot (`snapshot_for`, `snapshot_bundle`) and a
    /// broadcast event (`comment_event_for`) can never disagree about what a
    /// given author/owner combination is allowed to see or do. `author` is
    /// empty for an anonymous caller, who is never "mine" on anything and can
    /// only delete when `is_owner` is set.
    pub fn for_viewer(comment: &Comment, author: &str, is_owner: bool) -> CommentView {
        let mine = !author.is_empty() && comment.author == author;
        let deletable = deletable(comment, author, is_owner);
        let mut comment = comment.clone();
        // Source provenance and suggestion proposals are editorial material.
        // A rendered reader may discuss an annotation, but must never receive
        // source file identities, source quotations, or proposed source edits.
        if !is_owner {
            comment.original_anchor = None;
            comment.attachment = None;
            // Everything about a proposed source change: the id, and the two
            // projections of it. A reader sees the remark, not the edit.
            comment.proposal.clear();
            comment.proposed = None;
            comment.outcome.clear();
            comment.resolved_in.clear();
        }
        CommentView {
            comment,
            mine,
            deletable,
        }
    }
}

/// Editorial annotations belong to the editable project, so only an editor
/// may receive them. A nonempty publication id identifies a rendered
/// annotation, including one retained from an earlier publication.
pub fn visible_to_reader(comment: &Comment) -> bool {
    comment.motivation != "editing" && !comment.publication_id.is_empty()
}

/// Rule H's authorization test: the document's owner may delete anything on
/// it, and everyone else only their own -- and "their own" never matches on
/// two callers who both have no author key, which is what an anonymous caller
/// with no visitor cookie and a nobody's-in-particular seeded example both
/// look like.
pub fn deletable(item: &Comment, author: &str, is_owner: bool) -> bool {
    is_owner || (!author.is_empty() && item.author == author)
}

fn source_target(anchor: &OriginalAnchor) -> Option<&SourceTextTarget> {
    anchor.target.source()
}

pub(super) fn path_for_file_id(doc: &loro::LoroDoc, file_id: &FileId) -> Option<String> {
    session::paths_of(doc)
        .into_iter()
        .find_map(|(id, path)| (id == file_id.0).then_some(path))
}

/// The files a passage could have come from, main first.
///
/// Main first because a document is usually one file with the rest included
/// into it, so the file being read is the likeliest answer and the order
/// decides nothing else: a passage that occurs in two files is ambiguous
/// whichever order they are tried in.
fn source_files(doc: &loro::LoroDoc) -> Vec<(String, String, String)> {
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

/// Checks a range somebody else worked out.
///
/// An assistant reads the source itself and sends offsets into it, so there is
/// nothing to locate -- but there is something to verify, and it is not the
/// bounds. It is that the text at the range is the text the caller says is
/// there. A range that passes this is a range into the checkpoint's own text,
/// which is the same guarantee [`anchor_for`] gets by construction.
fn validate_original_anchor(
    config: &Configuration,
    doc: &loro::LoroDoc,
    checkpoint: &str,
    anchor: OriginalAnchor,
) -> Result<OriginalAnchor, &'static str> {
    // The checkpoint is the room's own name for a state of the document, not
    // a shape a client chooses: what matters is that the caller worked against
    // the one this is being written against, not what it looks like.
    if anchor.checkpoint_id.0.is_empty() || anchor.checkpoint_id.0 != checkpoint {
        return Err("a comment needs the current checkpoint provenance");
    }
    let Some(source) = anchor.target.source() else {
        return Ok(anchor);
    };
    if source.file_id.0.is_empty()
        || source.start_utf16 > source.end_utf16
        || source.exact.chars().count() > config.caps.exact
        || source.prefix.chars().count() > config.caps.context
        || source.suffix.chars().count() > config.caps.context
    {
        return Err("that source anchor is not valid");
    }
    let Some(path) = path_for_file_id(doc, &source.file_id) else {
        return Err("that source file is not part of this document");
    };
    let texts = session::texts_of(doc);
    let Some(text) = texts.get(&path) else {
        return Err("that source file is not text");
    };
    let units: Vec<u16> = text.encode_utf16().collect();
    if source.end_utf16 as usize > units.len() {
        return Err("that source range is not valid");
    }
    let at = utf16_slice(text, source.start_utf16 as usize, source.end_utf16 as usize);
    if at != source.exact {
        // The range and the quotation disagree, so at most one of them is
        // right and there is no way to tell which. Neither is stored.
        return Err("that passage is not where the anchor says it is");
    }
    Ok(anchor)
}

/// The source a comment was made against, when that is not the source as it
/// stands.
///
/// A reader annotating a published rendering is reading a page built from one
/// particular checkpoint, and the words in front of them are that
/// checkpoint's words -- not the draft an editor has been rewriting since. So
/// the comment is anchored there, in the state it was actually made in, and
/// where that passage has got to since is the resolver's question like any
/// other. Anchoring it to the current draft instead would either fail to find
/// the passage or, worse, find a different one.
pub(super) struct PublishedSource {
    checkpoint: String,
    files: Vec<(String, String, String)>,
}

impl Room {
    /// The checkpoint a publication was built from, and its files.
    ///
    /// `None` when there is no such publication, it kept no source version,
    /// or its source archive cannot be read. A rendered selection cannot be
    /// anchored to the current draft in any of those cases.
    async fn published_source(&self, publication_id: &str) -> Option<PublishedSource> {
        let id = uuid::Uuid::parse_str(publication_id).ok()?;
        let catalog = self.catalog.as_ref().get()?;
        let publication = catalog.publication(id).await.ok()??;
        let document_id = uuid::Uuid::parse_str(&self.storage_id).ok()?;
        if publication.document_id != document_id {
            return None;
        }
        let version = catalog
            .version(document_id, publication.source_version_id?)
            .await
            .ok()??;
        let point = super::checkpoint::checkpoint_from_version(&version);
        let checkpoint = point.sha.clone();
        let (tree, bodies) = self.checkpoint_texts(&point).await.ok()?;
        let files = tree
            .files
            .iter()
            .filter(|(_, file)| file.kind == "text")
            .filter_map(|(path, file)| {
                bodies
                    .get(&file.sha)
                    .map(|body| (file.id.clone(), path.clone(), body.clone()))
            })
            .collect();
        Some(PublishedSource { checkpoint, files })
    }
}

/// Works out what a selection is about, in the document as it stands.
///
/// This is the one place a rendered selection becomes a source range, and it
/// happens on the server because the server is what holds the checkpoint. A
/// browser sends what it can see -- the words, and the words around them --
/// and never an offset: an offset from a client is a claim about a file the
/// client may not even be allowed to read.
///
/// The range this returns is by construction a range of the checkpoint's own
/// text, and `exact` is the text at it, so there is nothing left to verify
/// afterwards. That is the point of deriving it here rather than checking a
/// submitted one.
fn anchor_for(
    config: &Configuration,
    doc: &loro::LoroDoc,
    current: &str,
    published: Option<&PublishedSource>,
    quote: &Quote<'_>,
    whole_document: bool,
) -> Result<OriginalAnchor, String> {
    let checkpoint = published.map(|p| p.checkpoint.as_str()).unwrap_or(current);
    if checkpoint.is_empty() {
        return Err("this document has no checkpoint to comment against".into());
    }
    let checkpoint_id = CheckpointId(checkpoint.to_string());
    // A remark about the document as a whole, which is the one kind of
    // comment that cannot be orphaned because it is not about a passage.
    if whole_document || quote.exact.trim().is_empty() {
        return Ok(OriginalAnchor {
            checkpoint_id,
            target: CommentTarget::Document,
        });
    }
    if quote.exact.chars().count() > config.caps.exact {
        return Err("that selection is too long to comment on".into());
    }
    let files = match published {
        Some(published) => published.files.clone(),
        None => source_files(doc),
    };
    let candidates: Vec<locate::Candidate<'_>> = files
        .iter()
        .map(|(id, path, text)| locate::Candidate {
            file_id: id,
            path,
            text,
        })
        .collect();
    match locate::locate(&candidates, quote) {
        Ok(target) => Ok(OriginalAnchor {
            checkpoint_id,
            target: CommentTarget::SourceText(target),
        }),
        Err(failure) => Err(failure.message().to_string()),
    }
}

/// Works out where every comment's passage is in the document as it stands,
/// and says which comments moved.
///
/// This is the only writer of `Comment::attachment`, and it writes nothing
/// else: the anchors it reads from are left exactly as they were. Comments
/// whose attachment is unchanged are not reported, so a document nobody is
/// editing costs one pass and no writes at all.
pub(super) fn reattach(state: &mut RoomState) -> Vec<(String, DerivedAttachment)> {
    // Reading the files out of the CRDT is the cost of a pass, and a document
    // nobody has commented on should not pay it on every keystroke.
    if state.comments.is_empty() {
        return Vec::new();
    }
    let checkpoint = state
        .manifest
        .latest()
        .map(|point| point.sha.clone())
        .unwrap_or_default();
    // The files, read once for the whole pass rather than once per comment:
    // a document with five hundred comments on it is resolved on every edit.
    let sources = resolve::Sources::of(&state.session.doc);
    let mut moved = Vec::new();
    for comment in state.comments.iter_mut() {
        let Some(anchor) = comment.original_anchor.as_ref() else {
            continue;
        };
        // Whatever the last resolution stored, including the cursors Loro
        // handed back then. Resolution starts from those, not from the
        // original range, which is what lets a passage be followed through
        // any number of edits rather than re-found after each one.
        let live = comment
            .attachment
            .as_ref()
            .and_then(|current| current.live_source_range.as_ref());
        let found = resolve::resolve(&state.session.doc, &sources, &checkpoint, anchor, live);
        if comment.attachment.as_ref() != Some(&found) {
            moved.push((comment.id.clone(), found.clone()));
            comment.attachment = Some(found);
        }
    }
    moved
}

impl Room {
    /// Which passage a message is about, for the callers that need the place
    /// rather than the anchor: the file it is in, where it starts, and the
    /// source text it covers.
    ///
    /// The same walk a comment takes, so a suggestion and a comment made on
    /// the same words are about the same words.
    pub(crate) async fn locate_passage(
        &self,
        message: &Message,
    ) -> Result<(String, usize, String), String> {
        let state = self.state.lock().await;
        let files = source_files(&state.session.doc);
        let candidates: Vec<locate::Candidate<'_>> = files
            .iter()
            .map(|(id, path, text)| locate::Candidate {
                file_id: id,
                path,
                text,
            })
            .collect();
        let quote = Quote {
            exact: &message.exact,
            prefix: &message.prefix,
            suffix: &message.suffix,
        };
        let found = locate::locate(&candidates, &quote).map_err(|failure| failure.message())?;
        let path = files
            .iter()
            .find(|(id, _, _)| *id == found.file_id.0)
            .map(|(_, path, _)| path.clone())
            .ok_or("that passage is not in this document's source")?;
        Ok((path, found.start_utf16 as usize, found.exact))
    }

    /// Reattaches every comment and records what changed.
    ///
    /// The durable write is only of the cache: a failure here leaves the
    /// comments right in memory and stale on disk, which the next pass fixes.
    /// It is never allowed to fail the edit that prompted it.
    pub(crate) async fn reattach_comments(&self) {
        let moved = {
            let mut state = self.state.lock().await;
            reattach(&mut state)
        };
        if moved.is_empty() {
            return;
        }
        let Some(catalog) = self.catalog.as_ref().get() else {
            return;
        };
        let rows: Vec<_> = moved
            .into_iter()
            .filter_map(|(id, attachment)| Some((uuid::Uuid::parse_str(&id).ok()?, attachment)))
            .collect();
        if let Err(error) = catalog.record_attachments(&rows).await {
            eprintln!(
                "warning: could not record where a comment's passage went in {}: {error}",
                self.slug
            );
        }
    }
}

/// A comment refused only because the document moved between its checkpoint
/// and the lock that would record it. Nothing about the comment is wrong, so
/// the caller checkpoints again and retries rather than reporting it.
pub(crate) const STALE_CHECKPOINT: &str = "stale_checkpoint";

/// Installs a comment the durable write has already accepted.
///
/// The comment is found again by identity rather than by the index the
/// preparation used, because room state was released for the write. Under the
/// comment gate nothing can have moved it, so this is a fence rather than a
/// merge: a comment that is no longer here is one a reload replaced with the
/// durable list, and that list is the one to keep.
pub(super) fn install_comment(state: &mut RoomState, updated: Comment) {
    if let Some(index) = state.comments.iter().position(|item| item.id == updated.id) {
        state.comments[index] = updated;
    }
}

impl Room {
    /// Adds assistant suggestions against one immutable text snapshot. Bad or
    /// stale anchors are item results; admission and rate-limit failures are
    /// whole-pass refusals so a caller cannot use a batch to bypass caps.
    pub async fn apply_suggestion_batch(
        &self,
        revision: &str,
        items: &[BatchSuggestion],
        caller: BatchCaller<'_>,
        actor: crate::document::store::MutationActor,
    ) -> Result<(String, Vec<Value>), BatchRefusal> {
        if items.is_empty() {
            return Err(BatchRefusal("a suggestion batch cannot be empty".into()));
        }
        if items.len() > 100 {
            return Err(BatchRefusal(
                "a suggestion batch may contain at most 100 items".into(),
            ));
        }
        if !valid_revision(revision) || revision.is_empty() {
            return Err(BatchRefusal("that revision is not valid".into()));
        }
        let _restore_writer = self.restore_write.lock().await;
        let _comment_writer = self.comment_write.lock().await;
        if !self.hold().await {
            return Err(BatchRefusal("this room is held by another server".into()));
        }
        let config = self.config.clone();
        // The PostgreSQL catalogue has the same hard ceiling as the default
        // configuration. Check its durable count too, since a hot room cache
        // may lag a previous process while the room lease is being acquired.
        // Asked before room state is taken: it is the catalogue's answer and
        // needs nothing of this room's.
        let durable_comments = match self.catalog.as_ref().get() {
            Some(catalog) => Some(
                count_catalog_comments(catalog, &self.slug)
                    .await
                    .map_err(BatchRefusal)?,
            ),
            None => None,
        };
        let mut state = self.state.lock().await;
        let latest = state.manifest.latest();
        let live_digest = tree_of(&state.session.doc, &state.session.asset_sizes)
            .0
            .digest();
        if latest.map(|point| point.sha.as_str()) != Some(revision)
            || latest.map(|point| point.tree_sha.as_str()) != Some(live_digest.as_str())
        {
            return Err(BatchRefusal(
                "source changed since that checkpoint; read it again".into(),
            ));
        }
        let existing_comments = durable_comments.unwrap_or_else(|| state.comments.len());
        let max_comments = config.max_comments.min(500);
        if existing_comments.saturating_add(items.len()) > max_comments {
            return Err(BatchRefusal(
                "this document has reached its comment limit".into(),
            ));
        }
        if !self.rate_reserve(
            &mut state,
            caller.address,
            caller.via,
            caller.budget,
            items.len() as i64,
        ) {
            return Err(BatchRefusal(
                "too many comments from this caller; try later".into(),
            ));
        }
        let pass = new_id();
        let mut results: Vec<Value> = Vec::with_capacity(items.len());
        let mut events = Vec::new();
        // The whole pass is prepared under state and persisted without it.
        // Each entry remembers which slot in `results` its outcome belongs to,
        // so an item refused during preparation keeps its place in the reply.
        // Each entry carries the branch its suggestion makes, because the branch
        // is built here where the document is, and written down after the lock
        // goes -- the room state cannot be taken twice.
        type PreparedBranch = (loro::Frontiers, loro::Frontiers, Vec<u8>, loro::PeerID);
        let mut prepared: Vec<(usize, Comment, PreparedBranch)> = Vec::new();
        let mut next_seq = state.seq;
        for item in items {
            let original_anchor = match validate_original_anchor(
                &config,
                &state.session.doc,
                revision,
                item.original_anchor.clone(),
            ) {
                Ok(anchor) => anchor,
                Err(reason) => {
                    results.push(json!({"status":"refused","reason":reason}));
                    continue;
                }
            };
            let Some(source) = source_target(&original_anchor) else {
                results.push(json!({"status":"refused","reason":"invalid anchor"}));
                continue;
            };
            let Some(path) = path_for_file_id(&state.session.doc, &source.file_id) else {
                results.push(json!({"status":"anchor-not-found"}));
                continue;
            };
            let proposed: String = item
                .proposed
                .chars()
                .filter(|character| {
                    let code = *character as u32;
                    !(code < 0x09
                        || (0x0b..=0x0c).contains(&code)
                        || (0x0e..=0x1f).contains(&code)
                        || code == 0x7f)
                })
                .collect();
            if proposed.chars().count() > config.caps.exact {
                results.push(json!({"status":"refused","reason":"the suggestion is too long"}));
                continue;
            }
            let body = clean(&item.body, config.caps.body).trim().to_string();
            // The suggested words become a branch here, under the lock, because
            // this is where the document is. It is written down after the lock
            // goes. A passage that is not where the anchor says refuses the
            // item rather than being placed somewhere plausible.
            let branch = crate::room::proposals::from_suggestion(
                &state.session.doc,
                &path,
                source.start_utf16 as usize,
                &source.exact,
                &proposed,
            )
            .ok()
            .and_then(|(made, base, tip, peer)| {
                session::encode_diff(&made, &session::encode_vector(&state.session.doc))
                    .ok()
                    .map(|bytes| (base, tip, bytes, peer))
            });
            let Some(branch) = branch else {
                results
                    .push(json!({"status":"refused","reason":"that passage is not where it was"}));
                continue;
            };
            next_seq = next_seq.saturating_add(1);
            let added = Comment {
                id: new_id(),
                seq: next_seq,
                attachment: Some(DerivedAttachment {
                    checkpoint_id: CheckpointId(revision.to_string()),
                    status: AnchorStatus::Exact,
                    live_source_range: resolve::capture(&state.session.doc, source),
                    resolved_range_utf16: Some((source.start_utf16, source.end_utf16)),
                    diagnostic: None,
                }),
                original_anchor: Some(original_anchor.clone()),
                presentation: PresentationContext::default(),
                motivation: "editing".into(),
                color: None,
                publication_id: String::new(),
                proposal: String::new(),
                // Projections, filled in when this is served rather than stored.
                proposed: Some(proposed.clone()),
                outcome: String::new(),
                pass: pass.clone(),
                body,
                creator: caller.creator.to_string(),
                created: timestamp(),
                resolved: false,
                resolved_at: None,
                resolved_in: String::new(),
                replies: Vec::new(),
                author: caller.author.to_string(),
            };
            results.push(Value::Null);
            prepared.push((results.len() - 1, added, branch));
        }
        drop(state);
        if let Some(catalog) = self.catalog.as_ref().get() {
            // Row by row, each acknowledged individually, with room state
            // released for the whole pass: a hundred inserts used to hold the
            // document's lock from the first to the last.
            let mut stored_rows = Vec::new();
            for (slot, mut added, branch) in prepared {
                // The branch becomes a proposal, and the comment carries its id.
                // A suggestion whose branch cannot be stored is refused rather
                // than kept as a remark with nothing behind it.
                match self
                    .store_proposal(&added.creator, branch.3, &branch.0, &branch.1, branch.2)
                    .await
                {
                    Ok(id) => added.proposal = id,
                    Err(error) => {
                        results[slot] = json!({"status":"refused","reason":error.to_string()});
                        continue;
                    }
                }
                let persisted = match catalog_comment_row(&self.slug, &added) {
                    Ok(row) => {
                        let key = crate::util::new_request_key();
                        let digest = request_digest(&json!({"comment": added}));
                        insert_comment_request(
                            catalog,
                            row,
                            key,
                            digest,
                            now_unix(),
                            actor.clone(),
                            true,
                        )
                        .await
                        .map(|(seq, _)| seq)
                        .map_err(|error| error.to_string())
                    }
                    Err(error) => Err(error),
                };
                match persisted {
                    Ok(seq) => {
                        let mut stored = added;
                        stored.seq = seq;
                        results[slot] = json!({"status":"created","id":stored.id});
                        events.push(json!({"type":"comment","comment":stored.clone()}));
                        stored_rows.push(stored);
                    }
                    Err(error) => results[slot] = json!({"status":"refused","reason":error}),
                }
            }
            let mut state = self.state.lock().await;
            for stored in stored_rows {
                state.seq = state.seq.max(stored.seq);
                state.comments.push(stored);
            }
        } else {
            return Err(BatchRefusal("durable catalog required".into()));
        }
        for event in events {
            let shared = self.comment_event_for(&event, "", false).await;
            self.broadcast(&shared).await;
        }
        Ok((pass, results))
    }

    pub(super) fn rate_reserve(
        &self,
        state: &mut RoomState,
        address: &str,
        link: &str,
        budget: Option<i64>,
        amount: i64,
    ) -> bool {
        if address.is_empty() && link.is_empty() {
            return true;
        }
        let hour = now_unix() / 3600;
        let caller = if link.is_empty() {
            format!("address:{}", rate_key(address))
        } else {
            format!("link:{link}")
        };
        let key = format!("{caller}:{hour}");
        let suffix = format!(":{hour}");
        state.rate.retain(|existing, _| existing.ends_with(&suffix));
        let count = state.rate.get(&key).copied().unwrap_or(0);
        if count.saturating_add(amount) > budget.unwrap_or(self.config.rate_per_hour) {
            return false;
        }
        state.rate.insert(key, count + amount);
        true
    }

    /// Adds the same caller-specific controls to a newly-created comment
    /// event that a hello or REST snapshot carries. The shared broadcast can
    /// use an empty author (so every other caller sees `mine: false`), while
    /// the submitting socket or HTTP response asks for its own view.
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
        let state = self.state.lock().await;
        let Some(comment) = state.comments.iter().find(|comment| comment.id == id) else {
            // Another mutation can delete a comment after its event was
            // prepared. Never fall back to the unprojected source-bearing
            // payload when the current visibility can no longer be checked.
            return if is_owner {
                payload.clone()
            } else {
                json!({"type": "annotation-redacted"})
            };
        };
        if !is_owner && !visible_to_reader(comment) {
            // Editorial suggestions contain source anchors and proposed source
            // text. Other empty-publication annotations are likewise local to
            // the editable project. Neither belongs in the public annotation
            // channel.
            return json!({"type": "annotation-redacted", "annotation_revision": comment.seq});
        }

        let view = CommentView::for_viewer(comment, author, is_owner);
        let mut event = payload.clone();
        if let Ok(value) = serde_json::to_value(view) {
            event["comment"] = value;
        }
        event
    }

    // Wire metadata and authority remain explicit at the command boundary.
    #[allow(clippy::too_many_arguments)]
    pub async fn apply_command_with_actor(
        &self,
        command: Command,
        address: &str,
        author: &str,
        via: &str,
        budget: Option<i64>,
        is_owner: bool,
        mut mutation_actor: crate::document::store::MutationActor,
    ) -> (Value, bool) {
        mutation_actor.owner_key = author.to_owned();
        let temp_id = command.temp_id().to_owned();
        let request_id = command.request_id().to_owned();
        let comment_id = command.comment_id().to_owned();
        // Suggestions are document operations as well as comment metadata.
        // Serialize comment decisions with acceptance so a resolve/delete
        // cannot race the CRDT edit and leave the catalogue outcome detached
        // from the checkpoint it describes.
        let _restore_writer = self.restore_write.lock().await;
        // Every comment mutation below prepares its change under state,
        // persists it with state released, and installs it only once storage
        // has taken it. This gate is what makes the middle phase safe: it is
        // the only thing that keeps a second comment writer from preparing
        // against the list this one is about to replace.
        let _comment_writer = self.comment_write.lock().await;
        if !self.hold().await {
            return (
                json!({"type": "error", "message": "this room is held by another server", "temp_id": temp_id,
                    "request_id": request_id}),
                false,
            );
        }
        // A comment on a published rendering is anchored in the checkpoint
        // that rendering was built from, so that checkpoint's source is read
        // here -- before the room state is taken, because it reads storage and
        // because the comment gate above already keeps other writers out.
        let published = match &command {
            Command::Comment { publication_id, .. } if !publication_id.is_empty() => {
                self.published_source(publication_id).await
            }
            _ => None,
        };
        if matches!(&command, Command::Comment { publication_id, .. }
            if !publication_id.is_empty())
            && published.is_none()
        {
            return (
                json!({"type": "error", "message": "published source is unavailable; refresh before annotating",
                    "temp_id": temp_id, "request_id": request_id}),
                false,
            );
        }
        let mut state = self.state.lock().await;
        let config = self.config.clone();

        let fail = |text: &str| -> (Value, bool) {
            let mut payload = json!({"type": "error", "message": text, "temp_id": temp_id,
                    "request_id": request_id});
            // Named so the reader knows which optimistic row to roll back.
            if !comment_id.is_empty() {
                payload["comment_id"] = json!(comment_id);
            }
            (payload, false)
        };
        const UNSAVED: &str = "could not save that comment; try again";

        // Retry before counting or writing again. Identity comes from the
        // server, so choosing another person's record ID cannot take it over.
        let requested_id = submission_id(&temp_id).map(str::to_owned).or_else(|| {
            (!request_id.is_empty())
                .then(|| format!("request:{}", crate::document::store::digest_of(&request_id)))
        });
        if let Some(id) = requested_id.as_deref() {
            if self.catalog.as_ref().get().is_none()
                && matches!(&command, Command::Comment { .. } | Command::Reply { .. })
            {
                for item in &state.comments {
                    if item.id == id {
                        if matches!(&command, Command::Comment { .. })
                            && !author.is_empty()
                            && item.author == author
                        {
                            let mut result = json!({"type": "comment", "comment": item, "temp_id": id,
                                    "request_id": request_id});
                            if !request_id.is_empty() {
                                result["noop"] = json!(true);
                            }
                            return (result, true);
                        }
                        return fail("that submission ID is already in use");
                    }
                    if let Some(reply) = item.replies.iter().find(|reply| reply.id == id) {
                        if matches!(&command, Command::Reply { .. })
                            && item.id == comment_id
                            && !author.is_empty()
                            && reply.author == author
                        {
                            let mut result = json!({"type": "reply", "comment_id": item.id,
                                "reply": reply, "temp_id": id,
                                "request_id": request_id});
                            if !request_id.is_empty() {
                                result["noop"] = json!(true);
                            }
                            return (result, true);
                        }
                        return fail("that submission ID is already in use");
                    }
                }
            }
        }

        let existing_submission = requested_id.as_deref().is_some_and(|id| {
            state.comments.iter().any(|comment| {
                comment.id == id || comment.replies.iter().any(|reply| reply.id == id)
            })
        });

        // What the document says at this moment, by name. The socket takes a
        // checkpoint before a comment reaches here, so for a comment this is
        // the text the reviewer was looking at; for a resolve it is the text
        // the author was looking at when they called it done. Either way it is
        // the server's to record and never the client's to send.
        let current = state
            .manifest
            .latest()
            .map(|point| point.sha.clone())
            .unwrap_or_default();
        if matches!(&command, Command::Comment { publication_id, .. } if publication_id.is_empty())
        {
            let live_digest = tree_of(&state.session.doc, &state.session.asset_sizes)
                .0
                .digest();
            let checkpoint_digest = state.manifest.latest().map(|point| point.tree_sha.as_str());
            if checkpoint_digest != Some(live_digest.as_str()) {
                // Somebody typed between the checkpoint and this lock. The
                // comment is fine; the moment it would be recorded against is
                // not. Named so the caller can take a fresh checkpoint and
                // come straight back rather than handing the reviewer an
                // error for somebody else's keystroke.
                let (mut payload, ok) =
                    fail("source changed since its checkpoint; retry the comment");
                payload["code"] = json!(STALE_CHECKPOINT);
                return (payload, ok);
            }
        }

        // Resolving and deleting cost a slot too, the same as posting: a
        // caller who could resolve or delete without limit could still make a
        // thread unusable, just by different means than flooding it with text.
        if !existing_submission && !self.rate_ok(&mut state, address, via, budget) {
            let source = if via.is_empty() { "address" } else { "link" };
            return fail(&format!("too many comments from this {source}; try later"));
        }

        match command {
            Command::Refine {
                comment_id,
                proposed,
                expected_proposed,
                body,
                revision,
                request_id,
                ..
            } => {
                let Some(index) = state.comments.iter().position(|item| item.id == comment_id)
                else {
                    return fail("unknown comment");
                };
                let target = &state.comments[index];
                if !is_owner && (author.is_empty() || target.author != author) {
                    return fail("only the suggestion author or an editor may refine it");
                }
                if target.motivation != "editing" || target.resolved || target.proposal.is_empty() {
                    return fail("only a pending suggestion can be refined");
                }
                if target.revision() != revision {
                    return fail("suggestion revision changed; read it again before refining");
                }
                if proposed.chars().count() > config.caps.exact
                    || body.chars().count() > config.caps.body
                {
                    return fail("refinement exceeds the suggestion size limit");
                }
                let proposed = clean(&proposed, config.caps.exact);
                let body = clean(&body, config.caps.body).trim().to_string();
                let original_anchor = target.original_anchor.clone();
                let was = target.proposal.clone();
                let target = target.clone();
                // Refining replaces the branch rather than editing it. The words
                // somebody is refining away were proposed, and a proposal that
                // quietly becomes a different proposal under the same id is one
                // a reviewer could have agreed to without having seen it.
                let rebuilt = original_anchor.as_ref().and_then(|anchor| {
                    let source = source_target(anchor)?;
                    let path = path_for_file_id(&state.session.doc, &source.file_id)?;
                    let (made, base, tip, peer) = crate::room::proposals::from_suggestion(
                        &state.session.doc,
                        &path,
                        source.start_utf16 as usize,
                        &source.exact,
                        &proposed,
                    )
                    .ok()?;
                    let bytes =
                        session::encode_diff(&made, &session::encode_vector(&state.session.doc))
                            .ok()?;
                    Some((base, tip, bytes, peer))
                });
                drop(state);
                let Some((base, tip, bytes, peer)) = rebuilt else {
                    return fail("that passage is not where it was; read it again before refining");
                };
                // What the reviewer last saw, so a refinement racing another
                // refinement is refused rather than silently winning.
                match self.proposed_text(&was).await {
                    Ok(Some(current)) if current == expected_proposed => {}
                    Ok(_) => return fail("suggestion changed; read it again before refining"),
                    Err(error) => return fail(&error.to_string()),
                }
                let refreshed = match self
                    .store_proposal(&target.creator, peer, &base, &tip, bytes)
                    .await
                {
                    Ok(id) => id,
                    Err(error) => return fail(&error.to_string()),
                };
                let mut refined = target;
                refined.proposal = refreshed;
                refined.body = body;
                let persisted = if let Some(catalog) = self.catalog.as_ref().get() {
                    match catalog_comment_row(&self.slug, &refined) {
                        Ok(row) => update_comment_row(catalog, row, mutation_actor.clone()).await,
                        Err(error) => Err(error),
                    }
                } else {
                    Err("durable catalog required".into())
                };
                state = self.state.lock().await;
                if persisted.is_err() {
                    return fail(UNSAVED);
                }
                if self.catalog.as_ref().get().is_some() {
                    install_comment(&mut state, refined.clone());
                }
                (
                    json!({"type":"refine","comment_id":comment_id,"comment":refined,"request_id":request_id}),
                    true,
                )
            }
            Command::Resolve {
                comment_id,
                resolved,
                request_id,
                ..
            } => {
                let Some(index) = state.comments.iter().position(|item| item.id == comment_id)
                else {
                    return fail("unknown comment");
                };
                // A suggestion's resolve doubles as its plain-language reject and
                // reopen, with one refusal an ordinary comment never needs: an
                // accepted suggestion already changed the document, and reopening
                // it here would say it is merely unresolved rather than say what
                // actually happened to the text.
                let is_suggestion = state.comments[index].motivation == "editing";
                if is_suggestion && !is_owner {
                    return fail("only an editor may decide a suggestion");
                }
                // Whether a suggestion was taken is not recorded here any more:
                // it is the state of its proposal, which only the server writes.
                // Settled means its hunks were decided and the words either
                // landed or did not -- either way that is done, and resolving
                // the remark again is a no-op rather than a second decision.
                let settled = state.comments[index].proposal.clone();
                if is_suggestion && !settled.is_empty() {
                    let open = self.open_proposals().await.unwrap_or_default();
                    let still_open = open.iter().any(|proposal| proposal["id"] == json!(settled));
                    if !still_open {
                        if !resolved {
                            return fail(
                                "a decided suggestion cannot be reopened; restore the checkpoint instead",
                            );
                        }
                        let target = &state.comments[index];
                        return (
                            json!({
                                "type": "resolve", "comment_id": target.id,
                                "resolved": target.resolved, "resolved_at": target.resolved_at,
                                "resolved_in": target.resolved_in,
                            }),
                            true,
                        );
                    }
                }
                let was_resolved = state.comments[index].resolved;
                if was_resolved == resolved {
                    let target = &state.comments[index];
                    let mut result = json!({
                        "type": "resolve", "comment_id": target.id,
                        "resolved": target.resolved, "resolved_at": target.resolved_at,
                        "resolved_in": target.resolved_in, "request_id": request_id,
                    });
                    if !request_id.is_empty() {
                        result["noop"] = json!(true);
                    }
                    return (result, true);
                }
                // The decision is prepared on a copy and applied to room state
                // only once it is durable. The catalogue write is awaited now,
                // and a caller that disappears at that await must not leave
                // this room showing a decision no peer was told about and no
                // row records.
                let mut decided = state.comments[index].clone();
                decided.resolved = resolved;
                decided.resolved_at = resolved.then(timestamp);
                // Which text it was resolved against. Cleared when a comment is
                // reopened, because it is no longer resolved in anything.
                decided.resolved_in = if resolved {
                    current.clone()
                } else {
                    String::new()
                };
                drop(state);
                let persisted = if let Some(catalog) = self.catalog.as_ref().get() {
                    match catalog_comment_row(&self.slug, &decided) {
                        Ok(row) => update_comment_row(catalog, row, mutation_actor.clone()).await,
                        Err(error) => Err(error),
                    }
                } else {
                    Err("durable catalog required".into())
                };
                state = self.state.lock().await;
                if persisted.is_err() {
                    return fail(UNSAVED);
                }
                if self.catalog.as_ref().get().is_some() {
                    install_comment(&mut state, decided.clone());
                }
                (
                    json!({
                        "type": "resolve", "comment_id": decided.id,
                        "resolved": decided.resolved, "resolved_at": decided.resolved_at,
                        "resolved_in": decided.resolved_in,
                        "request_id": request_id,
                    }),
                    true,
                )
            }
            Command::Delete {
                comment_id,
                request_id,
                ..
            } => {
                let Some(index) = state.comments.iter().position(|item| item.id == comment_id)
                else {
                    return fail("unknown comment");
                };
                if !deletable(&state.comments[index], author, is_owner) {
                    return fail("you may only delete your own comments");
                }
                // Removed from room state only once the row is gone, so a
                // cancelled caller cannot hide a comment from this room that
                // every other reader still has.
                drop(state);
                let persisted = if let Some(catalog) = self.catalog.as_ref().get() {
                    delete_comment_row(catalog, &self.slug, &comment_id, mutation_actor.clone())
                        .await
                } else {
                    Err("durable catalog required".into())
                };
                state = self.state.lock().await;
                if persisted.is_err() {
                    return fail(UNSAVED);
                }
                if self.catalog.as_ref().get().is_some() {
                    state.comments.retain(|item| item.id != comment_id);
                }
                (
                    json!({"type": "delete", "comment_id": comment_id,
                    "request_id": request_id}),
                    true,
                )
            }

            Command::Reply {
                comment_id,
                body: raw_body,
                creator: raw_creator,
                temp_id,
                request_id,
            } => {
                let body = clean(&raw_body, config.caps.body).trim().to_string();
                if body.is_empty() {
                    return fail("reply body is required");
                }
                let mut creator = clean(&raw_creator, config.caps.creator).trim().to_string();
                if creator.is_empty() {
                    creator = "Anonymous".to_string();
                }
                let Some(index) = state.comments.iter().position(|item| item.id == comment_id)
                else {
                    return fail("unknown comment");
                };
                if !existing_submission && state.comments[index].replies.len() >= config.max_replies
                {
                    return fail("this comment has reached its reply limit");
                }
                let mut added = Reply {
                    id: requested_id.clone().unwrap_or_else(new_id),
                    body,
                    creator,
                    created: timestamp(),
                    author: author.to_string(),
                };
                // The reply joins room state once its receipt is durable. A
                // retry of the same request id matches that receipt rather
                // than inserting the reply twice.
                let target_id = state.comments[index].id.clone();
                drop(state);
                let persisted = if let Some(catalog) = self.catalog.as_ref().get() {
                    let row = ReplyRow {
                        comment_id: target_id.clone(),
                        id: added.id.clone(),
                        body: added.body.clone(),
                        creator: added.creator.clone(),
                        author: added.author.clone(),
                    };
                    let digest = request_digest(&json!({
                        "kind": "reply",
                        "comment_id": row.comment_id,
                        "reply": row.id,
                        "body": row.body,
                        "creator": row.creator,
                        "author": row.author,
                    }));
                    insert_reply_request(
                        catalog,
                        row,
                        request_id.clone(),
                        digest,
                        now_unix(),
                        mutation_actor.clone(),
                    )
                    .await
                    .map(|created| added.created = created)
                } else {
                    Err(WriteError::Storage("durable catalog required".into()))
                };
                state = self.state.lock().await;
                if let Err(error) = persisted {
                    let (mut response, _) = fail(&error.client_message());
                    response["status"] = json!(error.status());
                    response["code"] = json!("annotation_refused");
                    response["retryable"] = json!(error.is_temporary());
                    return (response, false);
                }
                if self.catalog.as_ref().get().is_some() {
                    if let Some(target) =
                        state.comments.iter_mut().find(|item| item.id == target_id)
                    {
                        if !target.replies.iter().any(|reply| reply.id == added.id) {
                            target.replies.push(added.clone());
                        }
                    }
                }
                (
                    json!({
                        "type": "reply", "comment_id": target_id,
                        "reply": added, "temp_id": temp_id,
                        "request_id": request_id,
                    }),
                    true,
                )
            }
            Command::Comment {
                motivation: raw_motivation,
                publication_id,
                body: raw_body,
                creator: raw_creator,
                exact: raw_exact,
                prefix: raw_prefix,
                suffix: raw_suffix,
                position,
                document,
                color: raw_color,
                proposed: raw_proposed,
                temp_id,
                request_id,
            } => {
                let body = clean(&raw_body, config.caps.body).trim().to_string();
                let motivation = config.allowed_motivation(&raw_motivation);
                // A highlight is the passage itself: marking something as worth
                // returning to needs no words. A suggestion is its proposal: the
                // words are the replacement, and `body` beside it is an optional
                // note. Everything else is a remark, and a remark with no words is
                // nothing.
                if body.is_empty() && !matches!(motivation.as_str(), "highlighting" | "editing") {
                    return fail("comment body is required");
                }
                let mut creator = clean(&raw_creator, config.caps.creator).trim().to_string();
                if creator.is_empty() {
                    creator = "Anonymous".to_string();
                }
                if !existing_submission && state.comments.len() >= config.max_comments {
                    return fail("this document has reached its comment limit");
                }
                let color = match raw_color {
                    Some(raw) if !raw.trim().is_empty() => match valid_color(Some(&raw)) {
                        Some(color) => Some(color),
                        None => return fail("color must be a #RRGGBB value"),
                    },
                    _ => None,
                };
                // What the browser saw. Capped here, before any of it is used
                // to search with, and kept afterwards as the presentation
                // context: what a comment looked like where it was made is
                // worth showing, and is never what it is about.
                let seen = PresentationContext {
                    rendered_exact: clean(&raw_exact, config.caps.exact),
                    rendered_prefix: clean(&raw_prefix, config.caps.context),
                    rendered_suffix: clean(&raw_suffix, config.caps.context),
                    rendered_position_utf16: position
                        .filter(|at| *at >= 0)
                        .map(|at| at.min(u32::MAX as i64) as u32),
                };
                let quote = Quote {
                    exact: &seen.rendered_exact,
                    prefix: &seen.rendered_prefix,
                    suffix: &seen.rendered_suffix,
                };
                // The passage, found in the document's own source. A client
                // sends words; where those words are is the server's answer,
                // because the server is what holds the checkpoint -- and, for
                // a reader of a published render, what holds the source at all.
                let original_anchor = match anchor_for(
                    &config,
                    &state.session.doc,
                    &current,
                    published.as_ref(),
                    &quote,
                    document,
                ) {
                    Ok(anchor) => anchor,
                    Err(reason) => return fail(&reason),
                };
                // A suggestion is its proposal; without one it is an
                // annotation with nothing to act on. `proposed` on any other
                // motivation is not something a client meant to send, so it
                // is dropped rather than stored.
                if motivation == "editing" && raw_proposed.is_none() {
                    return fail("a suggestion needs a proposal");
                }

                let proposed = if motivation == "editing" {
                    let raw = raw_proposed.as_deref().unwrap_or_default();
                    let cleaned: String = raw
                        .chars()
                        .filter(|&c| {
                            let code = c as u32;
                            !(code < 0x09
                                || (0x0b..=0x0c).contains(&code)
                                || (0x0e..=0x1f).contains(&code)
                                || code == 0x7f)
                        })
                        .collect();
                    if cleaned.chars().count() > config.caps.exact {
                        return fail("the suggestion is too long");
                    }
                    Some(cleaned)
                } else {
                    None
                };
                // A suggestion replaces a passage, so there has to be one: a
                // proposal against the document as a whole has nothing to
                // put its words in place of.
                let source = match source_target(&original_anchor) {
                    Some(target) => Some(target.clone()),
                    None if matches!(motivation.as_str(), "editing" | "highlighting") => {
                        return fail("a suggestion or highlight has to be about a passage")
                    }
                    None => None,
                };
                // The cursors that will follow this passage from here on.
                // Taken now, against the checkpoint the range was found in,
                // because that is the only moment they are certainly right.
                // Where that passage is, now. For a comment made against the
                // current draft that is where it was just found, and cursors
                // can be taken there to follow it from here on. For one made
                // against a published checkpoint the offsets belong to that
                // checkpoint, so there is nothing to take a cursor at yet and
                // the resolver answers by the words instead.
                let attachment = source.as_ref().map(|target| {
                    if original_anchor.checkpoint_id.0 == current {
                        DerivedAttachment {
                            checkpoint_id: CheckpointId(current.clone()),
                            status: AnchorStatus::Exact,
                            live_source_range: resolve::capture(&state.session.doc, target),
                            resolved_range_utf16: Some((target.start_utf16, target.end_utf16)),
                            diagnostic: None,
                        }
                    } else {
                        resolve::resolve(
                            &state.session.doc,
                            &resolve::Sources::of(&state.session.doc),
                            &current,
                            &original_anchor,
                            None,
                        )
                    }
                });
                let mut added = Comment {
                    id: requested_id.unwrap_or_else(new_id),
                    seq: state.seq.saturating_add(1),
                    original_anchor: Some(original_anchor),
                    attachment,
                    presentation: seen,
                    motivation,
                    color,
                    publication_id,

                    proposal: String::new(),
                    proposed: proposed.clone(),
                    outcome: String::new(),
                    body,
                    creator,
                    created: timestamp(),
                    resolved: false,
                    resolved_at: None,
                    resolved_in: String::new(),
                    pass: String::new(),
                    replies: Vec::new(),
                    author: author.to_string(),
                };
                // A comment that proposes different words for a passage makes a
                // branch for them, built here where the document is (§1.2).
                let offered = added
                    .original_anchor
                    .as_ref()
                    .zip(proposed.clone())
                    .and_then(|(anchor, wanted)| {
                        let source = source_target(anchor)?;
                        let path = path_for_file_id(&state.session.doc, &source.file_id)?;
                        // A branch is made against the document as it stands,
                        // so it starts where the passage is now -- which is
                        // the range it was just anchored at for a suggestion
                        // made against the current draft, and whatever the
                        // resolver found for one made against an older
                        // checkpoint. `from_suggestion` refuses a range whose
                        // text is not the text the anchor quotes.
                        let at = added
                            .attachment
                            .as_ref()
                            .and_then(|found| found.resolved_range_utf16)
                            .map(|(start, _)| start)
                            .unwrap_or(source.start_utf16);
                        let (made, base, tip, peer) = crate::room::proposals::from_suggestion(
                            &state.session.doc,
                            &path,
                            at as usize,
                            &source.exact,
                            &wanted,
                        )
                        .ok()?;
                        let bytes = session::encode_diff(
                            &made,
                            &session::encode_vector(&state.session.doc),
                        )
                        .ok()?;
                        Some((base, tip, bytes, peer))
                    });
                drop(state);
                if proposed.is_some() {
                    let Some((base, tip, bytes, peer)) = offered else {
                        return fail("that passage is not where it was");
                    };
                    match self
                        .store_proposal(&added.creator, peer, &base, &tip, bytes)
                        .await
                    {
                        Ok(id) => added.proposal = id,
                        Err(error) => return fail(&error.to_string()),
                    }
                }
                let persisted = if let Some(catalog) = self.catalog.as_ref().get() {
                    let digest = request_digest(&json!({
                        "kind": "comment",
                        "comment": added,
                    }));
                    let row = match catalog_comment_row(&self.slug, &added) {
                        Ok(row) => row,
                        Err(_) => return fail(UNSAVED),
                    };
                    // Pushed into room state only once the receipt is durable,
                    // so a cancelled caller leaves neither an unbroadcast
                    // comment here nor a row nobody was told about.
                    match insert_comment_request(
                        catalog,
                        row,
                        request_id.clone(),
                        digest,
                        now_unix(),
                        mutation_actor.clone(),
                        false,
                    )
                    .await
                    {
                        Ok((seq, created)) => {
                            added.seq = seq;
                            added.created = created;
                            let stored = added.clone();
                            let mut state = self.state.lock().await;
                            state.seq = state.seq.max(seq);
                            if !state.comments.iter().any(|comment| comment.id == stored.id) {
                                state.comments.push(stored);
                            }
                            Ok(())
                        }
                        Err(error) => {
                            eprintln!("catalog comment insert failed for {}: {error}", self.slug);
                            Err(error)
                        }
                    }
                } else {
                    Err(WriteError::Storage("durable catalog required".into()))
                };
                if let Err(error) = persisted {
                    let (mut response, _) = fail(&error.client_message());
                    response["status"] = json!(error.status());
                    response["code"] = json!("annotation_refused");
                    response["retryable"] = json!(error.is_temporary());
                    return (response, false);
                }
                (
                    json!({"type": "comment", "comment": added, "temp_id": temp_id,
                        "request_id": request_id}),
                    true,
                )
            }
            Command::Accept { .. } | Command::Reject { .. } => {
                fail("suggestion decisions use the decision handler")
            }
            Command::RevisionDecide {
                revision_id,
                action,
                ..
            } => {
                let _ = (revision_id, action);
                fail("suggestion decisions use the decision handler")
            }
        }
    }

    /* ---------------------------------------------------------- the document */
}
