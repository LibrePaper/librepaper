use super::*;

#[derive(Clone, Debug)]
pub(crate) struct AgentAnnotationAuthority {
    pub account_id: String,
    pub generation: String,
    pub link_hash: String,
    pub policy_comment: bool,
    pub require_editor: bool,
}

pub(crate) struct AnnotationBatch {
    pub request_id: String,
    pub digest: String,
    pub base_revision: Option<String>,
    pub upserts: Vec<Comment>,
    pub expected: Vec<(String, String)>,
    pub deletes: Vec<String>,
    pub replies: Vec<(String, Reply)>,
    pub receipt: Value,
}

pub(crate) fn comment_version(comment: &Comment) -> String {
    request_digest(&json!(comment))
}

impl Room {
    pub(crate) async fn ensure_agent_checkpoint(
        &self,
        revision: &str,
        authority: super::agent::AgentAuthority,
        display: &str,
        _commit: impl Sized,
    ) -> Result<String, String> {
        if let Some(point) = self
            .manifest()
            .await
            .checkpoints
            .into_iter()
            .find(|point| point.tree_sha == revision)
        {
            return Ok(point.sha);
        }
        let live = self.tree().await.digest();
        if live != revision {
            return Err("source tree changed".into());
        }
        self.checkpoint_now("cli", Attribution::account(&authority.account_id, display))
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "checkpoint was not created".into())
    }

    pub(crate) async fn agent_comment(&self, id: &str) -> Option<Comment> {
        self.state
            .lock()
            .await
            .comments
            .iter()
            .find(|comment| comment.id == id)
            .cloned()
    }

    pub(crate) async fn agent_comment_version(&self, id: &str) -> Option<String> {
        self.agent_comment(id).await.as_ref().map(comment_version)
    }

    pub(crate) async fn apply_agent_annotations(
        &self,
        batch: AnnotationBatch,
        caller: BatchCaller<'_>,
        authority: AgentAnnotationAuthority,
    ) -> Result<Value, String> {
        let _restore = self.restore_write.lock().await;
        let _comment = self.comment_write.lock().await;
        if !self.hold().await {
            return Err("room lease unavailable".into());
        }
        if !authority.policy_comment {
            return Err("comment access changed".into());
        }
        let catalog = self
            .catalog
            .as_ref()
            .get()
            .ok_or("durable catalog required")?;
        let actor = crate::document::store::MutationActor {
            account_id: authority.account_id,
            owner_key: caller.author.into(),
            session_generation: authority.generation,
            link_hash: authority.link_hash,
            policy_editor: authority.require_editor,
            unowned_publisher: false,
        };
        {
            let mut state = self.state.lock().await;
            if let Some(revision) = &batch.base_revision {
                if tree_of(&state.session.doc, &state.session.asset_sizes)
                    .0
                    .digest()
                    != *revision
                {
                    return Err("source tree changed".into());
                }
            }
            for (id, version) in &batch.expected {
                let comment = state
                    .comments
                    .iter()
                    .find(|comment| comment.id == *id)
                    .ok_or("comment disappeared")?;
                if comment_version(comment) != *version {
                    return Err("comment version changed".into());
                }
            }
            let additions = batch
                .upserts
                .iter()
                .filter(|new| !state.comments.iter().any(|old| old.id == new.id))
                .count();
            if state.comments.len() + additions
                > self.config.max_comments.min(500) + batch.deletes.len()
            {
                return Err("comment quota exceeded".into());
            }
            if !self.rate_reserve(
                &mut state,
                caller.address,
                caller.via,
                caller.budget,
                (additions + batch.replies.len()) as i64,
            ) {
                return Err("comment rate limited".into());
            }
        }

        for comment in &batch.upserts {
            let row = catalog_comment_row(&self.slug, comment)?;
            if self.agent_comment(&comment.id).await.is_some() {
                update_comment_row(catalog, row, actor.clone()).await?;
            } else {
                insert_comment_request(
                    catalog,
                    row,
                    batch.request_id.clone(),
                    batch.digest.clone(),
                    now_unix(),
                    actor.clone(),
                    authority.require_editor,
                )
                .await
                .map_err(|e| e.to_string())?;
            }
        }
        for id in &batch.deletes {
            delete_comment_row(catalog, &self.slug, id, actor.clone()).await?;
        }
        for (comment_id, reply) in &batch.replies {
            insert_reply_request(
                catalog,
                ReplyRow {
                    comment_id: comment_id.clone(),
                    id: reply.id.clone(),
                    body: reply.body.clone(),
                    creator: reply.creator.clone(),
                    author: reply.author.clone(),
                },
                batch.request_id.clone(),
                batch.digest.clone(),
                now_unix(),
                actor.clone(),
            )
            .await
            .map_err(|e| e.to_string())?;
        }
        let (seq, comments) = load_catalog_comments(catalog, &self.slug).await?;
        {
            let mut state = self.state.lock().await;
            state.seq = seq;
            *state.comments = comments;
        }
        let snapshot = self.snapshot_for("", false).await;
        self.broadcast(&json!({"type":"comments","comments":snapshot,"annotation_revision":seq}))
            .await;
        Ok(batch.receipt)
    }
}
