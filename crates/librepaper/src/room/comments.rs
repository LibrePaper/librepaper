//! The comments: what one is, how a submission from a reader becomes one, and
//! the anchor that ties it to a passage.

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
    /// REST responses, none of which should carry it; `to_stored` below is the
    /// only shape that puts it on disk, and loading reads it back.
    #[serde(default, skip_serializing)]
    pub author: String,
}

/// A rectangle on an image, in percentages of the image's own size, so it
/// survives the document being displayed at any width.
///
/// Which image is a harder question than where on it. There is no text around
/// a figure to anchor to, so two identifiers are kept: a digest of the image
/// source, which survives the figure moving, and its position among the
/// document's images, which survives the image being re-encoded. The reader
/// tries the digest first.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Region {
    #[serde(default)]
    pub image_digest: String,
    #[serde(default)]
    pub image_index: i64,
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default, rename = "w")]
    pub width: f64,
    #[serde(default, rename = "h")]
    pub height: f64,
}

/// Where a passage sits in the file it actually came from, as opposed to the
/// rendered page a reader was looking at when they wrote the comment. The
/// source is what is versioned -- checkpoints and the CRDT both hold it, not
/// the HTML a browser produced from it -- so this is the anchor that survives
/// a re-render and that can be looked up in any checkpoint without rendering
/// it first. It becomes the anchor of record; the rendered TextQuoteSelector
/// stays only for display. Not every comment has one: a remark on generated
/// text (a bibliography entry, a numbered caption) may match nothing in the
/// source, and a region comment on a figure never has one.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SourceAnchor {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub exact: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
    #[serde(default)]
    pub position: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    #[serde(default)]
    pub seq: i64,
    #[serde(default)]
    pub motivation: String,
    #[serde(default)]
    pub exact: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
    /// Where the passage sat when the comment was made. Null for a comment
    /// written before it was recorded, rather than claiming offset 0.
    #[serde(default)]
    pub position: Option<i64>,
    /// Set instead of the text selector when the annotation is on part of a
    /// figure rather than on a run of words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<Region>,
    /// The anchor of record, into the source rather than the rendered page.
    /// Absent on a region comment, on a comment made before this existed, and
    /// on one whose passage could not be found in the source it was written
    /// against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceAnchor>,
    /// The text a suggestion (a comment whose motivation is `editing`) wants
    /// in place of the passage `source.exact` names. `Some("")` proposes
    /// deleting it outright. Absent on every other motivation: a suggestion
    /// is the one kind of comment that is inert until an editor acts on it,
    /// rather than a remark in its own right.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed: Option<String>,
    /// Batch identifier for assistant suggestions. Empty means this comment
    /// was created independently of a batch.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pass: String,
    /// `accepted` or `rejected` once an editor has decided a suggestion;
    /// empty while pending and on every comment that is not one.
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
    /// The checkpoint this comment was made on: what the reviewer was actually
    /// looking at. Ordinary comments use the checkpoint taken the moment they
    /// arrive, while anchored assistant comments preserve their captured
    /// revision -- so a passage can be looked up in the text as it was rather
    /// than reconstructed from one that has moved on. Empty on a comment from
    /// before the field existed, which is read as the oldest checkpoint the
    /// manifest still has.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub revision: String,
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
    /// The digest of the link this comment arrived on, empty for a commenter
    /// by name. It is what lets an owner group a blind reviewer's remarks
    /// without either reviewer having signed anything, and it is not exported:
    /// the export is the reviewer's words, not the mechanics of how they
    /// arrived. Kept off every client-bound shape, as `author` is.
    #[serde(default, skip_serializing)]
    pub via: String,
    /// The `request_id` an `accept` last actually applied, so a retry with
    /// the same id is recognized as the same request rather than accepted a
    /// second time -- an accept has a side effect on the live document, and
    /// the ordinary re-submit-with-the-same-id retry that a plain comment
    /// answers for free would otherwise reapply the edit. Kept off every
    /// client-bound shape, as `author` and `via` are.
    #[serde(default, skip_serializing)]
    pub accept_request: String,
}

pub(super) fn valid_revision(value: &str) -> bool {
    value.is_empty()
        || (value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()))
}

/// One proposal in an assistant pass. Validation and placement happen while
/// the room's restore lock and state snapshot are held by
/// [`Room::apply_suggestion_batch`].
#[derive(Clone, Debug)]
pub struct BatchSuggestion {
    pub source: SourceAnchor,
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

/// What lands on disk, one object per document. Author is excluded from a
/// comment's own JSON so that nothing marshaling one for a client leaks it by
/// accident; this is the one place that value is meant to travel.
#[derive(Deserialize)]
pub(super) struct RoomState_ {
    #[serde(default)]
    pub(super) seq: i64,
    #[serde(default)]
    pub(super) comments: Vec<Comment>,
}

pub(super) fn to_stored(items: &[Comment]) -> Value {
    Value::Array(
        items
            .iter()
            .map(|item| {
                let mut stored = json!(item);
                if !item.author.is_empty() {
                    stored["author"] = json!(item.author);
                }
                if !item.via.is_empty() {
                    stored["via"] = json!(item.via);
                }
                if !item.accept_request.is_empty() {
                    stored["accept_request"] = json!(item.accept_request);
                }
                stored["replies"] = Value::Array(
                    item.replies
                        .iter()
                        .map(|answer| {
                            let mut reply = json!(answer);
                            if !answer.author.is_empty() {
                                reply["author"] = json!(answer.author);
                            }
                            reply
                        })
                        .collect(),
                );
                stored
            })
            .collect(),
    )
}

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
        CommentView {
            comment: comment.clone(),
            mine: !author.is_empty() && comment.author == author,
            deletable: deletable(comment, author, is_owner),
        }
    }
}

/// Rule H's authorization test: the document's owner may delete anything on
/// it, and everyone else only their own -- and "their own" never matches on
/// two callers who both have no author key, which is what an anonymous caller
/// with no visitor cookie and a nobody's-in-particular seeded example both
/// look like.
pub fn deletable(item: &Comment, author: &str, is_owner: bool) -> bool {
    is_owner || (!author.is_empty() && item.author == author)
}

/// Keeps a rectangle only if it is one: inside the image, with a size worth
/// drawing. Percentages, so it holds at any display width.
pub fn valid_region(spot: Option<&Region>) -> Option<Region> {
    let spot = spot?;
    let inside = |v: f64| (0.0..=100.0).contains(&v);
    if !inside(spot.x) || !inside(spot.y) || !inside(spot.width) || !inside(spot.height) {
        return None;
    }
    if spot.width < 0.5
        || spot.height < 0.5
        || spot.x + spot.width > 100.5
        || spot.y + spot.height > 100.5
    {
        return None;
    }
    if spot.image_index < 0 {
        return None;
    }
    Some(Region {
        image_digest: clean(&spot.image_digest, 64),
        image_index: spot.image_index,
        x: spot.x,
        y: spot.y,
        width: spot.width,
        height: spot.height,
    })
}

/// Finds where a suggestion's anchor sits in `text`: the one place
/// `source.exact` occurs, or, when it occurs more than once, the occurrence
/// whose surrounding words best match `source.prefix` and `source.suffix` --
/// the longest common suffix of the prefix and longest common prefix of the
/// suffix -- ties broken by distance from `source.position`. `None` when the
/// passage does not occur at all, which is the caller's cue to fall back to a
/// three-way merge.
pub(super) fn locate_anchor(text: &str, anchor: &SourceAnchor) -> Option<usize> {
    if anchor.exact.is_empty() {
        return None;
    }
    let candidates: Vec<usize> = text
        .match_indices(anchor.exact.as_str())
        .map(|(at, _)| at)
        .collect();
    let (&first, rest) = candidates.split_first()?;
    if rest.is_empty() {
        return Some(byte_to_utf16(text, first));
    }
    fn common_suffix_len(a: &str, b: &str) -> usize {
        a.chars()
            .rev()
            .zip(b.chars().rev())
            .take_while(|(x, y)| x == y)
            .count()
    }
    fn common_prefix_len(a: &str, b: &str) -> usize {
        a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
    }
    let mut best = first;
    let mut best_score = -1i64;
    let mut best_distance = i64::MAX;
    for byte in candidates {
        let before = &text[..byte];
        let after = &text[byte + anchor.exact.len()..];
        let score = (common_suffix_len(&anchor.prefix, before)
            + common_prefix_len(&anchor.suffix, after)) as i64;
        let at16 = byte_to_utf16(text, byte) as i64;
        let distance = anchor
            .position
            .map(|position| (at16 - position).abs())
            .unwrap_or(0);
        if score > best_score || (score == best_score && distance < best_distance) {
            best_score = score;
            best_distance = distance;
            best = byte;
        }
    }
    Some(byte_to_utf16(text, best))
}

/// Keeps a source anchor only if it names a real place: a passage worth
/// keeping, and a path that stays inside the document rather than reading
/// somewhere else on the machine that renders it. Anything wrong with either
/// drops the whole anchor rather than keeping half of it, because half an
/// anchor -- a path with no passage, or a passage nobody can find the file
/// for -- is not one a client could ever act on.
pub(super) fn valid_source(
    config: &Configuration,
    anchor: Option<&SourceAnchor>,
) -> Option<SourceAnchor> {
    let anchor = anchor?;
    let cleaned: String = anchor
        .exact
        .chars()
        .filter(|&c| {
            let code = c as u32;
            !(code < 0x09
                || (0x0b..=0x0c).contains(&code)
                || (0x0e..=0x1f).contains(&code)
                || code == 0x7f)
        })
        .collect();
    let exact = cleaned.trim().to_string();
    if exact.is_empty() {
        return None;
    }
    if exact.chars().count() > config.caps.exact {
        return None;
    }
    let path = anchor.path.trim();
    if path.is_empty()
        || path.len() > 512
        || path.starts_with('/')
        || path.contains('\\')
        || path.split('/').any(|part| part == "..")
        || path.chars().any(|c| c.is_control())
    {
        return None;
    }
    Some(SourceAnchor {
        path: path.to_string(),
        exact,
        prefix: clean(&anchor.prefix, config.caps.context),
        suffix: clean(&anchor.suffix, config.caps.context),
        position: anchor.position.filter(|p| *p >= 0),
    })
}

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
    /// The comment list this room should hold after one comment is replaced,
    /// for a room with no catalogue -- which has nothing narrower to write
    /// than the whole list. Empty for a catalogue-backed room, which writes
    /// the one changed row and never needs the copy.
    pub(super) fn legacy_list_with(
        &self,
        state: &RoomState,
        index: usize,
        updated: &Comment,
    ) -> Vec<Comment> {
        if self.catalog.get().is_some() {
            return Vec::new();
        }
        let mut list = state.comments.clone();
        list[index] = updated.clone();
        list
    }

    /// The same, for a comment being removed.
    pub(super) fn legacy_list_without(&self, state: &RoomState, index: usize) -> Vec<Comment> {
        if self.catalog.get().is_some() {
            return Vec::new();
        }
        let mut list = state.comments.clone();
        list.remove(index);
        list
    }

    /// Adds assistant suggestions against one immutable text snapshot. Bad or
    /// stale anchors are item results; admission and rate-limit failures are
    /// whole-pass refusals so a caller cannot use a batch to bypass caps.
    pub async fn apply_suggestion_batch(
        &self,
        revision: &str,
        items: &[BatchSuggestion],
        caller: BatchCaller<'_>,
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
        // The SQLite catalogue has the same hard ceiling as the default
        // configuration. Check its durable count too, since a hot room cache
        // may lag a previous process while the room lease is being acquired.
        // Asked before room state is taken: it is the catalogue's answer and
        // needs nothing of this room's.
        let durable_comments = match self.catalog.get() {
            Some(catalog) => Some(
                count_catalog_comments(catalog, &self.slug)
                    .await
                    .map_err(BatchRefusal)?,
            ),
            None => None,
        };
        let mut state = self.state.lock().await;
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
        let texts = session::texts_of(&state.session.doc);
        let pass = new_id();
        let mut results: Vec<Value> = Vec::with_capacity(items.len());
        let mut events = Vec::new();
        // The whole pass is prepared under state and persisted without it.
        // Each entry remembers which slot in `results` its outcome belongs to,
        // so an item refused during preparation keeps its place in the reply.
        let mut prepared: Vec<(usize, Comment)> = Vec::new();
        let mut next_seq = state.seq;
        for item in items {
            let Some(source) = valid_source(&config, Some(&item.source)) else {
                results.push(json!({"status":"refused","reason":"invalid anchor"}));
                continue;
            };
            if source != item.source {
                results.push(json!({"status":"refused","reason":"invalid anchor"}));
                continue;
            }
            let Some(text) = texts.get(&source.path) else {
                results.push(json!({"status":"anchor-not-found"}));
                continue;
            };
            if locate_anchor(text, &source).is_none() {
                results.push(json!({"status":"anchor-not-found"}));
                continue;
            }
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
            next_seq = next_seq.saturating_add(1);
            let added = Comment {
                id: new_id(),
                seq: next_seq,
                motivation: "editing".into(),
                exact: source.exact.clone(),
                prefix: source.prefix.clone(),
                suffix: source.suffix.clone(),
                position: source.position,
                region: None,
                source: Some(source),
                proposed: Some(proposed),
                pass: pass.clone(),
                outcome: String::new(),
                body,
                creator: caller.creator.to_string(),
                created: timestamp(),
                resolved: false,
                resolved_at: None,
                revision: revision.to_string(),
                resolved_in: String::new(),
                replies: Vec::new(),
                author: caller.author.to_string(),
                via: caller.via.to_string(),
                accept_request: String::new(),
            };
            results.push(Value::Null);
            prepared.push((results.len() - 1, added));
        }
        // The list the room would hold if the whole pass lands, for a room
        // with no catalogue. A catalogue-backed room inserts one row per item
        // and never needs the copy.
        let mut legacy_list = if self.catalog.get().is_some() {
            Vec::new()
        } else {
            let mut list = state.comments.clone();
            list.extend(prepared.iter().map(|(_, added)| added.clone()));
            list
        };
        drop(state);
        if let Some(catalog) = self.catalog.get() {
            // Row by row, each acknowledged individually, with room state
            // released for the whole pass: a hundred inserts used to hold the
            // document's lock from the first to the last.
            let mut stored_rows = Vec::new();
            for (slot, added) in prepared {
                let persisted = match catalog_comment_row(&self.slug, &added) {
                    Ok(row) => insert_comment_row(catalog, row).await,
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
        } else if !prepared.is_empty() {
            // One conditional write for the whole pass, as before. It either
            // takes every prepared suggestion or none of them, and a failure
            // installs nothing, so an unrelated change made meanwhile is not
            // overwritten by a snapshot assembled before it.
            if let Err(error) = self
                .persist_comments(next_seq, std::mem::take(&mut legacy_list))
                .await
            {
                return Err(BatchRefusal(error));
            }
            for (slot, added) in prepared {
                results[slot] = json!({"status":"created","id":added.id});
                events.push(json!({"type":"comment","comment":added}));
            }
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
        if !matches!(
            payload.get("type").and_then(Value::as_str),
            Some("comment" | "refine")
        ) {
            return payload.clone();
        }
        let Some(id) = payload
            .get("comment")
            .and_then(|comment| comment.get("id"))
            .and_then(Value::as_str)
        else {
            return payload.clone();
        };
        let state = self.state.lock().await;
        let Some(comment) = state.comments.iter().find(|comment| comment.id == id) else {
            return payload.clone();
        };
        let view = CommentView::for_viewer(comment, author, is_owner);
        let mut event = payload.clone();
        if let Ok(value) = serde_json::to_value(view) {
            event["comment"] = value;
        }
        event
    }

    /// Validates, persists, and returns the event to broadcast. The second
    /// result is false when the event is an error, which goes only to its
    /// sender. `author` is the caller's own author key, and `is_owner` says
    /// whether the caller owns the document this room belongs to; both come
    /// from the caller's identity and are never taken from the message itself.
    #[allow(dead_code)]
    pub async fn apply(
        &self,
        incoming: Message,
        address: &str,
        author: &str,
        via: &str,
        budget: Option<i64>,
        is_owner: bool,
    ) -> (Value, bool) {
        let command = match incoming.into_command() {
            Ok(command) => command,
            Err(error) => return (error.response(), false),
        };
        self.apply_command(command, address, author, via, budget, is_owner)
            .await
    }

    /// Applies a command after the compatible wire adapter has validated its
    /// discriminator and operation-specific required fields.
    pub async fn apply_command(
        &self,
        command: Command,
        address: &str,
        author: &str,
        via: &str,
        budget: Option<i64>,
        is_owner: bool,
    ) -> (Value, bool) {
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
            if matches!(&command, Command::Comment { .. } | Command::Reply { .. }) {
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

        // Resolving and deleting cost a slot too, the same as posting: a
        // caller who could resolve or delete without limit could still make a
        // thread unusable, just by different means than flooding it with text.
        if !self.rate_ok(&mut state, address, via, budget) {
            let source = if via.is_empty() { "address" } else { "link" };
            return fail(&format!("too many comments from this {source}; try later"));
        }

        // A decision on a suggestion is refused while its acceptance is still
        // staged. That answer is the catalogue's, so it is asked for with room
        // state released -- an editor's keystrokes must not queue behind it.
        // It is asked here rather than inside the branches so that the state
        // release happens once, before any preparation; the branches keep
        // their own authorization refusals, which still come first for the
        // caller who is not allowed to decide at all.
        let deciding = match &command {
            Command::Resolve { .. } => is_owner,
            Command::Delete { .. } | Command::Refine { .. } => true,
            _ => false,
        };
        if let (Some(catalog), true) = (self.catalog.get(), deciding) {
            let is_suggestion = state
                .comments
                .iter()
                .any(|item| item.id == comment_id && item.motivation == "editing");
            if is_suggestion {
                drop(state);
                let pending = pending_suggestion_accept(catalog, &self.slug, &comment_id).await;
                state = self.state.lock().await;
                match pending {
                    Ok(true) => return fail("a suggestion acceptance is still pending"),
                    Ok(false) => {}
                    Err(_) => return fail(UNSAVED),
                }
            }
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
                if target.motivation != "editing" || target.resolved || !target.outcome.is_empty() {
                    return fail("only a pending suggestion can be refined");
                }
                if target.revision != revision {
                    return fail("suggestion revision changed; read it again before refining");
                }
                if proposed.chars().count() > config.caps.exact
                    || body.chars().count() > config.caps.body
                {
                    return fail("refinement exceeds the suggestion size limit");
                }
                let proposed = clean(&proposed, config.caps.exact);
                let body = clean(&body, config.caps.body).trim().to_string();
                if target.proposed.as_deref() == Some(&proposed) && target.body == body {
                    return (
                        json!({"type":"refine","comment_id":comment_id,"comment":target,"request_id":request_id,"noop":true}),
                        true,
                    );
                }
                if target.proposed.as_deref() != Some(&expected_proposed) {
                    return fail("suggestion changed; read it again before refining");
                }
                let mut refined = target.clone();
                refined.proposed = Some(proposed);
                refined.body = body;
                let prepared = self.legacy_list_with(&state, index, &refined);
                let seq = state.seq;
                drop(state);
                let persisted = if let Some(catalog) = self.catalog.get() {
                    match catalog_comment_row(&self.slug, &refined) {
                        Ok(row) => update_comment_row(catalog, row).await,
                        Err(error) => Err(error),
                    }
                } else {
                    self.persist_comments(seq, prepared).await
                };
                state = self.state.lock().await;
                if persisted.is_err() {
                    return fail(UNSAVED);
                }
                if self.catalog.get().is_some() {
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
                if is_suggestion && !resolved && state.comments[index].outcome == "accepted" {
                    return fail(
                        "an accepted suggestion cannot be reopened; restore the checkpoint instead",
                    );
                }
                if is_suggestion && resolved && state.comments[index].outcome == "accepted" {
                    // Already settled by acceptance; resolving it again is a
                    // no-op rather than a second decision.
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
                if is_suggestion {
                    decided.outcome = if resolved {
                        "rejected".to_string()
                    } else {
                        String::new()
                    };
                }
                let prepared = self.legacy_list_with(&state, index, &decided);
                let seq = state.seq;
                drop(state);
                let persisted = if let Some(catalog) = self.catalog.get() {
                    match catalog_comment_row(&self.slug, &decided) {
                        Ok(row) => update_comment_row(catalog, row).await,
                        Err(error) => Err(error),
                    }
                } else {
                    self.persist_comments(seq, prepared).await
                };
                state = self.state.lock().await;
                if persisted.is_err() {
                    return fail(UNSAVED);
                }
                if self.catalog.get().is_some() {
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
                let prepared = self.legacy_list_without(&state, index);
                let seq = state.seq;
                drop(state);
                let persisted = if let Some(catalog) = self.catalog.get() {
                    delete_comment_row(catalog, &self.slug, &comment_id).await
                } else {
                    self.persist_comments(seq, prepared).await
                };
                state = self.state.lock().await;
                if persisted.is_err() {
                    return fail(UNSAVED);
                }
                if self.catalog.get().is_some() {
                    state.comments.retain(|item| item.id != comment_id);
                }
                (
                    json!({"type": "delete", "comment_id": comment_id,
                    "request_id": request_id}),
                    true,
                )
            }
            // A backfill on an existing comment, for a passage anchored after the
            // fact -- a comment made before source anchors existed, or one made
            // on generated text that a later edit brought back into the source.
            // Guarded the same way a delete is: the author of the comment, or an
            // editor, and only once -- a comment that already has an anchor of
            // record is not overwritten by a second try.
            Command::Anchor {
                comment_id,
                source,
                request_id,
                ..
            } => {
                let Some(index) = state.comments.iter().position(|item| item.id == comment_id)
                else {
                    return fail("unknown comment");
                };
                if !deletable(&state.comments[index], author, is_owner) {
                    return fail("you may only anchor your own comments");
                }
                if state.comments[index].source.is_some() {
                    return fail("this comment already has a source anchor");
                }
                if state.comments[index].region.is_some() {
                    return fail("a figure comment cannot take a source anchor");
                }
                let Some(anchor) = valid_source(&config, Some(&source)) else {
                    return fail("that source anchor is not valid");
                };
                // Prepared on a copy and applied once durable, for the same
                // reason as a resolve.
                let mut anchored = state.comments[index].clone();
                anchored.source = Some(anchor.clone());
                let prepared = self.legacy_list_with(&state, index, &anchored);
                let seq = state.seq;
                drop(state);
                let persisted = if let Some(catalog) = self.catalog.get() {
                    match catalog_comment_row(&self.slug, &anchored) {
                        Ok(row) => update_comment_row(catalog, row).await,
                        Err(error) => Err(error),
                    }
                } else {
                    self.persist_comments(seq, prepared).await
                };
                state = self.state.lock().await;
                if persisted.is_err() {
                    return fail(UNSAVED);
                }
                let anchored_id = anchored.id.clone();
                if self.catalog.get().is_some() {
                    install_comment(&mut state, anchored);
                }
                (
                    json!({
                        "type": "anchor", "comment_id": anchored_id,
                        "source": anchor,
                        "request_id": request_id,
                    }),
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
                if state.comments[index].replies.len() >= config.max_replies {
                    return fail("this comment has reached its reply limit");
                }
                let added = Reply {
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
                let prepared = if self.catalog.get().is_some() {
                    Vec::new()
                } else {
                    let mut list = state.comments.clone();
                    list[index].replies.push(added.clone());
                    list
                };
                let seq = state.seq;
                drop(state);
                let persisted = if let Some(catalog) = self.catalog.get() {
                    let row = crate::storage::catalog::Reply {
                        slug: self.slug.clone(),
                        comment_id: target_id.clone(),
                        id: added.id.clone(),
                        body: added.body.clone(),
                        creator: added.creator.clone(),
                        author: added.author.clone(),
                        created: added.created.clone(),
                    };
                    let digest = request_digest(&json!({
                        "kind": "reply",
                        "comment_id": row.comment_id,
                        "reply": row.id,
                        "body": row.body,
                        "creator": row.creator,
                        "author": row.author,
                    }));
                    insert_reply_request(catalog, row, request_id.clone(), digest, now_unix()).await
                } else {
                    self.persist_comments(seq, prepared).await
                };
                state = self.state.lock().await;
                if persisted.is_err() {
                    return fail(UNSAVED);
                }
                if self.catalog.get().is_some() {
                    if let Some(target) =
                        state.comments.iter_mut().find(|item| item.id == target_id)
                    {
                        target.replies.push(added.clone());
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
                body: raw_body,
                creator: raw_creator,
                exact: raw_exact,
                prefix: raw_prefix,
                suffix: raw_suffix,
                position,
                region: raw_region,
                source: raw_source,
                proposed: raw_proposed,
                revision: supplied_revision,
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
                if state.comments.len() >= config.max_comments {
                    return fail("this document has reached its comment limit");
                }
                let exact = clean(&raw_exact, config.caps.exact).trim().to_string();
                let spot = valid_region(raw_region.as_ref());
                // An annotation is anchored to words or to part of a figure;
                // one or the other, never neither.
                if exact.is_empty() && spot.is_none() {
                    return fail("select some text or part of a figure to comment on");
                }
                // A suggestion is its proposal; without one it is an
                // annotation with nothing to act on. `proposed` on any other
                // motivation is not something a client meant to send, so it
                // is dropped rather than stored.
                if motivation == "editing" && raw_proposed.is_none() {
                    return fail("a suggestion needs a proposal");
                }
                if motivation == "editing" {
                    if let Some(source) = raw_source.as_ref() {
                        let cleaned: String = source
                            .exact
                            .chars()
                            .filter(|&c| {
                                let code = c as u32;
                                !(code < 0x09
                                    || (0x0b..=0x0c).contains(&code)
                                    || (0x0e..=0x1f).contains(&code)
                                    || code == 0x7f)
                            })
                            .collect();
                        if cleaned.trim().chars().count() > config.caps.exact {
                            return fail("the source anchor is too long");
                        }
                    }
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
                if !valid_revision(&supplied_revision) {
                    return fail("that revision is not valid");
                }
                let source = if spot.is_none() {
                    match raw_source.as_ref() {
                        Some(raw) => match valid_source(&config, Some(raw)) {
                            Some(cleaned) if cleaned == *raw || supplied_revision.is_empty() => {
                                Some(cleaned)
                            }
                            Some(_) => return fail("that source anchor is not valid"),
                            None if supplied_revision.is_empty() => None,
                            None => return fail("that source anchor is not valid"),
                        },
                        None if supplied_revision.is_empty() => None,
                        None => return fail("that source anchor is not valid"),
                    }
                } else {
                    None
                };
                let revision = if source.is_some() && !supplied_revision.is_empty() {
                    supplied_revision
                } else {
                    current.clone()
                };
                let next_seq = state.seq.saturating_add(1);
                // The selector is the durable anchor. Offsets are recomputed in
                // the reader against whatever version of the document is on
                // screen, so replacing a document needs no migration pass here.
                let added = Comment {
                    id: requested_id.clone().unwrap_or_else(new_id),
                    seq: next_seq,
                    motivation,
                    exact,
                    prefix: clean(&raw_prefix, config.caps.context),
                    suffix: clean(&raw_suffix, config.caps.context),
                    position: position.filter(|p| *p >= 0),
                    // A region comment is anchored to the figure; the source
                    // it might otherwise have carried is not kept.
                    source,
                    region: spot,
                    proposed,
                    outcome: String::new(),
                    body,
                    creator,
                    created: timestamp(),
                    resolved: false,
                    resolved_at: None,
                    revision,
                    resolved_in: String::new(),
                    pass: String::new(),
                    replies: Vec::new(),
                    author: author.to_string(),
                    via: via.to_string(),
                    accept_request: String::new(),
                };
                let prepared = if self.catalog.get().is_some() {
                    Vec::new()
                } else {
                    let mut list = state.comments.clone();
                    list.push(added.clone());
                    list
                };
                drop(state);
                let persisted = if let Some(catalog) = self.catalog.get() {
                    let row = match catalog_comment_row(&self.slug, &added) {
                        Ok(row) => row,
                        Err(_) => return fail(UNSAVED),
                    };
                    let digest = request_digest(&json!({
                        "kind": "comment",
                        "id": row.id,
                        "body": row.body,
                        "motivation": row.motivation,
                        "exact": row.exact,
                        "prefix": row.prefix,
                        "suffix": row.suffix,
                        "position": row.position,
                        "region": row.region,
                        "source_path": row.source_path,
                        "proposed": row.proposed,
                        "author": row.author,
                        "via": row.via,
                    }));
                    // Pushed into room state only once the receipt is durable,
                    // so a cancelled caller leaves neither an unbroadcast
                    // comment here nor a row nobody was told about.
                    match insert_comment_request(
                        catalog,
                        row,
                        request_id.clone(),
                        digest,
                        now_unix(),
                    )
                    .await
                    {
                        Ok(seq) => {
                            let mut stored = added.clone();
                            stored.seq = seq;
                            let mut state = self.state.lock().await;
                            state.seq = state.seq.max(seq);
                            state.comments.push(stored);
                            Ok(())
                        }
                        Err(error) => {
                            eprintln!("catalog comment insert failed for {}: {error}", self.slug);
                            Err(error)
                        }
                    }
                } else {
                    self.persist_comments(next_seq, prepared).await
                };
                if persisted.is_err() {
                    return fail(UNSAVED);
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
        }
    }

    /* ---------------------------------------------------------- the document */
}
