//! An immutable query capture, with no duplicate main source or file inventory.
//!
//! Under SPEC-server-is-a-log the source is read from the sequencer's
//! projection (§4.4) rather than from a resident `LoroDoc` the room held
//! itself, and there is no fence to check first: `room.unreadable()` is the
//! one "cannot serve this" signal now (§9.3), and it means the cache could
//! not be built, not that some earlier agent transaction left the room in a
//! state nobody had reconciled -- that apparatus no longer exists, because a
//! command's source is prepared on a fork and only ever imported after its
//! transaction commits (§7 step 5).
use serde_json::json;

use super::*;

impl Room {
    pub(crate) async fn agent_query_snapshot(
        &self,
    ) -> Result<crate::agent_query::QuerySnapshot, crate::room::agent::AgentError> {
        if let Some(why) = self.unreadable().await {
            return Err(crate::room::agent::AgentError::Storage(why));
        }
        let projected = self
            .projection()
            .await
            .map_err(|error| crate::room::agent::AgentError::Storage(error.to_string()))?;
        Ok(crate::agent_query::QuerySnapshot {
            tree_digest: projected.projection.digest(),
            main: projected.projection.main.clone(),
            projection: projected.projection.clone(),
            texts: projected.texts.clone(),
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
    /// `after` is the position a continuation cursor decoded to. The
    /// revision is what a cursor binds to, exactly as it bound to the old
    /// whole-collection digest: an agent that keeps paging across a change
    /// is told its cursor no longer matches rather than being handed two
    /// different collections interleaved.
    pub(crate) async fn thread_query_page(
        &self,
        author: &str,
        editor: bool,
        after: Option<comments::Position>,
        limit: usize,
    ) -> Result<crate::agent_query::ThreadWindow, crate::room::agent::AgentError> {
        let (views, page) = self
            .comment_page(after, limit, author, editor)
            .await
            .map_err(crate::room::agent::AgentError::from)?;
        let items = views
            .into_iter()
            .zip(page.comments.iter())
            .map(|(view, c)| {
                let mut view = serde_json::to_value(view).unwrap_or(Value::Null);
                view["comment_version"] = json!(c.version());
                if let Some(at) = c.at {
                    view["cursor"] =
                        json!(comments::encode_cursor(self.document_id, None, editor, at));
                }
                view
            })
            .collect();
        Ok(crate::agent_query::ThreadWindow {
            items,
            thread: None,
            complete: page.complete,
            missing: false,
        })
    }

    /// One comment and a bounded window of its replies, shaped as the
    /// `thread` query's `id` form returns them.
    pub(crate) async fn thread_reply_window(
        &self,
        comment_id: Uuid,
        author: &str,
        editor: bool,
        after: Option<comments::Position>,
        limit: usize,
    ) -> Result<crate::agent_query::ThreadWindow, crate::room::agent::AgentError> {
        let missing = crate::agent_query::ThreadWindow {
            missing: true,
            complete: true,
            ..Default::default()
        };
        let Some(comment) = self
            .comment_by_id(&comment_id.to_string(), editor)
            .await
            .map_err(crate::room::agent::AgentError::from)?
        else {
            return Ok(missing);
        };
        if !editor && !comments::visible_to_reader(&comment) {
            return Ok(missing);
        }
        let Some(page) = self
            .reply_page(comment_id, after, limit, editor)
            .await
            .map_err(crate::room::agent::AgentError::from)?
        else {
            return Ok(missing);
        };
        let mut thread = serde_json::to_value(CommentView::for_viewer(&comment, author, editor))
            .unwrap_or(Value::Null);
        thread["comment_version"] = json!(comment.version());
        let items = page
            .replies
            .iter()
            .map(|reply| {
                let mut value = serde_json::to_value(reply).unwrap_or(Value::Null);
                if let Some(at) = reply.at {
                    value["cursor"] = json!(comments::encode_cursor(
                        self.document_id,
                        Some(comment_id),
                        editor,
                        at
                    ));
                }
                value
            })
            .collect();
        Ok(crate::agent_query::ThreadWindow {
            items,
            thread: Some(thread),
            complete: page.complete,
            missing: false,
        })
    }
}
