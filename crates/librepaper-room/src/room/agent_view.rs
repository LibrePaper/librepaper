//! An immutable query capture, with no duplicate main source or file inventory.
//!
//! The source is read from the sequencer's projection rather than from a
//! resident `LoroDoc` the room held itself. The projection's own `readable()`
//! check is the one "cannot serve this" signal: it refuses a fenced document
//! and one whose cache could not be built. A command's source is prepared on a
//! fork and only imported after its transaction commits, so no earlier agent
//! transaction can leave the room unreconciled.
use serde_json::json;

use super::*;

impl Room {
    /// A cheap constructor: the sequencer already caches the projection
    /// behind an `Arc` (§4.3), so a capture shares it rather than cloning
    /// every file's text out of it. Only `tree_digest` costs anything here,
    /// and it is the projection's own content digest, computed once.
    pub async fn agent_query_snapshot(
        &self,
    ) -> Result<crate::room::agent_query::QuerySnapshot, crate::room::source_change::AgentError>
    {
        let projected = self.projection().await.map_err(|error| {
            crate::room::source_change::AgentError::Storage(match error {
                SequencerError::Unreadable(why) => why,
                other => other.to_string(),
            })
        })?;
        Ok(crate::room::agent_query::QuerySnapshot {
            tree_digest: projected.projection.digest().to_hex(),
            projected,
            // A capture is of the source. Comments are read live, a page at
            // a time, by whichever query asks for them: see
            // `Room::thread_query_page`. Storing them here made the stored
            // view grow with the collection, and nothing that depends on a
            // capture being immutable ever depended on them -- every
            // edit-safety check reads the comment it is about from the
            // catalogue.
            extras: Default::default(),
            threads: Default::default(),
            comment_revision: String::new(),
        })
    }

    /// One page of comments, shaped the way `agent_query`'s `thread` query
    /// reads them, plus the revision that page was read at.
    ///
    /// `anchor_of` gives each comment the `anchor` an agent reads it by. It
    /// sees the whole comment, not the stripped view, because the passage is
    /// worked out against the reader's own view of the source. A comment
    /// that gets one loses the records it restates; see `set_anchor`.
    ///
    /// `after` is the position a continuation cursor decoded to. The
    /// revision is what a cursor binds to, exactly as it bound to the old
    /// whole-collection digest: an agent that keeps paging across a change
    /// is told its cursor no longer matches rather than being handed two
    /// different collections interleaved.
    pub async fn thread_query_page(
        &self,
        author: &str,
        sees_source: bool,
        moderates: bool,
        after: Option<comments::Position>,
        limit: usize,
        anchor_of: &(dyn Fn(&crate::room::Comment) -> Option<Value> + Sync),
    ) -> Result<crate::room::agent_query::ThreadWindow, crate::room::source_change::AgentError>
    {
        // Derived for every reader, not only one who sees the source:
        // `comment_page` skips that below the suggest rung, and the anchor
        // is the one source-level thing a reader of any role is given.
        let mut page = comments::page(&self.catalog, self.document_id, after, limit)
            .await
            .map_err(crate::room::source_change::AgentError::from)?;
        self.derive(&mut page.comments).await;
        let items = page
            .comments
            .iter()
            .map(|c| {
                let view = CommentView::for_viewer(c, author, sees_source, moderates);
                let mut view = serde_json::to_value(view).unwrap_or(Value::Null);
                view["comment_version"] = json!(c.version());
                if let Some(anchor) = anchor_of(c) {
                    set_anchor(&mut view, anchor);
                }
                if let Some(at) = c.at {
                    view["cursor"] = json!(comments::encode_cursor(self.document_id, None, at));
                }
                view
            })
            .collect();
        Ok(crate::room::agent_query::ThreadWindow {
            items,
            thread: None,
            complete: page.complete,
            missing: false,
        })
    }

    /// One comment and a bounded window of its replies, shaped as the
    /// `thread` query's `id` form returns them.
    #[allow(clippy::too_many_arguments)]
    pub async fn thread_reply_window(
        &self,
        comment_id: Uuid,
        author: &str,
        sees_source: bool,
        moderates: bool,
        after: Option<comments::Position>,
        limit: usize,
        anchor_of: &(dyn Fn(&crate::room::Comment) -> Option<Value> + Sync),
    ) -> Result<crate::room::agent_query::ThreadWindow, crate::room::source_change::AgentError>
    {
        let missing = crate::room::agent_query::ThreadWindow {
            missing: true,
            complete: true,
            ..Default::default()
        };
        let Some(comment) = self
            .comment_by_id(&comment_id.to_string())
            .await
            .map_err(crate::room::source_change::AgentError::from)?
        else {
            return Ok(missing);
        };
        let Some(page) = self
            .reply_page(comment_id, after, limit)
            .await
            .map_err(crate::room::source_change::AgentError::from)?
        else {
            return Ok(missing);
        };
        let mut thread = serde_json::to_value(CommentView::for_viewer(
            &comment,
            author,
            sees_source,
            moderates,
        ))
        .unwrap_or(Value::Null);
        thread["comment_version"] = json!(comment.version());
        if let Some(anchor) = anchor_of(&comment) {
            set_anchor(&mut thread, anchor);
        }
        let items = page
            .replies
            .iter()
            .map(|reply| {
                let mut value = serde_json::to_value(reply).unwrap_or(Value::Null);
                value["mine"] = json!(!author.is_empty() && reply.author == author);
                if let Some(at) = reply.at {
                    value["cursor"] = json!(comments::encode_cursor(
                        self.document_id,
                        Some(comment_id),
                        at
                    ));
                }
                value
            })
            .collect();
        Ok(crate::room::agent_query::ThreadWindow {
            items,
            thread: Some(thread),
            complete: page.complete,
            missing: false,
        })
    }
}

/// Gives a comment view the `anchor` an agent reads the comment by, and
/// takes out the three records that anchor restates: the stored anchor,
/// its attachment and the rendered quotation.
///
/// Those are the browser's working data. Left in, they said the same
/// passage three more times in every comment an agent read, and they
/// still carried a file id and offsets for a passage the anchor reports
/// as gone. A comment that gets no anchor is a remark about the document
/// as a whole, never comes through here, and keeps its view as it is.
fn set_anchor(view: &mut Value, anchor: Value) {
    if let Some(object) = view.as_object_mut() {
        object.remove("original_anchor");
        object.remove("attachment");
        object.remove("presentation");
        object.insert("anchor".to_string(), anchor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_anchored_view_drops_the_records_its_anchor_restates() {
        let mut view = json!({
            "id": "c1",
            "body": "a remark",
            "original_anchor": {"file": "f1", "start": 0, "end": 4},
            "attachment": {"file": "f1"},
            "presentation": {"quote": "quoted"},
        });
        set_anchor(&mut view, json!({"status":"exact","path":"paper.md"}));
        assert!(view.get("original_anchor").is_none());
        assert!(view.get("attachment").is_none());
        assert!(view.get("presentation").is_none());
        assert_eq!(view["anchor"]["path"], "paper.md");
        assert_eq!(view["id"], "c1");
        assert_eq!(view["body"], "a remark");
    }
}
