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

/// Owns the closure heartbeat for the whole physical write and final
/// verification/commit. Dropping a cancelled checkpoint aborts the task, so
/// no detached renewal can keep leases alive after its request is gone.
struct CheckpointHeartbeat {
    handle: Option<tokio::task::JoinHandle<()>>,
}

/// Close a prepared ordinary checkpoint only after a definitive final SQL
/// rejection. Busy/locked/I/O failures may have an uncertain commit outcome,
/// so those remain prepared for recovery rather than being falsely aborted.
fn definitive_checkpoint_catalog_error(error: &crate::storage::catalog::CatalogExecError) -> bool {
    match error {
        crate::storage::catalog::CatalogExecError::Catalog(error) => match error {
            crate::storage::catalog::CatalogError::Conflict(_)
            | crate::storage::catalog::CatalogError::Invalid(_)
            | crate::storage::catalog::CatalogError::Refused(_, _)
            | crate::storage::catalog::CatalogError::NotFound => true,
            crate::storage::catalog::CatalogError::Sql(rusqlite::Error::SqliteFailure(
                error,
                _,
            )) => error.code == rusqlite::ErrorCode::ConstraintViolation,
            crate::storage::catalog::CatalogError::Sql(_) => false,
            crate::storage::catalog::CatalogError::Busy
            | crate::storage::catalog::CatalogError::Closed => false,
        },
        crate::storage::catalog::CatalogExecError::Saturated
        | crate::storage::catalog::CatalogExecError::ShuttingDown
        | crate::storage::catalog::CatalogExecError::TooLarge { .. }
        | crate::storage::catalog::CatalogExecError::Panicked => false,
    }
}

impl CheckpointHeartbeat {
    async fn stop(mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
            let _ = handle.await;
        }
    }
}

impl Drop for CheckpointHeartbeat {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
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

/// Test gate after native rate admission and before durable checkpoint staging.
#[cfg(test)]
pub(crate) static AFTER_CHECKPOINT_ADMISSION: super::TestGate = std::sync::Mutex::new(None);

#[cfg(test)]
async fn pause_after_checkpoint_admission(slug: &str) {
    super::ReservationGate::park(&AFTER_CHECKPOINT_ADMISSION, slug).await;
}

impl Room {
    /// Takes a checkpoint, if the text differs from the last one. Returns the
    /// SHA of the checkpoint that now stands for the current text, or None
    /// when the request was deferred.
    ///
    /// Native publication settles the immutable object closure before the SQL
    /// transaction commits its checkpoint event and current head together.
    pub async fn checkpoint(
        &self,
        why: &str,
        by: impl Into<Attribution>,
    ) -> Result<Option<String>, WriteError> {
        self.checkpoint_impl(why, &by.into(), true, false, None, None)
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
        self.checkpoint_impl(why, &by.into(), false, false, None, None)
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
        // An agent operation is a distinct source effect even when its
        // resulting bytes equal an existing checkpoint. Its prepared receipt
        // must be settled by this checkpoint transaction, so it cannot take
        // the ordinary content-deduplication branch.
        let force_event = actor.agent_checkpoint.is_some();
        self.checkpoint_impl(why, &by.into(), false, force_event, None, Some(actor))
            .await
    }

    /// Immediate checkpoint for callers that already hold the publication
    /// checkpoint write gate. Reacquiring its read side would deadlock Tokio's
    /// non-reentrant RwLock, so agent source effects use this entry point while
    /// settling their prepared operation.
    pub(crate) async fn checkpoint_now_with_authority_locked(
        &self,
        why: &str,
        by: impl Into<Attribution>,
        actor: crate::storage::catalog::MutationAuthority<'_>,
    ) -> Result<Option<String>, WriteError> {
        let force_event = actor.agent_checkpoint.is_some();
        self.checkpoint_impl_locked(why, &by.into(), false, force_event, None, Some(actor))
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
        self.checkpoint_impl("restore", by, false, true, None, None)
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
        actor: Option<crate::storage::catalog::MutationAuthority<'_>>,
    ) -> Result<Option<String>, WriteError> {
        let _publication_checkpoint = self.publication_checkpoint.read().await;
        self.checkpoint_impl_locked(why, by, defer, force_event, protected, actor)
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
        _protected: Option<&str>,
        actor: Option<crate::storage::catalog::MutationAuthority<'_>>,
    ) -> Result<Option<String>, WriteError> {
        let _checkpoint_writer = self.checkpoint_write.lock().await;
        let now = now_unix();
        if self.catalog.get().is_none() {
            return Err(WriteError::Storage("room has no catalog".into()));
        }
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
        // Capture the durable fences before taking the tree snapshot. The
        // final v2 commit compares both fences again, so an acknowledgement
        // racing this capture conservatively rejects this checkpoint rather
        // than assigning its newer journal cursor to older tree bytes.
        let (snapshot_source_generation, snapshot_journal) =
            if let Some(catalog) = self.catalog.get() {
                let document_id = crate::storage::catalog::DocumentId::new(self.storage_id.clone())
                    .map_err(|error| WriteError::Storage(error.to_string()))?;
                let (source_generation, journal_epoch, journal_sequence) = catalog
                    .execute_catalog(256, move |catalog| catalog.v2_document_fence(&document_id))
                    .await
                    .map_err(WriteError::from)?;
                (source_generation, (journal_epoch, journal_sequence))
            } else {
                (0, (0, 0))
            };
        // A catalogue checkpoint is also the durable boundary for the live
        // CRDT state.  Keep an encoded recovery base in the same physical
        // closure as the source tree so a restart cannot replay an older
        // journal tail over a successfully committed checkpoint.  Agent
        // effects use the same path; the distinction is the parent operation
        // they reuse, not whether the base is needed.
        let needs_recovery_snapshot = self.catalog.get().is_some()
            || actor
                .as_ref()
                .and_then(|authority| authority.agent_checkpoint.as_ref())
                .is_some();
        let (
            tree,
            bodies,
            format,
            last,
            deferred,
            tree_generation,
            snapshot_state,
            snapshot_permit,
            snapshot_estimate,
            captured_socket_sequences,
        ) = {
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
                    None,
                    None,
                    0,
                    Vec::new(),
                )
            } else {
                let snapshot_ceiling = self.config.persistence().max_encoded_snapshot_bytes;
                let snapshot_estimate = if needs_recovery_snapshot {
                    // Direct Y.Doc mutations can bypass mark_dirty, leaving
                    // encoded_bound stale. Reserve the configured upper bound
                    // before encoding and apply the exact check below.
                    snapshot_ceiling.max(1)
                } else {
                    0
                };
                if needs_recovery_snapshot && snapshot_estimate > snapshot_ceiling {
                    return Err(WriteError::Size(crate::config::SizeRefusal::Encoded {
                        bytes: snapshot_estimate,
                        ceiling: snapshot_ceiling,
                    }));
                }
                let snapshot_permit = if needs_recovery_snapshot {
                    self.journal
                        .get()
                        .map(|journal| {
                            journal
                                .memory()
                                .try_acquire(crate::config::PersistenceLimits::staging_cost(
                                    snapshot_estimate,
                                ))
                                .map_err(|_| WriteError::ServerBusy)
                        })
                        .transpose()?
                } else {
                    None
                };
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
                let snapshot =
                    needs_recovery_snapshot.then(|| session::encode_state(&state.session.doc));
                if let Some(snapshot) = snapshot.as_ref() {
                    if snapshot.len() > snapshot_ceiling {
                        return Err(WriteError::Size(crate::config::SizeRefusal::Encoded {
                            bytes: snapshot.len(),
                            ceiling: snapshot_ceiling,
                        }));
                    }
                }
                let captured_socket_sequences = state
                    .sockets
                    .iter()
                    .filter(|(_, peer)| peer.may_edit)
                    .map(|(id, peer)| (*id, peer.sent))
                    .collect::<Vec<_>>();
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
                    snapshot,
                    snapshot_permit,
                    snapshot_estimate,
                    captured_socket_sequences,
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
            let resident_duplicate = state.session.last_tree.as_ref() == Some(&tree)
                || state.manifest.checkpoints.iter().rev().any(|point| {
                    point.sha == content_sha
                        || (!point.tree_sha.is_empty() && point.tree_sha == content_sha)
                });
            // V2 identifies checkpoint events separately from their content.
            // A return to historical content must publish a new event and head;
            // only a tree unchanged since the current checkpoint is a no-op.
            let duplicate = if self.catalog.get().is_some() {
                !state.session.last_checkpoint.is_empty()
                    && state.session.last_tree.as_ref() == Some(&tree)
            } else {
                resident_duplicate
            };
            drop(state);
            duplicate
        };
        // Authenticated writes publish the native closure under the final SQL fence.
        if !duplicate {
            if let (Some(catalog), Some(actor)) = (self.catalog.get(), actor.as_ref()) {
                return self
                    .checkpoint_v2_canonical(
                        catalog,
                        why,
                        by,
                        &tree,
                        &bodies,
                        &format,
                        &last,
                        tree_generation,
                        actor,
                        snapshot_source_generation,
                        snapshot_journal,
                        actor.agent_checkpoint,
                        snapshot_state.as_deref().unwrap_or_default(),
                        snapshot_permit,
                        snapshot_estimate,
                        &captured_socket_sequences,
                    )
                    .await;
            }
            if let Some(catalog) = self.catalog.get() {
                // Timer/importer checkpoints still need a real catalogue
                // identity.  Resolve the document's active owner account;
                // never manufacture an actor from the slug or an empty key.
                let document = read_catalog_document(catalog, &self.slug)
                    .await
                    .map_err(WriteError::from)?
                    .ok_or(WriteError::NotFound)?;
                let owner_id = document.owner_id.ok_or_else(|| {
                    WriteError::Storage("document has no checkpoint owner".into())
                })?;
                catalog
                    .execute_catalog(256, {
                        let owner_id = owner_id.clone();
                        move |catalog| catalog.account(&owner_id)
                    })
                    .await
                    .map_err(WriteError::from)?
                    .ok_or(WriteError::NotFound)?;
                let system_actor = crate::storage::catalog::MutationAuthority {
                    account_id: "",
                    owner_key: "",
                    generation: "",
                    link_hash: "",
                    policy_editor: false,
                    automation: false,
                    unowned_publisher: true,
                    execution_epoch: "",
                    agent_checkpoint: None,
                };
                return self
                    .checkpoint_v2_canonical(
                        catalog,
                        why,
                        by,
                        &tree,
                        &bodies,
                        &format,
                        &last,
                        tree_generation,
                        &system_actor,
                        snapshot_source_generation,
                        snapshot_journal,
                        None,
                        snapshot_state.as_deref().unwrap_or_default(),
                        snapshot_permit,
                        snapshot_estimate,
                        &captured_socket_sequences,
                    )
                    .await;
            }
        }
        // Quiet after quiet costs nothing: the same text is the same
        // checkpoint, and a checkpoint already in the manifest is not written
        // again and adds no entry.
        {
            let mut state = self.state.lock().await;
            state.session.asked = None;
            if !force_event {
                // The cached manifest may lag a committed v2 event. Its older
                // event with matching content must never replace the live head.
                let existing = if self.catalog.get().is_some() {
                    (!state.session.last_checkpoint.is_empty()
                        && state.session.last_tree.as_ref() == Some(&tree))
                    .then(|| state.session.last_checkpoint.clone())
                } else {
                    state
                        .manifest
                        .checkpoints
                        .iter()
                        .rev()
                        .find(|point| {
                            point.sha == content_sha
                                || (!point.tree_sha.is_empty() && point.tree_sha == content_sha)
                        })
                        .map(|point| point.sha.clone())
                        .or_else(|| {
                            (state.session.last_tree.as_ref() == Some(&tree)
                                && !state.session.last_checkpoint.is_empty())
                            .then(|| state.session.last_checkpoint.clone())
                        })
                };
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
                    drop(state);
                    // Reusing a tree does not mean the current Y.Doc is
                    // already durable: CRDT item identities and concurrent
                    // edits can differ while the visible tree is identical.
                    // Persist and acknowledge that state before success.
                    self.write_session_inner(true, true).await?;
                    if let Some(catalog) = self.catalog.get() {
                        if let Some(actor) = actor {
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
                        } else {
                            let slug = self.slug.clone();
                            catalog
                                .execute_catalog(slug.len() + 128, move |catalog| {
                                    catalog.require_internal_checkpoint_authority(&slug)
                                })
                                .await
                                .map_err(WriteError::from)?;
                        }
                    }
                    let mut state = self.state.lock().await;
                    state.session.last_checkpoint = existing.clone();
                    state.session.last_tree = Some(tree.clone());
                    state.session.checkpoint_generation = tree_generation;
                    if state.session.generation == tree_generation {
                        state.session.pending_checkpoint_since = 0;
                    }
                    drop(state);
                    if why == "automatic" {
                        if let Some(catalog) = self.catalog.get() {
                            touch_auto_checkpoint(catalog, &self.slug, now).await;
                        }
                    }
                    return Ok(Some(existing));
                }
            }
        }
        Err(WriteError::Storage(
            "checkpoint state changed before publication".into(),
        ))
    }

    /// Write a catalogue checkpoint as one canonical v2 physical closure.
    ///
    /// Admission installs the operation, every allocation, counters, and
    /// stage leases before object I/O.  The final verified commit inserts the
    /// checkpoint graph, advances the document head, and settles the receipt
    /// in one SQLite transaction.  This is deliberately kept separate from
    /// the old source-history writer until callers without a MutationAuthority
    /// have been removed; those callers cannot satisfy the final auth fence.
    // Keep the explicit fields at this authenticated transaction boundary.
    #[allow(clippy::too_many_arguments)]
    async fn checkpoint_v2_canonical(
        &self,
        catalog: &Arc<crate::storage::catalog::Catalog>,
        why: &str,
        by: &Attribution,
        tree: &crate::document::history::Tree,
        bodies: &HashMap<String, String>,
        format: &str,
        parent: &str,
        tree_generation: u64,
        actor: &crate::storage::catalog::MutationAuthority<'_>,
        source_generation: i64,
        journal: (i64, i64),
        agent_checkpoint: Option<&crate::storage::catalog::AgentCheckpointCommit>,
        snapshot_state: &[u8],
        snapshot_permit: Option<crate::storage::journal::MemoryPermit>,
        snapshot_estimate: usize,
        captured_socket_sequences: &[(u64, i64)],
    ) -> Result<Option<String>, WriteError> {
        use crate::storage::blob::ObjectId as BlobObjectId;
        use crate::storage::catalog::{
            CheckpointCommit, DocumentId, ObjectId, ObjectKind, OperationId, OperationKind,
            OperationScope, SourceFormat, UnixMillis, V2AdmissionLimits,
            V2CheckpointAdmissionInput, V2ObjectAllocation, V2OperationInput,
        };
        use crate::storage::encoding::{
            PhysicalLocator, SourceRecipeEnvelope, TreeEnvelope, TreeFileLocator,
            SOURCE_ENVELOPE_VERSION, TREE_ENVELOPE_VERSION,
        };
        use sha2::{Digest, Sha256};

        if !self.hold().await {
            return Err(self.fenced());
        }
        let document_id = DocumentId::new(self.storage_id.clone())
            .map_err(|error| WriteError::Storage(error.to_string()))?;
        let now_ms = crate::util::now_millis();
        let now =
            UnixMillis::new(now_ms).map_err(|error| WriteError::Storage(error.to_string()))?;
        let (journal_epoch, journal_sequence) = journal;
        let durable_owner_id = catalog
            .execute_catalog(128, {
                let document_id = document_id.clone();
                move |catalog| catalog.v2_document_owner_id(&document_id)
            })
            .await
            .map_err(WriteError::from)?;
        let rate_owner = format!("account:{durable_owner_id}");
        let mut owner_rate = catalog
            .reserve_process_rate(
                &rate_owner,
                "checkpoint_owner",
                self.config.session.checkpoint_owner_per_hour.max(0) as usize,
            )
            .map_err(WriteError::from)?;
        let mut deployment_rate = catalog
            .reserve_process_rate(
                "deployment",
                "checkpoint_deployment",
                self.config.session.checkpoint_deployment_per_hour.max(0) as usize,
            )
            .map_err(WriteError::from)?;
        #[cfg(test)]
        pause_after_checkpoint_admission(&self.slug).await;
        let generated_operation_id = OperationId::new(hex::encode(crate::auth::random_bytes(16)))
            .map_err(|error| WriteError::Storage(error.to_string()))?;
        let checkpoint_id =
            crate::storage::catalog::CheckpointId::new(hex::encode(crate::auth::random_bytes(16)))
                .map_err(|error| WriteError::Storage(error.to_string()))?;

        let source_format = match format {
            "markdown" => SourceFormat::Markdown,
            "html" => SourceFormat::Html,
            "typst" => SourceFormat::Typst,
            "latex" => SourceFormat::Latex,
            "quarto" => SourceFormat::Quarto,
            _ => return Err(WriteError::Storage("invalid source format".into())),
        };
        let snapshot_ceiling = self.config.persistence().max_encoded_snapshot_bytes;
        if snapshot_state.len() > snapshot_estimate {
            return Err(WriteError::Size(crate::config::SizeRefusal::Encoded {
                bytes: snapshot_state.len(),
                ceiling: snapshot_estimate,
            }));
        }
        if snapshot_state.len() > snapshot_ceiling {
            return Err(WriteError::Size(crate::config::SizeRefusal::Encoded {
                bytes: snapshot_state.len(),
                ceiling: snapshot_ceiling,
            }));
        }
        let estimated_logical_bytes = tree.files.values().try_fold(0usize, |total, file| {
            let bytes = usize::try_from(file.size.max(0))
                .map_err(|_| WriteError::Storage("checkpoint size exceeds memory bounds".into()))?;
            total
                .checked_add(bytes)
                .ok_or_else(|| WriteError::Storage("checkpoint size exceeds memory bounds".into()))
        })?;
        if estimated_logical_bytes > self.config.persistence().max_encoded_snapshot_bytes {
            return Err(WriteError::Size(crate::config::SizeRefusal::Encoded {
                bytes: estimated_logical_bytes,
                ceiling: self.config.persistence().max_encoded_snapshot_bytes,
            }));
        }
        // The logical tree is copied into encoded chunks, recipe envelopes,
        // and the final tree envelope. Hold the shared journal budget for the
        // entire closure so concurrent rooms cannot all pass this estimate
        // and exhaust process memory during encoding and verification.
        let _memory_permit = self
            .journal
            .get()
            .map(|journal| {
                journal
                    .memory()
                    .try_acquire(crate::config::PersistenceLimits::staging_cost(
                        estimated_logical_bytes,
                    ))
                    .map_err(|_| WriteError::ServerBusy)
            })
            .transpose()?;
        let detached_snapshot_permit = snapshot_permit.map(std::sync::Arc::new);

        struct PhysicalObject {
            id: ObjectId,
            kind: ObjectKind,
            bytes: Vec<u8>,
            content_type: &'static str,
            digest: String,
            logical_digest: Option<String>,
            write: bool,
        }

        let mut physical = Vec::<PhysicalObject>::new();
        let mut files = std::collections::BTreeMap::new();
        for (path, entry) in &tree.files {
            if entry.kind == "text" {
                let body = bodies.get(&entry.sha).ok_or_else(|| {
                    WriteError::Storage(format!("checkpoint text body {} is missing", entry.sha))
                })?;
                let plan = source_encoding_pool()
                    .try_plan(body.as_bytes().to_vec())
                    .await
                    .map_err(|error| WriteError::Storage(error.to_string()))?;
                let encoded = source_encoding_pool()
                    .try_encode_planned(
                        body.as_bytes().to_vec(),
                        plan,
                        std::collections::HashSet::new(),
                    )
                    .await
                    .map_err(|error| WriteError::Storage(error.to_string()))?;
                if hex::encode(encoded.file_digest) != entry.sha {
                    return Err(WriteError::Storage(
                        "checkpoint source digest differs from tree".into(),
                    ));
                }
                let mut locators_by_digest =
                    std::collections::HashMap::with_capacity(encoded.objects.len());
                for object in &encoded.objects {
                    let id = ObjectId::new(hex::encode(crate::auth::random_bytes(16)))
                        .map_err(|error| WriteError::Storage(error.to_string()))?;
                    let object_digest: [u8; 32] = Sha256::digest(&object.encoded).into();
                    let locator = PhysicalLocator {
                        object_id: BlobObjectId::parse(id.as_str().to_owned())
                            .map_err(|error| WriteError::Storage(error.to_string()))?,
                        object_digest,
                        logical_digest: Some(object.digest),
                        logical_length: object.uncompressed_len as u64,
                        byte_length: object.encoded.len() as u64,
                        encoding_version: 1,
                    };
                    locators_by_digest.insert(object.digest, locator);
                    physical.push(PhysicalObject {
                        id,
                        kind: ObjectKind::SourceChunk,
                        bytes: object.encoded.clone(),
                        content_type: "application/vnd.librepaper.source-chunk",
                        digest: hex::encode(object_digest),
                        logical_digest: Some(hex::encode(object.digest)),
                        write: true,
                    });
                }
                let chunk_locators = encoded
                    .recipe
                    .chunks
                    .iter()
                    .map(|chunk| {
                        let locator = locators_by_digest.get(&chunk.digest).ok_or_else(|| {
                            WriteError::Storage("source recipe references an absent chunk".into())
                        })?;
                        if locator.logical_length != u64::from(chunk.length) {
                            return Err(WriteError::Storage(
                                "source recipe chunk length differs from encoded object".into(),
                            ));
                        }
                        Ok(locator.clone())
                    })
                    .collect::<Result<Vec<_>, WriteError>>()?;
                let recipe_envelope = SourceRecipeEnvelope {
                    version: SOURCE_ENVELOPE_VERSION,
                    recipe: encoded.recipe.clone(),
                    chunk_locators,
                };
                let recipe_bytes = recipe_envelope
                    .to_bytes()
                    .map_err(|error| WriteError::Storage(error.to_string()))?;
                let recipe_id = ObjectId::new(hex::encode(crate::auth::random_bytes(16)))
                    .map_err(|error| WriteError::Storage(error.to_string()))?;
                let recipe_digest: [u8; 32] = Sha256::digest(&recipe_bytes).into();
                let recipe_locator = PhysicalLocator {
                    object_id: BlobObjectId::parse(recipe_id.as_str().to_owned())
                        .map_err(|error| WriteError::Storage(error.to_string()))?,
                    object_digest: recipe_digest,
                    logical_digest: Some(encoded.file_digest),
                    logical_length: body.len() as u64,
                    byte_length: recipe_bytes.len() as u64,
                    encoding_version: 1,
                };
                physical.push(PhysicalObject {
                    id: recipe_id.clone(),
                    kind: ObjectKind::SourceRecipe,
                    bytes: recipe_bytes,
                    content_type: "application/vnd.librepaper.source-recipe",
                    digest: hex::encode(recipe_digest),
                    logical_digest: Some(entry.sha.clone()),
                    write: true,
                });
                files.insert(
                    path.clone(),
                    TreeFileLocator {
                        kind: "text".into(),
                        file_id: entry.id.clone(),
                        logical_digest: encoded.file_digest,
                        logical_length: body.len() as u64,
                        recipe: Some(recipe_locator),
                        asset: None,
                    },
                );
            } else if entry.kind == "asset" {
                let v2_asset = catalog
                    .execute_catalog(256, {
                        let document_id = document_id.clone();
                        let digest = entry.sha.clone();
                        move |catalog| {
                            catalog.available_object_by_logical_digest(
                                &document_id,
                                &digest,
                                ObjectKind::Asset,
                            )
                        }
                    })
                    .await
                    .map_err(WriteError::from)?;
                let asset_key = v2_asset
                    .as_ref()
                    .map(|object| object.storage_key.clone())
                    .ok_or_else(|| {
                        WriteError::Storage("asset is absent from the v2 object graph".into())
                    })?;
                let read_lease = if let Some(asset) = v2_asset.as_ref() {
                    let (_, writer_generation, _) = catalog
                        .execute_catalog(256, |catalog| catalog.v2_server_state())
                        .await
                        .map_err(WriteError::from)?;
                    let read_holder = format!("checkpoint-read:{}:{}", document_id, asset.id);
                    let lease_now = UnixMillis::new(crate::util::now_millis())
                        .map_err(|error| WriteError::Storage(error.to_string()))?;
                    let lease_expiry =
                        UnixMillis::new(lease_now.0.checked_add(120_000).ok_or_else(|| {
                            WriteError::Storage("asset read lease overflow".into())
                        })?)
                        .map_err(|error| WriteError::Storage(error.to_string()))?;
                    let document = document_id.clone();
                    let object = asset.id.clone();
                    let holder = read_holder.clone();
                    let generation = writer_generation.clone();
                    catalog
                        .execute_catalog(512, move |catalog| {
                            catalog.acquire_v2_read_set(
                                &document,
                                std::slice::from_ref(&object),
                                &holder,
                                &generation,
                                lease_expiry,
                                lease_now,
                            )
                        })
                        .await
                        .map_err(WriteError::from)?;
                    Some((asset.id.clone(), read_holder))
                } else {
                    None
                };
                let bytes_result = self.blobs.get(&asset_key).await;
                if let Some((object_id, holder)) = read_lease {
                    let document = document_id.clone();
                    let holder = holder.clone();
                    let _ = catalog
                        .execute_catalog(256, move |catalog| {
                            catalog.release_v2_lease(&document, &object_id, &holder)
                        })
                        .await;
                }
                let bytes = bytes_result.map_err(|error| WriteError::Storage(error.to_string()))?;
                let digest = hex::encode(Sha256::digest(&bytes));
                let expected_physical_digest = v2_asset
                    .as_ref()
                    .map(|asset| asset.digest.as_str())
                    .unwrap_or(entry.sha.as_str());
                if digest != expected_physical_digest
                    || entry.size < 0
                    || bytes.len() as i64 != entry.size
                {
                    return Err(WriteError::Storage(
                        "checkpoint asset bytes differ from tree".into(),
                    ));
                }
                let (id, object_digest, write) = if let Some(asset) = v2_asset.as_ref() {
                    let digest = hex::decode(&asset.digest).map_err(|_| {
                        WriteError::Storage("invalid canonical asset digest".into())
                    })?;
                    let object_digest: [u8; 32] = digest.try_into().map_err(|_| {
                        WriteError::Storage("invalid canonical asset digest".into())
                    })?;
                    (asset.id.clone(), object_digest, false)
                } else {
                    let id = ObjectId::new(hex::encode(crate::auth::random_bytes(16)))
                        .map_err(|error| WriteError::Storage(error.to_string()))?;
                    let object_digest: [u8; 32] = Sha256::digest(&bytes).into();
                    (id, object_digest, true)
                };
                let logical_digest: [u8; 32] = hex::decode(&entry.sha)
                    .map_err(|_| WriteError::Storage("invalid asset logical digest".into()))?
                    .try_into()
                    .map_err(|_| WriteError::Storage("invalid asset logical digest".into()))?;
                let locator = PhysicalLocator {
                    object_id: BlobObjectId::parse(id.as_str().to_owned())
                        .map_err(|error| WriteError::Storage(error.to_string()))?,
                    object_digest,
                    logical_digest: Some(logical_digest),
                    logical_length: bytes.len() as u64,
                    byte_length: bytes.len() as u64,
                    encoding_version: 1,
                };
                physical.push(PhysicalObject {
                    id,
                    kind: ObjectKind::Asset,
                    bytes,
                    content_type: "application/octet-stream",
                    digest: digest.clone(),
                    logical_digest: Some(entry.sha.clone()),
                    write,
                });
                files.insert(
                    path.clone(),
                    TreeFileLocator {
                        kind: "asset".into(),
                        file_id: String::new(),
                        logical_digest,
                        logical_length: entry.size as u64,
                        recipe: None,
                        asset: Some(locator),
                    },
                );
            } else {
                return Err(WriteError::Storage(
                    "checkpoint has unknown file kind".into(),
                ));
            }
        }

        let settings_json = serde_json::to_string(&tree.settings)
            .map_err(|error| WriteError::Storage(error.to_string()))?;
        let mut tree_envelope = TreeEnvelope {
            version: TREE_ENVELOPE_VERSION,
            main_path: tree.main.clone(),
            source_format: format.to_string(),
            settings_json,
            logical_digest: [0; 32],
            files,
        };
        let logical_tree = tree_envelope
            .logical_bytes()
            .map_err(|error| WriteError::Storage(error.to_string()))?;
        tree_envelope.logical_digest = Sha256::digest(&logical_tree).into();
        let tree_bytes = tree_envelope
            .to_bytes()
            .map_err(|error| WriteError::Storage(error.to_string()))?;
        let tree_id = ObjectId::new(hex::encode(crate::auth::random_bytes(16)))
            .map_err(|error| WriteError::Storage(error.to_string()))?;
        let tree_physical_digest: [u8; 32] = Sha256::digest(&tree_bytes).into();
        physical.push(PhysicalObject {
            id: tree_id.clone(),
            kind: ObjectKind::SourceTree,
            bytes: tree_bytes,
            content_type: "application/vnd.librepaper.source-tree",
            digest: hex::encode(tree_physical_digest),
            logical_digest: None,
            write: true,
        });

        // Every catalogue checkpoint carries the same encoded recovery base
        // used by the v2 journal runtime.  It is allocated under the ordinary
        // checkpoint operation or the parent agent operation and promoted
        // with the checkpoint, so a crash before a journal flush still
        // reopens the exact state captured by this checkpoint.
        let journal_base = if self.catalog.get().is_some() || agent_checkpoint.is_some() {
            let payload = snapshot_state.to_vec();
            let base_epoch = journal_epoch
                .checked_add(1)
                .ok_or_else(|| WriteError::Storage("journal epoch overflow".into()))?;
            let base_sequence = journal_sequence.max(1);
            let body = crate::storage::journal::RecoveryBaseBody {
                format_version: crate::storage::journal::SEGMENT_FORMAT,
                storage_id: self.storage_id.clone(),
                epoch: u64::try_from(base_epoch)
                    .map_err(|_| WriteError::Storage("journal epoch is negative".into()))?,
                sequence: u64::try_from(base_sequence)
                    .map_err(|_| WriteError::Storage("journal sequence is negative".into()))?,
                digest: hex::encode(Sha256::digest(&payload)),
                payload,
            };
            let bytes = crate::storage::journal::encode_recovery_base(&body)
                .map_err(|error| WriteError::Storage(error.to_string()))?;
            // Decode the exact bytes that will be written before admission
            // can name them.  The SQL commit rechecks the immutable object
            // digest and lease, while this proof covers the binary recovery
            // envelope and its payload identity outside SQLite.
            let decoded = crate::storage::journal::decode_recovery_base(&bytes)
                .map_err(|error| WriteError::Storage(error.to_string()))?;
            if decoded != body {
                return Err(WriteError::Storage(
                    "encoded journal base did not round-trip".into(),
                ));
            }
            let id = ObjectId::new(hex::encode(crate::auth::random_bytes(16)))
                .map_err(|error| WriteError::Storage(error.to_string()))?;
            let digest = hex::encode(Sha256::digest(&bytes));
            physical.push(PhysicalObject {
                id: id.clone(),
                kind: ObjectKind::JournalBase,
                bytes,
                content_type: "application/vnd.librepaper.journal-base",
                digest: digest.clone(),
                logical_digest: None,
                write: true,
            });
            Some((id, digest, base_epoch, base_sequence))
        } else {
            None
        };

        // The tree is committed first in the closure so replay and recovery
        // always have a canonical root, while the digest covers every object.
        let mut object_ids = Vec::with_capacity(physical.len());
        for object in physical.iter().rev() {
            if journal_base
                .as_ref()
                .is_none_or(|(base_id, _, _, _)| object.id != *base_id)
            {
                object_ids.push(object.id.clone());
            }
        }
        object_ids.reverse();
        let mut closure_hasher = Sha256::new();
        for id in &object_ids {
            closure_hasher.update(id.as_str().as_bytes());
            closure_hasher.update([0]);
        }
        let closure_digest = hex::encode(closure_hasher.finalize());
        let authority_account = if !actor.account_id.is_empty() {
            actor.account_id.to_owned()
        } else if !actor.owner_key.is_empty() {
            format!(
                "anonymous:{}",
                hex::encode(Sha256::digest(actor.owner_key.as_bytes()))
            )
        } else {
            String::new()
        };
        if authority_account.starts_with("anonymous:")
            && (!actor.owner_key.is_empty()
                && authority_account
                    != format!(
                        "anonymous:{}",
                        hex::encode(Sha256::digest(actor.owner_key.as_bytes()))
                    ))
        {
            return Err(WriteError::Storage(
                "anonymous actor credential does not match its account".into(),
            ));
        }
        let authority_generation = if !authority_account.is_empty() {
            let account = catalog
                .execute_catalog(256, {
                    let account_id = authority_account.clone();
                    move |catalog| catalog.account(&account_id)
                })
                .await
                .map_err(WriteError::from)?
                .ok_or(WriteError::NotFound)?;
            if account.status != "active" {
                return Err(WriteError::Storage(
                    "checkpoint actor account is not active".into(),
                ));
            }
            if !actor.generation.is_empty() && actor.generation != account.session_generation {
                return Err(WriteError::Conflict(
                    "checkpoint actor session generation changed".into(),
                ));
            }
            if actor.generation.is_empty() && !authority_account.starts_with("anonymous:") {
                return Err(WriteError::Conflict(
                    "checkpoint actor session generation is missing".into(),
                ));
            }
            account.session_generation
        } else {
            String::new()
        };
        let authority = serde_json::json!({
            "account_id": authority_account,
            "session_generation": authority_generation,
            "link_hash": actor.link_hash,
            "policy_editor": actor.policy_editor,
            "automation": actor.automation,
            "internal_checkpoint": actor.unowned_publisher
                && actor.account_id.is_empty()
                && actor.owner_key.is_empty()
                && actor.link_hash.is_empty(),
            "execution_epoch": actor.execution_epoch,
            "agent_source_revision": agent_checkpoint
                .map(|proof| proof.source_revision.as_str()),
        });
        let plan_json = serde_json::json!({
            "version": 2,
            "effect": "checkpoint",
            "source_format": format,
            "main": tree.main,
            "closure_digest": closure_digest,
            "tree_digest": hex::encode(tree_envelope.logical_digest),
            "tree_physical_digest": hex::encode(tree_physical_digest),
            "journal_base_object_id": journal_base
                .as_ref()
                .map(|(id, _, _, _)| id.as_str()),
            "journal_base_digest": journal_base
                .as_ref()
                .map(|(_, digest, _, _)| digest.as_str()),
            "journal_base_epoch": journal_base.as_ref().map(|(_, _, epoch, _)| epoch),
            "journal_base_sequence": journal_base.as_ref().map(|(_, _, _, sequence)| sequence),
            "authority": authority,
        })
        .to_string();
        let request_digest = hex::encode(Sha256::digest(
            serde_json::json!({
                "version": 2,
                "effect": "checkpoint",
                "document": self.slug.clone(),
                "why": why,
                "tree": hex::encode(tree_envelope.logical_digest),
                "generation": tree_generation,
                "objects": object_ids.iter().map(ObjectId::as_str).collect::<Vec<_>>(),
            })
            .to_string(),
        ));
        let actor_key = if !actor.account_id.is_empty() {
            format!("account:{}", actor.account_id)
        } else if !actor.link_hash.is_empty() {
            format!("link:{}", actor.link_hash)
        } else if actor.unowned_publisher {
            "internal".to_string()
        } else {
            return Err(WriteError::Storage(
                "checkpoint actor has no accountable identity".into(),
            ));
        };
        let operation_id = if let Some(agent) = agent_checkpoint {
            let document = document_id.clone();
            let actor_key = actor_key.clone();
            let request_key = agent.request_id.clone();
            catalog
                .execute_catalog(512, move |catalog| {
                    catalog.prepared_agent_operation_id(&document, &actor_key, &request_key)
                })
                .await
                .map_err(WriteError::from)?
                .ok_or_else(|| {
                    WriteError::Conflict(
                        "agent source operation is not prepared for checkpoint commit".into(),
                    )
                })?
        } else {
            generated_operation_id
        };
        let operation_expires = now_ms
            .checked_add(120_000)
            .ok_or_else(|| WriteError::Storage("operation expiry overflow".into()))?;
        let operation = V2OperationInput {
            scope: OperationScope::Document(document_id.clone()),
            actor_key,
            request_key: crate::util::new_request_key(),
            kind: OperationKind::Checkpoint,
            request_digest,
            plan_json,
            expected_document_generation: Some(source_generation),
            conversation_id: None,
            execution_epoch: (!actor.execution_epoch.is_empty())
                .then(|| actor.execution_epoch.to_owned()),
            work_expires_at: Some(
                UnixMillis::new(operation_expires)
                    .map_err(|error| WriteError::Storage(error.to_string()))?,
            ),
        };
        let allocations = physical
            .iter()
            .filter(|object| object.write)
            .map(|object| {
                Ok::<_, WriteError>(V2ObjectAllocation {
                    document_id: document_id.clone(),
                    id: object.id.clone(),
                    storage_key: format!("v2/documents/{}/objects/{}", document_id, object.id),
                    kind: object.kind,
                    digest: object.digest.clone(),
                    logical_digest: object.logical_digest.clone(),
                    encoding_version: 1,
                    reserved_bytes: i64::try_from(object.bytes.len()).map_err(|_| {
                        WriteError::Storage("checkpoint object is too large".into())
                    })?,
                    operation_id: operation_id.clone(),
                    now,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let reused_object_ids = physical
            .iter()
            .filter(|object| !object.write)
            .map(|object| object.id.clone())
            .collect::<Vec<_>>();
        let lease_expires = UnixMillis::new(
            now_ms
                .checked_add(120_000)
                .ok_or_else(|| WriteError::Storage("lease expiry overflow".into()))?,
        )
        .map_err(|error| WriteError::Storage(error.to_string()))?;
        let holder = format!("checkpoint:{}", operation_id.as_str());
        let (admitted_operation, writer_generation) = catalog
            .execute_catalog(4096 + physical.len() * 256, {
                let input = V2CheckpointAdmissionInput {
                    document_id: document_id.clone(),
                    operation_id: operation_id.clone(),
                    operation,
                    allocations,
                    reused_object_ids,
                    lease_holder: holder.clone(),
                    lease_expires_at: lease_expires,
                    limits: V2AdmissionLimits {
                        owner_bytes: if self.config.storage.per_owner < 0 {
                            i64::MAX
                        } else {
                            self.config.storage.per_owner
                        },
                        deployment_bytes: if self.config.storage.total < 0 {
                            i64::MAX
                        } else {
                            self.config.storage.total
                        },
                        owner_documents: self.config.storage.documents_per_owner as i64,
                    },
                    now,
                };
                move |catalog| catalog.admit_v2_checkpoint(input)
            })
            .await
            .map_err(WriteError::from)?;

        // A request-key replay is returned by admission before any new
        // allocation is written. Reuse its durable receipt instead of
        // generating a second physical closure. A prepared replay cannot be
        // safely resumed with fresh IDs, so leave it for the original fenced
        // writer to finish and make the caller retry with a new key.
        if admitted_operation.id != operation_id {
            if admitted_operation.state == "committed" {
                let document = document_id.clone();
                let replay_id = admitted_operation.id.clone();
                let result = catalog
                    .execute_catalog(512, move |catalog| {
                        catalog.v2_operation_result(&document, &replay_id)
                    })
                    .await
                    .map_err(WriteError::from)?;
                let Some((state, result_json)) = result else {
                    return Err(WriteError::Storage(
                        "checkpoint replay receipt is missing".into(),
                    ));
                };
                if state != "committed" {
                    return Err(WriteError::Conflict(
                        "checkpoint replay is no longer committed".into(),
                    ));
                }
                let value: serde_json::Value = serde_json::from_str(&result_json)
                    .map_err(|_| WriteError::Storage("invalid checkpoint replay receipt".into()))?;
                let checkpoint_id = value
                    .get("checkpoint_id")
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        WriteError::Storage("checkpoint replay receipt has no checkpoint id".into())
                    })?
                    .to_owned();
                return Ok(Some(checkpoint_id));
            }
            return Err(WriteError::Conflict(
                "checkpoint request is already being prepared; retry after it settles".into(),
            ));
        }

        let writer = crate::storage::v2_catalog::V2ObjectWriter::new(
            Arc::clone(catalog),
            Arc::clone(&self.blobs),
        );
        let heartbeat_error = Arc::new(std::sync::Mutex::new(None::<String>));
        let heartbeat_catalog = Arc::clone(catalog);
        let heartbeat_document = document_id.clone();
        let heartbeat_ids = physical
            .iter()
            .map(|object| object.id.clone())
            .collect::<Vec<_>>();
        let heartbeat_holder = holder.clone();
        let heartbeat_operation = admitted_operation.id.clone();
        let heartbeat_generation = writer_generation.clone();
        let heartbeat_error_slot = Arc::clone(&heartbeat_error);
        let heartbeat = CheckpointHeartbeat {
            handle: Some(tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
                loop {
                    interval.tick().await;
                    let current_ms = crate::util::now_millis();
                    let current = match UnixMillis::new(current_ms) {
                        Ok(value) => value,
                        Err(error) => {
                            if let Ok(mut slot) = heartbeat_error_slot.lock() {
                                *slot = Some(error.to_string());
                            }
                            break;
                        }
                    };
                    let expiry = match UnixMillis::new(current_ms.saturating_add(120_000)) {
                        Ok(value) => value,
                        Err(error) => {
                            if let Ok(mut slot) = heartbeat_error_slot.lock() {
                                *slot = Some(error.to_string());
                            }
                            break;
                        }
                    };
                    let result = heartbeat_catalog
                        .execute_catalog(heartbeat_ids.len() * 64 + 256, {
                            let document = heartbeat_document.clone();
                            let ids = heartbeat_ids.clone();
                            let holder = heartbeat_holder.clone();
                            let operation = heartbeat_operation.clone();
                            let generation = heartbeat_generation.clone();
                            move |catalog| {
                                catalog.renew_v2_lease_set(
                                    &document,
                                    &ids,
                                    &holder,
                                    &operation,
                                    &generation,
                                    expiry,
                                    current,
                                )
                            }
                        })
                        .await;
                    if let Err(error) = result {
                        if let Ok(mut slot) = heartbeat_error_slot.lock() {
                            *slot = Some(error.to_string());
                        }
                        break;
                    }
                }
            })),
        };
        for object in physical.iter().filter(|object| object.write) {
            let blob_id = BlobObjectId::parse(object.id.as_str().to_owned())
                .map_err(|error| WriteError::Storage(error.to_string()))?;
            let write_result = if let Some(permit) = detached_snapshot_permit.clone() {
                writer
                    .write_allocated_with_guard(
                        document_id.as_str(),
                        blob_id,
                        object.bytes.clone(),
                        object.content_type,
                        permit,
                    )
                    .await
            } else {
                writer
                    .write_allocated(
                        document_id.as_str(),
                        blob_id,
                        object.bytes.clone(),
                        object.content_type,
                    )
                    .await
            }
            .map_err(WriteError::Storage);
            if let Err(error) = write_result {
                heartbeat.stop().await;
                if agent_checkpoint.is_none() {
                    let operation = admitted_operation.id.clone();
                    let _ = catalog
                        .execute_catalog(256, move |catalog| {
                            catalog.finish_v2_operation(
                                &operation,
                                &serde_json::json!({
                                    "version": 2,
                                    "status": "aborted",
                                    "reason": "checkpoint physical write failed",
                                })
                                .to_string(),
                                false,
                                now,
                            )
                        })
                        .await;
                }
                return Err(error);
            }
        }
        let heartbeat_failure = heartbeat_error.lock().ok().and_then(|slot| slot.clone());
        if let Some(error) = heartbeat_failure {
            heartbeat.stop().await;
            if agent_checkpoint.is_none() {
                let operation = admitted_operation.id.clone();
                let _ = catalog
                    .execute_catalog(256, move |catalog| {
                        catalog.finish_v2_operation(
                            &operation,
                            &serde_json::json!({
                                "version": 2,
                                "status": "aborted",
                                "reason": "checkpoint lease heartbeat failed",
                            })
                            .to_string(),
                            false,
                            now,
                        )
                    })
                    .await;
            }
            return Err(WriteError::Storage(format!(
                "checkpoint closure heartbeat failed: {error}"
            )));
        }
        let parent_tree_for_metadata = self.parent_tree().await;
        let changed_paths = tree.changed_from(parent_tree_for_metadata.as_ref());
        let checkpoint = CheckpointCommit {
            document_id: document_id.clone(),
            id: checkpoint_id,
            tree_object_id: tree_id,
            tree_digest: hex::encode(tree_envelope.logical_digest),
            parent_id: (!parent.is_empty()).then(|| parent.to_string()),
            author_account_id: if actor.account_id.is_empty() {
                by.account_id().map(str::to_owned)
            } else {
                Some(actor.account_id.to_owned())
            },
            author_label: by.display().to_string(),
            reason: why.to_string(),
            source_format,
            logical_bytes: tree.files.values().map(|file| file.size).sum(),
            label: None,
            journal_epoch,
            journal_sequence,
            metadata_json: serde_json::json!({
                "version": 2,
                "tree": hex::encode(tree_envelope.logical_digest),
                "main": tree_envelope.main_path,
                "source_format": source_format.as_str(),
                "changed": changed_paths,
            })
            .to_string(),
            eligible_after: None,
            object_ids,
            make_current: true,
            now: UnixMillis::new(crate::util::now_millis())
                .map_err(|error| WriteError::Storage(error.to_string()))?,
        };
        let tree_bytes_for_verify = physical
            .iter()
            .find(|object| object.kind == ObjectKind::SourceTree)
            .map(|object| object.bytes.clone())
            .ok_or_else(|| WriteError::Storage("checkpoint tree object is missing".into()))?;
        let recipe_objects_for_verify = physical
            .iter()
            .filter(|object| object.kind == ObjectKind::SourceRecipe)
            .map(|object| (object.id.as_str().to_owned(), object.bytes.clone()))
            .collect::<Vec<_>>();
        let proof_input_bytes = tree_bytes_for_verify.len().saturating_add(
            recipe_objects_for_verify
                .iter()
                .map(|(_, bytes)| bytes.len())
                .sum::<usize>(),
        );
        let checkpoint_for_verify = checkpoint.clone();
        let proof = catalog
            .execute_catalog(proof_input_bytes.saturating_add(512), {
                let operation_id = admitted_operation.id.clone();
                let tree_bytes = tree_bytes_for_verify;
                let recipe_objects = recipe_objects_for_verify;
                move |catalog| {
                    catalog.verify_v2_source_closure_bundle(
                        &operation_id,
                        &checkpoint_for_verify,
                        &tree_bytes,
                        &recipe_objects,
                    )
                }
            })
            .await
            .map_err(WriteError::from)?;
        let checkpoint_id = checkpoint.id.as_str().to_string();
        let checkpoint_for_commit = checkpoint.clone();
        let agent_checkpoint_for_commit = agent_checkpoint.cloned();
        let commit_result = catalog
            .execute_catalog(checkpoint.object_ids.len() * 128 + 512, move |catalog| {
                catalog.commit_v2_checkpoint_verified_with_agent(
                    &proof,
                    &checkpoint_for_commit,
                    &serde_json::json!({"version": 2, "effect": "checkpoint", "checkpoint_id": checkpoint_id}).to_string(),
                    agent_checkpoint_for_commit.as_ref(),
                )
            })
            .await;
        if let Err(error) = commit_result {
            if agent_checkpoint.is_none() && definitive_checkpoint_catalog_error(&error) {
                let operation = admitted_operation.id.clone();
                let abort_result = catalog
                    .execute_catalog(256, move |catalog| {
                        catalog.finish_v2_operation(
                            &operation,
                            &serde_json::json!({
                                "version": 2,
                                "status": "aborted",
                                "reason": "checkpoint final transaction rejected",
                            })
                            .to_string(),
                            false,
                            now,
                        )
                    })
                    .await;
                if let Err(abort_error) = abort_result {
                    eprintln!(
                        "warning: could not close rejected checkpoint operation: {abort_error}"
                    );
                }
            }
            return Err(WriteError::from(error));
        }
        // The catalogue transaction is the durable boundary.  Commit the
        // process-local rate reservations before any best-effort cache reload;
        // a post-commit read failure must not refund a write that already
        // consumed its owner and deployment slots.
        owner_rate.commit();
        deployment_rate.commit();
        // The v2 catalogue is authoritative, but the resident manifest is
        // still the merge-base index used by restore while this room remains
        // open.  Add the committed event immediately; otherwise
        // `last_checkpoint` names a valid SQL row that the in-memory manifest
        // treats as shed and restore refuses its base.
        let committed_point = match catalog
            .execute_catalog(checkpoint.id.as_str().len() + self.slug.len() + 256, {
                let slug = self.slug.clone();
                let checkpoint_id = checkpoint.id.as_str().to_owned();
                move |catalog| {
                    let row = catalog.checkpoint(&slug, &checkpoint_id)?;
                    row.map(|row| {
                        let manifest = Manifest::from_catalog_rows(vec![row])
                            .map_err(crate::storage::catalog::CatalogError::Invalid)?;
                        manifest.checkpoints.into_iter().next().ok_or_else(|| {
                            crate::storage::catalog::CatalogError::Invalid(
                                "committed checkpoint row did not decode".into(),
                            )
                        })
                    })
                    .transpose()
                }
            })
            .await
        {
            Ok(point) => point,
            Err(error) => {
                eprintln!(
                    "warning: committed checkpoint {} could not refresh the room manifest: {error}",
                    checkpoint.id
                );
                None
            }
        };
        // Keep the lease heartbeat alive through closure verification and the
        // atomic head/receipt commit. Only after that transaction succeeds is
        // it safe to stop renewing the stage leases.
        heartbeat.stop().await;
        let heartbeat_failure = heartbeat_error.lock().ok().and_then(|slot| slot.clone());
        if let Some(error) = heartbeat_failure {
            eprintln!("warning: checkpoint heartbeat failed after commit: {error}");
        }
        if let Err(error) = catalog
            .execute_catalog(checkpoint.object_ids.len() * 64 + 256, {
                let document = document_id.clone();
                let ids = physical
                    .iter()
                    .map(|object| object.id.clone())
                    .collect::<Vec<_>>();
                let holder = holder.clone();
                move |catalog| catalog.release_v2_lease_set(&document, &ids, &holder)
            })
            .await
        {
            eprintln!("warning: checkpoint lease cleanup failed: {error}");
        }

        let mut state = self.state.lock().await;
        if let Some(point) = committed_point {
            state
                .manifest
                .checkpoints
                .retain(|existing| existing.sha != point.sha);
            state.manifest.checkpoints.push(point);
            let excess = state
                .manifest
                .checkpoints
                .len()
                .saturating_sub(RESIDENT_CATALOG_HISTORY as usize);
            if excess > 0 {
                state.manifest.checkpoints.drain(..excess);
            }
        }
        state.session.last_tree = Some(tree.clone());
        state.session.last_checkpoint = checkpoint.id.as_str().to_string();
        state.session.last_checkpoint_at = now_unix();
        state.session.checkpoint_generation = tree_generation;
        if state.session.generation == tree_generation {
            state.session.pending_checkpoint_since = 0;
        }
        // A checkpoint may overlap a newer edit. Acknowledge only the socket
        // high-water marks captured with this snapshot; later updates remain
        // pending for the next durable checkpoint.
        for (id, seq) in captured_socket_sequences {
            let Some(peer) = state.sockets.get_mut(id) else {
                continue;
            };
            if *seq <= peer.acked {
                continue;
            }
            peer.acked = *seq;
            let payload = serde_json::json!({"type": "y-ack", "seq": seq}).to_string();
            if peer.tx.try_send_durable(Outgoing::Text(payload)).is_err() {
                state.sockets.remove(id);
            }
        }
        drop(state);
        Ok(Some(checkpoint.id.as_str().to_string()))
    }

    /// The tree the newest checkpoint recorded, for the sake of the `changed`
    /// list on the next one. Kept in memory between checkpoints; read back
    /// only after a cold start, and read as a one-file tree when the entry is
    /// from before a document was a directory.
    pub(super) async fn parent_tree(&self) -> Option<crate::document::history::Tree> {
        let (held, point) = {
            let state = self.state.lock().await;
            (
                state.session.last_tree.clone(),
                state.manifest.latest().cloned(),
            )
        };
        if held.is_some() {
            return held;
        }
        let point = point?;
        let catalog = self.catalog.get()?;
        crate::document::history::load_tree(self.blobs.as_ref(), catalog, &self.slug, &point)
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
        let catalog = self.catalog.get().ok_or("durable catalog required")?;
        self.checkpoint_cache
            .load_checkpoint_v2(self.blobs.as_ref(), catalog, &self.slug, point)
            .await
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

    /// Lease every physical object a selected tree may install into the live
    /// document. This second lease is deliberately held by restore until the
    /// merged CRDT update and its forced checkpoint have committed; the
    /// materialization lease in `checkpoint_texts` ends sooner for ordinary
    /// read-only history responses.
    async fn lease_checkpoint_objects(
        &self,
        point: &Checkpoint,
        tree: &crate::document::history::Tree,
    ) -> Result<Option<crate::storage::catalog::CheckpointReadLease>, WriteError> {
        let catalog = self
            .catalog
            .get()
            .ok_or_else(|| WriteError::Storage("durable catalog required".into()))?;
        let owner = catalog.clone();
        let slug = self.slug.clone();
        let event = point.sha.clone();
        let lease = catalog
            .execute_catalog(4096, move |_| {
                owner.acquire_checkpoint_read(&slug, Some(&event), crate::util::now_millis())
            })
            .await?;
        if hex::encode(sha2::Sha256::digest(tree.to_bytes())) != lease.set.tree_digest {
            return Err(WriteError::Storage(
                "restore tree does not match checkpoint closure".into(),
            ));
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
    #[cfg(test)]
    pub async fn label(&self, sha: &str, label: &str) -> Result<bool, WriteError> {
        self.label_as(sha, label, None).await
    }

    /// Label with the caller identity carried through to the final catalogue
    /// transaction.  The test/legacy entry point above remains available for
    /// isolated rooms that have no account authority attached.
    #[cfg(test)]
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
        Err(WriteError::Storage("room has no catalog".into()))
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
                .checkpoint_impl("quiet", by, false, false, Some(&point.sha), None)
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
        };
        let base_point = match base_point {
            Some(point) => point,
            None => self
                .checkpoint_by_sha(&base_sha)
                .await
                .map_err(WriteError::Storage)?
                .ok_or_else(|| "restore base checkpoint was shed".to_string())?,
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
            if let Err(error) = self.checked_edit(&state.session.doc, |candidate| {
                session::restore_by_path(candidate, &effective_tree, &merged);
                Ok::<_, WriteError>(())
            }) {
                drop(state);
                if let Some(lease) = target_lease {
                    let _ = lease.finish().await;
                }
                return Err(error);
            }
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
                self.broadcast_editors_except(
                    None,
                    &json!({
                        "type": "y-update",
                        "update": encode_update(&update),
                    }),
                )
                .await;
                return Err(err);
            }
        };
        if let Some(lease) = target_lease {
            lease.finish().await?;
        }
        Ok((update, restored))
    }

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
        self.rollback_publication_memory(tree, bodies, format)
            .await?;
        self.write_session_inner(false, false).await.map(|_| ())
    }

    /// Restore only the resident CRDT state.  A prepared v2 source operation
    /// owns the document's source-writer slot, so persisting this state before
    /// that operation is aborted would try to create a competing
    /// `journal_append` row and violate the one-writer invariant.  Callers
    /// that are compensating an operation must abort its row first, then use
    /// `write_session_inner` once the slot is free.
    pub(crate) async fn rollback_publication_memory(
        &self,
        tree: &crate::document::history::Tree,
        bodies: &HashMap<String, String>,
        format: &str,
    ) -> Result<(), WriteError> {
        {
            let _assets_writer = self.assets_write.lock().await;
            let mut state = self.state.lock().await;
            self.checked_edit(&state.session.doc, |candidate| {
                session::restore(candidate, tree, bodies);
                Ok::<_, WriteError>(())
            })?;
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
        Ok(())
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
        self.checked_edit(&state.session.doc, |candidate| {
            session::put_text(candidate, path, body);
            Ok::<_, WriteError>(())
        })?;
        state.session.mark_dirty(now_unix());
        state.session.generation += 1;
        state.session.updated_at = now_unix();
        Ok(())
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
