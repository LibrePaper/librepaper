use super::*;

#[derive(Clone, Debug)]
pub(crate) struct AgentAnnotationAuthority {
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
    pub proposals: std::collections::HashMap<String, crate::storage::postgres::NewProposal>,
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
        let account_id = uuid::Uuid::parse_str(&authority.account_id).ok();
        let document_authority = crate::storage::postgres::Authority {
            principal_key: account_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| authority.owner_key.clone()),
            account_id,
            link_hash: (!authority.link_hash.is_empty())
                .then(|| hex::decode(&authority.link_hash).ok())
                .flatten(),
        };
        self.checkpoint(
            "cli",
            Attribution::account(&authority.account_id, display),
            &document_authority,
        )
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "checkpoint was not created".into())
    }

    pub(crate) async fn agent_comment(&self, id: &str) -> Option<Comment> {
        self.command_owner
            .state()
            .await
            .comments
            .iter()
            .find(|comment| comment.id == id)
            .cloned()
    }

    pub(crate) async fn agent_comment_version(&self, id: &str) -> Option<String> {
        self.agent_comment(id).await.as_ref().map(comment_version)
    }

    pub(crate) async fn apply_agent_annotations<F, Fut>(
        &self,
        mut batch: AnnotationBatch,
        caller: BatchCaller<'_>,
        authority: AgentAnnotationAuthority,
        recheck: F,
    ) -> Result<Value, String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<(), String>>,
    {
        let _command = self.command_owner.acquire().await;
        if !self.hold().await {
            return Err("room lease unavailable".into());
        }
        recheck().await?;
        if !authority.policy_comment {
            return Err("comment access changed".into());
        }
        let catalog = self
            .catalog
            .as_ref()
            .get()
            .ok_or("durable catalog required")?;
        let actor = crate::document::store::MutationActor {
            account_id: String::new(),
            owner_key: caller.author.into(),
            session_generation: String::new(),
            link_hash: authority.link_hash,
            policy_editor: false,
            unowned_publisher: false,
        };
        {
            let mut state = self.command_owner.state().await;
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

        let document = catalog
            .document_by_slug(&self.slug)
            .await
            .map_err(|error| error.to_string())?
            .ok_or("document missing")?;
        let mut upserts = Vec::with_capacity(batch.upserts.len());
        for comment in batch.upserts {
            let id = uuid::Uuid::parse_str(&comment.id).map_err(|_| "invalid annotation id")?;
            let input = super::catalog::annotation_input(document.id, &comment)
                .map_err(|error| error.to_string())?;
            upserts.push(crate::storage::postgres::AnnotationBatchUpsert {
                id,
                input,
                resolved: comment.resolved,
                proposal: batch.proposals.remove(&comment.id),
            });
        }
        if !batch.proposals.is_empty() {
            return Err("proposal has no matching annotation".into());
        }
        let deletes = batch
            .deletes
            .into_iter()
            .map(|id| uuid::Uuid::parse_str(&id).map_err(|_| "invalid annotation id".to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let replies = batch
            .replies
            .into_iter()
            .map(|(comment_id, reply)| {
                Ok(crate::storage::postgres::NewReply {
                    id: uuid::Uuid::parse_str(&reply.id)
                        .map_err(|_| "invalid reply id".to_string())?,
                    annotation_id: uuid::Uuid::parse_str(&comment_id)
                        .map_err(|_| "invalid annotation id".to_string())?,
                    author_account_id: reply
                        .author
                        .strip_prefix("account:")
                        .and_then(|id| uuid::Uuid::parse_str(id).ok()),
                    author_key: reply.author,
                    author_label: reply.creator,
                    body: reply.body,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let authorization =
            super::catalog::mutation_authorization(&actor).map_err(|error| error.to_string())?;
        let receipt = crate::storage::postgres::SemanticReceipt {
            request_id: uuid::Uuid::parse_str(&batch.request_id)
                .map_err(|_| "request_id must be a UUID")?,
            canonical_command: serde_json::json!({"digest": batch.digest}),
            stable_result: batch.receipt,
            status: "committed".into(),
        };
        let result = catalog
            .apply_annotation_batch(
                crate::storage::postgres::AnnotationBatchCommand {
                    document_id: document.id,
                    upserts,
                    deletes,
                    replies,
                    require_editor: authority.require_editor,
                    receipt,
                },
                &authorization,
            )
            .await
            .map_err(|error| error.to_string())?;
        let (seq, mut comments) = load_catalog_comments(catalog, &self.slug).await?;
        self.hydrate_proposed_comments(&mut comments)
            .await
            .map_err(|error| error.to_string())?;
        {
            let mut state = self.command_owner.state().await;
            state.seq = seq;
            *state.comments = comments;
        }
        self.broadcast_comment_snapshot(seq).await;
        Ok(result)
    }
}
