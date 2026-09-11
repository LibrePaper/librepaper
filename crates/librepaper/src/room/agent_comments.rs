//! Atomic annotation batches used by MCP, sharing the ordinary room gates.
use super::*;
use crate::storage::catalog::AgentAnnotationAuthority;

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
    /// Ensure that a captured tree has a retained checkpoint. Content lookup
    /// makes a retry after a lost receipt a no-op even if the live tree moved.
    pub(crate) async fn ensure_agent_checkpoint(
        &self,
        revision: &str,
        authority: super::agent::AgentAuthority,
        display: &str,
        commit: crate::storage::catalog::AgentCheckpointCommit,
    ) -> Result<String, String> {
        let _publication = self.publication_write.lock().await;
        if !self.hold().await {
            return Err("room lease unavailable".into());
        }
        let catalog = self.catalog.get().ok_or("durable catalog required")?;
        let actor = crate::storage::catalog::MutationAuthority {
            account_id: &authority.account_id,
            owner_key: &authority.owner_key,
            generation: &authority.generation,
            link_hash: &authority.link_hash,
            policy_editor: authority.policy_editor,
            automation: authority.automation,
            unowned_publisher: false,
            execution_epoch: &authority.execution_epoch,
            agent_checkpoint: Some(&commit),
        };
        let retained = read_checkpoint_by_content_sha(catalog, &self.slug, revision).await?;
        if let Some(point) = retained {
            let sha = point.sha.clone();
            let owned = super::catalog::OwnedAuthority::new(&actor);
            catalog
                .execute_catalog(4096, move |catalog| {
                    catalog.insert_checkpoints_atomic_with_authority(&[point], Some(owned.borrow()))
                })
                .await
                .map_err(|error| error.to_string())?;
            return Ok(sha);
        }
        {
            let state = self.state.lock().await;
            if tree_of(&state.session.doc, &state.session.asset_sizes)
                .0
                .digest()
                != revision
            {
                return Err("source tree changed".into());
            }
        }
        let checkpoint = self
            .checkpoint_now_with_authority(
                "cli",
                Attribution::account(&authority.account_id, display),
                actor,
            )
            .await
            .map_err(|e| e.to_string())?;
        let checkpoint = checkpoint.ok_or("checkpoint did not produce an identity")?;
        // An automatic checkpoint may have won the content race after our
        // initial lookup. This also records an idempotent receipt for that
        // existing checkpoint; new insertions already recorded it atomically.
        let point = read_checkpoint_by_content_sha(catalog, &self.slug, revision)
            .await?
            .ok_or("checkpoint descriptor unavailable")?;
        let owned = super::catalog::OwnedAuthority::new(&actor);
        catalog
            .execute_catalog(4096, move |catalog| {
                catalog.insert_checkpoints_atomic_with_authority(&[point], Some(owned.borrow()))
            })
            .await
            .map_err(|error| error.to_string())?;
        Ok(checkpoint)
    }

    pub(crate) async fn agent_comment(&self, id: &str) -> Option<Comment> {
        let state = self.state.lock().await;
        state
            .comments
            .iter()
            .find(|comment| comment.id == id)
            .cloned()
    }

    pub(crate) async fn agent_comment_version(&self, id: &str) -> Option<String> {
        let state = self.state.lock().await;
        state
            .comments
            .iter()
            .find(|comment| comment.id == id)
            .map(comment_version)
    }

    pub(crate) async fn apply_agent_annotations(
        &self,
        batch: AnnotationBatch,
        caller: BatchCaller<'_>,
        authority: AgentAnnotationAuthority,
    ) -> Result<Value, String> {
        let _restore = self.restore_write.lock().await;
        let _comment = self.comment_write.lock().await;
        let _publication = self.publication_write.lock().await;
        if !self.hold().await {
            return Err("room lease unavailable".into());
        }
        let catalog = self.catalog.get().ok_or("durable catalog required")?;
        let authority_check = authority.clone();
        let authority_slug = self.slug.clone();
        catalog
            .execute_catalog(256, move |c| {
                c.agent_annotation_authorized(&authority_slug, &authority_check)
            })
            .await
            .map_err(|e| e.to_string())?;
        let key = batch.request_id.clone();
        let storage_id = self.storage_id.clone();
        if let Some(old) = catalog
            .execute_catalog(256, move |c| c.operation(&storage_id, &key))
            .await
            .map_err(|e| e.to_string())?
        {
            if old.kind != "agent_annotations"
                || old.status != "committed"
                || old.request_digest != batch.digest
            {
                return Err("operation key reused".into());
            }
            return serde_json::from_str(&old.result).map_err(|e| format!("invalid receipt: {e}"));
        }
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
            let existing = state
                .comments
                .iter()
                .find(|c| c.id == *id)
                .ok_or("comment disappeared")?;
            if comment_version(existing) != *version {
                return Err("comment version changed".into());
            }
        }
        let new_count = batch
            .upserts
            .iter()
            .filter(|c| !state.comments.iter().any(|old| old.id == c.id))
            .count();
        if state.comments.len() + new_count
            > self.config.max_comments.min(500) + batch.deletes.len()
        {
            return Err("comment quota exceeded".into());
        }
        if !self.rate_reserve(
            &mut state,
            caller.address,
            caller.via,
            caller.budget,
            (new_count + batch.replies.len()) as i64,
        ) {
            return Err("comment rate limited".into());
        }
        let rows = batch
            .upserts
            .iter()
            .map(|c| catalog_comment_row(&self.slug, c))
            .collect::<Result<Vec<_>, _>>()?;
        let replies: Vec<_> = batch
            .replies
            .iter()
            .map(|(id, r)| crate::storage::catalog::Reply {
                slug: self.slug.clone(),
                comment_id: id.clone(),
                id: r.id.clone(),
                body: r.body.clone(),
                creator: r.creator.clone(),
                author: r.author.clone(),
                created: r.created.clone(),
            })
            .collect();
        drop(state);
        let slug = self.slug.clone();
        let receipt = batch.receipt.to_string();
        let bytes = batch
            .upserts
            .iter()
            .map(|c| json!(c).to_string().len())
            .sum::<usize>()
            + receipt.len()
            + 1024;
        let (request_id, digest, deletes) = (batch.request_id, batch.digest, batch.deletes.clone());
        let result = catalog
            .execute_catalog(bytes, move |c| {
                c.agent_annotations(
                    &slug,
                    &request_id,
                    &digest,
                    &rows,
                    &replies,
                    &deletes,
                    &receipt,
                    &authority,
                )
            })
            .await
            .map_err(|e| e.to_string())?;
        let (seq, comments) = match load_catalog_comments(catalog, &self.slug).await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                // SQL already committed both effects and receipt. Prevent a
                // later mutation from validating against stale room comments;
                // the retained receipt still resolves this call's outcome.
                self.fence(FenceReason::AgentRecoveryPending);
                return Err(error.to_string());
            }
        };
        {
            let mut state = self.state.lock().await;
            state.seq = seq;
            let _ = std::mem::replace(&mut *state.comments, comments);
        }
        let sequence = seq;
        let current_comments = self.snapshot_for("", false).await;
        for comment in &batch.upserts {
            let event_comment = current_comments
                .iter()
                .find(|item| item.comment.id == comment.id)
                .map(|item| json!(item));
            let event = self.comment_event_for(&json!({"type":"comment","comment":event_comment.unwrap_or_else(|| json!(comment)),"annotation_revision":sequence}),"",false).await;
            self.broadcast(&event).await;
        }
        for id in batch.deletes {
            self.broadcast(
                &json!({"type":"delete","comment_id":id,"annotation_revision":sequence}),
            )
            .await;
        }
        for (id, reply) in batch.replies {
            self.broadcast(&json!({"type":"reply","comment_id":id,"reply":reply,"annotation_revision":sequence}))
                .await;
        }
        serde_json::from_str(&result).map_err(|e| e.to_string())
    }
}
