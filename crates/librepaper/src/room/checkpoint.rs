//! The timeline as the room writes it: taking a checkpoint, labelling one,
//! restoring to one, and the budget that bounds how many a document keeps.

use super::*;

/// One process-wide bounded native source encoder.  Sharing it across rooms
/// keeps a burst of independent checkpoint requests from creating one Tokio
/// blocking worker per room. The room's checkpoint gate coalesces concurrent
/// collaborators; durable object lookup avoids recompressing shared chunks.
static SOURCE_ENCODING_POOL: std::sync::OnceLock<crate::storage::encoding::EncodingPool> =
    std::sync::OnceLock::new();

fn source_encoding_pool() -> &'static crate::storage::encoding::EncodingPool {
    SOURCE_ENCODING_POOL.get_or_init(|| {
        crate::storage::encoding::EncodingPool::new(
            if cfg!(test) { 32 } else { 2 },
            64 * 1024 * 1024,
            crate::storage::encoding::EncodingProfile::default(),
        )
        .expect("static source encoding profile is valid")
    })
}

/// Who a checkpoint is attributed to.
///
/// `display` is the mutable string the timeline shows: a handle, an owner
/// key, a pseudonym, or nothing at all. `account` is the stable provider id
/// of the authenticated caller, and is the only thing account erasure can
/// match on. A handle can be renamed, and a released handle can be taken by
/// somebody else, so a display string can never establish that a historical
/// checkpoint belongs to an account. Nothing in this type ever turns one into
/// the other: an account id is only ever supplied by a caller that
/// authenticated it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attribution {
    display: String,
    account: Option<String>,
}

impl Attribution {
    /// A write by a caller whose account this request authenticated. An empty
    /// id is not an identity and is recorded as unattributed rather than as an
    /// account named "".
    pub fn account(account_id: &str, display: &str) -> Self {
        if account_id.is_empty() {
            return Self::unattributed(display);
        }
        Self {
            display: display.to_string(),
            account: Some(account_id.to_string()),
        }
    }

    /// A write with a display string but no authoritative account behind it:
    /// an anonymous or link-bounded caller, a pseudonymous commenter, or a
    /// document seeded or imported by the command line. Erasure cannot reach
    /// these, and that is deliberate -- there is nothing to say whose they are.
    pub fn unattributed(display: &str) -> Self {
        Self {
            display: display.to_string(),
            account: None,
        }
    }

    /// A write the deployment made for itself, with no requester behind it:
    /// the automatic and quiet checkpoints a room takes on its own clock when
    /// no update author is known.
    pub fn system() -> Self {
        Self::default()
    }

    pub fn display(&self) -> &str {
        &self.display
    }

    pub fn account_id(&self) -> Option<&str> {
        self.account.as_deref()
    }

    pub(crate) fn is_account(&self, account_id: &str) -> bool {
        self.account.as_deref() == Some(account_id)
    }

    /// What this becomes once the account behind it has been erased: the same
    /// replacement the catalogue writes, and no stable id.
    pub(crate) fn erased() -> Self {
        Self::unattributed(crate::storage::catalog::ERASED_ATTRIBUTION)
    }
}

/// A bare display string is display-only attribution, never an account. This
/// exists so that tests and the callers that genuinely have nothing but a
/// name stay readable; a caller that does know its account must say so with
/// `Attribution::account`.
impl From<&str> for Attribution {
    fn from(display: &str) -> Self {
        Self::unattributed(display)
    }
}

impl From<&String> for Attribution {
    fn from(display: &String) -> Self {
        Self::unattributed(display)
    }
}

impl From<&Attribution> for Attribution {
    fn from(value: &Attribution) -> Self {
        value.clone()
    }
}

/// A charged checkpoint budget bucket held across an asynchronous publication.
/// Dropping it refunds the exact owner and hour admitted by SQLite.
pub struct PublicationCheckpointToken {
    catalog: Option<Arc<crate::storage::catalog::Catalog>>,
    /// The admitted owner and hour, held in the shared slot so that the
    /// admission's completion hook and this token's `Drop` cannot both refund
    /// it — and so that a caller cancelled while the admitting transaction is
    /// still running is refunded by the hook rather than not at all.
    slot: Arc<ReservationSlot<(String, i64)>>,
}

impl PublicationCheckpointToken {
    fn none() -> Self {
        let slot = ReservationSlot::default();
        slot.keep();
        Self {
            catalog: None,
            slot: Arc::new(slot),
        }
    }

    fn new(catalog: Arc<crate::storage::catalog::Catalog>, owner: String, bucket: i64) -> Self {
        let slot = ReservationSlot::default();
        slot.record((owner, bucket));
        Self {
            catalog: Some(catalog),
            slot: Arc::new(slot),
        }
    }

    /// Mark the publication token consumed after the surrounding publication
    /// transaction has committed. A successful checkpoint alone is not the
    /// publication's final durable boundary.
    pub fn commit(&mut self) {
        self.slot.keep();
    }
}

impl Drop for PublicationCheckpointToken {
    fn drop(&mut self) {
        let Some((owner, bucket)) = self.slot.abandon() else {
            return;
        };
        let Some(catalog) = &self.catalog else {
            return;
        };
        // Synchronous because a `Drop` cannot await. This is the unwind path
        // for a publication that never happened; the refund is one statement
        // and it names the exact owner and hour SQLite admitted, so it can
        // never give back a bucket another publication has since taken.
        if let Err(error) = catalog.refund_checkpoint_token(&owner, bucket) {
            eprintln!("warning: could not refund checkpoint budget: {error}");
        }
    }
}

/// The service's half of the checkpoint budget handshake.
struct CheckpointTokenCleanup {
    slot: Arc<ReservationSlot<(String, i64)>>,
}

impl crate::storage::catalog::CatalogServiceCompletion for CheckpointTokenCleanup {
    fn complete(
        self: Box<Self>,
        outcome: crate::storage::catalog::CatalogOutcome<'_>,
        catalog: &crate::storage::catalog::Catalog,
    ) {
        if !matches!(outcome, crate::storage::catalog::CatalogOutcome::Committed) {
            return;
        }
        let Some((owner, bucket)) = self.slot.on_completion() else {
            return;
        };
        if let Err(error) = catalog.refund_checkpoint_token(&owner, bucket) {
            eprintln!("warning: could not refund checkpoint budget: {error}");
        }
    }
}

/// A test-only gate in the window between a checkpoint's budget admission
/// committing and the checkpoint that would consume it, keyed by slug. It is
/// the publication-side counterpart of the edit reservation's gate: the
/// window `Drop` covers rather than the completion hook.
#[cfg(test)]
pub(crate) static AFTER_CHECKPOINT_ADMISSION: super::TestGate = std::sync::Mutex::new(None);

#[cfg(test)]
async fn pause_after_checkpoint_admission(slug: &str) {
    super::ReservationGate::park(&AFTER_CHECKPOINT_ADMISSION, slug).await;
}

/// Charge one checkpoint against the hourly budgets, through the execution
/// boundary, and hand back a token that refunds it unless a checkpoint
/// actually happens.
async fn admit_checkpoint_token(
    catalog: &Arc<crate::storage::catalog::Catalog>,
    slug: &str,
    now: i64,
    automatic: bool,
    owner_limit: i64,
    deployment_limit: i64,
) -> Result<Option<PublicationCheckpointToken>, WriteError> {
    let slot = Arc::new(ReservationSlot::default());
    let token = PublicationCheckpointToken {
        catalog: Some(catalog.clone()),
        slot: slot.clone(),
    };
    let cleanup = CheckpointTokenCleanup { slot: slot.clone() };
    let job_slug = slug.to_string();
    let admitted = catalog
        .reserve_execution(slug.len() + DESCRIPTOR_BYTES)
        .await
        .map_err(WriteError::from)?
        .execute_catalog_with_completion(
            move |catalog| {
                let admitted = catalog.admit_checkpoint_token_with_limits(
                    &job_slug,
                    now,
                    automatic,
                    owner_limit,
                    deployment_limit,
                )?;
                if let Some((owner, bucket)) = admitted.clone() {
                    slot.record((owner, bucket));
                }
                Ok(admitted.is_some())
            },
            cleanup,
        )
        .await
        .map_err(WriteError::from)?;
    if !admitted {
        // Nothing was charged, so the token owes nothing.
        return Ok(None);
    }
    Ok(Some(token))
}

impl Room {
    /// Takes a checkpoint, if the text differs from the last one. Returns the
    /// SHA of the checkpoint that now stands for the current text, or None
    /// when the request was deferred.
    ///
    /// The order of writes puts every object before anything that names it,
    /// with the text blobs first, so that a crash leaves nothing worse than an
    /// untidy history: the blobs, then the tree, then the session state, then
    /// the index entry, then the manifest. Nothing ever names an object that
    /// is not there; what a crash can leave is an object nothing names, which
    /// costs storage and loses nothing. A manifest missing its newest entry is
    /// repaired by the next checkpoint, which finds the object present and
    /// names it as `parent` -- `repair` below is that.
    pub async fn checkpoint(
        &self,
        why: &str,
        by: impl Into<Attribution>,
    ) -> Result<Option<String>, WriteError> {
        self.checkpoint_impl(why, &by.into(), true, false, None, None, None)
            .await
    }

    /// Takes a checkpoint immediately, even when the ordinary deliberate-save
    /// debounce window is still open. An accept uses this: the edit it just
    /// made is a deliberate act by the editor, not a keystroke to wait out.
    pub async fn checkpoint_now(
        &self,
        why: &str,
        by: impl Into<Attribution>,
    ) -> Result<Option<String>, WriteError> {
        self.checkpoint_impl(why, &by.into(), false, false, None, None, None)
            .await
    }

    /// Immediate checkpoint whose final catalogue insertion rechecks the
    /// caller's editor rights and session generation in the same transaction.
    pub async fn checkpoint_now_with_authority(
        &self,
        why: &str,
        by: impl Into<Attribution>,
        actor: crate::storage::catalog::MutationAuthority<'_>,
    ) -> Result<Option<String>, WriteError> {
        self.checkpoint_impl(why, &by.into(), false, false, None, None, Some(actor))
            .await
    }

    /// Reserve the explicit checkpoint budget before a publication mutates
    /// the live CRDT. The subsequent publication checkpoint skips its normal
    /// admission because this token already belongs to it.
    pub fn reserve_publication_checkpoint(&self) -> Result<PublicationCheckpointToken, String> {
        if let Some(catalog) = self.catalog.get() {
            let Some((owner, bucket)) = catalog
                .admit_checkpoint_token_with_limits(
                    &self.slug,
                    now_unix(),
                    false,
                    self.config.session.checkpoint_owner_per_hour,
                    self.config.session.checkpoint_deployment_per_hour,
                )
                .map_err(|error| error.to_string())?
            else {
                return Err("checkpoint budget exhausted; retry later".into());
            };
            return Ok(PublicationCheckpointToken::new(
                catalog.clone(),
                owner,
                bucket,
            ));
        }
        Ok(PublicationCheckpointToken::none())
    }

    pub async fn checkpoint_publication_now(
        &self,
        why: &str,
        by: impl Into<Attribution>,
        token: &mut PublicationCheckpointToken,
    ) -> Result<Option<String>, WriteError> {
        let _restore_writer = self.restore_write.lock().await;
        let _publication_writer = self.publication_write.lock().await;
        let _publication_checkpoint = self.publication_checkpoint.write().await;
        self.checkpoint_publication_now_locked(why, by, token).await
    }

    /// Publication checkpoint for a caller that already owns the publication
    /// and checkpoint barriers across its CRDT mutation.
    pub(crate) async fn checkpoint_publication_now_locked(
        &self,
        why: &str,
        by: impl Into<Attribution>,
        token: &mut PublicationCheckpointToken,
    ) -> Result<Option<String>, WriteError> {
        self.checkpoint_impl_locked(why, &by.into(), false, false, None, Some(token), None)
            .await
    }

    /// Takes a checkpoint event even when its tree has the same content as a
    /// checkpoint already in the manifest. A restore is an event in the
    /// linear history, and deduplicating it would make restoring to an old
    /// revision silently disappear from the timeline. The event gets its own
    /// object key while retaining the same immutable tree bytes.
    pub(super) async fn checkpoint_restore(
        &self,
        by: &Attribution,
    ) -> Result<Option<String>, WriteError> {
        self.checkpoint_impl("restore", by, false, true, None, None, None)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn checkpoint_impl(
        &self,
        why: &str,
        by: &Attribution,
        defer: bool,
        force_event: bool,
        protected: Option<&str>,
        budget_token: Option<&mut PublicationCheckpointToken>,
        actor: Option<crate::storage::catalog::MutationAuthority<'_>>,
    ) -> Result<Option<String>, WriteError> {
        let _publication_checkpoint = self.publication_checkpoint.read().await;
        self.checkpoint_impl_locked(why, by, defer, force_event, protected, budget_token, actor)
            .await
    }

    /// Checkpoint implementation for callers that already hold the room's
    /// publication mutation gate.
    #[allow(clippy::too_many_arguments)]
    async fn checkpoint_impl_locked(
        &self,
        why: &str,
        by: &Attribution,
        defer: bool,
        force_event: bool,
        protected: Option<&str>,
        budget_token: Option<&mut PublicationCheckpointToken>,
        actor: Option<crate::storage::catalog::MutationAuthority<'_>>,
    ) -> Result<Option<String>, WriteError> {
        let _checkpoint_writer = self.checkpoint_write.lock().await;
        let now = now_unix();
        // Publications reserve their checkpoint token before mutating the
        // document. Keep that reservation live until this call completes so
        // read-only, lease, and lookup failures cannot consume it forever.
        let mut admitted: Option<PublicationCheckpointToken> = None;
        if self.read_only() {
            // Refused outright rather than left to fall through to the
            // deduplication branch below: on a read-only room whose direct
            // mutators are now no-ops (R23), the live tree never moves away
            // from the last checkpoint, so that branch would otherwise
            // report an unearned success for a request this server has no
            // business recording.
            return Err(self.fenced());
        }
        // Counted for the whole of this call, every early return included:
        // the guard's drop is what lets the sweep at the end of another
        // checkpoint know it is alone again.
        let _in_flight = InFlight::new(&self.checkpointing);
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
                state.session.asked = Some((why.to_string(), by.clone()));
                (
                    crate::document::history::Tree::default(),
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
        let catalog_duplicate = match (force_event, self.catalog.get()) {
            (false, Some(catalog)) => {
                read_checkpoint_by_content_sha(catalog, &self.slug, &content_sha).await?
            }
            _ => None,
        };
        let duplicate = if force_event {
            false
        } else {
            let state = self.state.lock().await;
            let resident_duplicate = state.manifest.checkpoints.iter().rev().any(|point| {
                point.sha == content_sha
                    || (!point.tree_sha.is_empty() && point.tree_sha == content_sha)
            });
            drop(state);
            resident_duplicate || catalog_duplicate.is_some()
        };
        if !duplicate && budget_token.is_none() {
            if let Some(catalog) = self.catalog.get() {
                let automatic = matches!(why, "automatic" | "quiet");
                match admit_checkpoint_token(
                    catalog,
                    &self.slug,
                    now_unix(),
                    automatic,
                    self.config.session.checkpoint_owner_per_hour,
                    self.config.session.checkpoint_deployment_per_hour,
                )
                .await?
                {
                    Some(token) => admitted = Some(token),
                    None => return Ok(None),
                }
                #[cfg(test)]
                pause_after_checkpoint_admission(&self.slug).await;
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
                let existing = existing.or_else(|| {
                    // In catalogue mode the room keeps only a bounded tail.
                    // A revert to an older tree is still the same immutable
                    // checkpoint, even when that row is no longer resident.
                    catalog_duplicate.as_ref().map(|point| point.sha.clone())
                });
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
                    let format = state.session.format.clone();
                    let main = tree.main.clone();
                    drop(state);
                    // Reusing a tree does not mean the current Y.Doc is
                    // already durable: CRDT item identities and concurrent
                    // edits can differ while the visible tree is identical.
                    // Persist and acknowledge that state before success.
                    self.write_session_inner(true, true).await?;
                    if let (Some(catalog), Some(actor)) = (self.catalog.get(), actor) {
                        let slug = self.slug.clone();
                        let actor = crate::room::catalog::OwnedAuthority::new(&actor);
                        catalog
                            .execute_catalog(
                                slug.len() + crate::room::catalog::DESCRIPTOR_BYTES,
                                move |catalog| {
                                    catalog.require_mutation_authority(&slug, actor.borrow())
                                },
                            )
                            .await
                            .map_err(WriteError::from)?;
                    }
                    let mut state = self.state.lock().await;
                    state.session.last_checkpoint = existing.clone();
                    state.session.last_tree = Some(tree.clone());
                    state.session.checkpoint_generation = tree_generation;
                    if state.session.generation == tree_generation {
                        state.session.pending_checkpoint_since = 0;
                    }
                    drop(state);
                    // A replacement of identical content still has a
                    // prepared publication receipt. Reuse the already
                    // durable checkpoint descriptor in that receipt so the
                    // strict staged commit has the same coverage proof as a
                    // newly written tree.
                    if let Some(catalog) = self.catalog.get() {
                        stage_existing_publication_checkpoint(
                            catalog,
                            &self.slug,
                            &existing,
                            actor.map(|actor| crate::room::catalog::OwnedAuthority::new(&actor)),
                        )
                        .await?;
                    }
                    if moved {
                        self.record_size_now(Some(&existing), &format, &main).await;
                    }
                    if why == "automatic" {
                        if let Some(catalog) = self.catalog.get() {
                            touch_auto_checkpoint(catalog, &self.slug, now).await;
                        }
                    }
                    return Ok(Some(existing));
                }
            }
        }
        if !self.hold().await {
            return Err(self.fenced());
        }

        // 1. Encode complete text files outside room state locks.  The
        // catalogue lease is installed before object I/O, and the normal
        // put_accounted path still owns quota reservation/accounting for each
        // physical object.  Recipes are complete file representations, so a
        // later checkpoint can read any retained tree directly.
        let mut encoded_sources = Vec::new();
        let encoding_started = std::time::Instant::now();
        let mut planned_sources = Vec::new();
        let mut requested_chunk_keys = std::collections::HashSet::new();
        let mut durable_chunk_sizes = std::collections::HashMap::new();
        for (digest, body) in &bodies {
            if let Some(catalog) = self.catalog.get() {
                if let Some(record) = read_source_history_record(catalog, &self.storage_id, digest)
                    .await
                    .map_err(|error| error.to_string())?
                {
                    // A retained record already carries its measured graph
                    // sizes. Only repair an old/unresolved zero sentinel by
                    // asking accounting for that particular key.
                    for object in &record.objects {
                        if object.kind == "source_chunk" {
                            if object.bytes > 0 {
                                durable_chunk_sizes.insert(object.object_key.clone(), object.bytes);
                            } else {
                                requested_chunk_keys.insert(object.object_key.clone());
                            }
                        }
                    }
                    encoded_sources.push((record, None));
                    continue;
                }
            }
            let plan = source_encoding_pool()
                .try_plan(body.as_bytes().to_vec())
                .await
                .map_err(|error| WriteError::Storage(error.to_string()))?;
            for chunk in &plan.recipe.chunks {
                requested_chunk_keys.insert(crate::storage::blob::content_chunk_key(
                    &self.storage_id,
                    &hex::encode(chunk.digest),
                ));
            }
            planned_sources.push((digest.clone(), body.clone(), plan));
        }
        if let Some(catalog) = self.catalog.get() {
            let requested = requested_chunk_keys.into_iter().collect::<Vec<_>>();
            durable_chunk_sizes
                .extend(read_source_object_sizes(catalog, &self.storage_id, &requested).await?);
        }
        for (digest, body, plan) in planned_sources {
            let existing: std::collections::HashSet<[u8; 32]> = plan
                .recipe
                .chunks
                .iter()
                .filter(|chunk| {
                    let key = crate::storage::blob::content_chunk_key(
                        &self.storage_id,
                        &hex::encode(chunk.digest),
                    );
                    durable_chunk_sizes
                        .get(&key)
                        .is_some_and(|bytes| *bytes > 0)
                })
                .map(|chunk| chunk.digest)
                .collect();
            let encoded = source_encoding_pool()
                .try_encode_planned(body.as_bytes().to_vec(), plan, existing)
                .await
                .map_err(|error| WriteError::Storage(error.to_string()))?;
            if hex::encode(encoded.file_digest) != *digest {
                return Err(WriteError::Storage(
                    "source encoder returned a digest different from the tree".into(),
                ));
            }
            let record = crate::storage::catalog::SourceHistoryRecord::from_encoded(
                &self.storage_id,
                &encoded,
            )
            .map_err(|error| WriteError::Storage(error.to_string()))?;
            for object in &record.objects {
                if object.kind == "source_chunk" && object.bytes > 0 {
                    durable_chunk_sizes.insert(object.object_key.clone(), object.bytes);
                }
            }
            encoded_sources.push((record, Some(encoded)));
        }
        encoded_sources.sort_by(|left, right| left.0.file_digest.cmp(&right.0.file_digest));
        eprintln!(
            "{}",
            serde_json::json!({
                "event": "source_history_encoded",
                "files": encoded_sources.len(),
                "logical_bytes": bodies.values().map(|body| body.len()).sum::<usize>(),
                "introduced_bytes": encoded_sources.iter().filter_map(|(_, encoded)| encoded.as_ref())
                    .map(|encoded| encoded.recipe_bytes.len() + encoded.objects.iter()
                        .map(|object| object.encoded.len()).sum::<usize>()).sum::<usize>(),
                "reused_files": encoded_sources.iter().filter(|(_, encoded)| encoded.is_none()).count(),
                "elapsed_micros": encoding_started.elapsed().as_micros(),
            })
        );
        let lease_operation = crate::util::new_id();
        let lease_objects: Vec<_> = encoded_sources
            .iter()
            .flat_map(|record| {
                std::iter::once(crate::storage::catalog::SourceHistoryObject {
                    object_key: record.0.recipe_key.clone(),
                    kind: "source_recipe".to_string(),
                    bytes: record.0.recipe_bytes,
                })
                .chain(record.0.objects.iter().cloned())
                .collect::<Vec<_>>()
            })
            .chain(std::iter::once(
                crate::storage::catalog::SourceHistoryObject {
                    object_key: crate::storage::blob::checkpoint_key(&self.storage_id, &sha),
                    kind: "checkpoint_tree".to_string(),
                    bytes: i64::try_from(tree.to_bytes().len())
                        .map_err(|_| WriteError::Storage("checkpoint tree is too large".into()))?,
                },
            ))
            .chain(
                tree.files
                    .values()
                    .filter(|entry| entry.kind == "asset")
                    .map(|entry| {
                        if entry.size < 0 || entry.sha.is_empty() {
                            return Err(WriteError::Storage(
                                "checkpoint asset has invalid identity or size".into(),
                            ));
                        }
                        Ok(crate::storage::catalog::SourceHistoryObject {
                            object_key: crate::storage::blob::asset_key(
                                &self.storage_id,
                                &entry.sha,
                            ),
                            kind: "asset".into(),
                            bytes: entry.size,
                        })
                    })
                    .collect::<Result<Vec<_>, WriteError>>()?,
            )
            .map(|mut object| {
                if object.kind == "source_chunk" && object.bytes == 0 {
                    object.bytes =
                        *durable_chunk_sizes.get(&object.object_key).ok_or_else(|| {
                            WriteError::Storage(
                                "reused source chunk has no durable accounting".into(),
                            )
                        })?;
                    if object.bytes <= 0 {
                        return Err(WriteError::Storage(
                            "reused source chunk has invalid accounting".into(),
                        ));
                    }
                }
                Ok(object)
            })
            .collect::<Result<Vec<_>, WriteError>>()?;
        let source_lease = if let Some(catalog) = self.catalog.get() {
            Some(
                begin_source_history_lease(
                    catalog,
                    &self.storage_id,
                    &lease_operation,
                    lease_objects,
                    now,
                    now.saturating_add(3600),
                )
                .await?,
            )
        } else {
            None
        };

        // Put chunks before recipes, then the logical tree. A digest already
        // present in the object ledger is harmlessly reused by the accounted
        // writer and is never charged twice.
        for (record, encoded) in &encoded_sources {
            let Some(encoded) = encoded else { continue };
            for object in &encoded.objects {
                self.put_accounted(
                    &crate::storage::blob::content_chunk_key(
                        &self.storage_id,
                        &hex::encode(object.digest),
                    ),
                    object.encoded.clone(),
                    "source_chunk",
                    None,
                )
                .await?;
            }
            self.put_accounted(
                &record.recipe_key,
                encoded.recipe_bytes.clone(),
                "source_recipe",
                None,
            )
            .await?;
        }

        // Keep this compatibility set for legacy whole-file blobs. New
        // encoded files are read through recipes and do not enter `blobs/`.
        let unwritten: Vec<(String, String)> = {
            let state = self.state.lock().await;
            bodies
                .iter()
                .filter(|(digest, _)| !state.session.blobs_written.contains(*digest))
                .map(|(digest, body)| (digest.clone(), body.clone()))
                .filter(|(digest, _)| {
                    !encoded_sources
                        .iter()
                        .any(|(record, _)| record.file_digest.as_str() == digest.as_str())
                })
                .collect()
        };
        for (digest, body) in &unwritten {
            self.put_accounted(
                &crate::storage::blob::blob_key(&self.storage_id, digest),
                body.clone().into_bytes(),
                "text",
                None,
            )
            .await?;
        }
        {
            let mut state = self.state.lock().await;
            for (digest, _) in &unwritten {
                state.session.blobs_written.insert(digest.clone());
            }
        }

        // 2. the tree, which names them.
        self.put_accounted(
            &checkpoint_key(&self.storage_id, &sha),
            tree.to_bytes(),
            "tree",
            None,
        )
        .await?;

        // 3. the session state, so a restart comes back at or after the
        //    checkpoint rather than before it. The generation is captured in
        //    the same breath as the bytes: if a newer edit lands before this
        //    server gets to clear `dirty` below, the two disagree and `dirty`
        //    is left set, so that edit is never reported as saved when it is
        //    not yet on disk (R07).
        let (session_size, durable_sequence) = match self.write_session_inner(false, true).await {
            Ok(Some(result)) => result,
            Ok(None) => unreachable!("an unconditional session write returns its size"),
            Err(err) => {
                return Err(err);
            }
        };

        // What the parent recorded, so this entry can say which paths moved.
        // Held in memory from one checkpoint to the next; read back only on
        // the first checkpoint after a cold start, which is the only time
        // this server has not seen the parent itself.
        let parent_tree = self.parent_tree().await;
        let ceiling = self.allowance(session_size).await;
        let keep_count = self.config.session.history_max;

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
            let before_repair = staged.checkpoints.len();
            self.repair(&mut staged, &repair_format, &last).await;
            // The resident manifest is only a bounded tail in catalogue
            // mode. After reverting to an older checkpoint, the current
            // parent may therefore be absent from `staged`; using the tail's
            // latest row would forge a timeline that disagrees with SQLite.
            let repaired = staged.checkpoints.len() != before_repair;
            let parent = if repaired {
                staged
                    .latest()
                    .map(|point| point.sha.clone())
                    .unwrap_or_default()
            } else if !last.is_empty() {
                last.clone()
            } else {
                staged
                    .latest()
                    .map(|point| point.sha.clone())
                    .unwrap_or_default()
            };
            staged.checkpoints.push(Checkpoint {
                sha: sha.clone(),
                tree_sha: content_sha.clone(),
                parent: parent.clone(),
                at: timestamp(),
                by: by.display().to_string(),
                by_account: by.account_id().map(str::to_string),
                why: why.to_string(),
                source_format: format.clone(),
                size: tree.size(),
                label: String::new(),
                commit: String::new(),
                dirty: false,
                tree: true,
                changed: tree.changed_from(parent_tree.as_ref()),
                seq: -1,
                original_parent: parent,
                ancestry_gap: false,
            });

            // A checkpoint is never refused, because refusing it would lose
            // work. What gives instead is the oldest history: the ceilings
            // shed the oldest unlabelled checkpoints, and the oldest
            // labelled ones after them, until the document fits.
            // In catalogue mode this is the resident tail, not necessarily
            // the complete history.  It is still safe to shed entries that
            // are present here: every removed row is explicitly deleted from
            // SQLite below, while rows omitted from the tail are never
            // inferred to be garbage.  (With the normal tail of 64 this also
            // handles small configured history caps exactly.)
            let shed_now = staged.shed_protected(protected.unwrap_or_default(), |manifest| {
                (keep_count == 0 || manifest.checkpoints.len() <= keep_count)
                    && ceiling.is_none_or(|limit| manifest.bytes() <= limit)
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
                read_checkpoint_stats(catalog, &self.slug)
                    .await
                    .map(|(_, bytes)| bytes)
                    // A failed catalogue read is not evidence for a logical
                    // tree size.  Keep admission conservative only through
                    // the durable physical accounting rows that were read;
                    // the normal `record_size_now` path fences unreadable
                    // catalogue state rather than guessing.
                    .unwrap_or(0)
            } else {
                staged.bytes()
            };
            let mut checkpoint_assets = std::collections::BTreeMap::<String, i64>::new();
            for entry in tree.files.values().filter(|entry| entry.kind == "asset") {
                if entry.size < 0 || entry.sha.is_empty() {
                    return Err(WriteError::Storage(
                        "checkpoint asset has invalid identity or size".into(),
                    ));
                }
                let key = crate::storage::blob::asset_key(&self.storage_id, &entry.sha);
                if checkpoint_assets
                    .insert(key, entry.size)
                    .is_some_and(|previous| previous != entry.size)
                {
                    return Err(WriteError::Storage(
                        "checkpoint asset has conflicting sizes".into(),
                    ));
                }
            }
            let checkpoint_assets = checkpoint_assets
                .into_iter()
                .map(
                    |(object_key, bytes)| crate::storage::catalog::CheckpointAssetRef {
                        object_key,
                        bytes,
                    },
                )
                .collect::<Vec<_>>();
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
            self.write_manifest(
                staged,
                durable_sequence,
                actor.map(|actor| crate::room::catalog::OwnedAuthority::new(&actor)),
                &encoded_sources
                    .iter()
                    .map(|(record, _)| record.clone())
                    .collect::<Vec<_>>(),
                &checkpoint_assets,
                Some(&lease_operation),
            )
            .await?;
            // The ordinary checkpoint is durable once its manifest is
            // committed. Release its admission before pruning or any later
            // asynchronous cleanup; cancellation there must not refund a
            // checkpoint that already exists.
            if budget_token.is_none() {
                if let Some(token) = admitted.as_mut() {
                    token.commit();
                }
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
            state.session.checkpoint_generation = tree_generation;
            if state.session.generation == tree_generation {
                state.session.pending_checkpoint_since = 0;
            }
            shed = shed_now;
        }
        if let Some(catalog) = self.catalog.get() {
            let publication_pending = read_catalog_document(catalog, &self.slug)
                .await
                .map_err(WriteError::from)?
                .and_then(|document| document.pending_publication)
                .is_some();
            if !publication_pending {
                if let Some(lease) = source_lease {
                    lease.finish().await?;
                }
                // Restore may still be fixing its merge base before leasing
                // the selected old tree. Do not let this checkpoint's routine
                // policy pass retire that explicit protected selection.
                if protected.is_none() {
                    let retention_bounds = crate::document::quota::RetentionBounds {
                        hard_quota: self.config.storage.per_owner,
                        max_checkpoint_count: if self.config.session.history_max == 0 {
                            None
                        } else {
                            Some(self.config.session.history_max as u32)
                        },
                        ..Default::default()
                    };
                    let slug = self.slug.clone();
                    let scheduled = catalog
                        .execute_catalog(slug.len() + DESCRIPTOR_BYTES, move |catalog| {
                            catalog.schedule_document_balanced(
                                &slug,
                                crate::util::now_unix(),
                                retention_bounds,
                            )
                        })
                        .await;
                    if matches!(scheduled, Ok(count) if count > 0) {
                        self.refresh_retained_manifest().await;
                    }
                }
            }
        }
        let mut shed = shed;
        if let Some(catalog) = self.catalog.get() {
            match shed_checkpoints_to_limits(
                catalog,
                &self.slug,
                keep_count,
                ceiling,
                protected.unwrap_or_default().to_string(),
            )
            .await
            {
                Ok(extra) => shed.extend(extra),
                Err(error) => eprintln!(
                    "warning: could not apply full history retention for {}: {error}",
                    self.slug
                ),
            }
        }
        shed.sort_unstable();
        shed.dedup();
        if !shed.is_empty() {
            let removed: std::collections::HashSet<&str> =
                shed.iter().map(String::as_str).collect();
            let mut state = self.state.lock().await;
            state
                .manifest
                .checkpoints
                .retain(|point| !removed.contains(point.sha.as_str()));
        }
        if !shed.is_empty() {
            // Remove catalogue metadata before deleting the object it names.
            // A crash after object deletion but before the SQL delete would
            // otherwise leave a durable row pointing at a missing tree. A
            // failed metadata delete keeps its object for a later retry.
            let keys: Vec<String> = if let Some(catalog) = self.catalog.get() {
                delete_checkpoints(catalog, &self.slug, shed.clone())
                    .await
                    .iter()
                    .map(|sha| checkpoint_key(&self.storage_id, sha))
                    .collect()
            } else {
                shed.iter()
                    .map(|sha| checkpoint_key(&self.storage_id, sha))
                    .collect()
            };
            let _ = self.blobs.delete(&keys).await;
            for key in &keys {
                self.checkpoint_cache.invalidate(key).await;
            }
        }
        // The figures nothing refers to any more, once the tree and the
        // manifest that name what is kept are both written. This order is
        // what makes a crash leave an unreferenced object rather than a tree
        // pointing at one that is gone.
        self.prune_retained(&tree).await;
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
                touch_auto_checkpoint(catalog, &self.slug, now).await;
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
                    self.fence(FenceReason::UnreadableState);
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
            let recovered: Option<crate::document::history::Tree> =
                serde_json::from_slice(&raw).ok();
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
                parent: parent.clone(),
                at: timestamp(),
                // A recovered entry is one this server found standing in
                // storage with nothing naming it. Nothing records who wrote
                // it, so it is attributed to nobody rather than guessed at.
                by: String::new(),
                by_account: None,
                why: "recovered".to_string(),
                source_format: format.to_string(),
                size,
                label: String::new(),
                commit: String::new(),
                dirty: false,
                tree: recovered.is_some(),
                changed: Vec::new(),
                seq: -1,
                original_parent: parent,
                ancestry_gap: false,
            });
        }
    }

    /// The tree the newest checkpoint recorded, for the sake of the `changed`
    /// list on the next one. Kept in memory between checkpoints; read back
    /// only after a cold start, and read as a one-file tree when the entry is
    /// from before a document was a directory.
    pub(super) async fn parent_tree(&self) -> Option<crate::document::history::Tree> {
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
        crate::document::history::load_tree(
            self.blobs.as_ref(),
            &self.storage_id,
            &point,
            &path,
            &id,
        )
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
    ) -> Result<(crate::document::history::Tree, HashMap<String, String>), String> {
        let (path, id) = {
            let state = self.state.lock().await;
            (
                session::main_path(&state.session.doc),
                session::main_id(&state.session.doc),
            )
        };
        // Install a lease for the event tree before the first object-store
        // read. Retention queues the tree in the same transaction that removes
        // its checkpoint row, so this admission either protects the read or
        // fails closed instead of leasing an already-deleted object.
        let lease = if let Some(catalog) = self.catalog.get() {
            let operation = crate::util::new_id();
            Some(
                begin_source_history_lease(
                    catalog,
                    &self.storage_id,
                    &operation,
                    vec![crate::storage::catalog::SourceHistoryObject {
                        object_key: crate::storage::blob::checkpoint_key(
                            &self.storage_id,
                            &point.sha,
                        ),
                        kind: "checkpoint_tree".into(),
                        bytes: 0,
                    }],
                    crate::util::now_unix(),
                    crate::util::now_unix().saturating_add(600),
                )
                .await
                .map_err(|error| error.to_string())?,
            )
        } else {
            None
        };
        let mut native_digests = std::collections::HashSet::new();
        let result = async {
            // Membership is part of the read admission for both native trees
            // and legacy one-file checkpoints.  A lease protects bytes, but
            // cannot make a checkpoint row that retention has already removed
            // authoritative again.
            if self
                .checkpoint_by_sha(&point.sha)
                .await?
                .is_none_or(|current| current.sha != point.sha)
            {
                return Err("checkpoint was removed while it was being read".into());
            }
            if point.tree {
                let (Some(catalog), Some(lease)) = (self.catalog.get(), lease.as_ref()) else {
                    return self
                        .checkpoint_cache
                        .load_checkpoint(self.blobs.as_ref(), &self.storage_id, point, &path, &id)
                        .await;
                };
                let raw = self
                    .checkpoint_cache
                    .get(
                        self.blobs.as_ref(),
                        &checkpoint_key(&self.storage_id, &point.sha),
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                let tree: history::Tree =
                    serde_json::from_slice(raw.as_slice()).map_err(|error| error.to_string())?;
                let mut objects = vec![crate::storage::catalog::SourceHistoryObject {
                    object_key: crate::storage::blob::checkpoint_key(&self.storage_id, &point.sha),
                    kind: "checkpoint_tree".into(),
                    bytes: i64::try_from(raw.len()).unwrap_or(i64::MAX),
                }];
                let mut seen = std::collections::HashSet::new();
                for digest in tree
                    .files
                    .values()
                    .filter(|entry| entry.kind == "text")
                    .map(|entry| entry.sha.as_str())
                    .filter(|digest| digest.len() == 64 && hex::decode(digest).is_ok())
                {
                    if !seen.insert(digest.to_owned()) {
                        continue;
                    }
                    if let Some(record) =
                        read_source_history_record(catalog, &self.storage_id, digest)
                            .await
                            .map_err(|error| error.to_string())?
                    {
                        native_digests.insert(digest.to_owned());
                        objects.push(crate::storage::catalog::SourceHistoryObject {
                            object_key: record.recipe_key,
                            kind: "source_recipe".into(),
                            bytes: record.recipe_bytes,
                        });
                        objects.extend(record.objects);
                    } else {
                        objects.push(crate::storage::catalog::SourceHistoryObject {
                            object_key: crate::storage::blob::content_blob_key(
                                &self.storage_id,
                                digest,
                            ),
                            kind: "source_legacy".into(),
                            bytes: 0,
                        });
                    }
                }
                // Assets are tree references too, but were previously absent
                // from the read lease. Their recorded byte size is the exact
                // physical upload size used by the asset writer.
                for entry in tree.files.values().filter(|entry| entry.kind == "asset") {
                    if entry.size < 0 {
                        return Err(format!(
                            "checkpoint asset {} has an invalid size",
                            entry.sha
                        ));
                    }
                    objects.push(crate::storage::catalog::SourceHistoryObject {
                        object_key: crate::storage::blob::asset_key(&self.storage_id, &entry.sha),
                        kind: "asset".into(),
                        bytes: entry.size,
                    });
                }
                // The tree itself was admitted before the first object read
                // with a provisional zero byte count.  Do not replay that
                // row with its discovered size: read leases protect lifetime,
                // while publication leases are the source of accounting, and
                // begin_source_history_lease intentionally rejects a changed
                // plan for an existing operation.  Every newly discovered
                // object (including assets with their exact physical size)
                // is added below before it is materialized.
                let tree_key = crate::storage::blob::checkpoint_key(&self.storage_id, &point.sha);
                let extension = objects
                    .into_iter()
                    .filter(|object| object.object_key != tree_key)
                    .collect();
                lease
                    .extend(
                        extension,
                        crate::util::now_unix(),
                        crate::util::now_unix().saturating_add(600),
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                // A checkpoint can be removed only after its tree row is
                // gone, but assets have no checkpoint graph table. Verify
                // their physical presence while the lease now blocks queued
                // cleanup, before a restore can install their references.
                for entry in tree.files.values().filter(|entry| entry.kind == "asset") {
                    let key = crate::storage::blob::asset_key(&self.storage_id, &entry.sha);
                    if !self
                        .blobs
                        .exists(&key)
                        .await
                        .map_err(|error| error.to_string())?
                    {
                        return Err(format!("checkpoint asset {} is not readable", entry.sha));
                    }
                }
            }
            self.checkpoint_cache
                .load_checkpoint_with_native_digests(
                    self.blobs.as_ref(),
                    &self.storage_id,
                    point,
                    &path,
                    &id,
                    &native_digests,
                )
                .await
        }
        .await;
        if let Some(lease) = lease {
            lease.finish().await.map_err(|error| error.to_string())?;
        }
        result
    }

    /// Look up one checkpoint without requiring the bounded resident history
    /// to contain it.  This is the cold-history path used by restore and
    /// checkpoint reads after the room has loaded only its recent tail.
    pub async fn checkpoint_by_sha(&self, sha: &str) -> Result<Option<Checkpoint>, String> {
        if let Some(catalog) = self.catalog.get() {
            let slug = self.slug.clone();
            let sha = sha.to_owned();
            return catalog
                .execute_catalog(512 + slug.len() + sha.len(), move |catalog| {
                    let Some(row) = catalog.checkpoint(&slug, &sha)? else {
                        return Ok(None);
                    };
                    let mut point = Manifest::from_catalog_rows(vec![row])
                        .map_err(crate::storage::catalog::CatalogError::Invalid)?
                        .checkpoints
                        .into_iter()
                        .next()
                        .expect("one checkpoint row");
                    if let Some((parent, gap)) =
                        catalog.checkpoint_retention_metadata(&slug, &sha)?
                    {
                        point.original_parent = parent;
                        point.ancestry_gap = gap;
                    }
                    Ok(Some(point))
                })
                .await
                .map_err(|error| error.to_string());
        }
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
                let Some(row) = read_catalog_checkpoint(catalog, &self.slug, sha)
                    .await
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
            let rows = read_checkpoints_prefix(catalog, &self.slug, prefix).await?;
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
            let rows = read_checkpoints_page(catalog, &self.slug, after_seq, limit).await?;
            let next = (rows.len() == limit.clamp(1, 200) as usize)
                .then(|| rows.last().map(|row| row.seq))
                .flatten();
            return Ok((rows, next));
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
        let (tree, bodies) = self
            .checkpoint_cache
            .load_checkpoint(self.blobs.as_ref(), &self.storage_id, &point, named, "")
            .await?;
        session::restore(doc, &tree, &bodies);
        Ok(())
    }

    /// Lease every physical object a selected tree may install into the live
    /// document. This second lease is deliberately held by restore until the
    /// merged CRDT update and its forced checkpoint have committed; the
    /// materialization lease in `checkpoint_texts` ends sooner for ordinary
    /// read-only history responses.
    async fn lease_checkpoint_objects(
        &self,
        point: &Checkpoint,
        tree: &history::Tree,
    ) -> Result<Option<crate::room::catalog::SourceHistoryLeaseGuard>, WriteError> {
        let Some(catalog) = self.catalog.get() else {
            return Ok(None);
        };
        if self
            .checkpoint_by_sha(&point.sha)
            .await
            .map_err(WriteError::Storage)?
            .is_none_or(|current| current.sha != point.sha)
        {
            return Err(WriteError::Storage(
                "checkpoint was removed while it was being restored".into(),
            ));
        }
        let mut objects = vec![crate::storage::catalog::SourceHistoryObject {
            object_key: crate::storage::blob::checkpoint_key(&self.storage_id, &point.sha),
            kind: "checkpoint_tree".into(),
            bytes: i64::try_from(tree.to_bytes().len()).unwrap_or(i64::MAX),
        }];
        let mut seen = std::collections::HashSet::new();
        for digest in tree
            .files
            .values()
            .filter(|entry| entry.kind == "text")
            .map(|entry| entry.sha.as_str())
            .filter(|digest| digest.len() == 64 && hex::decode(digest).is_ok())
        {
            if !seen.insert(digest.to_owned()) {
                continue;
            }
            if let Some(record) = read_source_history_record(catalog, &self.storage_id, digest)
                .await
                .map_err(|error| WriteError::Storage(error.to_string()))?
            {
                objects.push(crate::storage::catalog::SourceHistoryObject {
                    object_key: record.recipe_key,
                    kind: "source_recipe".into(),
                    bytes: record.recipe_bytes,
                });
                objects.extend(record.objects);
            } else {
                objects.push(crate::storage::catalog::SourceHistoryObject {
                    object_key: crate::storage::blob::content_blob_key(&self.storage_id, digest),
                    kind: "source_legacy".into(),
                    bytes: 0,
                });
            }
        }
        for entry in tree.files.values().filter(|entry| entry.kind == "asset") {
            if entry.size < 0 {
                return Err(WriteError::Storage(
                    "checkpoint asset has an invalid size".into(),
                ));
            }
            objects.push(crate::storage::catalog::SourceHistoryObject {
                object_key: crate::storage::blob::asset_key(&self.storage_id, &entry.sha),
                kind: "asset".into(),
                bytes: entry.size,
            });
        }
        let operation = crate::util::new_id();
        let lease = begin_source_history_lease(
            catalog,
            &self.storage_id,
            &operation,
            objects,
            crate::util::now_unix(),
            crate::util::now_unix().saturating_add(600),
        )
        .await?;
        for entry in tree.files.values().filter(|entry| entry.kind == "asset") {
            let key = crate::storage::blob::asset_key(&self.storage_id, &entry.sha);
            if !self
                .blobs
                .exists(&key)
                .await
                .map_err(|error| WriteError::Storage(error.to_string()))?
            {
                let _ = lease.finish().await;
                return Err(WriteError::Storage(format!(
                    "checkpoint asset {} is not readable",
                    entry.sha
                )));
            }
        }
        Ok(Some(lease))
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
    pub async fn label(&self, sha: &str, label: &str) -> Result<bool, WriteError> {
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
    ) -> Result<bool, WriteError> {
        self.label_as_authority(
            sha,
            label,
            actor.map(|actor| crate::storage::catalog::MutationAuthority {
                account_id: actor.0,
                owner_key: actor.1,
                generation: actor.2,
                link_hash: "",
                policy_editor: true,
                automation: false,
                unowned_publisher: false,
                execution_epoch: "",
                agent_checkpoint: None,
            }),
        )
        .await
    }

    /// Label using the complete request authority.  The authority reaches
    /// the catalogue write so links are checked again after the body read.
    pub async fn label_as_authority(
        &self,
        sha: &str,
        label: &str,
        actor: Option<crate::storage::catalog::MutationAuthority<'_>>,
    ) -> Result<bool, WriteError> {
        let _manifest_writer = self.manifest_write.lock().await;
        let resident = self.state.lock().await.manifest.has(sha);
        let catalog_checkpoint = match (resident, self.catalog.get()) {
            (false, Some(catalog)) => read_catalog_checkpoint(catalog, &self.slug, sha)
                .await
                .map_err(WriteError::from)?,
            _ => None,
        };
        if !resident && catalog_checkpoint.is_none() {
            return Ok(false);
        }
        if !self.hold().await {
            return Err(self.fenced());
        }
        // An old checkpoint need not be resident.  Label it directly in the
        // catalogue rather than manufacturing a partial Manifest and risking
        // a replacement of the unseen history.
        if let Some(catalog) = self.catalog.get() {
            let result = label_checkpoint(
                catalog,
                &self.slug,
                sha,
                label,
                actor.as_ref().map(OwnedAuthority::new),
            )
            .await;
            match result {
                Ok(_) => {}
                Err(crate::storage::catalog::CatalogError::NotFound) => return Ok(false),
                Err(error) => return Err(WriteError::from(error)),
            }
            let mut state = self.state.lock().await;
            if let Some(point) = state.manifest.checkpoints.iter_mut().find(|p| p.sha == sha) {
                point.label = label.to_string();
            }
            return Ok(true);
        }
        let mut staged = self.state.lock().await.manifest.clone();
        for point in staged.checkpoints.iter_mut() {
            if point.sha == sha {
                point.label = label.to_string();
            }
        }
        self.write_manifest(staged, 0, None, &[], &[], None).await?;
        Ok(true)
    }

    /// The caller holds manifest_write from staging through commit. Keep the
    /// room state available during storage I/O and expose changes only once
    /// the conditional write succeeds, so failed writes and labels remain safe.
    pub(super) async fn write_manifest(
        &self,
        mut staged: Manifest,
        durable_seq: i64,
        actor: Option<crate::room::catalog::OwnedAuthority>,
        sources: &[crate::storage::catalog::SourceHistoryRecord],
        assets: &[crate::storage::catalog::CheckpointAssetRef],
        lease_operation: Option<&str>,
    ) -> Result<(), WriteError> {
        if let Some(catalog) = self.catalog.get() {
            let previous = self.state.lock().await.manifest.clone();
            save_catalog_manifest_with_assets(
                catalog,
                &self.slug,
                &previous,
                &staged,
                durable_seq,
                actor,
                sources,
                assets,
                lease_operation,
                self.config.storage.per_owner,
                self.config.storage.total,
            )
            .await?;
            let excess = staged
                .checkpoints
                .len()
                .saturating_sub(RESIDENT_CATALOG_HISTORY as usize);
            staged.checkpoints.drain(..excess);
            let mut state = self.state.lock().await;
            *state.manifest = staged;
            return Ok(());
        }
        let body =
            serde_json::to_vec(&staged).map_err(|err| WriteError::Storage(err.to_string()))?;
        let mut version = self.state.lock().await.manifest_version.clone();
        self.write_owned(
            &crate::storage::blob::history_index_key(&self.slug),
            body,
            &mut version,
        )
        .await?;
        let mut state = self.state.lock().await;
        state.manifest_version = version;
        *state.manifest = staged;
        Ok(())
    }

    /// Restores a checkpoint and records both sides of the operation. The
    /// pre-restore checkpoint is the merge base for edits that arrive while
    /// the selected checkpoint is being read; the final checkpoint is a
    /// forced event even when its tree content is an older, already-known SHA.
    pub async fn restore_and_checkpoint(
        &self,
        point: &Checkpoint,
        by: impl Into<Attribution>,
    ) -> Result<(Vec<u8>, String), WriteError> {
        let by = &by.into();
        let _restore_writer = self.restore_write.lock().await;
        if self.read_only() {
            return Err(self.fenced());
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
                .checkpoint_impl("quiet", by, false, false, Some(&point.sha), None, None)
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
            return Err(self.fenced());
        }
        // Load the selected tree only after the base snapshot is fixed. Edits
        // arriving while this read is in flight are then merged against the
        // checkpoint that existed before the restore began.
        let (target_tree, target_bodies) = self.checkpoint_texts(point).await?;
        if self.read_only() || !self.hold().await {
            return Err(self.fenced());
        }
        // Re-admit every object the selected tree may install, immediately
        // before touching the live CRDT.  This lease remains alive through
        // the forced checkpoint below; a tree read lease alone would leave a
        // window in which retention could reclaim an asset while restore was
        // applying it.
        let target_lease = self.lease_checkpoint_objects(point, &target_tree).await?;

        let update = {
            let _assets_writer = self.assets_write.lock().await;
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
                        wasm_helpers::text::merge(&base, &live, &target).text
                    }
                    _ => target.clone(),
                };
                if body != target {
                    let mut effective = entry.clone();
                    effective.sha = crate::document::store::digest_of(&body);
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
        let restore_result = self.checkpoint_restore(by).await;
        let restored = match restore_result {
            Ok(Some(sha)) => sha,
            Ok(None) => {
                if let Some(lease) = target_lease {
                    let _ = lease.finish().await;
                }
                return Err(WriteError::Storage(
                    "could not create the restore checkpoint".into(),
                ));
            }
            Err(err) => {
                if let Some(lease) = target_lease {
                    let _ = lease.finish().await;
                }
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
        if let Some(lease) = target_lease {
            lease.finish().await?;
        }
        Ok((update, restored))
    }

    /* -------------------------------------------------------- suggestions */

    /// The most this document's session and history may occupy before it
    /// carries its owner or the deployment over a ceiling, less what the
    /// session state already costs. `None` means there is no catalogue quota;
    /// a negative `Some` value means the session alone is already over quota.
    pub(super) async fn allowance(&self, session_size: i64) -> Option<i64> {
        let store = self.store.get()?;
        store
            .room_for(&self.slug)
            .await
            .map(|room| room - session_size)
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

    /// The document's directory as a checkpoint would record it. What the
    /// timeline reads, what the document endpoint lists the paths of, and what
    /// a test asks when it wants to know the name the next checkpoint will
    /// have.
    pub async fn tree(&self) -> crate::document::history::Tree {
        let _publication_writer = self.publication_write.lock().await;
        let state = self.state.lock().await;
        tree_of(&state.session.doc, &state.session.asset_sizes).0
    }

    /// The caller already holds `publication_write`.  Keeping the actual
    /// compensating write separate avoids trying to take a non-reentrant
    /// mutex while a failed publication or suggestion is being unwound.
    pub(crate) async fn rollback_publication_inner(
        &self,
        tree: &crate::document::history::Tree,
        bodies: &HashMap<String, String>,
        format: &str,
    ) -> Result<(), WriteError> {
        {
            let _assets_writer = self.assets_write.lock().await;
            let mut state = self.state.lock().await;
            session::restore(&state.session.doc, tree, bodies);
            // Asset uploads are deliberately allowed to overlap the storage
            // portion of a publication so an independent upload cannot block
            // on a paused object write. Keep every digest currently held in
            // memory: the bytes remain durable even when this publication is
            // rolled back, and the normal pruning pass can reclaim anything
            // left unreferenced after the rollback.
            for entry in tree.files.values().filter(|entry| entry.kind == "asset") {
                state
                    .session
                    .asset_sizes
                    .insert(entry.sha.clone(), entry.size);
            }
            state.session.format = format.to_string();
            state.session.generation += 1;
            state.session.mark_dirty(now_unix());
            state.session.updated_at = now_unix();
        }
        self.write_session_inner(false, false).await.map(|_| ())
    }

    /// Puts a text at a path in the document, beside whatever is already
    /// there. What a directory publish adds each of its chapters with.
    pub async fn add_text(&self, path: &str, body: &str) -> Result<(), super::WriteError> {
        let _publication_writer = self.publication_write.lock().await;
        if self.read_only() {
            // Another server owns this room; adding to our copy would only
            // diverge from the one being persisted (R23).
            return Err(self.fenced());
        }
        let mut state = self.state.lock().await;
        if self.read_only() {
            return Err(self.fenced());
        }
        session::put_text(&state.session.doc, path, body);
        state.session.mark_dirty(now_unix());
        state.session.generation += 1;
        state.session.updated_at = now_unix();
        Ok(())
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
    pub(super) async fn prune_blobs(
        &self,
        written: &history::Tree,
        references: &retention::RetainedReferences,
    ) {
        let mut kept = {
            let state = self.state.lock().await;
            session::texts_of(&state.session.doc)
                .into_values()
                .map(|body| crate::document::store::digest_of(&body))
                .collect::<HashSet<_>>()
        };
        kept.extend(references.texts.iter().cloned());
        kept.extend(
            written
                .files
                .values()
                .filter(|entry| entry.kind == "text")
                .map(|entry| entry.sha.clone()),
        );
        // Standalone rooms have no catalogue graph/collector. Their writer
        // gate protects the native namespace while we resolve every retained
        // recipe before deleting anything. Catalogue rooms use durable edges
        // and leases instead of this fallback sweep.
        if self.catalog.get().is_none() {
            let recipe_prefix = crate::storage::blob::content_recipe_prefix(&self.storage_id);
            let chunk_prefix = crate::storage::blob::content_chunk_prefix(&self.storage_id);
            let Ok(recipes) = self.blobs.list(&recipe_prefix).await else {
                return;
            };
            let Ok(chunks) = self.blobs.list(&chunk_prefix).await else {
                return;
            };
            let mut reachable = HashSet::new();
            let mut obsolete = Vec::new();
            for object in recipes {
                let digest = object.key.rsplit('/').next().unwrap_or_default();
                if kept.contains(digest) {
                    let Ok(bytes) = self.blobs.get(&object.key).await else {
                        return;
                    };
                    let Ok(recipe) = crate::storage::encoding::Recipe::from_bytes(&bytes) else {
                        return;
                    };
                    reachable.extend(recipe.chunks.iter().map(|chunk| hex::encode(chunk.digest)));
                } else {
                    obsolete.push(object.key);
                }
            }
            obsolete.extend(
                chunks
                    .into_iter()
                    .filter(|object| {
                        !reachable.contains(object.key.rsplit('/').next().unwrap_or_default())
                    })
                    .map(|object| object.key),
            );
            if !obsolete.is_empty() && self.blobs.delete(&obsolete).await.is_err() {
                return;
            }
        }
        let Ok(found) = self
            .blobs
            .list(&crate::storage::blob::blob_prefix(&self.storage_id))
            .await
        else {
            return;
        };
        let mut gone: Vec<(String, i64)> = found
            .into_iter()
            .filter_map(|object| {
                let digest = object.key.rsplit('/').next()?;
                if kept.contains(digest) {
                    None
                } else {
                    Some((object.key.clone(), object.size))
                }
            })
            .collect();
        if gone.is_empty() {
            return;
        }
        // A source edit may have arrived while the object list was read.
        // Refresh live references at the deletion boundary. Text-object
        // writers remain excluded by this pass's checkpoint_write ownership;
        // later edits carry their text in the CRDT and a later checkpoint
        // rewrites any digest removed from blobs_written below.
        {
            let state = self.state.lock().await;
            let live: HashSet<_> = session::texts_of(&state.session.doc)
                .into_values()
                .map(|body| crate::document::store::digest_of(&body))
                .collect();
            gone.retain(|(key, _)| {
                key.rsplit('/')
                    .next()
                    .is_none_or(|digest| !live.contains(digest))
            });
        }
        if gone.is_empty() {
            return;
        }
        let digests: std::collections::HashSet<String> = gone
            .iter()
            .filter_map(|(key, _)| key.rsplit('/').next().map(str::to_owned))
            .collect();
        if let Some(catalog) = self.catalog.get() {
            // Catalogue rooms must hand deletion to the durable worker.  A
            // direct object-store delete would race a source-history reader,
            // and would also release accounting without a durable deletion
            // record if the process stopped between the two operations.
            let now = now_unix();
            for (key, bytes) in &gone {
                if let Err(error) =
                    crate::room::catalog::queue_object_delete(catalog, &self.slug, key, *bytes, now)
                        .await
                {
                    eprintln!(
                        "warning: could not queue obsolete source object {}: {error}",
                        key
                    );
                }
            }
            return;
        }
        let keys: Vec<String> = gone.into_iter().map(|(key, _)| key).collect();
        if self.blobs.delete(&keys).await.is_ok() {
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
        let (session_size, resident_history) = {
            let mut state = self.state.lock().await;
            let generation = state.session.generation;
            let size = match state.session.encoded_size {
                Some((encoded, size)) if encoded == generation => size,
                _ => {
                    let size = session::encode_state(&state.session.doc).len();
                    state.session.note_encoded_len(generation, size);
                    size as i64
                }
            };
            (size, state.manifest.bytes())
        };
        // Read with room state released. This is an accounting figure, not a
        // fence: the catalogue is the authority for history bytes and the
        // number it returns is whatever was durable when it was asked, so
        // stalling every editor of the document for the length of the query
        // bought nothing.
        let history = match self.catalog.get() {
            Some(catalog) => match read_checkpoint_stats(catalog, &self.slug).await {
                Ok((_, bytes)) => bytes,
                Err(error) => {
                    eprintln!(
                        "warning: could not read checkpoint accounting for {}: {error}",
                        self.slug
                    );
                    self.fence(FenceReason::UnreadableState);
                    return;
                }
            },
            None => resident_history,
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
                .await
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
