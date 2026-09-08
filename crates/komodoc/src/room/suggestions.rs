//! Accepting and rejecting a suggestion: the one kind of comment that changes
//! the text when it is resolved.

use super::*;

impl Room {
    /// Accepts a suggestion: applies its proposal to the live source through
    /// the session, exactly as `restore_and_checkpoint` applies a restore,
    /// and records a checkpoint. Held under `restore_write`, the same lock a
    /// restore takes, so the two can never interleave -- an accept mutates
    /// the document and takes a checkpoint just as a restore does, and a
    /// restore reading the tree mid-accept would see half of one.
    ///
    /// `request_id` is the caller's idempotency token: retrying the request
    /// that already accepted this comment answers with what happened the
    /// first time rather than reapplying the edit.
    pub async fn accept_suggestion(
        &self,
        comment_id: &str,
        request_id: &str,
        by: &str,
    ) -> Result<Accepted, AcceptError> {
        let _restore_writer = self.restore_write.lock().await;
        if self.read_only() {
            return Err(AcceptError::Failed(
                "this room is held by another server".into(),
            ));
        }

        let comment = {
            let state = self.state.lock().await;
            state
                .comments
                .iter()
                .find(|item| item.id == comment_id)
                .cloned()
        };
        let Some(comment) = comment else {
            return Err(AcceptError::Refused("unknown comment".into()));
        };
        if comment.motivation != "editing" {
            return Err(AcceptError::Refused(
                "this comment is not a suggestion".into(),
            ));
        }
        if comment.outcome == "accepted" {
            // Reversing a rejection is allowed; repeating an acceptance
            // itself is a retry, told apart from a second, unrelated accept
            // by the request id the first one recorded.
            if !request_id.is_empty() && comment.accept_request == request_id {
                return Ok(Accepted::Noop {
                    sha: comment.resolved_in.clone(),
                    resolved_at: comment.resolved_at.clone().unwrap_or_default(),
                });
            }
            return Err(AcceptError::Refused(
                "this suggestion is already accepted".into(),
            ));
        }
        let Some(source) = comment.source.clone() else {
            return Err(AcceptError::Refused(
                "this suggestion has no source anchor; apply it by hand".into(),
            ));
        };
        let proposed = comment.proposed.clone().unwrap_or_default();
        // Reserve the decision in cold SQLite before touching the CRDT.  This
        // makes a retry after a process crash resume the same acceptance and
        // lets a retry answer from the durable receipt even when this room's
        // comment cache is stale.
        let acceptance_digest = request_digest(&json!({
            "comment_id": comment_id,
            "by": by,
            "proposed": comment.proposed.clone(),
        }));
        if let (Some(catalog), false) = (self.catalog.get(), request_id.is_empty()) {
            match catalog.begin_suggestion_accept(
                &self.slug,
                comment_id,
                request_id,
                &acceptance_digest,
                now_unix(),
            ) {
                Ok(Some(done)) => {
                    return Ok(Accepted::Noop {
                        sha: done.resolved_in,
                        resolved_at: done.resolved_at.unwrap_or_default(),
                    });
                }
                Ok(None) => {}
                Err(error) => return Err(AcceptError::Failed(error.to_string())),
            }
        }

        // Step 2 of the spec: the passage as the live text has it now. A
        // first look, without the lock held across the storage read below:
        // it decides whether the checkpoint the suggestion was made on is
        // needed at all. The edit itself is computed again under the lock
        // that applies it, because `restore_write` keeps restores out but
        // not keystrokes, and an offset measured before an `await` is an
        // offset into a text that may since have moved.
        let live_text_at_first = {
            let state = self.state.lock().await;
            session::texts_of(&state.session.doc)
                .get(&source.path)
                .cloned()
                .unwrap_or_default()
        };
        let base_text = if locate_anchor(&live_text_at_first, &source).is_some() {
            None
        } else {
            // Step 3: the passage is not where it was. Merge against the
            // checkpoint the suggestion was made on, the same three-way merge
            // `komodoc sync` uses for a stale file.
            let mut base_point = {
                let state = self.state.lock().await;
                state
                    .manifest
                    .checkpoints
                    .iter()
                    .find(|point| point.sha == comment.revision)
                    .cloned()
            };
            // A resident room intentionally keeps only a bounded history
            // tail.  Resolve an older suggestion base from SQLite so cold
            // restart and long-lived documents do not turn a valid request
            // into a spurious stale error.
            if base_point.is_none() {
                if let Some(catalog) = self.catalog.get() {
                    base_point = catalog
                        .checkpoint(&self.slug, &comment.revision)
                        .map_err(|error| AcceptError::Failed(error.to_string()))?
                        .map(|point| crate::document::history::Checkpoint {
                            sha: point.sha,
                            tree_sha: point.tree_sha,
                            parent: point.parent,
                            at: point.at,
                            by: point.by,
                            why: point.why,
                            source_format: point.source_format,
                            size: point.size,
                            label: point.label,
                            commit: point.git_commit,
                            dirty: point.dirty,
                            tree: true,
                            changed: point
                                .changed
                                .and_then(|value| serde_json::from_str(&value).ok())
                                .unwrap_or_default(),
                        });
                }
            }
            let Some(base_point) = base_point else {
                return Err(AcceptError::Stale);
            };
            let (base_tree, base_bodies) = self
                .checkpoint_texts(&base_point)
                .await
                .map_err(AcceptError::Failed)?;
            let Some(base_text) = base_tree
                .files
                .get(&source.path)
                .and_then(|entry| base_bodies.get(&entry.sha))
                .cloned()
            else {
                return Err(AcceptError::Stale);
            };
            Some(base_text)
        };

        let (rollback_tree, rollback_bodies, rollback_format) = {
            let state = self.state.lock().await;
            let (tree, bodies) = tree_of(&state.session.doc, &state.session.asset_sizes);
            (tree, bodies, state.session.format.clone())
        };
        let update = {
            let mut state = self.state.lock().await;
            let live_text = session::texts_of(&state.session.doc)
                .get(&source.path)
                .cloned()
                .unwrap_or_default();
            let edits = if let Some(at) = locate_anchor(&live_text, &source) {
                vec![komodoc_text::Edit {
                    at,
                    delete: len16(&source.exact),
                    insert: proposed.clone(),
                }]
            } else {
                // Found a moment ago and gone now is a keystroke that landed
                // in between; the base was not fetched for it, and refusing
                // is honest -- the editor's retry takes the merge path.
                let Some(base_text) = base_text.as_deref() else {
                    return Err(AcceptError::Stale);
                };
                let Some(base_at) = locate_anchor(base_text, &source) else {
                    return Err(AcceptError::Stale);
                };
                let remote = apply_edit_str(
                    base_text,
                    &komodoc_text::Edit {
                        at: base_at,
                        delete: len16(&source.exact),
                        insert: proposed.clone(),
                    },
                );
                let merged = komodoc_text::merge(base_text, &live_text, &remote);
                if !merged.conflicts.is_empty() {
                    return Err(AcceptError::Stale);
                }
                komodoc_text::diff(&live_text, &merged.text)
            };
            let Some(update) = session::apply_path_edits(&state.session.doc, &source.path, &edits)
            else {
                // The path the anchor named is no longer part of the
                // document at all -- its file was renamed or removed since
                // the suggestion was made. Nothing here is a passage anymore.
                return Err(AcceptError::Stale);
            };
            state.session.mark_dirty(now_unix());
            state.session.generation += 1;
            state.session.updated_at = now_unix();
            state.session.by = by.to_string();
            update
        };

        let sha = match self.checkpoint_now("accept", by).await {
            Ok(Some(sha)) => sha,
            Ok(None) => {
                let _ = self
                    .rollback_publication(&rollback_tree, &rollback_bodies, &rollback_format)
                    .await;
                return Err(AcceptError::Failed(
                    "could not create the accept checkpoint".to_string(),
                ));
            }
            Err(err) => {
                let _ = self
                    .rollback_publication(&rollback_tree, &rollback_bodies, &rollback_format)
                    .await;
                return Err(AcceptError::Failed(err));
            }
        };

        let resolved_at = timestamp();
        if let Some(catalog) = self.catalog.get() {
            if !request_id.is_empty() {
                if let Err(error) = catalog.finish_suggestion_accept(
                    &self.slug,
                    comment_id,
                    request_id,
                    &acceptance_digest,
                    &sha,
                    &resolved_at,
                ) {
                    // The checkpoint remains valid and the prepared receipt
                    // makes the next identical request resumable.  Do not
                    // claim success while the durable comment outcome lags.
                    return Err(AcceptError::Failed(error.to_string()));
                }
            }
        }
        let mut state = self.state.lock().await;
        if let Some(index) = state.comments.iter().position(|item| item.id == comment_id) {
            state.comments[index].resolved = true;
            state.comments[index].resolved_at = Some(resolved_at.clone());
            state.comments[index].resolved_in = sha.clone();
            state.comments[index].outcome = "accepted".to_string();
            state.comments[index].accept_request = request_id.to_string();
            if self.catalog.get().is_none() || request_id.is_empty() {
                self.save(&mut state).await.map_err(AcceptError::Failed)?;
            }
        }

        Ok(Accepted::Applied {
            update,
            sha,
            resolved_at,
        })
    }

    /// Rejects a suggestion: resolves it without touching the document. Like
    /// `resolve`, which this shares its shape with -- the two differ only in
    /// that a reject also records the outcome and always resolves (rather
    /// than toggling), and that an accepted suggestion refuses it, because
    /// the text it proposed is already in the document.
    pub async fn reject_suggestion(&self, comment_id: &str) -> Result<Value, String> {
        let mut state = self.state.lock().await;
        let Some(index) = state.comments.iter().position(|item| item.id == comment_id) else {
            return Err("unknown comment".into());
        };
        if state.comments[index].motivation != "editing" {
            return Err("this comment is not a suggestion".into());
        }
        if state.comments[index].outcome == "accepted" {
            return Err(
                "an accepted suggestion cannot be rejected; restore the checkpoint instead".into(),
            );
        }
        let current = state
            .manifest
            .latest()
            .map(|point| point.sha.clone())
            .unwrap_or_default();
        let (was_resolved, was_resolved_at, was_resolved_in, was_outcome) = (
            state.comments[index].resolved,
            state.comments[index].resolved_at.clone(),
            state.comments[index].resolved_in.clone(),
            state.comments[index].outcome.clone(),
        );
        state.comments[index].resolved = true;
        state.comments[index].resolved_at = Some(timestamp());
        state.comments[index].resolved_in = current;
        state.comments[index].outcome = "rejected".to_string();
        if self.save(&mut state).await.is_err() {
            state.comments[index].resolved = was_resolved;
            state.comments[index].resolved_at = was_resolved_at;
            state.comments[index].resolved_in = was_resolved_in;
            state.comments[index].outcome = was_outcome;
            return Err("could not save that comment; try again".into());
        }
        let target = &state.comments[index];
        Ok(json!({
            "type": "reject", "comment_id": target.id,
            "resolved": target.resolved, "resolved_at": target.resolved_at,
            "resolved_in": target.resolved_in,
        }))
    }
}
