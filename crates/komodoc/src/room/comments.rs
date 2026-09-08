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
    /// looking at. Set by the server, never by the client, from the checkpoint
    /// taken the moment the comment arrived -- so a passage can be looked up
    /// in the text as it was rather than reconstructed from one that has moved
    /// on. Empty on a comment from before the field existed, which is read as
    /// the oldest checkpoint the manifest still has.
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
    let exact = clean(&anchor.exact, config.caps.exact).trim().to_string();
    if exact.is_empty() {
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

impl Room {
    /// Adds the same caller-specific controls to a newly-created comment
    /// event that a hello or REST snapshot carries. The shared broadcast can
    /// use an empty author (so every other caller sees `mine: false`), while
    /// the submitting socket or HTTP response asks for its own view.
    pub async fn comment_event_for(&self, payload: &Value, author: &str, is_owner: bool) -> Value {
        if payload.get("type").and_then(Value::as_str) != Some("comment") {
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
        let view = CommentView {
            comment: comment.clone(),
            mine: !author.is_empty() && comment.author == author,
            deletable: deletable(comment, author, is_owner),
        };
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
    pub async fn apply(
        &self,
        incoming: Message,
        address: &str,
        author: &str,
        via: &str,
        budget: Option<i64>,
        is_owner: bool,
    ) -> (Value, bool) {
        let mut state = self.state.lock().await;
        let config = self.config.clone();

        let fail = |text: &str| -> (Value, bool) {
            let mut payload = json!({"type": "error", "message": text, "temp_id": incoming.temp_id,
                    "request_id": incoming.request_id});
            // Named so the reader knows which optimistic row to roll back.
            if !incoming.comment_id.is_empty() {
                payload["comment_id"] = json!(incoming.comment_id);
            }
            (payload, false)
        };
        const UNSAVED: &str = "could not save that comment; try again";

        // Retry before counting or writing again. Identity comes from the
        // server, so choosing another person's record ID cannot take it over.
        let requested_id = submission_id(&incoming.temp_id)
            .map(str::to_owned)
            .or_else(|| {
                (!incoming.request_id.is_empty()).then(|| {
                    format!(
                        "request:{}",
                        crate::document::store::digest_of(&incoming.request_id)
                    )
                })
            });
        if let Some(id) = requested_id.as_deref() {
            if incoming.kind == "comment" || incoming.kind == "reply" {
                for item in &state.comments {
                    if item.id == id {
                        if incoming.kind == "comment" && !author.is_empty() && item.author == author
                        {
                            let mut result = json!({"type": "comment", "comment": item, "temp_id": id,
                                    "request_id": incoming.request_id});
                            if !incoming.request_id.is_empty() {
                                result["noop"] = json!(true);
                            }
                            return (result, true);
                        }
                        return fail("that submission ID is already in use");
                    }
                    if let Some(reply) = item.replies.iter().find(|reply| reply.id == id) {
                        if incoming.kind == "reply"
                            && item.id == incoming.comment_id
                            && !author.is_empty()
                            && reply.author == author
                        {
                            let mut result = json!({"type": "reply", "comment_id": item.id,
                                "reply": reply, "temp_id": id,
                                "request_id": incoming.request_id});
                            if !incoming.request_id.is_empty() {
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

        if incoming.kind == "resolve" {
            let Some(index) = state
                .comments
                .iter()
                .position(|item| item.id == incoming.comment_id)
            else {
                return fail("unknown comment");
            };
            // A suggestion's resolve doubles as its plain-language reject and
            // reopen, with one refusal an ordinary comment never needs: an
            // accepted suggestion already changed the document, and reopening
            // it here would say it is merely unresolved rather than say what
            // actually happened to the text.
            let is_suggestion = state.comments[index].motivation == "editing";
            if is_suggestion && !incoming.resolved && state.comments[index].outcome == "accepted" {
                return fail(
                    "an accepted suggestion cannot be reopened; restore the checkpoint instead",
                );
            }
            if is_suggestion && incoming.resolved && state.comments[index].outcome == "accepted" {
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
            let (was_resolved, was_resolved_at, was_resolved_in, was_outcome) = (
                state.comments[index].resolved,
                state.comments[index].resolved_at.clone(),
                state.comments[index].resolved_in.clone(),
                state.comments[index].outcome.clone(),
            );
            if was_resolved == incoming.resolved {
                let target = &state.comments[index];
                let mut result = json!({
                    "type": "resolve", "comment_id": target.id,
                    "resolved": target.resolved, "resolved_at": target.resolved_at,
                    "resolved_in": target.resolved_in, "request_id": incoming.request_id,
                });
                if !incoming.request_id.is_empty() {
                    result["noop"] = json!(true);
                }
                return (result, true);
            }
            state.comments[index].resolved = incoming.resolved;
            state.comments[index].resolved_at = incoming.resolved.then(timestamp);
            // Which text it was resolved against. Cleared when a comment is
            // reopened, because it is no longer resolved in anything.
            state.comments[index].resolved_in = if incoming.resolved {
                current.clone()
            } else {
                String::new()
            };
            if is_suggestion {
                state.comments[index].outcome = if incoming.resolved {
                    "rejected".to_string()
                } else {
                    String::new()
                };
            }
            if self.save(&mut state).await.is_err() {
                state.comments[index].resolved = was_resolved;
                state.comments[index].resolved_at = was_resolved_at;
                state.comments[index].resolved_in = was_resolved_in;
                state.comments[index].outcome = was_outcome;
                return fail(UNSAVED);
            }
            let target = &state.comments[index];
            return (
                json!({
                    "type": "resolve", "comment_id": target.id,
                    "resolved": target.resolved, "resolved_at": target.resolved_at,
                    "resolved_in": target.resolved_in,
                    "request_id": incoming.request_id,
                }),
                true,
            );
        }

        if incoming.kind == "delete" {
            let Some(index) = state
                .comments
                .iter()
                .position(|item| item.id == incoming.comment_id)
            else {
                return fail("unknown comment");
            };
            if !deletable(&state.comments[index], author, is_owner) {
                return fail("you may only delete your own comments");
            }
            let removed = state.comments.remove(index);
            let persisted = if let Some(catalog) = self.catalog.get() {
                catalog
                    .delete_comment(&self.slug, &removed.id)
                    .map(|_| ())
                    .map_err(|err| err.to_string())
            } else {
                self.save(&mut state).await
            };
            if persisted.is_err() {
                state.comments.insert(index, removed);
                return fail(UNSAVED);
            }
            return (
                json!({"type": "delete", "comment_id": incoming.comment_id,
                    "request_id": incoming.request_id}),
                true,
            );
        }

        // A backfill on an existing comment, for a passage anchored after the
        // fact -- a comment made before source anchors existed, or one made
        // on generated text that a later edit brought back into the source.
        // Guarded the same way a delete is: the author of the comment, or an
        // editor, and only once -- a comment that already has an anchor of
        // record is not overwritten by a second try.
        if incoming.kind == "anchor" {
            let Some(index) = state
                .comments
                .iter()
                .position(|item| item.id == incoming.comment_id)
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
            let Some(anchor) = valid_source(&config, incoming.source.as_ref()) else {
                return fail("that source anchor is not valid");
            };
            state.comments[index].source = Some(anchor.clone());
            if self.save(&mut state).await.is_err() {
                state.comments[index].source = None;
                return fail(UNSAVED);
            }
            return (
                json!({
                    "type": "anchor", "comment_id": state.comments[index].id,
                    "source": anchor,
                    "request_id": incoming.request_id,
                }),
                true,
            );
        }

        let body = clean(&incoming.body, config.caps.body).trim().to_string();
        let motivation = config.allowed_motivation(&incoming.motivation);
        // A highlight is the passage itself: marking something as worth
        // returning to needs no words. A suggestion is its proposal: the
        // words are the replacement, and `body` beside it is an optional
        // note. Everything else is a remark, and a remark with no words is
        // nothing.
        if body.is_empty()
            && !(incoming.kind == "comment"
                && matches!(motivation.as_str(), "highlighting" | "editing"))
        {
            return fail("comment body is required");
        }
        let mut creator = clean(&incoming.creator, config.caps.creator)
            .trim()
            .to_string();
        if creator.is_empty() {
            creator = "Anonymous".to_string();
        }

        match incoming.kind.as_str() {
            "reply" => {
                let Some(index) = state
                    .comments
                    .iter()
                    .position(|item| item.id == incoming.comment_id)
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
                state.comments[index].replies.push(added.clone());
                let persisted = if let Some(catalog) = self.catalog.get() {
                    let row = crate::storage::catalog::Reply {
                        slug: self.slug.clone(),
                        comment_id: state.comments[index].id.clone(),
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
                    catalog
                        .insert_reply_request(&row, &incoming.request_id, &digest, now_unix())
                        .map(|_| ())
                        .map_err(|err| err.to_string())
                } else {
                    self.save(&mut state).await
                };
                if persisted.is_err() {
                    state.comments[index].replies.pop();
                    return fail(UNSAVED);
                }
                (
                    json!({
                        "type": "reply", "comment_id": state.comments[index].id,
                        "reply": added, "temp_id": incoming.temp_id,
                        "request_id": incoming.request_id,
                    }),
                    true,
                )
            }
            "comment" => {
                if state.comments.len() >= config.max_comments {
                    return fail("this document has reached its comment limit");
                }
                let exact = clean(&incoming.exact, config.caps.exact).trim().to_string();
                let spot = valid_region(incoming.region.as_ref());
                // An annotation is anchored to words or to part of a figure;
                // one or the other, never neither.
                if exact.is_empty() && spot.is_none() {
                    return fail("select some text or part of a figure to comment on");
                }
                // A suggestion is its proposal; without one it is an
                // annotation with nothing to act on. `proposed` on any other
                // motivation is not something a client meant to send, so it
                // is dropped rather than stored.
                if motivation == "editing" && incoming.proposed.is_none() {
                    return fail("a suggestion needs a proposal");
                }
                let proposed = (motivation == "editing").then(|| {
                    clean(
                        incoming.proposed.as_deref().unwrap_or_default(),
                        config.caps.exact,
                    )
                });
                state.seq += 1;
                // The selector is the durable anchor. Offsets are recomputed in
                // the reader against whatever version of the document is on
                // screen, so replacing a document needs no migration pass here.
                let added = Comment {
                    id: requested_id.clone().unwrap_or_else(new_id),
                    seq: state.seq,
                    motivation,
                    exact,
                    prefix: clean(&incoming.prefix, config.caps.context),
                    suffix: clean(&incoming.suffix, config.caps.context),
                    position: incoming.position.filter(|p| *p >= 0),
                    // A region comment is anchored to the figure; the source
                    // it might otherwise have carried is not kept.
                    source: spot
                        .is_none()
                        .then(|| valid_source(&config, incoming.source.as_ref()))
                        .flatten(),
                    region: spot,
                    proposed,
                    outcome: String::new(),
                    body,
                    creator,
                    created: timestamp(),
                    resolved: false,
                    resolved_at: None,
                    revision: current,
                    resolved_in: String::new(),
                    replies: Vec::new(),
                    author: author.to_string(),
                    via: via.to_string(),
                    accept_request: String::new(),
                };
                state.comments.push(added.clone());
                let persisted = if let Some(catalog) = self.catalog.get() {
                    let row = match catalog_comment_row(&self.slug, &added) {
                        Ok(row) => row,
                        Err(_) => {
                            state.comments.pop();
                            state.seq -= 1;
                            return fail(UNSAVED);
                        }
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
                    match catalog.insert_comment_request(
                        &row,
                        &incoming.request_id,
                        &digest,
                        now_unix(),
                    ) {
                        Ok(inserted) => {
                            state.comments.last_mut().expect("comment was pushed").seq =
                                inserted.seq;
                            state.seq = state.seq.max(inserted.seq);
                            Ok(())
                        }
                        Err(err) => {
                            eprintln!("catalog comment insert failed for {}: {err}", self.slug);
                            Err(err.to_string())
                        }
                    }
                } else {
                    self.save(&mut state).await
                };
                if persisted.is_err() {
                    state.comments.pop();
                    state.seq -= 1;
                    return fail(UNSAVED);
                }
                (
                    json!({"type": "comment", "comment": added, "temp_id": incoming.temp_id,
                        "request_id": incoming.request_id}),
                    true,
                )
            }
            _ => fail("unknown message type"),
        }
    }

    /* ---------------------------------------------------------- the document */
}
