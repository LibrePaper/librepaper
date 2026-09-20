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
        author: &str,
        editor: bool,
    ) -> Result<crate::agent_query::QuerySnapshot, crate::room::agent::AgentError> {
        if let Some(why) = self.unreadable().await {
            return Err(crate::room::agent::AgentError::Storage(why));
        }
        let projected = self
            .projection()
            .await
            .map_err(|error| crate::room::agent::AgentError::Storage(error.to_string()))?;
        let comments = self.comments().await.unwrap_or_default();
        // There is no room-held sequence counter any more, so this reads the
        // annotation state itself: a hash of it changes exactly when the
        // state a cursor was issued against has, which is the one property
        // `agent_query`'s pagination cursors need from a "digest".
        let comment_digest = super::comments::comment_digest_of(&comments) as u64;
        let views = comments
            .iter()
            .map(|c| {
                let mut view = serde_json::to_value(CommentView::for_viewer(c, author, editor))
                    .unwrap_or(Value::Null);
                view["comment_version"] = json!(super::agent_comments::comment_version(c));
                view
            })
            .collect();
        Ok(crate::agent_query::QuerySnapshot {
            tree_digest: projected.projection.digest(),
            comment_digest,
            main: projected.projection.main.clone(),
            tree: json!(projected.projection),
            texts: projected.texts.clone(),
            comments: views,
        })
    }
}
