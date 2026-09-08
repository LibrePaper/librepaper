//! The timeline as the room writes it: taking a checkpoint, labelling one,
//! restoring to one, and the budget that bounds how many a document keeps.

use super::*;

impl Room {
    /// Takes a checkpoint, if the text differs from the last one. Returns the
    /// SHA of the checkpoint that now stands for the current text, or None
    /// when the request was deferred.
    ///
    /// The order of writes is the one `docs/specs/history.md` sets, with the text
    /// blobs in front of it, so that a crash leaves nothing worse than an
    /// untidy history: the blobs, then the tree, then the session state, then
    /// the index entry, then the manifest. Nothing ever names an object that
    /// is not there; what a crash can leave is an object nothing names, which
    /// costs storage and loses nothing. A manifest missing its newest entry is
    /// repaired by the next checkpoint, which finds the object present and
    /// names it as `parent` -- `repair` below is that.
    pub async fn checkpoint(&self, why: &str, by: &str) -> Result<Option<String>, String> {
        self.checkpoint_impl(why, by, true, false, None, false)
            .await
    }

    /// Takes a checkpoint immediately, even when the ordinary deliberate-save
    /// debounce window is still open. An accept uses this: the edit it just
    /// made is a deliberate act by the editor, not a keystroke to wait out.
    pub async fn checkpoint_now(&self, why: &str, by: &str) -> Result<Option<String>, String> {
        self.checkpoint_impl(why, by, false, false, None, false)
            .await
    }

    /// Reserve the explicit checkpoint budget before a publication mutates
    /// the live CRDT. The subsequent publication checkpoint skips its normal
    /// admission because this token already belongs to it.
    pub fn reserve_publication_checkpoint(&self) -> Result<(), String> {
        if let Some(catalog) = self.catalog.get() {
            catalog
                .admit_checkpoint_with_limits(
                    &self.slug,
                    now_unix(),
                    false,
                    self.config.session.checkpoint_owner_per_hour,
                    self.config.session.checkpoint_deployment_per_hour,
                )
                .map(|_| ())
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub async fn checkpoint_publication_now(
        &self,
        why: &str,
        by: &str,
    ) -> Result<Option<String>, String> {
        self.checkpoint_impl(why, by, false, false, None, true)
            .await
    }

    /// The same immediate checkpoint while retaining one older checkpoint
    /// long enough for a restore to read its assets after quota shedding.
    pub(super) async fn checkpoint_now_protected(
        &self,
        why: &str,
        by: &str,
        protected: &str,
    ) -> Result<Option<String>, String> {
        self.checkpoint_impl(why, by, false, false, Some(protected), false)
            .await
    }

    /// Takes a checkpoint event even when its tree has the same content as a
    /// checkpoint already in the manifest. A restore is an event in the
    /// linear history, and deduplicating it would make restoring to an old
    /// revision silently disappear from the timeline. The event gets its own
    /// object key while retaining the same immutable tree bytes.
    pub(super) async fn checkpoint_restore(&self, by: &str) -> Result<Option<String>, String> {
        self.checkpoint_impl("restore", by, false, true, None, false)
            .await
    }

    pub(super) async fn checkpoint_impl(
        &self,
        why: &str,
        by: &str,
        defer: bool,
        force_event: bool,
        protected: Option<&str>,
        budget_reserved: bool,
    ) -> Result<Option<String>, String> {
        let _checkpoint_writer = self.checkpoint_write.lock().await;
        if self.read_only() {
            // Refused outright rather than left to fall through to the
            // deduplication branch below: on a read-only room whose direct
            // mutators are now no-ops (R23), the live tree never moves away
            // from the last checkpoint, so that branch would otherwise
            // report an unearned success for a request this server has no
            // business recording.
            return Err("this room is held by another server".into());
        }
        // Counted for the whole of this call, every early return included:
        // the guard's drop is what lets the sweep at the end of another
        // checkpoint know it is alone again.
        let _in_flight = InFlight::new(&self.checkpointing);
        let now = now_unix();
        let (tree, bodies, format, last, deferred, tree_generation) = {
            let mut state = self.state.lock().await;
            // A deliberate write inside the defer window is not refused; it
            // waits, and is taken when the window passes, if the text still
            // differs. A burst of saves is one mark in the timeline.
            let deferrable = defer && matches!(why, "cli" | "sync" | "restore" | "label");
            if deferrable
                && state.session.last_checkpoint_at > 0
                && now - state.session.last_checkpoint_at < CHECKPOINT_DEFER_SECONDS
            {
                state.session.asked = Some((why.to_string(), by.to_string()));
                (
                    crate::history::Tree::default(),
                    HashMap::new(),
                    String::new(),
                    String::new(),
                    true,
                    0,
                )
            } else {
                let (tree, bodies) = tree_of(&state.session.doc, &state.session.asset_sizes);
                // The main file can change by paths this room does not itself
                // mediate through a dedicated setter -- an applied CRDT
                // update is the ordinary one, but this is the safety net,
                // checked at the moment of every checkpoint: the tree's own
                // main path is what is actually about to be recorded, so the
                // format that travels with it has to agree, rather than
                // trust that `session.format` was already kept in step
                // (R27).
                let derived = format_from_path(&tree.main);
                let format = if !derived.is_empty() && derived != state.session.format {
                    state.session.format = derived.clone();
                    derived
                } else {
                    state.session.format.clone()
                };
                (
                    tree,
                    bodies,
                    format,
                    state.session.last_checkpoint.clone(),
                    false,
                    // What this checkpoint's tree covers, so a later tick can
                    // tell whether the document has moved on without hashing
                    // the whole tree again (R26).
                    state.session.generation,
                )
            }
        };
        if deferred {
            return Ok(None);
        }
        let content_sha = tree.digest();
        let duplicate = if force_event {
            false
        } else {
            let state = self.state.lock().await;
            let resident_duplicate = state.manifest.checkpoints.iter().rev().any(|point| {
                point.sha == content_sha
                    || (!point.tree_sha.is_empty() && point.tree_sha == content_sha)
            });
            drop(state);
            let catalog_duplicate = match self.catalog.get() {
                Some(catalog) => catalog
                    .checkpoint(&self.slug, &content_sha)
                    .map_err(|error| error.to_string())?
                    .is_some(),
                None => false,
            };
            resident_duplicate || catalog_duplicate
        };
        let mut budget_taken = budget_reserved && self.catalog.get().is_some();
        if !duplicate && !budget_reserved {
            if let Some(catalog) = self.catalog.get() {
                let automatic = matches!(why, "automatic" | "quiet");
                match catalog.admit_checkpoint_with_limits(
                    &self.slug,
                    now_unix(),
                    automatic,
                    self.config.session.checkpoint_owner_per_hour,
                    self.config.session.checkpoint_deployment_per_hour,
                ) {
                    Ok(true) => budget_taken = true,
                    Ok(false) => return Ok(None),
                    Err(error) => {
                        return Err(error.to_string());
                    }
                }
            }
        }

        let sha = if force_event {
            // Include the current parent and a fresh timestamp. The random
            // tail prevents two same-second restores of the same tree from
            // ever aliasing one storage object.
            let mut identity = tree.to_bytes();
            identity.extend_from_slice(timestamp().as_bytes());
            identity.extend_from_slice(crate::auth::random_bytes(8).as_slice());
            hex::encode(sha2::Sha256::digest(identity))
        } else {
            content_sha.clone()
        };
        // Quiet after quiet costs nothing: the same text is the same
        // checkpoint, and a checkpoint already in the manifest is not written
        // again and adds no entry.
        {
            let mut state = self.state.lock().await;
            state.session.asked = None;
            if !force_event {
                let existing = state
                    .manifest
                    .checkpoints
                    .iter()
                    .rev()
                    .find(|point| {
                        point.sha == content_sha
                            || (!point.tree_sha.is_empty() && point.tree_sha == content_sha)
                    })
                    .map(|point| point.sha.clone());
                if let Some(existing) = existing {
                    // Reusing immutable content is not a new checkpoint -- the
                    // manifest's chronology and its bytes are left alone, since
                    // `shed` and pruning key on SHAs and a repeated entry would
                    // only confuse them -- but it does become the current
                    // revision again, so the in-memory pointer and the index
                    // head both have to say so, or a reader asking what the
                    // document says now gets an old answer (R17).
                    // `last_checkpoint_at` is deliberately left untouched:
                    // unchanged content is not a new checkpoint, and refreshing
                    // it would push the next deliberate checkpoint that actually
                    // changes something into the defer window (R26).
                    let moved = state.session.last_checkpoint != existing;
                    state.session.last_checkpoint = existing.clone();
                    state.session.last_tree = Some(tree.clone());
                    state.session.checkpoint_generation = tree_generation;
                    let format = state.session.format.clone();
                    let main = tree.main.clone();
                    drop(state);
                    // A replacement of identical content still has a
                    // prepared publication receipt. Reuse the already
                    // durable checkpoint descriptor in that receipt so the
                    // strict staged commit has the same coverage proof as a
                    // newly written tree.
                    if let Some(catalog) = self.catalog.get() {
                        if catalog
                            .document(&self.slug)
                            .map_err(|error| error.to_string())?
                            .and_then(|document| document.pending_publication)
                            .is_some()
                        {
                            if let Some(checkpoint) = catalog
                                .checkpoint(&self.slug, &existing)
                                .map_err(|error| error.to_string())?
                            {
                                catalog
                                    .stage_publication_checkpoint(&self.slug, &checkpoint)
                                    .map_err(|error| error.to_string())?;
                            }
                        }
                    }
                    if moved {
                        self.record_size_now(Some(&existing), &format, &main).await;
                    }
                    if budget_reserved {
                        self.refund_checkpoint_budget(now);
                    }
                    if why == "automatic" {
                        if let Some(catalog) = self.catalog.get() {
                            let _ = catalog.touch_auto_checkpoint(&self.slug, now);
                        }
                    }
                    return Ok(Some(existing));
                }
            }
        }
        if !self.hold().await {
            if budget_taken {
                self.refund_checkpoint_budget(now);
            }
            return Err("this room is held by another server".into());
        }

        // 1. the text blobs, before anything names them. A digest already
        //    written is not written again, which is what makes a chapter
        //    untouched between twenty checkpoints cost one object.
        let unwritten: Vec<(String, String)> = {
            let state = self.state.lock().await;
            bodies
                .iter()
                .filter(|(digest, _)| !state.session.blobs_written.contains(*digest))
                .map(|(digest, body)| (digest.clone(), body.clone()))
                .collect()
        };
        for (digest, body) in &unwritten {
            if let Err(err) = self
                .put_accounted(
                    &crate::blob::blob_key(&self.storage_id, digest),
                    body.clone().into_bytes(),
                    "text",
                )
                .await
            {
                if budget_taken {
                    self.refund_checkpoint_budget(now);
                }
                return Err(err.to_string());
            }
        }
        {
            let mut state = self.state.lock().await;
            for (digest, _) in &unwritten {
                state.session.blobs_written.insert(digest.clone());
            }
        }

        // 2. the tree, which names them.
        if let Err(err) = self
            .put_accounted(
                &checkpoint_key(&self.storage_id, &sha),
                tree.to_bytes(),
                "tree",
            )
            .await
        {
            if budget_taken {
                self.refund_checkpoint_budget(now);
            }
            return Err(err.to_string());
        }

        // 3. the session state, so a restart comes back at or after the
        //    checkpoint rather than before it. The generation is captured in
        //    the same breath as the bytes: if a newer edit lands before this
        //    server gets to clear `dirty` below, the two disagree and `dirty`
        //    is left set, so that edit is never reported as saved when it is
        //    not yet on disk (R07).
        let (session_size, durable_sequence) = match self.write_session(false, false).await {
            Ok(Some(result)) => result,
            Ok(None) => unreachable!("an unconditional session write returns its size"),
            Err(err) => {
                if budget_taken {
                    self.refund_checkpoint_budget(now);
                }
                return Err(err);
            }
        };

        // What the parent recorded, so this entry can say which paths moved.
        // Held in memory from one checkpoint to the next; read back only on
        // the first checkpoint after a cold start, which is the only time
        // this server has not seen the parent itself.
        let parent_tree = self.parent_tree().await;

        // 4. the index entry, then 5. the manifest -- staged from
        // `state.manifest` and written under the manifest write gate, so a
        // concurrent `label` can never land between the staging and the write
        // and be discarded by this checkpoint's now-stale idea of the
        // manifest (R09), and a failed write never lands in memory, so a
        // retry recomputes from the real manifest rather than quietly
        // no-op-ing through the deduplication branch above (R08).
        let shed;
        {
            let _manifest_writer = self.manifest_write.lock().await;
            let (mut staged, repair_format) = {
                let state = self.state.lock().await;
                (state.manifest.clone(), state.session.format.clone())
            };
            self.repair(&mut staged, &repair_format, &last).await;
            let parent = staged
                .latest()
                .map(|point| point.sha.clone())
                .unwrap_or_default();
            staged.checkpoints.push(Checkpoint {
                sha: sha.clone(),
                tree_sha: content_sha.clone(),
                parent,
                at: timestamp(),
                by: by.to_string(),
                why: why.to_string(),
                source_format: format.clone(),
                size: tree.size(),
                label: String::new(),
                commit: String::new(),
                dirty: false,
                tree: true,
                changed: tree.changed_from(parent_tree.as_ref()),
            });

            // A checkpoint is never refused, because refusing it would lose
            // work. What gives instead is the oldest history: the ceilings
            // shed the oldest unlabelled checkpoints, and the oldest
            // labelled ones after them, until the document fits.
            let ceiling = self.allowance(session_size).await;
            let keep_count = self.config.session.history_max;
            // In catalogue mode this is the resident tail, not necessarily
            // the complete history.  It is still safe to shed entries that
            // are present here: every removed row is explicitly deleted from
            // SQLite below, while rows omitted from the tail are never
            // inferred to be garbage.  (With the normal tail of 64 this also
            // handles small configured history caps exactly.)
            let shed_now = staged.shed_protected(protected.unwrap_or_default(), |manifest| {
                (keep_count == 0 || manifest.checkpoints.len() <= keep_count)
                    && (ceiling < 0 || manifest.bytes() <= ceiling)
            });

            // What the document costs: the live session, its history, its
            // figures and its renderings. The quota counts each object once
            // -- a text blob and an asset are each charged where they are
            // stored, and the tree that names them is bookkeeping rather
            // than a third copy. Read straight off the locked state instead
            // of through `assets_bytes`/`renderings_bytes`, which lock it
            // themselves.
            let (assets, renderings): (i64, i64) = {
                let state = self.state.lock().await;
                (
                    state.session.asset_sizes.values().sum(),
                    state.session.rendering_sizes.values().sum(),
                )
            };
            let staged_bytes = if let Some(catalog) = self.catalog.get() {
                // `staged` is intentionally only the resident tail.  Charge
                // the complete persisted history rather than silently
                // undercounting older rows that are not resident here.
                catalog
                    .checkpoint_stats(&self.slug)
                    .map(|(_, bytes)| bytes.saturating_add(tree.size()))
                    .unwrap_or_else(|_| staged.bytes())
            } else {
                staged.bytes()
            };
            // The legacy object layout has no catalogue transaction to join
            // the checkpoint graph.  Publish its index head before the
            // manifest, so a crash/failure between the two leaves a durable
            // receipt that `repair` can discover on the next checkpoint.  In
            // catalogue mode the manifest rows and head are committed by
            // `write_manifest` together, and the measurement is deliberately
            // staged only after all referenced objects are durable.
            if self.catalog.get().is_none() {
                self.record_size(
                    session_size + staged_bytes + assets + renderings,
                    Some(&sha),
                    &format,
                    &tree.main,
                )
                .await;
            }
            if let Err(err) = self.write_manifest(staged, durable_sequence).await {
                if budget_taken {
                    self.refund_checkpoint_budget(now);
                }
                return Err(err);
            }
            // The checkpoint graph is now durable. Only then advance the
            // catalogue head and measured accounting, so a reader can never
            // observe a SHA whose tree or manifest was lost between object
            // writes and publication.
            if self.catalog.get().is_some() {
                self.record_size(
                    session_size + staged_bytes + assets + renderings,
                    Some(&sha),
                    &format,
                    &tree.main,
                )
                .await;
            }
            let mut state = self.state.lock().await;
            state.session.last_tree = Some(tree.clone());
            state.session.last_checkpoint = sha.clone();
            state.session.last_checkpoint_at = now;
            // `write_session` may have advanced the durable cursor when an
            // internal mutation changed the document without incrementing
            // the in-memory generation first.  In that case the checkpoint
            // covers the generation now resident in state.  If a newer edit
            // arrived while objects were being written, retain the captured
            // generation so the room stays dirty and is flushed again.
            state.session.checkpoint_generation = if !state.session.dirty {
                state.session.generation
            } else {
                tree_generation
            };
            shed = shed_now;
        }
        if !shed.is_empty() {
            let keys: Vec<String> = shed
                .iter()
                .map(|sha| checkpoint_key(&self.storage_id, sha))
                .collect();
            let _ = self.blobs.delete(&keys).await;
            if let Some(catalog) = self.catalog.get() {
                for sha in &shed {
                    if let Err(error) = catalog.delete_checkpoint(&self.slug, sha) {
                        // The manifest has already been committed without the
                        // shed row.  A failed catalogue cleanup is safe to
                        // retry on the next checkpoint and must not make the
                        // successful checkpoint look like a publication
                        // failure.
                        eprintln!(
                            "warning: could not remove shed checkpoint {} for {}: {error}",
                            sha, self.slug
                        );
                    }
                }
            }
            for key in &keys {
                self.checkpoint_cache.invalidate(key).await;
            }
        }
        // The figures nothing refers to any more, once the tree and the
        // manifest that name what is kept are both written. This order is
        // what makes a crash leave an unreferenced object rather than a tree
        // pointing at one that is gone.
        self.prune_assets().await;
        // And the renderings the new manifest no longer keeps, on the same
        // pass and under the same write-order rule.
        self.prune_renderings().await;
        // And the text blobs under `history/<slug>/blobs/` that shedding a
        // checkpoint's tree just now, or some earlier one, left behind (R21).
        // Only when no other checkpoint of this room is mid-write: one that
        // has written its blobs and not yet its tree has objects nothing names,
        // and this pass would take them for garbage. The next lone checkpoint
        // sweeps instead; nothing is lost by waiting.
        if self.checkpointing.load(Ordering::Relaxed) == 1 {
            self.prune_blobs(&tree).await;
        }
        // The migration's one and only cleanup. A document stored the old way
        // has a rendered page and a source under the old keys; both are copies
        // of what is now a checkpoint, and this is the first moment at which
        // that is true. Before this point nothing has been removed, so a
        // deployment rolled back before its first checkpoint loses nothing.
        if let Some(store) = self.store.get() {
            store.drop_derived(&self.slug).await;
        }
        if why == "automatic" {
            if let Some(catalog) = self.catalog.get() {
                let _ = catalog.touch_auto_checkpoint(&self.slug, now);
            }
        }
        Ok(Some(sha))
    }

    /// Puts back a checkpoint the manifest lost, into a staged manifest the
    /// caller has not committed yet. The index names the newest checkpoint,
    /// so an index entry naming a SHA the manifest does not have, whose
    /// object is still there, is a write that got as far as recording the
    /// index but no further. `last` is this server's own idea of the newest
    /// checkpoint, carried in memory since the checkpoint before; the store's
    /// own head is consulted too, because a crash between recording the index
    /// and writing the manifest leaves exactly that head standing with
    /// nothing in the manifest naming it, and `load_session` seeds `last`
    /// from the manifest, not the index, so a restart does not otherwise
    /// supply it. Either candidate is added as the newest entry rather than
    /// guessed at.
    pub(super) async fn repair(&self, manifest: &mut Manifest, format: &str, last: &str) {
        let mut candidates = vec![last.to_string()];
        if let Some(store) = self.store.get() {
            match store.get_result(&self.slug).await {
                Ok(Some(entry)) => candidates.push(entry.sha),
                Ok(None) => {}
                Err(error) => {
                    eprintln!(
                        "warning: could not read catalogue entry {} during repair: {error}",
                        self.slug
                    );
                    self.read_only.store(true, Ordering::Relaxed);
                }
            }
        }
        for candidate in candidates {
            if candidate.is_empty() || manifest.has(&candidate) {
                continue;
            }
            let raw = match self
                .blobs
                .get(&checkpoint_key(&self.storage_id, &candidate))
                .await
            {
                Ok(raw) => raw,
                Err(_) => continue,
            };
            // Whether what was recovered is a tree is answered by the object
            // itself, since a checkpoint written by this code and one written
            // before there were directories sit under the same key. Reading
            // it as a tree is the test: a source that happens to parse as
            // this exact JSON shape is not a source anybody wrote.
            let recovered: Option<crate::history::Tree> = serde_json::from_slice(&raw).ok();
            let size = match &recovered {
                Some(tree) => tree.size(),
                None => raw.len() as i64,
            };
            let parent = manifest
                .latest()
                .map(|point| point.sha.clone())
                .unwrap_or_default();
            manifest.checkpoints.push(Checkpoint {
                sha: candidate,
                tree_sha: String::new(),
                parent,
                at: timestamp(),
                by: String::new(),
                why: "recovered".to_string(),
                source_format: format.to_string(),
                size,
                label: String::new(),
                commit: String::new(),
                dirty: false,
                tree: recovered.is_some(),
                changed: Vec::new(),
            });
        }
    }

    /// The tree the newest checkpoint recorded, for the sake of the `changed`
    /// list on the next one. Kept in memory between checkpoints; read back
    /// only after a cold start, and read as a one-file tree when the entry is
    /// from before a document was a directory.
    pub(super) async fn parent_tree(&self) -> Option<crate::history::Tree> {
        let (held, point, path, id) = {
            let state = self.state.lock().await;
            (
                state.session.last_tree.clone(),
                state.manifest.latest().cloned(),
                session::main_path(&state.session.doc),
                session::main_id(&state.session.doc),
            )
        };
        if held.is_some() {
            return held;
        }
        let point = point?;
        crate::history::load_tree(self.blobs.as_ref(), &self.storage_id, &point, &path, &id)
            .await
            .ok()
    }

    /// What one checkpoint said: its tree, and the text of every file in it by
    /// digest. Read whole, before anything acts on it, because a caller that
    /// got half the files would be worse off than one that was refused -- a
    /// restore would leave a chapter and the file that includes it out of
    /// step, and the timeline would show a document that never existed.
    pub async fn checkpoint_texts(
        &self,
        point: &Checkpoint,
    ) -> Result<(crate::history::Tree, HashMap<String, String>), String> {
        let (path, id) = {
            let state = self.state.lock().await;
            (
                session::main_path(&state.session.doc),
                session::main_id(&state.session.doc),
            )
        };
        self.checkpoint_cache
            .load_checkpoint(self.blobs.as_ref(), &self.storage_id, point, &path, &id)
            .await
    }

    /// Look up one checkpoint without requiring the bounded resident history
    /// to contain it.  This is the cold-history path used by restore and
    /// checkpoint reads after the room has loaded only its recent tail.
    pub async fn checkpoint_by_sha(&self, sha: &str) -> Result<Option<Checkpoint>, String> {
        let resident = self.state.lock().await.manifest.checkpoints.clone();
        if let Some(point) = resident.iter().find(|point| point.sha == sha).cloned() {
            return Ok(Some(point));
        }
        // An opened room with an explicitly empty manifest has authoritative
        // evidence that its history was pruned. Do not resurrect a checkpoint
        // merely because an obsolete object/catalog row still exists.
        if resident.is_empty() {
            return Ok(None);
        }
        match self.catalog.get() {
            Some(catalog) => {
                let Some(row) = catalog
                    .checkpoint(&self.slug, sha)
                    .map_err(|error| error.to_string())?
                else {
                    return Ok(None);
                };
                let manifest = Manifest::from_catalog_rows(vec![row])?;
                Ok(manifest.checkpoints.into_iter().next())
            }
            None => Ok(None),
        }
    }

    /// Resolve a checkpoint prefix against the authoritative catalogue. The
    /// result is capped at two rows because callers only need to distinguish
    /// no match, one match, and ambiguity.
    pub async fn checkpoints_prefix(&self, prefix: &str) -> Result<Vec<Checkpoint>, String> {
        if let Some(catalog) = self.catalog.get() {
            let rows = catalog
                .checkpoints_prefix(&self.slug, prefix)
                .map_err(|error| error.to_string())?;
            return Manifest::from_catalog_rows(rows)
                .map(|manifest| manifest.checkpoints)
                .map_err(|error| error.to_string());
        }
        Ok(self
            .state
            .lock()
            .await
            .manifest
            .checkpoints
            .iter()
            .filter(|point| point.sha.starts_with(prefix))
            .take(2)
            .cloned()
            .collect())
    }

    /// Read one keyset page of the authoritative checkpoint timeline.  The
    /// cursor is the SQLite sequence, so paging never uses an offset scan and
    /// remains stable while newer checkpoints are appended.
    pub async fn checkpoint_page(
        &self,
        after_seq: Option<i64>,
        limit: u32,
    ) -> Result<(Vec<Checkpoint>, Option<i64>), String> {
        if let Some(catalog) = self.catalog.get() {
            let rows = catalog
                .checkpoints(&self.slug, after_seq, limit)
                .map_err(|error| error.to_string())?;
            let next = (rows.len() == limit.clamp(1, 200) as usize)
                .then(|| rows.last().map(|row| row.seq))
                .flatten();
            let manifest = Manifest::from_catalog_rows(rows).map_err(|error| error.to_string())?;
            return Ok((manifest.checkpoints, next));
        }
        let points = self.state.lock().await.manifest.checkpoints.clone();
        Ok((points, None))
    }

    /// Rebuilds a document, in place, from its newest checkpoint's own tree --
    /// the same texts a `restore` would put back. Used when the saved session
    /// itself cannot be trusted, so it takes the document to work on directly
    /// rather than locking the room, and touches nothing but storage reads.
    pub(super) async fn rebuild_from_checkpoint(
        &self,
        doc: &yrs::Doc,
        manifest: &Manifest,
        named: &str,
    ) -> Result<(), String> {
        let point = manifest
            .latest()
            .cloned()
            .ok_or_else(|| "no checkpoint to recover from".to_string())?;
        // The document has nothing in it yet at this point, so the path/id a
        // one-file checkpoint would fall back to come from the index entry
        // rather than the (empty) document.
        let tree =
            crate::history::load_tree(self.blobs.as_ref(), &self.storage_id, &point, named, "")
                .await?;
        let mut bodies = HashMap::new();
        for entry in tree.files.values() {
            if entry.kind != "text" || bodies.contains_key(&entry.sha) {
                continue;
            }
            let raw = if point.tree {
                self.blobs
                    .get(&crate::blob::blob_key(&self.storage_id, &entry.sha))
                    .await
            } else {
                self.blobs
                    .get(&checkpoint_key(&self.storage_id, point.sha.as_str()))
                    .await
            }
            .map_err(|err| err.to_string())?;
            bodies.insert(entry.sha.clone(), String::from_utf8_lossy(&raw).to_string());
        }
        session::restore(doc, &tree, &bodies);
        Ok(())
    }

    /// Names a checkpoint, or takes its name away when `label` is empty.
    ///
    /// This is the one write that changes a manifest entry after it is made,
    /// and it changes exactly one field. Nothing else about a checkpoint is
    /// ever rewritten: what it recorded is what it recorded, and a label is
    /// somebody's remark about it rather than a claim about the text.
    ///
    /// Goes through `write_manifest`, the same as `checkpoint`, so the two can
    /// never race each other into overwriting one's success with the other's
    /// stale snapshot.
    ///
    /// `Ok(false)` means the manifest has no such checkpoint, which is a
    /// 404 for the caller rather than a failure here.
    #[allow(dead_code)]
    pub async fn label(&self, sha: &str, label: &str) -> Result<bool, String> {
        self.label_as(sha, label, None).await
    }

    /// Label with the caller identity carried through to the final catalogue
    /// transaction.  The test/legacy entry point above remains available for
    /// isolated rooms that have no account authority attached.
    pub async fn label_as(
        &self,
        sha: &str,
        label: &str,
        actor: Option<(&str, &str, &str)>,
    ) -> Result<bool, String> {
        let _manifest_writer = self.manifest_write.lock().await;
        let resident = self.state.lock().await.manifest.has(sha);
        let catalog_checkpoint = if !resident {
            self.catalog
                .get()
                .map(|catalog| catalog.checkpoint(&self.slug, sha))
                .transpose()
                .map_err(|error| error.to_string())?
                .flatten()
        } else {
            None
        };
        if !resident && catalog_checkpoint.is_none() {
            return Ok(false);
        }
        if !self.hold().await {
            return Err("this room is held by another server".into());
        }
        // An old checkpoint need not be resident.  Label it directly in the
        // catalogue rather than manufacturing a partial Manifest and risking
        // a replacement of the unseen history.
        if let (Some(catalog), Some(actor)) = (self.catalog.get(), actor) {
            catalog
                .label_checkpoint_authorized(&self.slug, sha, label, actor)
                .map_err(|error| error.to_string())?;
            let mut state = self.state.lock().await;
            if let Some(point) = state.manifest.checkpoints.iter_mut().find(|p| p.sha == sha) {
                point.label = label.to_string();
            }
            return Ok(true);
        }
        if catalog_checkpoint.is_some() {
            let catalog = self
                .catalog
                .get()
                .ok_or_else(|| "catalogue disappeared while labelling".to_string())?;
            if let Some(actor) = actor {
                catalog
                    .label_checkpoint_authorized(&self.slug, sha, label, actor)
                    .map_err(|error| error.to_string())?;
            } else {
                catalog
                    .label_checkpoint(&self.slug, sha, label)
                    .map_err(|error| error.to_string())?;
            }
            return Ok(true);
        }
        let mut staged = self.state.lock().await.manifest.clone();
        for point in staged.checkpoints.iter_mut() {
            if point.sha == sha {
                point.label = label.to_string();
            }
        }
        self.write_manifest(staged, 0).await?;
        Ok(true)
    }

    /// The caller holds manifest_write from staging through commit. Keep the
    /// room state available during storage I/O and expose changes only once
    /// the conditional write succeeds, so failed writes and labels remain safe.
    pub(super) async fn write_manifest(
        &self,
        staged: Manifest,
        durable_seq: i64,
    ) -> Result<(), String> {
        if let Some(catalog) = self.catalog.get() {
            let previous = self.state.lock().await.manifest.clone();
            save_catalog_manifest(catalog, &self.slug, &previous, &staged, durable_seq)?;
            let mut state = self.state.lock().await;
            state.manifest = staged;
            return Ok(());
        }
        let body = serde_json::to_vec(&staged).map_err(|err| err.to_string())?;
        let mut version = self.state.lock().await.manifest_version.clone();
        self.write_owned(
            &crate::blob::history_index_key(&self.slug),
            body,
            &mut version,
        )
        .await?;
        let mut state = self.state.lock().await;
        state.manifest_version = version;
        state.manifest = staged;
        Ok(())
    }

    /// Restores a checkpoint and records both sides of the operation. The
    /// pre-restore checkpoint is the merge base for edits that arrive while
    /// the selected checkpoint is being read; the final checkpoint is a
    /// forced event even when its tree content is an older, already-known SHA.
    pub async fn restore_and_checkpoint(
        &self,
        point: &Checkpoint,
        by: &str,
    ) -> Result<(Vec<u8>, String), String> {
        let _restore_writer = self.restore_write.lock().await;
        if self.read_only() {
            return Err("this room is held by another server".into());
        }
        let base_sha = {
            let state = self.state.lock().await;
            let dirty = state.session.generation != state.session.checkpoint_generation;
            if dirty {
                None
            } else {
                state.session.last_checkpoint.clone().into()
            }
        };
        let base_sha = match base_sha {
            Some(sha) if !sha.is_empty() => sha,
            _ => self
                .checkpoint_now_protected("quiet", by, &point.sha)
                .await?
                .ok_or_else(|| "could not create a restore base checkpoint".to_string())?,
        };
        // If edits were dirty, checkpoint_now captured the state that existed
        // at the start of its call. They remain a valid merge base even when
        // more edits arrive while storage is being read below.
        let base_point = {
            let state = self.state.lock().await;
            state
                .manifest
                .checkpoints
                .iter()
                .find(|candidate| candidate.sha == base_sha)
                .cloned()
                .ok_or_else(|| "restore base checkpoint was shed".to_string())?
        };
        let (base_tree, base_bodies) = self.checkpoint_texts(&base_point).await?;
        if self.read_only() || !self.hold().await {
            return Err("this room is held by another server".into());
        }
        // Load the selected tree only after the base snapshot is fixed. Edits
        // arriving while this read is in flight are then merged against the
        // checkpoint that existed before the restore began.
        let (target_tree, target_bodies) = self.checkpoint_texts(point).await?;
        if self.read_only() || !self.hold().await {
            return Err("this room is held by another server".into());
        }

        let update = {
            let mut state = self.state.lock().await;
            let (live_tree, live_bodies) = tree_of(&state.session.doc, &state.session.asset_sizes);
            // Start from the selected checkpoint, then reconcile directory
            // membership against edits made after the pre-restore base. A
            // restore must not erase a file another peer added while the
            // checkpoint was being loaded, and a peer deletion must not be
            // silently undone by recreating the target file.
            let mut effective_tree = target_tree.clone();
            let mut merged = HashMap::new();
            let paths: std::collections::HashSet<String> = base_tree
                .files
                .keys()
                .chain(live_tree.files.keys())
                .chain(target_tree.files.keys())
                .cloned()
                .collect();
            for path in paths {
                let base = base_tree.files.get(&path);
                let live = live_tree.files.get(&path);
                let target = target_tree.files.get(&path);
                match (base, live, target) {
                    // A path absent from the target is a deletion. Keep a
                    // live add/change made since the base, while allowing a
                    // deletion of an unchanged base path to stand.
                    (Some(base), Some(live), None)
                        if base.kind != live.kind || base.sha != live.sha =>
                    {
                        effective_tree.files.insert(path.clone(), live.clone());
                        if live.kind == "text" {
                            if let Some(body) = live_bodies.get(&live.sha) {
                                merged.insert(path.clone(), body.clone());
                            }
                        }
                    }
                    (None, Some(live), None) => {
                        // A file added after the base is independent of the
                        // selected tree's omission and survives the restore.
                        effective_tree.files.insert(path.clone(), live.clone());
                        if live.kind == "text" {
                            if let Some(body) = live_bodies.get(&live.sha) {
                                merged.insert(path.clone(), body.clone());
                            }
                        }
                    }
                    // A live deletion of a file that existed in the base is
                    // a concurrent edit and wins over restoring that path.
                    (Some(_), None, Some(_)) => {
                        effective_tree.files.remove(&path);
                    }
                    _ => {}
                }
            }
            for (path, entry) in &target_tree.files {
                if effective_tree.files.get(path) != Some(entry) {
                    continue;
                }
                if entry.kind != "text" {
                    continue;
                }
                let target = target_bodies.get(&entry.sha).cloned().unwrap_or_default();
                let body = match (base_tree.files.get(path), live_tree.files.get(path)) {
                    (Some(base), Some(live)) if base.kind == "text" && live.kind == "text" => {
                        let base = base_bodies.get(&base.sha).cloned().unwrap_or_default();
                        let live = live_bodies.get(&live.sha).cloned().unwrap_or_default();
                        komodoc_text::merge(&base, &live, &target).text
                    }
                    _ => target.clone(),
                };
                if body != target {
                    let mut effective = entry.clone();
                    effective.sha = crate::store::digest_of(&body);
                    effective.size = body.len() as i64;
                    effective_tree.files.insert(path.clone(), effective);
                }
                merged.insert(path.clone(), body);
            }
            let before = session::encode_vector(&state.session.doc);
            session::restore_by_path(&state.session.doc, &effective_tree, &merged);
            // The membership merge may retain a peer deletion of the target's
            // old main path, so derive the format from the actual CRDT main
            // path after restore rather than from a possibly discarded tree
            // pointer.
            let derived = format_from_path(&session::main_path(&state.session.doc));
            if !derived.is_empty() {
                state.session.format = derived;
            }
            state.session.mark_dirty(now_unix());
            state.session.generation += 1;
            state.session.updated_at = now_unix();
            session::encode_diff(&state.session.doc, &before)
                .unwrap_or_else(|_| session::encode_state(&state.session.doc))
        };
        let restored = match self.checkpoint_restore(by).await {
            Ok(Some(sha)) => sha,
            Ok(None) => return Err("could not create the restore checkpoint".to_string()),
            Err(err) => {
                // The Yrs mutation already happened. Relay it even when a
                // later manifest write failed, otherwise connected clients
                // retain a different document from this room and the next
                // edit appears to resurrect the pre-restore text.
                self.broadcast(&json!({
                    "type": "y-update",
                    "update": encode_update(&update),
                }))
                .await;
                return Err(err);
            }
        };
        Ok((update, restored))
    }

    /* -------------------------------------------------------- suggestions */

    /// The most this document's session and history may occupy before it
    /// carries its owner or the deployment over a ceiling, less what the
    /// session state already costs. Negative means "no ceiling here", which is
    /// what a document with no index entry gets.
    pub(super) async fn allowance(&self, session_size: i64) -> i64 {
        let Some(store) = self.store.get() else {
            return -1;
        };
        match store.room_for(&self.slug).await {
            Some(room) => room - session_size,
            None => -1,
        }
    }

    /// Records what this document now costs, and the checkpoint the index
    /// names. A failure here is a failure of the checkpoint's bookkeeping, not
    /// of the checkpoint: the object and the state are already written, and
    /// the next checkpoint repairs the entry.
    ///
    /// Takes `format` and `main` rather than reading them off `self.state`
    /// itself, so a caller that already holds the room's lock can call this
    /// without deadlocking on it.
    pub(super) async fn record_size(&self, size: i64, sha: Option<&str>, format: &str, main: &str) {
        let Some(store) = self.store.get() else {
            return;
        };
        if let Err(err) = store
            .record_history(&self.slug, sha, size, format, main)
            .await
        {
            eprintln!(
                "warning: could not record the history of {}: {err}",
                self.slug
            );
        }
    }

    pub(super) fn refund_checkpoint_budget(&self, now: i64) {
        if let Some(catalog) = self.catalog.get() {
            if let Err(error) = catalog.refund_checkpoint(&self.slug, now) {
                eprintln!(
                    "warning: could not refund checkpoint budget for {}: {error}",
                    self.slug
                );
            }
        }
    }

    /// The document's directory as a checkpoint would record it. What the
    /// timeline reads, what the document endpoint lists the paths of, and what
    /// a test asks when it wants to know the name the next checkpoint will
    /// have.
    pub async fn tree(&self) -> crate::history::Tree {
        let state = self.state.lock().await;
        tree_of(&state.session.doc, &state.session.asset_sizes).0
    }

    /// Restore the exact pre-publication tree after staging or catalogue
    /// commit fails. This is a compensating CRDT mutation, persisted before
    /// the failed request returns, and is never broadcast as an accepted
    /// publication.
    pub async fn rollback_publication(
        &self,
        tree: &crate::history::Tree,
        bodies: &HashMap<String, String>,
        format: &str,
    ) -> Result<(), String> {
        {
            let mut state = self.state.lock().await;
            session::restore(&state.session.doc, tree, bodies);
            state.session.asset_sizes = tree
                .files
                .values()
                .filter(|entry| entry.kind == "asset")
                .map(|entry| (entry.sha.clone(), entry.size))
                .collect();
            state.session.format = format.to_string();
            state.session.generation += 1;
            state.session.mark_dirty(now_unix());
            state.session.updated_at = now_unix();
        }
        self.write_session(false, false).await.map(|_| ())
    }

    /// Puts a text at a path in the document, beside whatever is already
    /// there. What a directory publish adds each of its chapters with.
    pub async fn add_text(&self, path: &str, body: &str) {
        if self.read_only() {
            // Another server owns this room; adding to our copy would only
            // diverge from the one being persisted (R23).
            return;
        }
        let mut state = self.state.lock().await;
        session::put_text(&state.session.doc, path, body);
        state.session.mark_dirty(now_unix());
        state.session.generation += 1;
        state.session.updated_at = now_unix();
    }

    /// Names a checkpoint, for the tests that ask what pruning keeps. The
    /// route that does this for an author is the timeline's.
    #[cfg(test)]
    pub async fn label_checkpoint(&self, sha: &str, label: &str) {
        let mut state = self.state.lock().await;
        for point in &mut state.manifest.checkpoints {
            if point.sha == sha {
                point.label = label.to_string();
            }
        }
    }

    /// Drops the text blobs under `history/<slug>/blobs/` that no surviving
    /// checkpoint and no live text still names. A checkpoint's tree is its
    /// bookkeeping; the bodies it names are separate objects, written once
    /// and shared by every tree that mentions their digest -- so shedding a
    /// checkpoint's tree, on its own, leaves its text bodies exactly where
    /// they were. Nothing else in `checkpoint` ever swept this namespace,
    /// which is what let repeated distinct revisions grow it without bound
    /// even under a tight `history_max` (R21).
    ///
    /// Run after the manifest naming what survives, and the tree/session/
    /// index of the checkpoint just taken, are all written -- the same
    /// ordering `prune_assets` and `prune_renderings` keep, so a crash here
    /// leaves an object nothing names rather than a name pointing at one
    /// that is gone.
    ///
    /// `BlobInfo` carries no modification time, so there is no grace window
    /// to fall back on the way `prune_assets` protects a figure between its
    /// upload and its naming. Instead this protects the two things that
    /// could otherwise race it: the live in-memory tree (a digest just
    /// written but not yet the newest checkpoint) and `written`, the tree
    /// this very checkpoint just committed. Anything a retained older
    /// checkpoint still names survives independently, because that
    /// checkpoint stays in the manifest until its own turn to be shed.
    ///
    /// A retained tree this pass cannot read is not proof its blobs are
    /// unreferenced -- only that this attempt could not tell -- so nothing at
    /// all is deleted on a pass where that happens, the same rule
    /// `prune_assets` follows for the same reason (R15).
    pub(super) async fn prune_blobs(&self, written: &crate::history::Tree) {
        let catalog_history = if let Some(catalog) = self.catalog.get() {
            match load_catalog_history(catalog, &self.slug) {
                Ok(rows) => Some(rows),
                Err(error) => {
                    eprintln!(
                        "warning: could not read checkpoint history of {} while pruning blobs ({error})",
                        self.slug
                    );
                    return;
                }
            }
        } else {
            None
        };
        let (live, trees, path, id) = {
            let state = self.state.lock().await;
            let live: std::collections::HashSet<String> = session::texts_of(&state.session.doc)
                .into_values()
                .map(|body| crate::store::digest_of(&body))
                .collect();
            (
                live,
                catalog_history.unwrap_or_else(|| state.manifest.checkpoints.clone()),
                session::main_path(&state.session.doc),
                session::main_id(&state.session.doc),
            )
        };
        let mut kept = live;
        for entry in written.files.values() {
            if entry.kind == "text" {
                kept.insert(entry.sha.clone());
            }
        }
        for point in &trees {
            if !point.tree {
                continue; // a checkpoint from before directories names none
            }
            match crate::history::load_tree(
                self.blobs.as_ref(),
                &self.storage_id,
                point,
                &path,
                &id,
            )
            .await
            {
                Ok(tree) => {
                    for entry in tree.files.values() {
                        if entry.kind == "text" {
                            kept.insert(entry.sha.clone());
                        }
                    }
                }
                Err(err) => {
                    eprintln!(
                        "warning: could not read checkpoint {} of {} while pruning text blobs \
                         ({err}); skipping this pass rather than risk a blob it still names",
                        point.sha, self.slug
                    );
                    return;
                }
            }
        }
        let Ok(found) = self
            .blobs
            .list(&crate::blob::blob_prefix(&self.storage_id))
            .await
        else {
            return;
        };
        let gone: Vec<String> = found
            .into_iter()
            .filter_map(|object| {
                let digest = object.key.rsplit('/').next()?;
                if kept.contains(digest) {
                    None
                } else {
                    Some(object.key.clone())
                }
            })
            .collect();
        if gone.is_empty() {
            return;
        }
        let digests: std::collections::HashSet<&str> = gone
            .iter()
            .filter_map(|key| key.rsplit('/').next())
            .collect();
        if self.blobs.delete(&gone).await.is_ok() {
            // Forgotten here too, or a later checkpoint that happens to
            // reuse this exact digest would believe it is already written
            // and never restore the object it just deleted.
            let mut state = self.state.lock().await;
            state
                .session
                .blobs_written
                .retain(|digest| !digests.contains(digest.as_str()));
        }
    }

    /// Records what this document costs as it stands, without a checkpoint:
    /// what an asset upload changes, and -- with `sha` set -- what reusing an
    /// old tree's content moves the index head to (R17), without pretending a
    /// new checkpoint was taken.
    pub(super) async fn record_size_now(&self, sha: Option<&str>, format: &str, main: &str) {
        let (session_size, history) = {
            let mut state = self.state.lock().await;
            let generation = state.session.generation;
            let size = match state.session.encoded_size {
                Some((encoded, size)) if encoded == generation => size,
                _ => {
                    let size = session::encode_state(&state.session.doc).len() as i64;
                    state.session.encoded_size = Some((generation, size));
                    size
                }
            };
            let history = match self.catalog.get() {
                Some(catalog) => match catalog.checkpoint_stats(&self.slug) {
                    Ok((_, bytes)) => bytes,
                    Err(error) => {
                        eprintln!(
                            "warning: could not read checkpoint accounting for {}: {error}",
                            self.slug
                        );
                        self.read_only.store(true, Ordering::Relaxed);
                        return;
                    }
                },
                None => state.manifest.bytes(),
            };
            (size, history)
        };
        let assets = self.assets_bytes().await;
        let renderings = self.renderings_bytes().await;
        self.record_size(
            session_size + history + assets + renderings,
            sha,
            format,
            main,
        )
        .await;
    }

    /// The manifest, for the timeline and for the tests.
    pub async fn manifest(&self) -> Manifest {
        let resident = self.state.lock().await.manifest.clone();
        // Preserve the historical API's complete response for callers that
        // explicitly request a manifest (the timeline endpoint and tests),
        // while keeping cold-open room state bounded to the resident tail.
        // This materialization is short-lived and never becomes room state.
        if let Some(catalog) = self.catalog.get() {
            match load_catalog_history(catalog, &self.slug)
                .map(|checkpoints| Manifest { checkpoints })
            {
                Ok(full) => full,
                Err(error) => {
                    eprintln!(
                        "warning: could not materialize history for {}: {error}",
                        self.slug
                    );
                    resident
                }
            }
        } else {
            resident
        }
    }
}
