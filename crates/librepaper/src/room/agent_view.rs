//! An immutable query capture, with no duplicate main source or file inventory.
use super::*;

impl Room {
    pub(crate) async fn agent_query_snapshot(
        &self,
        author: &str,
        editor: bool,
    ) -> Result<crate::agent_query::QuerySnapshot, crate::room::agent::AgentError> {
        let _publication = self.publication_write.lock().await;
        if self.fence_reason.load(std::sync::atomic::Ordering::Relaxed)
            == super::FenceReason::AgentRecoveryPending as u8
        {
            return Err(crate::room::agent::AgentError::Storage(
                "room state is awaiting agent operation recovery".into(),
            ));
        }
        let state = self.state.lock().await;
        let (tree, _) = tree_of(&state.session.doc, &state.session.asset_sizes);
        let comments = state
            .comments
            .iter()
            .map(|c| {
                let mut view = serde_json::to_value(CommentView::for_viewer(c, author, editor))
                    .unwrap_or(Value::Null);
                view["comment_version"] = json!(super::agent_comments::comment_version(c));
                view
            })
            .collect();
        Ok(crate::agent_query::QuerySnapshot {
            source_revision: tree.digest(),
            annotation_revision: state.seq.max(0) as u64,
            main: tree.main.clone(),
            tree: json!(tree),
            texts: session::texts_of(&state.session.doc),
            comments,
        })
    }
}
