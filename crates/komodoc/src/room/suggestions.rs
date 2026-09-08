//! Accepting and rejecting a suggestion: the one kind of comment that changes
//! the text when it is resolved.

use super::*;

/// A deleted passage has no text to match during rollback. Capture its
/// CRDT position so repeated passages and preceding edits remain distinct.
fn sticky_index_at_path(
    doc: &yrs::Doc,
    path: &str,
    at: u32,
    assoc: yrs::Assoc,
) -> Option<yrs::StickyIndex> {
    use yrs::{IndexedSequence, Map, Transact};
    let id = session::paths_of(doc)
        .into_iter()
        .find_map(|(id, name)| (name == path).then_some(id))?;
    let files = doc.get_or_insert_map(session::FILES);
    let txn = doc.transact();
    let yrs::Out::YText(text) = files.get(&txn, &id)? else {
        return None;
    };
    text.sticky_index(&txn, at, assoc)
}

fn offset_of_sticky_index(doc: &yrs::Doc, index: &yrs::StickyIndex) -> Option<u32> {
    use yrs::Transact;
    index.get_offset(&doc.transact()).map(|offset| offset.index)
}

impl Room {
    async fn broadcast_current_state(&self) {
        let update = {
            let state = self.state.lock().await;
            session::encode_state(&state.session.doc)
        };
        self.broadcast(&json!({
            "type": "y-update",
            "update": encode_update(&update),
        }))
        .await;
    }

    async fn broadcast_accept_and_current(&self, update: &[u8]) {
        self.broadcast(&json!({
            "type": "y-update",
            "update": encode_update(update),
        }))
        .await;
        self.broadcast_current_state().await;
    }

    /// Undo only the acceptance edit in the current document. Restoring the
    /// complete pre-accept tree would erase keystrokes that arrived while the
    /// checkpoint was being written. The source context identifies the
    /// proposal's current occurrence, so the compensating CRDT edit leaves
    /// unrelated changes in place and is relayed like any other server edit.
    async fn rollback_accept_edit(
        &self,
        source: &SourceAnchor,
        proposed: &str,
        accepted_update: &[u8],
        rollback_position: Option<yrs::StickyIndex>,
    ) -> Result<(), WriteError> {
        let compensation = {
            let mut state = self.state.lock().await;
            let live = session::texts_of(&state.session.doc)
                .get(&source.path)
                .cloned()
                .unwrap_or_default();
            let at = if proposed.is_empty() {
                // An empty proposal deleted the source passage. Do not use
                // `locate_anchor` as an indication that the edit was not
                // applied: with repeated passages it can find a later copy
                // after the accepted copy was deleted. Resolve the CRDT
                // position captured before the edit instead.
                rollback_position.as_ref().and_then(|position| {
                    offset_of_sticky_index(&state.session.doc, position).map(|at| at as usize)
                })
            } else {
                let proposed_anchor = SourceAnchor {
                    path: source.path.clone(),
                    exact: proposed.to_string(),
                    prefix: source.prefix.clone(),
                    suffix: source.suffix.clone(),
                    position: source.position,
                };
                locate_anchor(&live, &proposed_anchor)
            };
            if let Some(at) = at {
                match session::apply_path_edits(
                    &state.session.doc,
                    &source.path,
                    &[komodoc_text::Edit {
                        at,
                        delete: len16(proposed),
                        insert: source.exact.clone(),
                    }],
                ) {
                    Some(update) => {
                        state.session.mark_dirty(now_unix());
                        state.session.generation += 1;
                        state.session.updated_at = now_unix();
                        (Some(update), Vec::new())
                    }
                    None => (None, session::encode_state(&state.session.doc)),
                }
            } else {
                (None, session::encode_state(&state.session.doc))
            }
        };
        if let (Some(update), _) = &compensation {
            self.broadcast(&json!({
                "type": "y-update",
                "update": encode_update(accepted_update),
            }))
            .await;
            self.broadcast(&json!({
                "type": "y-update",
                "update": encode_update(update),
            }))
            .await;
            // The peers must see both sides of the compensating edit even if
            // persisting the compensation fails.  A caller can retry the
            // write, but it cannot reconstruct a broadcast that was skipped
            // behind the failed storage operation.
            return self.write_session(false, true).await.map(|_| ());
        }
        let full = compensation.1;
        self.broadcast(&json!({
            "type": "y-update",
            "update": encode_update(accepted_update),
        }))
        .await;
        self.broadcast(&json!({
            "type": "y-update",
            "update": encode_update(&full),
        }))
        .await;
        Err(WriteError::Storage(
            "could not locate the accepted proposal to roll it back".into(),
        ))
    }

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
        by: impl Into<Attribution>,
    ) -> Result<Accepted, AcceptError> {
        let by = &by.into();
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
                self.broadcast_current_state().await;
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
        if self.catalog.get().is_some() && request_id.is_empty() {
            return Err(AcceptError::Refused(
                "accepting a suggestion requires a request id".into(),
            ));
        }
        // Reserve the decision in cold SQLite before touching the CRDT.  This
        // makes a retry after a process crash resume the same acceptance and
        // lets a retry answer from the durable receipt even when this room's
        // comment cache is stale.
        let acceptance_digest = request_digest(&json!({
            "comment_id": comment_id,
            "by": by.display(),
            "proposed": comment.proposed.clone(),
        }));
        let staged_update = if let (Some(catalog), false) =
            (self.catalog.get(), request_id.is_empty())
        {
            match begin_suggestion_accept(
                catalog,
                &self.slug,
                comment_id,
                request_id,
                &acceptance_digest,
                now_unix(),
            )
            .await
            .map_err(AcceptError::Failed)?
            {
                Some(done) => {
                    let resolved_at = done.resolved_at.clone().unwrap_or_default();
                    let mut state = self.state.lock().await;
                    if let Some(index) =
                        state.comments.iter().position(|item| item.id == comment_id)
                    {
                        state.comments[index].resolved = true;
                        state.comments[index].resolved_at = Some(resolved_at.clone());
                        state.comments[index].resolved_in = done.resolved_in.clone();
                        state.comments[index].outcome = "accepted".to_string();
                        state.comments[index].accept_request = request_id.to_string();
                    }
                    drop(state);
                    self.broadcast_current_state().await;
                    return Ok(Accepted::Noop {
                        sha: done.resolved_in,
                        resolved_at,
                    });
                }
                None => {
                    if let Some((recorded_id, sha, resolved_at)) = suggestion_accept_checkpoint(
                        catalog,
                        &self.slug,
                        request_id,
                        &acceptance_digest,
                    )
                    .await
                    .map_err(AcceptError::Failed)?
                    {
                        if recorded_id != comment_id {
                            return Err(AcceptError::Failed(
                                "suggestion acceptance receipt names another comment".into(),
                            ));
                        }
                        finish_suggestion_accept(
                            catalog,
                            &self.slug,
                            comment_id,
                            request_id,
                            &acceptance_digest,
                            &sha,
                            &resolved_at,
                        )
                        .await
                        .map_err(AcceptError::Failed)?;
                        let mut state = self.state.lock().await;
                        if let Some(index) =
                            state.comments.iter().position(|item| item.id == comment_id)
                        {
                            state.comments[index].resolved = true;
                            state.comments[index].resolved_at = Some(resolved_at.clone());
                            state.comments[index].resolved_in = sha.clone();
                            state.comments[index].outcome = "accepted".to_string();
                            state.comments[index].accept_request = request_id.to_string();
                        }
                        drop(state);
                        self.broadcast_current_state().await;
                        return Ok(Accepted::Noop { sha, resolved_at });
                    }
                    suggestion_accept_update(catalog, &self.slug, request_id, &acceptance_digest)
                        .await
                        .map_err(AcceptError::Failed)?
                }
            }
        } else {
            None
        };

        // Step 2 of the spec: the passage as the live text has it now. A
        // first look, without the lock held across the storage read below:
        // it decides whether the checkpoint the suggestion was made on is
        // needed at all. The edit itself is computed again under the lock
        // that applies it, because `restore_write` keeps restores out but
        // not keystrokes, and an offset measured before an `await` is an
        // offset into a text that may since have moved.
        let base_text = if staged_update.is_some() {
            None
        } else {
            let live_text_at_first = {
                let state = self.state.lock().await;
                session::texts_of(&state.session.doc)
                    .get(&source.path)
                    .cloned()
                    .unwrap_or_default()
            };
            if locate_anchor(&live_text_at_first, &source).is_some() {
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
                        .find(|point| {
                            point.sha == comment.revision || point.content_sha() == comment.revision
                        })
                        .cloned()
                };
                // A resident room intentionally keeps only a bounded history
                // tail. Resolve an older suggestion base from SQLite so cold
                // restart and long-lived documents do not turn a valid request
                // into a spurious stale error.
                if base_point.is_none() {
                    if let Some(catalog) = self.catalog.get() {
                        let point =
                            read_checkpoint_by_content_sha(catalog, &self.slug, &comment.revision)
                                .await
                                .map_err(AcceptError::Failed)?;
                        base_point = point.map(|point| crate::document::history::Checkpoint {
                            sha: point.sha,
                            tree_sha: point.tree_sha,
                            parent: point.parent,
                            at: point.at,
                            by: point.by,
                            by_account: point.by_account,
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
            }
        };

        let mut rollback_position = None;
        let update = if let Some(update) = staged_update {
            update
        } else {
            // Plan the edit on a scratch Y.Doc first. This produces the exact
            // CRDT update without mutating the live document, so the prepared
            // receipt can durably record it before the live apply below.
            let update = {
                let state = self.state.lock().await;
                let live_text = session::texts_of(&state.session.doc)
                    .get(&source.path)
                    .cloned()
                    .unwrap_or_default();
                let edits = if let Some(at) = locate_anchor(&live_text, &source) {
                    if proposed.is_empty() {
                        rollback_position = sticky_index_at_path(
                            &state.session.doc,
                            &source.path,
                            at as u32,
                            yrs::Assoc::Before,
                        );
                    }
                    vec![komodoc_text::Edit {
                        at,
                        delete: len16(&source.exact),
                        insert: proposed.clone(),
                    }]
                } else {
                    // Found a moment ago and gone now is a keystroke that
                    // landed in between; retrying takes the merge path.
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
                let scratch = session::new_doc();
                session::apply_update(&scratch, &session::encode_state(&state.session.doc))
                    .map_err(AcceptError::Failed)?;
                let planned = session::apply_path_edits(&scratch, &source.path, &edits)
                    .ok_or(AcceptError::Stale)?;
                // The scratch copy is the candidate state, so the encoded
                // ceiling is decided here -- before the live document takes
                // the edit and before any peer is shown it. An acceptance
                // that could not be journalled would be acknowledged to the
                // editor and then lost.
                let ceiling = self.config.persistence().max_encoded_snapshot_bytes;
                let encoded = session::encode_state(&scratch);
                if encoded.len() > ceiling {
                    return Err(AcceptError::Refused(
                        crate::config::WriteRefusal::Permanent(
                            crate::config::SizeRefusal::Encoded {
                                bytes: encoded.len(),
                                ceiling,
                            },
                        )
                        .message(),
                    ));
                }
                // A diff can refer to Yjs structs created by an unsaved
                // keystroke immediately before accept.  If the process dies
                // before the checkpoint writes that session, replaying only
                // the diff against the old session leaves those structs
                // missing.  A prepared catalogue receipt therefore carries
                // the complete post-accept state, which is itself an
                // idempotent Yjs update and includes every dependency.
                if self.catalog.get().is_some() && !request_id.is_empty() {
                    encoded
                } else {
                    planned
                }
            };
            if let (Some(catalog), false) = (self.catalog.get(), request_id.is_empty()) {
                stage_suggestion_accept_update(
                    catalog,
                    &self.slug,
                    comment_id,
                    request_id,
                    &acceptance_digest,
                    &update,
                )
                .await
                .map_err(AcceptError::Failed)?;
            }
            update
        };
        {
            let mut state = self.state.lock().await;
            session::apply_update(&state.session.doc, &update).map_err(AcceptError::Failed)?;
            state.session.mark_dirty(now_unix());
            state.session.generation += 1;
            state.session.updated_at = now_unix();
            state.session.by = by.clone();
        }

        let sha = match self.checkpoint_now("accept", by).await {
            Ok(Some(sha)) => sha,
            Ok(None) => {
                if self.catalog.get().is_some() && !request_id.is_empty() {
                    // The prepared receipt contains the exact update. Keep
                    // it pending and let the retry apply those same bytes;
                    // aborting here would leave a crash window in which the
                    // live edit is durable but its receipt is gone.
                    self.broadcast_accept_and_current(&update).await;
                } else {
                    let _ = self
                        .rollback_accept_edit(
                            &source,
                            &proposed,
                            &update,
                            rollback_position.clone(),
                        )
                        .await;
                }
                return Err(AcceptError::Failed(
                    "could not create the accept checkpoint".to_string(),
                ));
            }
            Err(err) => {
                if self.catalog.get().is_some() && !request_id.is_empty() {
                    self.broadcast_accept_and_current(&update).await;
                } else {
                    let _ = self
                        .rollback_accept_edit(
                            &source,
                            &proposed,
                            &update,
                            rollback_position.clone(),
                        )
                        .await;
                }
                return Err(AcceptError::from(err));
            }
        };

        let resolved_at = timestamp();
        if let Some(catalog) = self.catalog.get() {
            if !request_id.is_empty() {
                // Recording the checkpoint in the receipt and settling the
                // comment are one job: a caller that goes away between them
                // used to leave the second unissued, and the service now owns
                // both once the request is dispatched. On failure the
                // checkpoint remains valid and the prepared receipt makes the
                // next identical request resumable, so this does not claim
                // success while the durable comment outcome lags.
                if let Err(error) = record_and_finish_suggestion_accept(
                    catalog,
                    &self.slug,
                    comment_id,
                    request_id,
                    &acceptance_digest,
                    &sha,
                    &resolved_at,
                )
                .await
                {
                    self.broadcast_accept_and_current(&update).await;
                    return Err(AcceptError::Failed(error));
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
        let _restore_writer = self.restore_write.lock().await;
        if !self.hold().await {
            return Err("this room is held by another server".into());
        }
        let mut state = self.state.lock().await;
        let Some(index) = state.comments.iter().position(|item| item.id == comment_id) else {
            return Err("unknown comment".into());
        };
        if state.comments[index].motivation != "editing" {
            return Err("this comment is not a suggestion".into());
        }
        if let Some(catalog) = self.catalog.get() {
            match pending_suggestion_accept(catalog, &self.slug, comment_id).await {
                Ok(true) => return Err("a suggestion acceptance is still pending".into()),
                Ok(false) => {}
                Err(_) => return Err("could not save that comment; try again".into()),
            }
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
        // Prepared on a copy and applied once the row is durable: a caller
        // cancelled at the write must not leave this room showing a rejection
        // that nothing recorded and no peer was told about.
        let mut rejected = state.comments[index].clone();
        rejected.resolved = true;
        rejected.resolved_at = Some(timestamp());
        rejected.resolved_in = current;
        rejected.outcome = "rejected".to_string();
        let persisted = if let Some(catalog) = self.catalog.get() {
            match catalog_comment_row(&self.slug, &rejected) {
                Ok(row) => update_comment_row(catalog, row).await,
                Err(error) => Err(error),
            }
        } else {
            state.comments[index] = rejected.clone();
            self.save(&mut state).await
        };
        if persisted.is_err() {
            state.comments[index].resolved = was_resolved;
            state.comments[index].resolved_at = was_resolved_at;
            state.comments[index].resolved_in = was_resolved_in;
            state.comments[index].outcome = was_outcome;
            return Err("could not save that comment; try again".into());
        }
        state.comments[index] = rejected;
        let target = &state.comments[index];
        Ok(json!({
            "type": "reject", "comment_id": target.id,
            "resolved": target.resolved, "resolved_at": target.resolved_at,
            "resolved_in": target.resolved_in,
        }))
    }
}
