//! The bridge to the catalogue: reading a room's comments, manifest and
//! renderings out of it when the room loads, and writing them back.
//!
//! Every function here submits its SQL through the catalogue's execution
//! boundary rather than running it on the caller's Tokio worker.  The shapes
//! are the ones that boundary asks for: capacity is reserved before large
//! owned inputs are built, one job carries a whole existing sequence of
//! catalogue calls, and a job never performs object-store I/O or calls back
//! into the room.

use super::*;
use crate::storage::catalog::{Catalog, CatalogExecError, MutationAuthority, RoomEditReservation};

/// What a job's owned arguments cost beyond the strings it carries: the
/// identifiers, limits and flags every catalogue descriptor has.  Small
/// enough that such a request may wait for admission rather than being shed,
/// which is what keeps an ordinary edit from failing under a burst.
pub(super) const DESCRIPTOR_BYTES: usize = 128;

const SOURCE_HISTORY_LEASE_SECONDS: i64 = 3_600;
const SOURCE_HISTORY_LEASE_HEARTBEAT_SECONDS: u64 = 300;

/// An in-flight source publication/read lease with a bounded heartbeat.  The
/// task only extends rows that still exist and have not expired; if the
/// catalogue becomes unavailable it exits and the final publication
/// transaction will fail closed on its lease check.
pub(super) struct SourceHistoryLeaseGuard {
    catalog: Arc<Catalog>,
    storage_id: String,
    operation_id: String,
    heartbeat: Option<tokio::task::JoinHandle<()>>,
}

impl SourceHistoryLeaseGuard {
    fn new(catalog: Arc<Catalog>, storage_id: String, operation_id: String) -> Self {
        let heartbeat_catalog = catalog.clone();
        let heartbeat_storage = storage_id.clone();
        let heartbeat_operation = operation_id.clone();
        let heartbeat = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(
                SOURCE_HISTORY_LEASE_HEARTBEAT_SECONDS,
            ));
            // `interval` ticks immediately; the initial lease already covers
            // the operation, so the first renewal is deliberately delayed.
            interval.tick().await;
            loop {
                interval.tick().await;
                let now = crate::util::now_unix();
                let expires_at = now.saturating_add(SOURCE_HISTORY_LEASE_SECONDS);
                let storage_id = heartbeat_storage.clone();
                let operation_id = heartbeat_operation.clone();
                let result = heartbeat_catalog
                    .execute_catalog(
                        storage_id.len() + operation_id.len() + DESCRIPTOR_BYTES,
                        move |catalog| {
                            catalog.renew_source_history_lease(
                                &storage_id,
                                &operation_id,
                                now,
                                expires_at,
                            )
                        },
                    )
                    .await;
                if result.is_err() {
                    // A missing/expired lease is terminal for this guard. Do
                    // not retry in a way that could recreate protection after
                    // maintenance has made the operation an orphan.
                    break;
                }
            }
        });
        Self {
            catalog,
            storage_id,
            operation_id,
            heartbeat: Some(heartbeat),
        }
    }

    /// Add objects discovered from an immutable checkpoint tree to the same
    /// operation lease. The tree lease is installed before the first tree
    /// read; extending it after the tree is known keeps the source plan
    /// bounded without opening a second heartbeat/finish lifetime.
    pub(super) async fn extend(
        &self,
        objects: Vec<crate::storage::catalog::SourceHistoryObject>,
        created_at: i64,
        expires_at: i64,
    ) -> Result<(), WriteError> {
        let input_bytes = self.storage_id.len()
            + self.operation_id.len()
            + DESCRIPTOR_BYTES
            + objects
                .iter()
                .map(|object| object.object_key.len() + object.kind.len() + 16)
                .sum::<usize>();
        let storage_id = self.storage_id.clone();
        let operation_id = self.operation_id.clone();
        self.catalog
            .execute_catalog(
                input_bytes.min(crate::storage::catalog::MAX_REQUEST_BYTES),
                move |catalog| {
                    catalog.begin_source_history_lease(
                        &storage_id,
                        &operation_id,
                        &objects,
                        created_at,
                        expires_at,
                    )
                },
            )
            .await
            .map_err(WriteError::from)
    }

    pub(super) async fn finish(mut self) -> Result<(), WriteError> {
        if let Some(heartbeat) = self.heartbeat.take() {
            heartbeat.abort();
        }
        let catalog = self.catalog.clone();
        // This type implements `Drop`, so move-out is not allowed. Cloning
        // these tiny identifiers also keeps the guard's fields intact until
        // the destructor runs if the finish request fails.
        let storage_id = self.storage_id.clone();
        let operation_id = self.operation_id.clone();
        catalog
            .execute_catalog(
                storage_id.len() + operation_id.len() + DESCRIPTOR_BYTES,
                move |catalog| catalog.finish_source_history_lease(&storage_id, &operation_id),
            )
            .await
            .map_err(WriteError::from)
    }
}

impl Drop for SourceHistoryLeaseGuard {
    fn drop(&mut self) {
        if let Some(heartbeat) = self.heartbeat.take() {
            heartbeat.abort();
        }
    }
}

/// A `MutationAuthority` a job can own.
///
/// The borrowed form cannot cross the boundary — a job takes owned inputs —
/// and the authority is re-checked inside the catalogue transaction, so it
/// has to arrive there intact rather than being validated early and dropped.
#[derive(Clone, Debug)]
pub(super) struct OwnedAuthority {
    account_id: String,
    owner_key: String,
    generation: String,
    link_hash: String,
    policy_editor: bool,
    automation: bool,
    unowned_publisher: bool,
    execution_epoch: String,
    agent_checkpoint: Option<crate::storage::catalog::AgentCheckpointCommit>,
}

impl OwnedAuthority {
    pub(super) fn new(actor: &MutationAuthority<'_>) -> Self {
        Self {
            account_id: actor.account_id.to_string(),
            owner_key: actor.owner_key.to_string(),
            generation: actor.generation.to_string(),
            link_hash: actor.link_hash.to_string(),
            policy_editor: actor.policy_editor,
            automation: actor.automation,
            unowned_publisher: actor.unowned_publisher,
            execution_epoch: actor.execution_epoch.to_string(),
            agent_checkpoint: actor.agent_checkpoint.cloned(),
        }
    }

    pub(super) fn borrow(&self) -> MutationAuthority<'_> {
        MutationAuthority {
            account_id: &self.account_id,
            owner_key: &self.owner_key,
            generation: &self.generation,
            link_hash: &self.link_hash,
            policy_editor: self.policy_editor,
            automation: self.automation,
            unowned_publisher: self.unowned_publisher,
            execution_epoch: &self.execution_epoch,
            agent_checkpoint: self.agent_checkpoint.as_ref(),
        }
    }

    fn bytes(&self) -> usize {
        self.account_id.len() + self.owner_key.len() + self.generation.len() + self.link_hash.len()
    }
}

/// The pending-snapshot reservation one accepted edit owns between the
/// catalogue transaction that took it and the room state that accounts for
/// it.
///
/// Cancellation is why this is a type rather than two catalogue calls.  A
/// caller can disappear at two different moments — while the reserving
/// transaction is still running, and after it has committed but before the
/// awaiting future resumes — and only the first is visible to the execution
/// boundary's completion hook, which runs on the executing thread before the
/// result is handed back.  The hook settles the first case, this guard's
/// `Drop` the second, and one shared slot makes sure only one of them acts.
/// Both undo the reservation by its generation, so a reservation that a newer
/// edit or session write has already replaced is left alone; that also makes
/// a duplicated rollback a no-op rather than a way to release quota twice.
pub(super) struct PendingEditReservation {
    catalog: Arc<Catalog>,
    slug: String,
    slot: Arc<ReservationSlot<RoomEditReservation>>,
}

/// The handshake between the caller that will own a reservation and the
/// completion hook that must settle it if that caller disappears.
///
/// `T` is whatever the undo needs: the generation of a room edit, the owner
/// and hour of a checkpoint token, or nothing at all when the operation id
/// already identifies what to release.  Exactly one of the two sides ever
/// takes the value out, because both go through this mutex.
pub(super) struct ReservationSlot<T> {
    state: std::sync::Mutex<ReservationSlotState<T>>,
}

struct ReservationSlotState<T> {
    /// What the job reserved, once its transaction has committed.
    reserved: Option<T>,
    /// The awaiting caller has gone, or has handed the reservation on.
    caller_gone: bool,
    /// Somebody has taken responsibility for this reservation.
    settled: bool,
}

impl<T> Default for ReservationSlot<T> {
    fn default() -> Self {
        Self {
            state: std::sync::Mutex::new(ReservationSlotState {
                reserved: None,
                caller_gone: false,
                settled: false,
            }),
        }
    }
}

impl<T> ReservationSlot<T> {
    /// Recorded by the job itself, on the executing thread, before the
    /// completion hook can look at it.
    pub(super) fn record(&self, reserved: T) {
        self.lock().reserved = Some(reserved);
    }

    /// The completion hook's claim: it owns the undo only if the caller was
    /// already gone when the request settled.
    pub(super) fn on_completion(&self) -> Option<T> {
        let mut state = self.lock();
        if state.settled || !state.caller_gone {
            return None;
        }
        state.settled = true;
        state.reserved.take()
    }

    /// The caller's claim, on cancellation or an explicit undo.  When the job
    /// has not committed yet there is nothing to undo and the hook will find
    /// `caller_gone` set.
    pub(super) fn abandon(&self) -> Option<T> {
        let mut state = self.lock();
        state.caller_gone = true;
        if state.settled {
            return None;
        }
        let reserved = state.reserved.take()?;
        state.settled = true;
        Some(reserved)
    }

    /// What the caller would have to undo, without settling it: the guard
    /// stays responsible until the undo has actually happened.
    pub(super) fn peek(&self) -> Option<T>
    where
        T: Clone,
    {
        let state = self.lock();
        if state.settled {
            return None;
        }
        state.reserved.clone()
    }

    /// Nobody owes anything: the room has taken the reservation on, or the
    /// undo has run.
    pub(super) fn keep(&self) {
        let mut state = self.lock();
        state.caller_gone = true;
        state.settled = true;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ReservationSlotState<T>> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl PendingEditReservation {
    /// The room state now accounts for these bytes.  The reservation stays
    /// charged until the room's next reservation replaces it, which is what
    /// makes room admission include unsaved work.
    pub(super) fn keep(self) {
        self.slot.keep();
    }

    /// The update was refused after the bytes were reserved, so give them
    /// back now rather than leaving the document charged for a snapshot that
    /// will never exist.  Cancellation here is safe: the guard stays armed
    /// across the await, and a rollback that ran twice is refused by the
    /// generation the second time.
    pub(super) async fn rollback(self) {
        let Some(reservation) = self.slot.peek() else {
            return;
        };
        let slug = self.slug.clone();
        let restored = self
            .catalog
            .execute_catalog(slug.len() + DESCRIPTOR_BYTES, move |catalog| {
                catalog.restore_room_edit(&slug, reservation)
            })
            .await;
        if restored.is_ok() {
            self.slot.keep();
        }
    }
}

impl Drop for PendingEditReservation {
    fn drop(&mut self) {
        let Some(reservation) = self.slot.abandon() else {
            return;
        };
        // The caller was cancelled after the reservation committed and before
        // it took ownership.  A `Drop` cannot await, so this is the one
        // catalogue call the room still makes synchronously; it runs only on
        // that cancellation path, it is a single conditional statement, and
        // the alternative — leaving the bytes charged until the document is
        // edited again — is a quota leak on a document nobody may touch
        // again.
        if let Err(error) = self.catalog.restore_room_edit(&self.slug, reservation) {
            eprintln!(
                "warning: could not release the pending edit reservation for {}: {error}",
                self.slug
            );
        }
    }
}

/// A test-only gate for the window between an edit reservation committing and
/// the room taking ownership of it.
///
/// That window is the one the completion hook cannot observe — the hook runs
/// on the executing thread before the result is handed back — so it is the
/// window `Drop` covers, and a test needs a way to park a caller inside it.
/// The gate is keyed by slug so that one test's gate cannot stop another
/// test's room in the same process.
#[cfg(test)]
pub(crate) static AFTER_EDIT_RESERVATION: super::TestGate = std::sync::Mutex::new(None);

#[cfg(test)]
pub(super) async fn pause_after_edit_reservation(slug: &str) {
    super::ReservationGate::park(&AFTER_EDIT_RESERVATION, slug).await;
}

/// Reserve a complete pending snapshot for one update.
///
/// The reservation is taken on a blocking thread under the boundary's
/// admission, so the quota decision no longer parks a Tokio worker on the
/// connection; what it does still hold is the caller's room gates, which is
/// recorded in the track 1 inventory for track 2 to shorten.
pub(super) async fn reserve_pending_edit(
    catalog: &Arc<Catalog>,
    slug: &str,
    bytes: i64,
    owner_limit: i64,
    total_limit: i64,
) -> Result<PendingEditReservation, CatalogExecError> {
    let slot = Arc::new(ReservationSlot::default());
    let guard = PendingEditReservation {
        catalog: catalog.clone(),
        slug: slug.to_string(),
        slot: slot.clone(),
    };
    let job_slot = slot.clone();
    let job_slug = slug.to_string();
    let cleanup = EditReservationCleanup {
        slug: slug.to_string(),
        slot,
    };
    catalog
        .reserve_execution(slug.len() + DESCRIPTOR_BYTES)
        .await?
        .execute_catalog_with_completion(
            move |catalog| {
                let reservation =
                    catalog.reserve_room_edit(&job_slug, bytes, owner_limit, total_limit)?;
                job_slot.record(reservation);
                Ok(())
            },
            cleanup,
        )
        .await?;
    Ok(guard)
}

/// The single session writer's reservation, held from the transaction that
/// moves the pending snapshot into it until the snapshot is durable.
///
/// This is the `RoomWriteQuota` the room used to build by hand, with the
/// cancellation window closed: the reservation is created by the same call
/// that takes it, so there is no longer a gap between the catalogue
/// transaction committing and the guard existing in which a cancelled writer
/// would leave `writing_bytes` charged forever.  Settlement is unconditional
/// because the session writer gate serialises these: no newer session write
/// can have taken the row while this one still holds the gate.
pub(super) struct RoomWriteReservation {
    catalog: Arc<Catalog>,
    storage_id: String,
    slot: Arc<ReservationSlot<()>>,
}

impl RoomWriteReservation {
    /// The snapshot is durable, so its cost has moved into ordinary object
    /// accounting.
    pub(super) async fn commit(self) -> Result<(), CatalogExecError> {
        if self.slot.abandon().is_none() {
            return Ok(());
        }
        let storage_id = self.storage_id.clone();
        self.catalog
            .execute_catalog(storage_id.len() + DESCRIPTOR_BYTES, move |catalog| {
                catalog.finish_room_write(&storage_id, true)
            })
            .await
    }
}

impl Drop for RoomWriteReservation {
    fn drop(&mut self) {
        if self.slot.abandon().is_none() {
            return;
        }
        // The write failed or its caller went away: the bytes go back to the
        // room's pending reservation, which is what admission counts. A
        // `Drop` cannot await, and this is the same synchronous call the
        // hand-written quota guard made before.
        if let Err(error) = self.catalog.finish_room_write(&self.storage_id, false) {
            eprintln!(
                "warning: could not restore pending edit quota for {}: {error}",
                self.storage_id
            );
        }
    }
}

/// The service's half of the session writer's reservation.
struct RoomWriteCleanup {
    storage_id: String,
    slot: Arc<ReservationSlot<()>>,
}

impl crate::storage::catalog::CatalogServiceCompletion for RoomWriteCleanup {
    fn complete(
        self: Box<Self>,
        outcome: crate::storage::catalog::CatalogOutcome<'_>,
        catalog: &Catalog,
    ) {
        if !matches!(outcome, crate::storage::catalog::CatalogOutcome::Committed) {
            return;
        }
        if self.slot.on_completion().is_none() {
            return;
        }
        if let Err(error) = catalog.finish_room_write(&self.storage_id, false) {
            eprintln!(
                "warning: could not restore pending edit quota for {}: {error}",
                self.storage_id
            );
        }
    }
}

/// Move a room's pending snapshot into the session writer's reservation.
pub(super) async fn begin_room_write(
    catalog: &Arc<Catalog>,
    slug: &str,
    storage_id: &str,
    bytes: i64,
    owner_limit: i64,
    total_limit: i64,
) -> Result<RoomWriteReservation, CatalogExecError> {
    let slot = Arc::new(ReservationSlot::default());
    let guard = RoomWriteReservation {
        catalog: catalog.clone(),
        storage_id: storage_id.to_string(),
        slot: slot.clone(),
    };
    let cleanup = RoomWriteCleanup {
        storage_id: storage_id.to_string(),
        slot: slot.clone(),
    };
    let job_slug = slug.to_string();
    catalog
        .reserve_execution(slug.len() + storage_id.len() + DESCRIPTOR_BYTES)
        .await?
        .execute_catalog_with_completion(
            move |catalog| {
                catalog.begin_room_write(&job_slug, bytes, owner_limit, total_limit)?;
                slot.record(());
                Ok(())
            },
            cleanup,
        )
        .await?;
    Ok(guard)
}

/// The service's half of the reservation handshake: it runs on the executing
/// thread whether or not the caller is still waiting, and undoes the
/// reservation only when the caller had already gone by the time the
/// transaction settled.
struct EditReservationCleanup {
    slug: String,
    slot: Arc<ReservationSlot<RoomEditReservation>>,
}

impl crate::storage::catalog::CatalogServiceCompletion for EditReservationCleanup {
    fn complete(
        self: Box<Self>,
        outcome: crate::storage::catalog::CatalogOutcome<'_>,
        catalog: &Catalog,
    ) {
        if !matches!(outcome, crate::storage::catalog::CatalogOutcome::Committed) {
            // Nothing committed, so there is no reservation to undo.
            return;
        }
        let Some(reservation) = self.slot.on_completion() else {
            return;
        };
        if let Err(error) = catalog.restore_room_edit(&self.slug, reservation) {
            eprintln!(
                "warning: could not release the pending edit reservation for {}: {error}",
                self.slug
            );
        }
    }
}

/// The owned identity of one accounted object mutation.  Reserve, commit and
/// abort all name the same `operation_id`, which is the receipt this
/// reservation is reconciled by: an abort can only ever release the operation
/// it names, so a late cleanup cannot touch a newer upload of the same key.
#[derive(Clone, Debug)]
pub(super) struct ObjectChange {
    pub(super) slug: String,
    pub(super) storage_id: String,
    pub(super) operation_id: String,
    pub(super) object_key: String,
    pub(super) kind: String,
}

impl ObjectChange {
    fn bytes(&self) -> usize {
        DESCRIPTOR_BYTES
            + self.slug.len()
            + self.storage_id.len()
            + self.operation_id.len()
            + self.object_key.len()
            + self.kind.len()
    }
}

/// An object reservation the room owns between the catalogue transaction that
/// took it and the object-store write that settles it.
///
/// Before this guard existed the abort was only issued on the error paths a
/// caller reached by returning, so a caller cancelled during the upload — the
/// long await in the middle — left the bytes reserved for the life of the
/// process.  The guard settles that case and the cancelled-during-SQL case
/// the same way the pending edit reservation does: a completion hook for the
/// window the caller cannot observe, `Drop` for the window after it.
pub(super) struct ObjectChangeGuard {
    catalog: Arc<Catalog>,
    change: ObjectChange,
    slot: Arc<ReservationSlot<()>>,
    actor: Option<OwnedAuthority>,
    quarto_selection: Option<crate::quarto::Selection>,
}

impl ObjectChangeGuard {
    pub(super) fn with_quarto_selection(mut self, selection: crate::quarto::Selection) -> Self {
        self.quarto_selection = Some(selection);
        self
    }

    /// The object is written: turn the reservation into durable accounting.
    /// A failed commit aborts inside the same job, so the reservation is
    /// released even if this caller never sees the answer.
    pub(super) async fn commit(self, at: String) -> Result<(), CatalogExecError> {
        let change = self.change.clone();
        let input_bytes = change.bytes() + at.len();
        let actor = self.actor.clone();
        let change_selection = self.quarto_selection.clone();
        let committed = self
            .catalog
            .execute_catalog(input_bytes, move |catalog| {
                let result = match (actor.as_ref(), change_selection.as_ref()) {
                    (Some(actor), Some(selection)) => catalog
                        .commit_quarto_selection_with_authority(
                            &change.storage_id,
                            &change.operation_id,
                            &change.object_key,
                            &change.kind,
                            &at,
                            selection,
                            actor.borrow(),
                        ),
                    (Some(actor), None) => catalog.commit_object_change_with_authority(
                        &change.storage_id,
                        &change.operation_id,
                        &change.object_key,
                        &change.kind,
                        &at,
                        actor.borrow(),
                    ),
                    (None, None) => catalog.commit_object_change(
                        &change.storage_id,
                        &change.operation_id,
                        &change.object_key,
                        &change.kind,
                        &at,
                    ),
                    (None, Some(_)) => Err(crate::storage::catalog::CatalogError::Invalid(
                        "Quarto selection requires mutation authority".into(),
                    )),
                };
                match result {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        let _ = catalog.abort_object_change(
                            &change.storage_id,
                            &change.operation_id,
                            &change.object_key,
                        );
                        Err(error)
                    }
                }
            })
            .await;
        match committed {
            Ok(()) => {
                self.slot.keep();
                Ok(())
            }
            // The job's own abort ran, or nothing was submitted at all; either
            // way `Drop` re-checks and a repeated abort of the same operation
            // id is a no-op.
            Err(error) => Err(error),
        }
    }

    /// The object was not written.  Release the reservation now rather than
    /// leaving it to the guard, so the failure path stays off the connection
    /// on the caller's thread.
    pub(super) async fn abort(self) {
        if self.slot.abandon().is_none() {
            return;
        }
        let change = self.change.clone();
        let input_bytes = change.bytes();
        let aborted = self
            .catalog
            .execute_catalog(input_bytes, move |catalog| {
                catalog.abort_object_change(
                    &change.storage_id,
                    &change.operation_id,
                    &change.object_key,
                )
            })
            .await;
        if let Err(error) = aborted {
            eprintln!(
                "warning: could not release the object reservation for {}: {error}",
                self.change.object_key
            );
        }
    }
}

impl Drop for ObjectChangeGuard {
    fn drop(&mut self) {
        if self.slot.abandon().is_none() {
            return;
        }
        // Cancelled while holding a live reservation.  A `Drop` cannot await,
        // and leaving the bytes charged would count an object that was never
        // written against the document's quota until the process restarts.
        if let Err(error) = self.catalog.abort_object_change(
            &self.change.storage_id,
            &self.change.operation_id,
            &self.change.object_key,
        ) {
            eprintln!(
                "warning: could not release the object reservation for {}: {error}",
                self.change.object_key
            );
        }
    }
}

/// The service's half of the object reservation handshake.
struct ObjectChangeCleanup {
    change: ObjectChange,
    slot: Arc<ReservationSlot<()>>,
}

impl crate::storage::catalog::CatalogServiceCompletion for ObjectChangeCleanup {
    fn complete(
        self: Box<Self>,
        outcome: crate::storage::catalog::CatalogOutcome<'_>,
        catalog: &Catalog,
    ) {
        if !matches!(outcome, crate::storage::catalog::CatalogOutcome::Committed) {
            return;
        }
        if self.slot.on_completion().is_none() {
            return;
        }
        if let Err(error) = catalog.abort_object_change(
            &self.change.storage_id,
            &self.change.operation_id,
            &self.change.object_key,
        ) {
            eprintln!(
                "warning: could not release the object reservation for {}: {error}",
                self.change.object_key
            );
        }
    }
}

/// Reserve the bytes one object write is about to add.
pub(super) async fn reserve_object_change(
    catalog: &Arc<Catalog>,
    change: ObjectChange,
    new_bytes: i64,
    owner_limit: i64,
    total_limit: i64,
    actor: Option<OwnedAuthority>,
) -> Result<ObjectChangeGuard, CatalogExecError> {
    let slot = Arc::new(ReservationSlot::default());
    let guard = ObjectChangeGuard {
        catalog: catalog.clone(),
        change: change.clone(),
        slot: slot.clone(),
        actor: actor.clone(),
        quarto_selection: None,
    };
    let cleanup = ObjectChangeCleanup {
        change: change.clone(),
        slot: slot.clone(),
    };
    let input_bytes = change.bytes()
        + actor
            .as_ref()
            .map(OwnedAuthority::bytes)
            .unwrap_or_default();
    catalog
        .reserve_execution(input_bytes)
        .await?
        .execute_catalog_with_completion(
            move |catalog| {
                let request = crate::storage::catalog::ObjectReservationRequest {
                    slug: &change.slug,
                    operation_id: &change.operation_id,
                    object_key: &change.object_key,
                    kind: &change.kind,
                    new_bytes,
                    owner_limit,
                    total_limit,
                };
                match &actor {
                    Some(actor) => {
                        catalog.reserve_object_change_with_authority(request, actor.borrow())?
                    }
                    None => catalog.reserve_object_change(request)?,
                };
                slot.record(());
                Ok(())
            },
            cleanup,
        )
        .await?;
    Ok(guard)
}

/// One document row, read through the boundary.  Every room path that only
/// needs the catalogue's view of a document goes through here rather than
/// taking the connection on a Tokio worker.
pub(super) async fn read_catalog_document(
    catalog: &Arc<Catalog>,
    slug: &str,
) -> Result<Option<crate::storage::catalog::Document>, CatalogExecError> {
    let slug = slug.to_string();
    catalog
        .execute_catalog(slug.len() + DESCRIPTOR_BYTES, move |catalog| {
            catalog.document(&slug)
        })
        .await
}

/// Register the complete encoded-object plan before the checkpoint writer
/// starts object I/O.  The lease is intentionally separate from accounting:
/// `put_accounted` still owns the normal reservation/commit path, while this
/// row protects slow or ambiguous source writes from graph GC.
pub(super) async fn begin_source_history_lease(
    catalog: &Arc<Catalog>,
    storage_id: &str,
    operation_id: &str,
    objects: Vec<crate::storage::catalog::SourceHistoryObject>,
    created_at: i64,
    expires_at: i64,
) -> Result<SourceHistoryLeaseGuard, WriteError> {
    let input_bytes = storage_id.len()
        + operation_id.len()
        + DESCRIPTOR_BYTES
        + objects
            .iter()
            .map(|object| object.object_key.len() + object.kind.len() + 16)
            .sum::<usize>();
    let storage_id = storage_id.to_string();
    let operation_id = operation_id.to_string();
    let lease_storage_id = storage_id.clone();
    let lease_operation_id = operation_id.clone();
    catalog
        .execute_catalog(
            input_bytes.min(crate::storage::catalog::MAX_REQUEST_BYTES),
            move |catalog| {
                catalog.begin_source_history_lease(
                    &lease_storage_id,
                    &lease_operation_id,
                    &objects,
                    created_at,
                    expires_at,
                )
            },
        )
        .await
        .map_err(WriteError::from)?;
    Ok(SourceHistoryLeaseGuard::new(
        catalog.clone(),
        storage_id,
        operation_id,
    ))
}

/// Queue a catalogue-owned object for the normal guarded deletion worker.
/// Active source-history leases are checked again by that worker, so callers
/// never have to make a racy lease lookup beside physical deletion.
pub(super) async fn queue_object_delete(
    catalog: &Arc<Catalog>,
    slug: &str,
    object_key: &str,
    bytes: i64,
    now: i64,
) -> Result<(), WriteError> {
    let slug = slug.to_string();
    let object_key = object_key.to_string();
    catalog
        .execute_catalog(
            slug.len() + object_key.len() + DESCRIPTOR_BYTES,
            move |catalog| {
                catalog.queue_delete(&crate::storage::catalog::PendingDelete {
                    slug,
                    object_key,
                    bytes,
                    queued_at: now,
                    delete_after: now,
                })
            },
        )
        .await
        .map(|_| ())
        .map_err(WriteError::from)
}

pub(super) async fn read_source_history_record(
    catalog: &Arc<Catalog>,
    storage_id: &str,
    file_digest: &str,
) -> Result<Option<crate::storage::catalog::SourceHistoryRecord>, WriteError> {
    let storage_id = storage_id.to_string();
    let file_digest = file_digest.to_string();
    catalog
        .execute_catalog(
            storage_id.len() + file_digest.len() + DESCRIPTOR_BYTES,
            move |catalog| catalog.source_history_record(&storage_id, &file_digest),
        )
        .await
        .map_err(WriteError::from)
}

pub(super) async fn read_source_object_sizes(
    catalog: &Arc<Catalog>,
    storage_id: &str,
    object_keys: &[String],
) -> Result<std::collections::HashMap<String, i64>, WriteError> {
    // SQLite's variable limit is finite, and a large room can have more
    // chunks than fit in one IN clause. Keep each catalogue request bounded
    // while querying only keys named by this checkpoint's source plans.
    const LOOKUP_BATCH: usize = 256;
    let mut sizes = std::collections::HashMap::new();
    for batch in object_keys.chunks(LOOKUP_BATCH) {
        let storage_id = storage_id.to_string();
        let keys = batch.to_vec();
        let input_bytes =
            storage_id.len() + DESCRIPTOR_BYTES + keys.iter().map(String::len).sum::<usize>();
        let found = catalog
            .execute_catalog(
                input_bytes.min(crate::storage::catalog::MAX_REQUEST_BYTES),
                move |catalog| catalog.source_history_object_sizes(&storage_id, &keys),
            )
            .await
            .map_err(WriteError::from)?;
        sizes.extend(found);
    }
    Ok(sizes)
}

/// The checkpoint that already records this content, if there is one.
pub(super) async fn read_checkpoint_by_content_sha(
    catalog: &Arc<Catalog>,
    slug: &str,
    content_sha: &str,
) -> Result<Option<crate::storage::catalog::Checkpoint>, String> {
    let slug = slug.to_string();
    let content_sha = content_sha.to_string();
    catalog
        .execute_catalog(
            slug.len() + content_sha.len() + DESCRIPTOR_BYTES,
            move |catalog| catalog.checkpoint_by_content_sha(&slug, &content_sha),
        )
        .await
        .map_err(|error| error.to_string())
}

/// Advance the automatic-checkpoint clock for a document.
pub(super) async fn touch_auto_checkpoint(catalog: &Arc<Catalog>, slug: &str, at: i64) {
    let slug = slug.to_string();
    let _ = catalog
        .execute_catalog(slug.len() + DESCRIPTOR_BYTES, move |catalog| {
            catalog.touch_auto_checkpoint(&slug, at)
        })
        .await;
}

/// The complete persisted checkpoint count and byte total.
pub(super) async fn read_checkpoint_stats(
    catalog: &Arc<Catalog>,
    slug: &str,
) -> Result<(u64, i64), CatalogExecError> {
    let slug = slug.to_string();
    catalog
        .execute_catalog(slug.len() + DESCRIPTOR_BYTES, move |catalog| {
            catalog.checkpoint_stats(&slug)
        })
        .await
}

/// Keep an already durable checkpoint descriptor inside a prepared
/// publication receipt.
///
/// The document read, the checkpoint read and the staging write are one job.
/// They were three transactions and they still are; what they no longer do is
/// take the connection three times from the checkpoint path's Tokio worker,
/// and a caller that goes away after the first two cannot leave the third
/// unissued, because the job owns the whole sequence once it is dispatched.
pub(super) async fn stage_existing_publication_checkpoint(
    catalog: &Arc<Catalog>,
    slug: &str,
    sha: &str,
    actor: Option<OwnedAuthority>,
) -> Result<(), String> {
    let slug = slug.to_string();
    let sha = sha.to_string();
    catalog
        .execute_catalog(slug.len() + sha.len() + DESCRIPTOR_BYTES, move |catalog| {
            if catalog
                .document(&slug)?
                .and_then(|document| document.pending_publication)
                .is_none()
            {
                return Ok(());
            }
            if let Some(checkpoint) = catalog.checkpoint(&slug, &sha)? {
                catalog.stage_publication_checkpoint_with_authority(
                    &slug,
                    &checkpoint,
                    actor.as_ref().map(OwnedAuthority::borrow),
                )?;
            }
            Ok(())
        })
        .await
        .map_err(|error| error.to_string())
}

/// Apply the catalogue's own retention limits to a document's history.
pub(super) async fn shed_checkpoints_to_limits(
    catalog: &Arc<Catalog>,
    slug: &str,
    keep_count: usize,
    ceiling: Option<i64>,
    protected: String,
) -> Result<Vec<String>, String> {
    let input_bytes = slug.len() + DESCRIPTOR_BYTES + protected.len();
    let slug = slug.to_string();
    catalog
        .execute_catalog(
            input_bytes.min(crate::storage::catalog::MAX_REQUEST_BYTES),
            move |catalog| {
                catalog.shed_checkpoints_to_limits(&slug, keep_count, ceiling, &protected)
            },
        )
        .await
        .map_err(|error| error.to_string())
}

/// Remove shed checkpoint rows, and report which ones went, so the caller
/// deletes only the objects whose metadata is actually gone.  One job for the
/// whole set: each row is still its own transaction, and a row that fails
/// keeps its object for a later retry exactly as before.
pub(super) async fn delete_checkpoints(
    catalog: &Arc<Catalog>,
    slug: &str,
    shas: Vec<String>,
) -> Vec<String> {
    let input_bytes = slug.len() + DESCRIPTOR_BYTES + shas.iter().map(String::len).sum::<usize>();
    let slug_owned = slug.to_string();
    let reported = catalog
        .execute_catalog(
            input_bytes.min(crate::storage::catalog::MAX_REQUEST_BYTES),
            move |catalog| {
                // Source-history edges and their pending deletion handoff
                // move with the checkpoint row in one SQLite transaction.
                // The existing object worker keeps accounting until the
                // physical delete is confirmed.
                catalog.delete_checkpoints_with_source_history(
                    &slug_owned,
                    &shas,
                    crate::util::now_unix(),
                    crate::util::now_unix(),
                )
            },
        )
        .await;
    match reported {
        Ok(removed) => removed,
        Err(error) => {
            eprintln!("warning: could not remove shed checkpoints for {slug}: {error}");
            Vec::new()
        }
    }
}

/// Resolve a checkpoint prefix against the authoritative catalogue.
pub(super) async fn read_checkpoints_prefix(
    catalog: &Arc<Catalog>,
    slug: &str,
    prefix: &str,
) -> Result<Vec<crate::storage::catalog::Checkpoint>, String> {
    let slug = slug.to_string();
    let prefix = prefix.to_string();
    catalog
        .execute_catalog(
            slug.len() + prefix.len() + DESCRIPTOR_BYTES,
            move |catalog| catalog.checkpoints_prefix(&slug, &prefix),
        )
        .await
        .map_err(|error| error.to_string())
}

/// One keyset page of the authoritative checkpoint timeline.
pub(super) async fn read_checkpoints_page(
    catalog: &Arc<Catalog>,
    slug: &str,
    after_seq: Option<i64>,
    limit: u32,
) -> Result<Vec<crate::document::history::Checkpoint>, String> {
    let slug = slug.to_string();
    catalog
        .execute_catalog(slug.len() + DESCRIPTOR_BYTES, move |catalog| {
            let rows = catalog.checkpoints(&slug, after_seq, limit)?;
            let mut points = Manifest::from_catalog_rows(rows)
                .map_err(crate::storage::catalog::CatalogError::Invalid)?
                .checkpoints;
            let first = points.iter().map(|point| point.seq).min().unwrap_or(0);
            let last = points.iter().map(|point| point.seq).max().unwrap_or(0);
            let metadata = catalog.retention_metadata_range(&slug, first, last)?;
            for point in &mut points {
                if let Some((parent, gap)) = metadata.get(&point.sha) {
                    point.original_parent = parent.clone();
                    point.ancestry_gap = *gap;
                }
            }
            Ok(points)
        })
        .await
        .map_err(|error| error.to_string())
}

/// Label a checkpoint, with the request's authority re-checked inside the
/// write.
///
/// There is no operation id here, and adding one would change a persisted
/// record for a mutation that is already idempotent: a label write sets the
/// stored label to exactly what the request asked for, so a repeat is the
/// same write and a cancelled caller leaves either the old label or the new
/// one, never a half-applied state.  That is the reconciliation rule for this
/// operation, and it is why it needs no completion hook.
pub(super) async fn label_checkpoint(
    catalog: &Arc<Catalog>,
    slug: &str,
    sha: &str,
    label: &str,
    actor: Option<OwnedAuthority>,
) -> crate::storage::catalog::CatalogResult<()> {
    let input_bytes = slug.len()
        + sha.len()
        + label.len()
        + DESCRIPTOR_BYTES
        + actor
            .as_ref()
            .map(OwnedAuthority::bytes)
            .unwrap_or_default();
    let slug = slug.to_string();
    let sha = sha.to_string();
    let label = label.to_string();
    match catalog
        .execute_catalog(input_bytes, move |catalog| match &actor {
            Some(actor) => catalog
                .label_checkpoint_with_authority(&slug, &sha, &label, actor.borrow())
                .map(|_| ()),
            None => catalog.label_checkpoint(&slug, &sha, &label).map(|_| ()),
        })
        .await
    {
        Ok(()) => Ok(()),
        Err(CatalogExecError::Catalog(error)) => Err(error),
        Err(error) => Err(crate::storage::catalog::CatalogError::Invalid(
            error.to_string(),
        )),
    }
}

/// One rendering row, read through the boundary.
pub(super) async fn read_catalog_rendering(
    catalog: &Arc<Catalog>,
    slug: &str,
    tree_sha: &str,
) -> Option<crate::storage::catalog::Rendering> {
    let slug = slug.to_string();
    let tree_sha = tree_sha.to_string();
    catalog
        .execute_catalog(
            slug.len() + tree_sha.len() + DESCRIPTOR_BYTES,
            move |catalog| catalog.rendering(&slug, &tree_sha),
        )
        .await
        .ok()
        .flatten()
}

pub(super) async fn read_catalog_renderings(
    catalog: &Arc<Catalog>,
    slug: &str,
) -> Result<Vec<crate::storage::catalog::Rendering>, CatalogExecError> {
    let slug = slug.to_string();
    catalog
        .execute_catalog(slug.len() + DESCRIPTOR_BYTES, move |catalog| {
            catalog.renderings(&slug)
        })
        .await
}

/// One checkpoint row, read through the boundary.
pub(super) async fn read_catalog_checkpoint(
    catalog: &Arc<Catalog>,
    slug: &str,
    sha: &str,
) -> Result<Option<crate::storage::catalog::Checkpoint>, CatalogExecError> {
    let slug = slug.to_string();
    let sha = sha.to_string();
    catalog
        .execute_catalog(slug.len() + sha.len() + DESCRIPTOR_BYTES, move |catalog| {
            catalog.checkpoint(&slug, &sha)
        })
        .await
}

/// Read a committed Quarto selection pointer through the catalogue boundary.
pub(super) async fn read_quarto_selection(
    catalog: &Arc<Catalog>,
    storage_id: &str,
    document_id: &str,
    context_id: &str,
) -> Result<Option<crate::storage::catalog::QuartoSelection>, CatalogExecError> {
    let storage_id = storage_id.to_owned();
    let document_id = document_id.to_owned();
    let context_id = context_id.to_owned();
    catalog
        .execute_catalog(
            storage_id.len() + document_id.len() + context_id.len() + DESCRIPTOR_BYTES,
            move |catalog| catalog.quarto_selection(&storage_id, &document_id, &context_id),
        )
        .await
}

/// Read the durable selection epoch, including when the current pointer was
/// cleared by a source restore.
pub(super) async fn read_quarto_selection_generation(
    catalog: &Arc<Catalog>,
    storage_id: &str,
    document_id: &str,
    context_id: &str,
) -> Result<u64, CatalogExecError> {
    let storage_id = storage_id.to_owned();
    let document_id = document_id.to_owned();
    let context_id = context_id.to_owned();
    catalog
        .execute_catalog(
            storage_id.len() + document_id.len() + context_id.len() + DESCRIPTOR_BYTES,
            move |catalog| {
                catalog.quarto_selection_generation(&storage_id, &document_id, &context_id)
            },
        )
        .await
}

pub(super) async fn read_quarto_selection_epochs(
    catalog: &Arc<Catalog>,
    storage_id: &str,
    document_id: &str,
) -> Result<Vec<(String, u64)>, CatalogExecError> {
    let storage_id = storage_id.to_owned();
    let document_id = document_id.to_owned();
    catalog
        .execute_catalog(
            storage_id.len() + document_id.len() + DESCRIPTOR_BYTES,
            move |catalog| catalog.quarto_selection_epochs(&storage_id, &document_id),
        )
        .await
}

pub(super) async fn read_quarto_selections(
    catalog: &Arc<Catalog>,
    storage_id: &str,
    document_id: &str,
) -> Result<Vec<crate::storage::catalog::QuartoSelection>, CatalogExecError> {
    let storage_id = storage_id.to_owned();
    let document_id = document_id.to_owned();
    catalog
        .execute_catalog(
            storage_id.len() + document_id.len() + DESCRIPTOR_BYTES,
            move |catalog| catalog.quarto_selections(&storage_id, &document_id),
        )
        .await
}

pub(super) async fn clear_quarto_selection_with_authority(
    catalog: &Arc<Catalog>,
    storage_id: &str,
    document_id: &str,
    context_id: &str,
    actor: OwnedAuthority,
) -> Result<usize, CatalogExecError> {
    let storage_id = storage_id.to_owned();
    let document_id = document_id.to_owned();
    let context_id = context_id.to_owned();
    catalog
        .execute_catalog(
            storage_id.len()
                + document_id.len()
                + context_id.len()
                + actor.bytes()
                + DESCRIPTOR_BYTES,
            move |catalog| {
                catalog.clear_quarto_selection_with_authority(
                    &storage_id,
                    &document_id,
                    &context_id,
                    actor.borrow(),
                )
            },
        )
        .await
}

pub(super) async fn quarto_object_committed(
    catalog: &Arc<Catalog>,
    storage_id: &str,
    object_key: &str,
    version: &str,
) -> Result<bool, CatalogExecError> {
    let storage_id = storage_id.to_owned();
    let object_key = object_key.to_owned();
    let version = version.to_owned();
    catalog
        .execute_catalog(
            storage_id.len() + object_key.len() + version.len() + DESCRIPTOR_BYTES,
            move |catalog| catalog.quarto_object_committed(&storage_id, &object_key, &version),
        )
        .await
}

/// The newest rendering this document could show, read through the boundary.
pub(super) async fn read_rendering_candidates(
    catalog: &Arc<Catalog>,
    slug: &str,
) -> Option<Vec<crate::storage::catalog::RenderingCandidate>> {
    let slug = slug.to_string();
    catalog
        .execute_catalog(slug.len() + DESCRIPTOR_BYTES, move |catalog| {
            catalog.rendering_candidates(&slug)
        })
        .await
        .ok()
}

/// Release the object accounting for a blob that has been deleted.
pub(super) async fn release_object_accounting(catalog: &Arc<Catalog>, key: &str) {
    let key = key.to_string();
    let _ = catalog
        .execute_catalog(key.len() + DESCRIPTOR_BYTES, move |catalog| {
            catalog.release_object_accounting_key(&key)
        })
        .await;
}

/// Retire a pruning pass's renderings.  One job for the whole pass: the
/// per-rendering retirements were already separate transactions, and issuing
/// them from one blocking thread keeps a long pass from taking the connection
/// once per rendering from a Tokio worker.
pub(super) async fn retire_renderings(
    catalog: &Arc<Catalog>,
    slug: &str,
    shas: Vec<String>,
    now: i64,
) {
    let input_bytes = slug.len()
        + DESCRIPTOR_BYTES
        + shas
            .iter()
            .map(|sha| sha.len())
            .sum::<usize>()
            .min(crate::storage::catalog::MAX_REQUEST_BYTES);
    let slug = slug.to_string();
    let _ = catalog
        .execute_catalog(
            input_bytes.min(crate::storage::catalog::MAX_REQUEST_BYTES),
            move |catalog| {
                for sha in &shas {
                    let _ = catalog.retire_rendering(&slug, sha, now, now);
                }
                Ok(())
            },
        )
        .await;
}

/// Load the mutable annotation state from SQLite.  The JSON room object is
/// retained only for isolated legacy fixtures; a catalogue-backed room never
/// consults it, so a restart has one authoritative source for comments and
/// replies.
///
/// The comment listing and its per-comment reply reads are one job: they were
/// already one read each, and issuing them from a single blocking thread
/// keeps a cold room's load from taking the connection five hundred separate
/// times from a Tokio worker.
pub(super) async fn load_catalog_comments(
    catalog: &Arc<Catalog>,
    slug: &str,
) -> Result<(i64, Vec<Comment>), CatalogExecError> {
    let slug = slug.to_string();
    catalog
        .execute_catalog(slug.len() + DESCRIPTOR_BYTES, move |catalog| {
            load_catalog_comments_blocking(catalog, &slug)
        })
        .await
}

fn load_catalog_comments_blocking(
    catalog: &Catalog,
    slug: &str,
) -> crate::storage::catalog::CatalogResult<(i64, Vec<Comment>)> {
    let annotation_seq = catalog.agent_annotation_sequence(slug)?;
    let rows = catalog.comments(slug, None, 500)?;
    let mut comments = Vec::with_capacity(rows.len());
    for row in rows {
        let region = row
            .region
            .map(|raw| {
                serde_json::from_str::<Region>(&raw).map_err(|err| {
                    crate::storage::catalog::CatalogError::Invalid(format!(
                        "comment region is invalid: {err}"
                    ))
                })
            })
            .transpose()?;
        let source = row.source_path.map(|path| SourceAnchor {
            path,
            exact: row.source_exact.unwrap_or_default(),
            prefix: row.source_prefix.unwrap_or_default(),
            suffix: row.source_suffix.unwrap_or_default(),
            position: row.source_position,
        });
        let output_anchor = row
            .quarto_output
            .map(|raw| {
                serde_json::from_str::<QuartoOutputAnchor>(&raw).map_err(|err| {
                    crate::storage::catalog::CatalogError::Invalid(format!(
                        "comment Quarto output anchor is invalid: {err}"
                    ))
                })
            })
            .transpose()?;
        let replies = catalog.replies(slug, &row.id, 100)?;
        comments.push(Comment {
            id: row.id,
            seq: row.seq,
            motivation: row.motivation,
            body: row.body,
            creator: row.creator,
            author: row.author,
            via: row.via,
            created: row.created,
            exact: row.exact,
            prefix: row.prefix,
            suffix: row.suffix,
            position: row.position,
            point: row.point,
            color: row.color.clone(),
            region,
            output_anchor,
            source,
            proposed: row.proposed,
            pass: row.pass,
            outcome: row.outcome,
            accept_request: row.accept_request,
            revision: row.revision,
            resolved: row.resolved,
            resolved_at: row.resolved_at,
            resolved_in: row.resolved_in,
            replies: replies
                .into_iter()
                .map(|reply| Reply {
                    id: reply.id,
                    body: reply.body,
                    creator: reply.creator,
                    author: reply.author,
                    created: reply.created,
                })
                .collect(),
        });
    }
    Ok((annotation_seq, comments))
}

/// What one catalogue comment row costs as an owned job input.
fn comment_row_bytes(row: &crate::storage::catalog::Comment) -> usize {
    DESCRIPTOR_BYTES
        + row.slug.len()
        + row.id.len()
        + row.body.len()
        + row.exact.len()
        + row.prefix.len()
        + row.suffix.len()
        + row.proposed.as_ref().map(String::len).unwrap_or_default()
        + row.region.as_ref().map(String::len).unwrap_or_default()
}

/// How many comments this document durably has.  Read rather than counted in
/// memory, because a hot room cache may lag a previous process while the room
/// lease is being acquired.
pub(super) async fn count_catalog_comments(
    catalog: &Arc<Catalog>,
    slug: &str,
) -> Result<usize, String> {
    let slug = slug.to_string();
    catalog
        .execute_catalog(slug.len() + DESCRIPTOR_BYTES, move |catalog| {
            catalog.comments(&slug, None, 500).map(|rows| rows.len())
        })
        .await
        .map_err(|error| error.to_string())
}

/// Insert one comment with no request receipt.  Only the seeding path uses
/// this; every request-serving insert carries a receipt.
pub(super) async fn insert_comment_row(
    catalog: &Arc<Catalog>,
    row: crate::storage::catalog::Comment,
) -> Result<i64, String> {
    let input_bytes = comment_row_bytes(&row);
    catalog
        .execute_catalog(input_bytes, move |catalog| {
            catalog.insert_comment(&row).map(|row| row.seq)
        })
        .await
        .map_err(|error| error.to_string())
}

/// Insert one comment under its request receipt, which is what makes a retry
/// of the same request return the first insert rather than a second comment.
pub(super) async fn insert_comment_request(
    catalog: &Arc<Catalog>,
    row: crate::storage::catalog::Comment,
    request_id: String,
    digest: String,
    at: i64,
) -> Result<i64, String> {
    let input_bytes = comment_row_bytes(&row) + request_id.len() + digest.len();
    catalog
        .execute_catalog(input_bytes, move |catalog| {
            catalog
                .insert_comment_request(&row, &request_id, &digest, at)
                .map(|row| row.seq)
        })
        .await
        .map_err(|error| error.to_string())
}

/// Write one comment row back.
///
/// There is no receipt for a decision — resolve, reopen, anchor — and it does
/// not need one: the row is written to exactly the value the request asked
/// for, so repeating it is the same write. What cancellation must not do is
/// leave the room's in-memory copy ahead of the row, which is why every
/// caller here applies its change to room state only after this returns.
pub(super) async fn update_comment_row(
    catalog: &Arc<Catalog>,
    row: crate::storage::catalog::Comment,
) -> Result<(), String> {
    let input_bytes = comment_row_bytes(&row);
    catalog
        .execute_catalog(input_bytes, move |catalog| {
            catalog.update_comment(&row).map(|_| ())
        })
        .await
        .map_err(|error| error.to_string())
}

/// Remove one comment row.
pub(super) async fn delete_comment_row(
    catalog: &Arc<Catalog>,
    slug: &str,
    id: &str,
) -> Result<(), String> {
    let slug = slug.to_string();
    let id = id.to_string();
    catalog
        .execute_catalog(slug.len() + id.len() + DESCRIPTOR_BYTES, move |catalog| {
            catalog.delete_comment(&slug, &id).map(|_| ())
        })
        .await
        .map_err(|error| error.to_string())
}

/// Insert one reply under its request receipt.
pub(super) async fn insert_reply_request(
    catalog: &Arc<Catalog>,
    row: crate::storage::catalog::Reply,
    request_id: String,
    digest: String,
    at: i64,
) -> Result<(), String> {
    let input_bytes = DESCRIPTOR_BYTES
        + row.slug.len()
        + row.comment_id.len()
        + row.id.len()
        + row.body.len()
        + request_id.len()
        + digest.len();
    catalog
        .execute_catalog(input_bytes, move |catalog| {
            catalog
                .insert_reply_request(&row, &request_id, &digest, at)
                .map(|_| ())
        })
        .await
        .map_err(|error| error.to_string())
}

/// The suggestion-accept cluster, through the boundary.
///
/// Every one of these carries the acceptance receipt — the request id and the
/// digest of what was asked for — so cancellation needs no completion hook
/// here: a caller that disappears leaves a durable receipt, and the retry
/// resumes from it rather than applying the proposal a second time. That is
/// the same reconciliation the crash path already used.
pub(super) async fn begin_suggestion_accept(
    catalog: &Arc<Catalog>,
    slug: &str,
    comment_id: &str,
    request_id: &str,
    digest: &str,
    at: i64,
) -> Result<Option<crate::storage::catalog::Comment>, String> {
    let slug = slug.to_string();
    let comment_id = comment_id.to_string();
    let request_id = request_id.to_string();
    let digest = digest.to_string();
    let input_bytes =
        slug.len() + comment_id.len() + request_id.len() + digest.len() + DESCRIPTOR_BYTES;
    catalog
        .execute_catalog(input_bytes, move |catalog| {
            catalog.begin_suggestion_accept(&slug, &comment_id, &request_id, &digest, at)
        })
        .await
        .map_err(|error| error.to_string())
}

pub(super) async fn suggestion_accept_checkpoint(
    catalog: &Arc<Catalog>,
    slug: &str,
    request_id: &str,
    digest: &str,
) -> Result<Option<(String, String, String)>, String> {
    let slug = slug.to_string();
    let request_id = request_id.to_string();
    let digest = digest.to_string();
    let input_bytes = slug.len() + request_id.len() + digest.len() + DESCRIPTOR_BYTES;
    catalog
        .execute_catalog(input_bytes, move |catalog| {
            catalog.suggestion_accept_checkpoint(&slug, &request_id, &digest)
        })
        .await
        .map_err(|error| error.to_string())
}

pub(super) async fn suggestion_accept_update(
    catalog: &Arc<Catalog>,
    slug: &str,
    request_id: &str,
    digest: &str,
) -> Result<Option<Vec<u8>>, String> {
    let slug = slug.to_string();
    let request_id = request_id.to_string();
    let digest = digest.to_string();
    let input_bytes = slug.len() + request_id.len() + digest.len() + DESCRIPTOR_BYTES;
    catalog
        .execute_catalog(input_bytes, move |catalog| {
            catalog.suggestion_accept_update(&slug, &request_id, &digest)
        })
        .await
        .map_err(|error| error.to_string())
}

/// Stage the exact post-accept state in the receipt.  The update is the large
/// input here, so capacity is reserved for it before it is copied.
pub(super) async fn stage_suggestion_accept_update(
    catalog: &Arc<Catalog>,
    slug: &str,
    comment_id: &str,
    request_id: &str,
    digest: &str,
    update: &[u8],
) -> Result<(), String> {
    let input_bytes = slug.len()
        + comment_id.len()
        + request_id.len()
        + digest.len()
        + update.len()
        + DESCRIPTOR_BYTES;
    let reservation = catalog
        .reserve_execution(input_bytes.min(crate::storage::catalog::MAX_REQUEST_BYTES))
        .await
        .map_err(|error| error.to_string())?;
    let slug = slug.to_string();
    let comment_id = comment_id.to_string();
    let request_id = request_id.to_string();
    let digest = digest.to_string();
    let update = update.to_vec();
    reservation
        .execute_catalog(move |catalog| {
            catalog.stage_suggestion_accept_update(
                &slug,
                &comment_id,
                &request_id,
                &digest,
                &update,
            )
        })
        .await
        .map_err(|error| error.to_string())
}

/// Record the accept checkpoint in the receipt and settle the comment, in one
/// job: they were two transactions and remain two, but a caller that goes
/// away between them no longer leaves the second unissued.
pub(super) async fn record_and_finish_suggestion_accept(
    catalog: &Arc<Catalog>,
    slug: &str,
    comment_id: &str,
    request_id: &str,
    digest: &str,
    sha: &str,
    resolved_at: &str,
) -> Result<(), String> {
    let slug = slug.to_string();
    let comment_id = comment_id.to_string();
    let request_id = request_id.to_string();
    let digest = digest.to_string();
    let sha = sha.to_string();
    let resolved_at = resolved_at.to_string();
    let input_bytes = slug.len()
        + comment_id.len()
        + request_id.len()
        + digest.len()
        + sha.len()
        + resolved_at.len()
        + DESCRIPTOR_BYTES;
    catalog
        .execute_catalog(input_bytes, move |catalog| {
            catalog.record_suggestion_accept_checkpoint(
                &slug,
                &comment_id,
                &request_id,
                &digest,
                &sha,
                &resolved_at,
            )?;
            catalog
                .finish_suggestion_accept(
                    &slug,
                    &comment_id,
                    &request_id,
                    &digest,
                    &sha,
                    &resolved_at,
                )
                .map(|_| ())
        })
        .await
        .map_err(|error| error.to_string())
}

pub(super) async fn finish_suggestion_accept(
    catalog: &Arc<Catalog>,
    slug: &str,
    comment_id: &str,
    request_id: &str,
    digest: &str,
    sha: &str,
    resolved_at: &str,
) -> Result<(), String> {
    let slug = slug.to_string();
    let comment_id = comment_id.to_string();
    let request_id = request_id.to_string();
    let digest = digest.to_string();
    let sha = sha.to_string();
    let resolved_at = resolved_at.to_string();
    let input_bytes = slug.len()
        + comment_id.len()
        + request_id.len()
        + digest.len()
        + sha.len()
        + resolved_at.len()
        + DESCRIPTOR_BYTES;
    catalog
        .execute_catalog(input_bytes, move |catalog| {
            catalog
                .finish_suggestion_accept(
                    &slug,
                    &comment_id,
                    &request_id,
                    &digest,
                    &sha,
                    &resolved_at,
                )
                .map(|_| ())
        })
        .await
        .map_err(|error| error.to_string())
}

/// Whether a suggestion acceptance is still staged for this comment.
pub(super) async fn pending_suggestion_accept(
    catalog: &Arc<Catalog>,
    slug: &str,
    comment_id: &str,
) -> Result<bool, String> {
    let slug = slug.to_string();
    let comment_id = comment_id.to_string();
    catalog
        .execute_catalog(
            slug.len() + comment_id.len() + DESCRIPTOR_BYTES,
            move |catalog| catalog.pending_suggestion_accept(&slug, &comment_id),
        )
        .await
        .map_err(|error| error.to_string())
}

pub(super) fn catalog_comment_row(
    slug: &str,
    item: &Comment,
) -> Result<crate::storage::catalog::Comment, String> {
    let region = item
        .region
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|err| format!("comment region is not serializable: {err}"))?;
    let (source_path, source_exact, source_prefix, source_suffix, source_position) =
        match &item.source {
            Some(source) => (
                Some(source.path.clone()),
                Some(source.exact.clone()),
                Some(source.prefix.clone()),
                Some(source.suffix.clone()),
                source.position,
            ),
            None => (None, None, None, None, None),
        };
    let quarto_output = item
        .output_anchor
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|err| format!("comment Quarto output anchor is not serializable: {err}"))?;
    Ok(crate::storage::catalog::Comment {
        slug: slug.to_string(),
        id: item.id.clone(),
        seq: item.seq,
        motivation: item.motivation.clone(),
        body: item.body.clone(),
        creator: item.creator.clone(),
        author: item.author.clone(),
        via: item.via.clone(),
        created: item.created.clone(),
        exact: item.exact.clone(),
        prefix: item.prefix.clone(),
        suffix: item.suffix.clone(),
        position: item.position,
        point: item.point,
        color: item.color.clone(),
        region,
        quarto_output,
        source_path,
        source_exact,
        source_prefix,
        source_suffix,
        source_position,
        proposed: item.proposed.clone(),
        pass: item.pass.clone(),
        outcome: item.outcome.clone(),
        accept_request: item.accept_request.clone(),
        revision: item.revision.clone(),
        resolved: item.resolved,
        resolved_at: item.resolved_at.clone(),
        resolved_in: item.resolved_in.clone(),
    })
}

pub(super) fn request_digest(value: &Value) -> String {
    fn canonical(value: &Value, out: &mut String) {
        match value {
            Value::Null => out.push_str("null"),
            Value::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Value::Number(value) => out.push_str(&value.to_string()),
            Value::String(value) => {
                out.push_str(&serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into()))
            }
            Value::Array(values) => {
                out.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        out.push(',');
                    }
                    canonical(value, out);
                }
                out.push(']');
            }
            Value::Object(values) => {
                let mut keys: Vec<_> = values.keys().collect();
                keys.sort();
                out.push('{');
                for (index, key) in keys.into_iter().enumerate() {
                    if index != 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::to_string(key).unwrap_or_else(|_| "\"\"".into()));
                    out.push(':');
                    canonical(&values[key], out);
                }
                out.push('}');
            }
        }
    }
    let mut bytes = String::new();
    canonical(value, &mut bytes);
    // The hashing step -- not the canonicalization above it -- is shared with
    // every other content digest in this codebase, so a stored receipt and a
    // stored document digest are hex(sha256(...)) by the same one function
    // rather than by two copies that could drift.
    crate::document::store::digest_of_bytes(bytes.as_bytes())
}

/// Reconciles the *entire* in-memory comment list against the catalogue, one
/// row (and one reply listing) at a time -- an upsert per comment plus a
/// bounded reply read, for every comment the room currently holds, on every
/// call. Every request-serving mutation (add/delete/resolve/reply/anchor/
/// accept/reject in `room/comments.rs` and `room/suggestions.rs`) instead
/// writes only the one row it changed and never reaches this function; the
/// only production caller is `seed::seed_annotations`, which uses it to write
/// a handful of demo annotations once per document when `librepaper seed`
/// populates an empty deployment. That caller's input is small and bounded by
/// the fixed example set, so the O(comments²) catalogue traffic (this runs
/// once per comment added, each time reconciling every comment added so far)
/// costs nothing worth batching. Do not add a new caller on a request path:
/// update the one row that changed instead, the way every handler above
/// already does.
pub(super) async fn save_catalog_comments(
    catalog: &Arc<Catalog>,
    slug: &str,
    seq: &mut i64,
    comments: &mut [Comment],
) -> Result<(), String> {
    // Reserved from the borrowed comments, before the owned copy the job
    // carries is built: a caller that copied first would already have spent
    // the memory the budget exists to bound.
    let input_bytes = slug.len()
        + DESCRIPTOR_BYTES
        + comments
            .iter()
            .map(comment_bytes)
            .sum::<usize>()
            .min(crate::storage::catalog::MAX_REQUEST_BYTES);
    let reservation = catalog
        .reserve_execution(input_bytes.min(crate::storage::catalog::MAX_REQUEST_BYTES))
        .await
        .map_err(|error| error.to_string())?;
    let slug_owned = slug.to_string();
    let mut owned: Vec<Comment> = comments.to_vec();
    let assigned = reservation
        .execute_catalog(move |catalog| {
            save_catalog_comments_blocking(catalog, &slug_owned, &mut owned)
                .map_err(crate::storage::catalog::CatalogError::Invalid)?;
            Ok(owned.iter().map(|item| item.seq).collect::<Vec<i64>>())
        })
        .await
        .map_err(|error| error.to_string())?;
    for (item, item_seq) in comments.iter_mut().zip(assigned) {
        item.seq = item_seq;
        *seq = (*seq).max(item_seq);
    }
    Ok(())
}

/// What one comment costs as an owned job input.  An estimate rather than a
/// measurement: it names the fields that actually carry a person's text.
fn comment_bytes(item: &Comment) -> usize {
    DESCRIPTOR_BYTES
        + item.body.len()
        + item.exact.len()
        + item.prefix.len()
        + item.suffix.len()
        + item.proposed.as_ref().map(String::len).unwrap_or_default()
        + item
            .replies
            .iter()
            .map(|reply| DESCRIPTOR_BYTES + reply.body.len())
            .sum::<usize>()
}

fn save_catalog_comments_blocking(
    catalog: &Catalog,
    slug: &str,
    comments: &mut [Comment],
) -> Result<(), String> {
    for item in comments.iter_mut() {
        let current = match catalog.comment(slug, &item.id) {
            Ok(row) => Some(row),
            Err(crate::storage::catalog::CatalogError::NotFound) => None,
            Err(err) => return Err(err.to_string()),
        };
        let mut row = catalog_comment_row(slug, item)?;
        if let Some(current) = current.as_ref() {
            row.seq = current.seq;
            catalog
                .update_comment(&row)
                .map_err(|err| err.to_string())?;
            item.seq = current.seq;
        } else {
            row.seq = -1;
            let inserted = catalog
                .insert_comment(&row)
                .map_err(|err| err.to_string())?;
            item.seq = inserted.seq;
        }
        let current_replies = catalog
            .replies(slug, &item.id, 100)
            .map_err(|err| err.to_string())?
            .into_iter()
            .map(|reply| Reply {
                id: reply.id,
                body: reply.body,
                creator: reply.creator,
                author: reply.author,
                created: reply.created,
            })
            .collect::<Vec<_>>();
        let desired_reply_ids: std::collections::HashSet<String> =
            item.replies.iter().map(|reply| reply.id.clone()).collect();
        for reply in &item.replies {
            let row = crate::storage::catalog::Reply {
                slug: slug.to_string(),
                comment_id: item.id.clone(),
                id: reply.id.clone(),
                body: reply.body.clone(),
                creator: reply.creator.clone(),
                author: reply.author.clone(),
                created: reply.created.clone(),
            };
            if current_replies.iter().any(|old| old.id == reply.id) {
                catalog.update_reply(&row).map_err(|err| err.to_string())?;
            } else {
                catalog.insert_reply(&row).map_err(|err| err.to_string())?;
            }
        }
        for reply in current_replies {
            if !desired_reply_ids.contains(&reply.id) {
                catalog
                    .delete_reply(slug, &item.id, &reply.id)
                    .map_err(|err| err.to_string())?;
            }
        }
    }
    // Deletions are issued by the delete operation itself.  Never infer them
    // from a room snapshot: a cold/stale room may not contain a comment another
    // process inserted, and replacing all rows would erase that concurrent
    // write.
    // comment_seq is only ever advanced by insert_comment.  Never write the
    // room's possibly stale cached value back over the authoritative counter;
    // the caller only raises its cached maximum from the seqs assigned here.
    Ok(())
}

/// Number of history entries a live room retains for hot-path operations.
/// SQLite remains the source of truth for the complete timeline; keeping the
/// tail here bounds resident memory for documents with years of checkpoints.
pub(super) const RESIDENT_CATALOG_HISTORY: u32 = 64;

pub(super) async fn load_catalog_manifest(
    catalog: &Arc<Catalog>,
    slug: &str,
) -> Result<Manifest, CatalogExecError> {
    let slug = slug.to_string();
    catalog
        .execute_catalog(slug.len() + DESCRIPTOR_BYTES, move |catalog| {
            let rows = catalog.checkpoints_tail(&slug, RESIDENT_CATALOG_HISTORY)?;
            let mut manifest = Manifest::from_catalog_rows(rows)
                .map_err(crate::storage::catalog::CatalogError::Invalid)?;
            let first = manifest
                .checkpoints
                .iter()
                .map(|point| point.seq)
                .min()
                .unwrap_or(0);
            let last = manifest
                .checkpoints
                .iter()
                .map(|point| point.seq)
                .max()
                .unwrap_or(0);
            let metadata = catalog.retention_metadata_range(&slug, first, last)?;
            for point in &mut manifest.checkpoints {
                if let Some((original_parent, gap)) = metadata.get(&point.sha) {
                    point.original_parent = original_parent.clone();
                    point.ancestry_gap = *gap;
                }
            }
            Ok(manifest)
        })
        .await
}

/// The complete timeline, paged inside one job.
///
/// The paging stays: a room with years of checkpoints must not read them in
/// one statement.  What changes is that the whole loop is one admitted
/// request on a blocking thread instead of one connection acquisition per
/// page from a Tokio worker.
pub(super) async fn load_catalog_history(
    catalog: &Arc<Catalog>,
    slug: &str,
) -> Result<Vec<Checkpoint>, CatalogExecError> {
    let slug = slug.to_string();
    catalog
        .execute_catalog(slug.len() + DESCRIPTOR_BYTES, move |catalog| {
            let rows = load_catalog_checkpoint_rows(catalog, &slug)?;
            let mut manifest = Manifest::from_catalog_rows(rows)
                .map_err(crate::storage::catalog::CatalogError::Invalid)?;
            let first = manifest
                .checkpoints
                .iter()
                .map(|point| point.seq)
                .min()
                .unwrap_or(0);
            let last = manifest
                .checkpoints
                .iter()
                .map(|point| point.seq)
                .max()
                .unwrap_or(0);
            let metadata = catalog.retention_metadata_range(&slug, first, last)?;
            for point in &mut manifest.checkpoints {
                if let Some((original_parent, gap)) = metadata.get(&point.sha) {
                    point.original_parent = original_parent.clone();
                    point.ancestry_gap = *gap;
                }
            }
            Ok(manifest.checkpoints)
        })
        .await
}

fn load_catalog_checkpoint_rows(
    catalog: &Catalog,
    slug: &str,
) -> crate::storage::catalog::CatalogResult<Vec<crate::storage::catalog::Checkpoint>> {
    // Catalog reads are deliberately bounded.  Never load only the first
    // page and then let a later metadata write treat that prefix as the whole
    // history: doing so would silently discard every newer checkpoint.
    let mut rows = Vec::new();
    let mut after = None;
    loop {
        let page = catalog.checkpoints(slug, after, 200)?;
        if page.is_empty() {
            break;
        }
        after = page.last().map(|row| row.seq);
        let complete = page.len() < 200;
        rows.extend(page);
        if complete {
            break;
        }
    }
    Ok(rows)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn save_catalog_manifest_with_assets(
    catalog: &Arc<Catalog>,
    slug: &str,
    previous: &Manifest,
    manifest: &Manifest,
    durable_seq: i64,
    actor: Option<OwnedAuthority>,
    sources: &[crate::storage::catalog::SourceHistoryRecord],
    assets: &[crate::storage::catalog::CheckpointAssetRef],
    lease_operation: Option<&str>,
    owner_limit: i64,
    total_limit: i64,
) -> Result<(), WriteError> {
    let rows = manifest_rows_to_write(slug, previous, manifest, durable_seq)?;
    // Reserved from the rows that will actually be written, before they are
    // handed to the job.
    let input_bytes = slug.len()
        + DESCRIPTOR_BYTES
        + rows
            .iter()
            .map(|row| DESCRIPTOR_BYTES + row.sha.len() + row.label.len() + row.by.len())
            .sum::<usize>()
        + sources
            .iter()
            .map(|source| {
                source.file_digest.len()
                    + source.recipe_key.len()
                    + source.recipe_digest.len()
                    + source
                        .objects
                        .iter()
                        .map(|object| object.object_key.len() + object.kind.len() + 16)
                        .sum::<usize>()
            })
            .sum::<usize>()
        + assets
            .iter()
            .map(|asset| asset.object_key.len() + 16)
            .sum::<usize>()
        + lease_operation.map_or(0, str::len);
    let slug_owned = slug.to_string();
    let sources = sources.to_vec();
    let assets = assets.to_vec();
    let lease_operation = lease_operation.map(str::to_owned);
    catalog
        .execute_catalog(
            input_bytes.min(crate::storage::catalog::MAX_REQUEST_BYTES),
            move |catalog| {
                // A publication has a prepared receipt.  Keep its checkpoint
                // descriptor in that receipt until the final commit
                // transaction; ordinary checkpoints retain the direct atomic
                // insert path.
                if catalog
                    .document(&slug_owned)?
                    .and_then(|document| document.pending_publication)
                    .is_some()
                {
                    if actor
                        .as_ref()
                        .is_some_and(|actor| actor.agent_checkpoint.is_some())
                    {
                        return Err(crate::storage::catalog::CatalogError::Conflict(
                            "publication must finish before agent checkpoint".into(),
                        ));
                    }
                    if let Some(row) = rows.last() {
                        catalog.stage_publication_checkpoint_with_sources_assets_and_quota(
                            &slug_owned,
                            row,
                            actor.as_ref().map(OwnedAuthority::borrow),
                            &sources,
                            &assets,
                            lease_operation.as_deref(),
                            owner_limit,
                            total_limit,
                        )?;
                    }
                } else {
                    catalog.insert_checkpoints_atomic_with_sources_assets_and_quota(
                        &rows,
                        actor.as_ref().map(OwnedAuthority::borrow),
                        &sources,
                        &assets,
                        lease_operation.as_deref(),
                        owner_limit,
                        total_limit,
                    )?;
                }
                Ok(())
            },
        )
        .await
        // A quota or a lost right refused here is exactly what the caller has
        // to tell a client apart; keep it typed rather than flattening it.
        .map_err(WriteError::from)
    // A resident-tail snapshot intentionally omits older rows.  Absence from
    // `previous`/`manifest` therefore never means deletion; destructive
    // retention is an explicit catalogue operation with its own policy.
}

fn manifest_rows_to_write(
    slug: &str,
    previous: &Manifest,
    manifest: &Manifest,
    durable_seq: i64,
) -> Result<Vec<crate::storage::catalog::Checkpoint>, WriteError> {
    let previous_by_sha: HashMap<_, _> = previous
        .checkpoints
        .iter()
        .map(|point| (point.sha.as_str(), point))
        .collect();
    let mut rows = Vec::new();
    for point in &manifest.checkpoints {
        let row = Manifest::catalog_row(slug, point, -1, durable_seq)
            .map_err(|error| WriteError::Storage(error.to_string()))?;
        // Keep labels from the staged resident manifest, but avoid requiring
        // a read transaction for each row. The catalogue method performs the
        // existence check and all inserts/updates in one write transaction.
        if !previous_by_sha.contains_key(point.sha.as_str())
            || previous_by_sha
                .get(point.sha.as_str())
                .is_some_and(|old| old.label != point.label)
        {
            rows.push(row);
        }
    }
    Ok(rows)
}

/// Record a rendering's catalogue row: read what is there, merge this
/// upload's half of it, publish.
///
/// The read and the publish are one job.  They were two transactions before
/// and they still are, but a caller cancelled between them used to leave the
/// blocking read having parked a Tokio worker for nothing; now the whole
/// sequence belongs to the service once it is dispatched, and the publish
/// happens whether or not the uploader is still waiting for its answer.  The
/// authority travels owned into the job because it is re-checked inside the
/// publishing transaction.
pub(super) async fn save_catalog_rendering_with_authority(
    catalog: &Arc<Catalog>,
    slug: &str,
    tree_sha: &str,
    synctex: bool,
    size: i64,
    actor: Option<OwnedAuthority>,
) -> Result<(), WriteError> {
    let input_bytes = slug.len()
        + tree_sha.len()
        + DESCRIPTOR_BYTES
        + actor
            .as_ref()
            .map(OwnedAuthority::bytes)
            .unwrap_or_default();
    let slug = slug.to_string();
    let tree_sha = tree_sha.to_string();
    catalog
        .execute_catalog(input_bytes, move |catalog| {
            save_catalog_rendering_blocking(catalog, &slug, &tree_sha, synctex, size, actor)
        })
        .await
        .map_err(WriteError::from)
}

fn save_catalog_rendering_blocking(
    catalog: &Catalog,
    slug: &str,
    tree_sha: &str,
    synctex: bool,
    size: i64,
    actor: Option<OwnedAuthority>,
) -> crate::storage::catalog::CatalogResult<()> {
    let previous = catalog.rendering(slug, tree_sha)?;
    let rendering = crate::storage::catalog::Rendering {
        slug: slug.to_string(),
        tree_sha: tree_sha.to_string(),
        at: timestamp(),
        backend: previous
            .as_ref()
            .map(|row| row.backend.clone())
            .unwrap_or_default(),
        engine: previous
            .as_ref()
            .map(|row| row.engine.clone())
            .unwrap_or_default(),
        release: previous
            .as_ref()
            .map(|row| row.release.clone())
            .unwrap_or_default(),
        tools: previous
            .as_ref()
            .map(|row| row.tools.clone())
            .unwrap_or_default(),
        bytes: if synctex {
            previous.as_ref().map(|row| row.bytes).unwrap_or_default()
        } else {
            size
        },
        synctex: synctex || previous.as_ref().is_some_and(|row| row.synctex),
        synctex_bytes: if synctex {
            size
        } else {
            previous
                .as_ref()
                .map(|row| row.synctex_bytes)
                .unwrap_or_default()
        },
    };
    if let Some(actor) = actor {
        catalog
            .publish_rendering_with_authority(&rendering, actor.borrow())
            .map(|_| ())
    } else {
        catalog.publish_rendering(&rendering).map(|_| ())
    }
}

#[cfg(test)]
mod request_digest_tests {
    use super::request_digest;
    use serde_json::json;

    /// `request_digest` is what makes a stored comment/suggestion receipt
    /// idempotent: the same canonical bytes must hash the same way forever,
    /// or a retried request could be charged or applied twice after a
    /// canonicalization change nobody meant to make. These fix the digest of
    /// a handful of representative values so any future edit to `canonical`
    /// (key order, string escaping, or number formatting) that is not
    /// byte-for-byte compatible fails loudly here rather than silently
    /// breaking retry matching against already-stored receipts.
    #[test]
    fn nested_object_keys_are_sorted_before_hashing() {
        // Two JSON objects with the same keys inserted in a different order
        // must canonicalize (and therefore hash) identically.
        let forward = json!({"a": 1, "b": 2, "z": 3});
        let reversed = json!({"z": 3, "b": 2, "a": 1});
        assert_eq!(request_digest(&forward), request_digest(&reversed));
        assert_eq!(
            request_digest(&forward),
            "329d4b5a274b8081ef038bb735813dc3082cf6d95855f8029c9cd8432168c112"
        );
    }

    #[test]
    fn string_escapes_are_included_in_the_canonical_bytes() {
        let value = json!({"body": "quote \" and backslash \\ and newline \n"});
        assert_eq!(
            request_digest(&value),
            "dde0899efbe2c590bf2985f9ff257a896714a5907f558f811d0006ee7e998ee5"
        );
    }

    /// Distinguishes an integer from an equal-valued float: `1` and `1.0`
    /// must not collide, because a stored receipt's request payload keeps
    /// whatever numeric shape the client sent.
    #[test]
    fn integer_and_float_of_equal_value_hash_differently() {
        let integer = json!({"position": 1});
        let float = json!({"position": 1.0});
        assert_ne!(request_digest(&integer), request_digest(&float));
    }

    #[test]
    fn nested_arrays_and_objects_canonicalize_deterministically() {
        let value = json!({
            "outer": {"z": [1, 2, {"inner": "value"}], "a": null},
            "flag": true,
        });
        // Recomputing twice from equivalent but differently-ordered input
        // must agree; this is the property the receipt matching relies on.
        let same_value_reordered = json!({
            "flag": true,
            "outer": {"a": null, "z": [1, 2, {"inner": "value"}]},
        });
        assert_eq!(
            request_digest(&value),
            request_digest(&same_value_reordered)
        );
    }
}
