use super::*;

impl Room {
    pub async fn accept_suggestion_with_actor(
        &self,
        comment_id: &str,
        request_id: &str,
        by: impl Into<Attribution>,
        actor: crate::document::store::MutationActor,
    ) -> Result<Accepted, AcceptError> {
        let by = by.into();
        let _restore = self.restore_write.lock().await;
        let _comments = self.comment_write.lock().await;
        if !self.hold().await {
            return Err(AcceptError::Failed(self.fenced().client_message()));
        }
        if !actor.policy_editor && actor.account_id.is_empty() && actor.link_hash.is_empty() {
            return Err(AcceptError::Refused("edit access changed".into()));
        }

        let comment = {
            let state = self.state.lock().await;
            state
                .comments
                .iter()
                .find(|item| item.id == comment_id)
                .cloned()
        }
        .ok_or_else(|| AcceptError::Refused("unknown comment".into()))?;
        if comment.motivation != "editing" || comment.proposed.is_none() {
            return Err(AcceptError::Refused(
                "that comment is not a suggestion".into(),
            ));
        }
        if comment.outcome == "accepted" && comment.accept_request == request_id {
            return Ok(Accepted::Noop {
                sha: comment.resolved_in,
                resolved_at: comment.resolved_at.unwrap_or_default(),
            });
        }
        if !comment.outcome.is_empty() {
            return Err(AcceptError::Refused(
                "that suggestion was already decided".into(),
            ));
        }
        let source = comment.source.clone().ok_or(AcceptError::Stale)?;
        let proposed = comment.proposed.clone().unwrap_or_default();

        let update = {
            let mut state = self.state.lock().await;
            let current = session::texts_of(&state.session.doc)
                .get(&source.path)
                .cloned()
                .unwrap_or_default();
            let at = locate_anchor(&current, &source).ok_or(AcceptError::Stale)?;
            let before = session::encode_vector(&state.session.doc);
            self.checked_edit(&state.session.doc, |candidate| {
                session::apply_path_edits(
                    candidate,
                    &source.path,
                    &[wasm_helpers::text::Edit {
                        at,
                        delete: len16(&source.exact),
                        insert: proposed.clone(),
                    }],
                );
                Ok::<_, WriteError>(())
            })
            .map_err(AcceptError::from)?;
            state.session.generation += 1;
            state.session.mark_dirty(now_unix());
            session::encode_diff(&state.session.doc, &before).map_err(AcceptError::Failed)?
        };

        let resolved_at = timestamp();
        let mut updated = comment.clone();
        updated.outcome = "accepted".into();
        updated.resolved = true;
        updated.resolved_at = Some(resolved_at.clone());
        updated.accept_request = request_id.into();
        self.write_session_inner_with_acceptance(false, true, Some((&updated, &actor)))
            .await
            .map_err(AcceptError::from)?;
        {
            let mut state = self.state.lock().await;
            let item = state
                .comments
                .iter_mut()
                .find(|item| item.id == comment_id)
                .ok_or_else(|| AcceptError::Failed("suggestion disappeared".into()))?;
            *item = updated.clone();
        }
        let sha = self
            .checkpoint_now("accept", by)
            .await
            .map_err(AcceptError::from)?
            .ok_or_else(|| AcceptError::Failed("acceptance checkpoint was not created".into()))?;
        updated = {
            let mut state = self.state.lock().await;
            let item = state
                .comments
                .iter_mut()
                .find(|item| item.id == comment_id)
                .ok_or_else(|| AcceptError::Failed("suggestion disappeared".into()))?;
            item.resolved_in = sha.clone();
            item.clone()
        };
        if let Some(catalog) = self.catalog.get() {
            let row = catalog_comment_row(&self.slug, &updated).map_err(AcceptError::Failed)?;
            update_comment_row(catalog, row, actor)
                .await
                .map_err(AcceptError::Failed)?;
        }
        self.broadcast_editors_except(
            None,
            &json!({"type":"y-update","update":encode_update(&update)}),
        )
        .await;
        self.broadcast(&json!({"type":"resolve","comment":updated}))
            .await;
        Ok(Accepted::Applied {
            update,
            sha,
            resolved_at,
        })
    }

    pub async fn reject_suggestion_with_actor(
        &self,
        comment_id: &str,
        actor: crate::document::store::MutationActor,
    ) -> Result<Value, String> {
        let _restore = self.restore_write.lock().await;
        let _comments = self.comment_write.lock().await;
        if !self.hold().await {
            return Err(self.fenced().client_message());
        }
        if !actor.policy_editor && actor.account_id.is_empty() && actor.link_hash.is_empty() {
            return Err("edit access changed".into());
        }
        let resolved_at = timestamp();
        let updated = {
            let state = self.state.lock().await;
            let item = state
                .comments
                .iter()
                .find(|item| item.id == comment_id)
                .ok_or("unknown comment")?;
            if item.motivation != "editing" {
                return Err("that comment is not a suggestion".into());
            }
            if item.outcome == "accepted" {
                return Err("that suggestion was already accepted".into());
            }
            if item.outcome == "rejected" {
                return Ok(json!(item));
            }
            let mut updated = item.clone();
            updated.outcome = "rejected".into();
            updated.resolved = true;
            updated.resolved_at = Some(resolved_at);
            updated
        };
        if let Some(catalog) = self.catalog.get() {
            let row = catalog_comment_row(&self.slug, &updated)?;
            update_comment_row(catalog, row, actor).await?;
        }
        let mut state = self.state.lock().await;
        install_comment(&mut state, updated.clone());
        drop(state);
        self.broadcast(&json!({"type":"resolve","comment":updated}))
            .await;
        Ok(json!(updated))
    }
}
