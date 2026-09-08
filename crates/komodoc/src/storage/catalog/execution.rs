//! The asynchronous catalogue execution boundary.
//!
//! Every catalogue operation is synchronous SQL against one connection.  Run
//! directly from an asynchronous task, a busy wait, a long query, or simply
//! contention for the connection mutex parks a Tokio worker, which stalls
//! unrelated sockets and heartbeats.  This module adds an *additional* entry
//! point that submits an owned job to a blocking thread under bounded
//! admission and returns an owned result.  The synchronous API stays exactly
//! as it was: both go through `Catalog::lock_connection`, so they share one
//! connection and therefore one TEMP `room_edit_reservations` table.
//!
//! Jobs must not perform object-store I/O, call back into asynchronous room
//! code, or require a gate their waiting caller already owns: a job runs on a
//! blocking thread with no reactor, and a caller blocked on a gate that a
//! queued job needs would deadlock the single connection.  The catalogue
//! journal gate and the deployment writer lock are unchanged and are still
//! acquired by callers outside this boundary.

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use rusqlite::{Connection, Transaction};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

use super::{Catalog, CatalogError, CatalogResult};

/// Requests admitted at once, queued plus executing.  The connection is
/// serial, so this is a backlog bound rather than a parallelism setting: at
/// the observed cost of a catalogue transaction it is roughly a second of
/// work, which is long enough to absorb an editing burst and short enough
/// that a saturated deployment reports pressure instead of hiding it.
pub const MAX_ADMITTED_REQUESTS: usize = 64;

/// Retained owned input bytes across all admitted requests.  Sixteen mebibytes
/// is four maximum-size requests; the point of the budget is that a burst of
/// large checkpoint or asset descriptors cannot multiply into hundreds of
/// megabytes of queued copies while one connection works through them.
pub const MAX_QUEUED_BYTES: usize = 16 * 1024 * 1024;

/// The largest declared input for a single request.  Anything larger belongs
/// in the object store, not in a queued SQL job.
pub const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;

/// Jobs dispatched to blocking threads at once.  One would suffice for
/// throughput because the connection mutex serialises them; a second lets the
/// next job take the connection the instant the previous commit releases it,
/// without letting a backlog occupy the shared blocking pool.
pub const MAX_EXECUTING: usize = 2;

/// Producers allowed to wait for capacity at once.  Past this, waiting is
/// itself the unbounded queue the budgets exist to prevent.
pub const MAX_WAITING_PRODUCERS: usize = 128;

/// A request that declares no more than this may wait for capacity.  Larger
/// requests fail fast, because a waiting producer holding a multi-megabyte
/// owned input is precisely the memory the budget is meant to bound.
pub const SMALL_REQUEST_BYTES: usize = 4 * 1024;

/// Errors from the execution boundary itself, as distinct from the SQL a job
/// runs.
#[derive(Debug)]
pub enum CatalogExecError {
    /// Admission budgets are full.  Temporary: the caller may retry, and
    /// nothing was submitted.
    Saturated,
    /// The boundary has stopped admitting work, or rejected this request
    /// during shutdown before any SQL ran.
    ShuttingDown,
    /// The declared input exceeds `MAX_REQUEST_BYTES`.
    TooLarge { bytes: usize, limit: usize },
    /// The job panicked.  The connection, its open transaction's rollback,
    /// and the TEMP reservation table all survive; only this request failed.
    Panicked,
    /// The job ran and returned a catalogue error.
    Catalog(CatalogError),
}

impl std::fmt::Display for CatalogExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Saturated => f.write_str("catalogue execution is saturated"),
            Self::ShuttingDown => f.write_str("catalogue execution is shutting down"),
            Self::TooLarge { bytes, limit } => {
                write!(f, "catalogue request of {bytes} bytes exceeds {limit}")
            }
            Self::Panicked => f.write_str("catalogue job panicked"),
            Self::Catalog(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for CatalogExecError {}

impl From<CatalogError> for CatalogExecError {
    fn from(err: CatalogError) -> Self {
        Self::Catalog(err)
    }
}

/// Collapse a boundary error into the catalogue error the migrated call sites
/// already handle, so moving a caller onto `execute` does not force a new
/// error type through every layer above it.
///
/// The mapping preserves what callers act on: admission saturation is
/// `Busy`, which every retry path already treats as temporary; shutdown is
/// `Closed`, the same error the synchronous API returns once SQLite is gone;
/// an oversized declaration and a panicked job are programming faults, not
/// conditions a caller can retry into success, so both surface as `Invalid`.
impl From<CatalogExecError> for CatalogError {
    fn from(err: CatalogExecError) -> Self {
        match err {
            CatalogExecError::Catalog(err) => err,
            CatalogExecError::Saturated => CatalogError::Busy,
            CatalogExecError::ShuttingDown => CatalogError::Closed,
            CatalogExecError::TooLarge { bytes, limit } => CatalogError::Invalid(format!(
                "catalogue request of {bytes} bytes exceeds {limit}"
            )),
            CatalogExecError::Panicked => {
                CatalogError::Invalid("catalogue job panicked".to_string())
            }
        }
    }
}

/// How a submitted job ended, as seen by the service rather than the caller.
#[derive(Clone, Copy, Debug)]
pub enum CatalogOutcome<'a> {
    /// The job returned successfully; a transaction job committed.
    Committed,
    /// The job returned an error; a transaction job did not commit.
    Failed(&'a CatalogError),
    /// The job panicked.  Any open transaction rolled back.
    Panicked,
    /// Shutdown rejected the job before it started.  No SQL ran.
    Rejected,
}

/// A service-owned completion hook.
///
/// It runs on the executing thread after the job finishes, before the
/// request's permits are released, and it runs whether or not the caller is
/// still waiting for the result.  It exists so a later caller migration can
/// attach reservation reconciliation — releasing exactly the temporary edit
/// reservation this request created, keyed by its own generation — to a
/// request whose caller has gone away.  It is given the connection because
/// that reconciliation is itself SQL; it must obey the same rules as a job.
pub trait CatalogCompletion: Send + 'static {
    fn complete(self: Box<Self>, outcome: CatalogOutcome<'_>, connection: &mut Connection);
}

impl<F> CatalogCompletion for F
where
    F: FnOnce(CatalogOutcome<'_>, &mut Connection) + Send + 'static,
{
    fn complete(self: Box<Self>, outcome: CatalogOutcome<'_>, connection: &mut Connection) {
        (*self)(outcome, connection)
    }
}

#[derive(Default)]
struct Counters {
    queued: AtomicUsize,
    executing: AtomicUsize,
    queued_bytes: AtomicUsize,
    waiting_producers: AtomicUsize,
    admitted: AtomicU64,
    completed: AtomicU64,
    failed: AtomicU64,
    panicked: AtomicU64,
    rejected: AtomicU64,
    saturated: AtomicU64,
    queue_wait_micros_total: AtomicU64,
    queue_wait_micros_max: AtomicU64,
    execution_micros_total: AtomicU64,
    execution_micros_max: AtomicU64,
}

/// Admission budgets, counters, and shutdown state.  Held inside `Catalog`
/// so any `Arc<Catalog>` is already an execution service.
pub(crate) struct CatalogExecution {
    admission: Arc<Semaphore>,
    bytes: Arc<Semaphore>,
    executing: Arc<Semaphore>,
    shutting_down: AtomicBool,
    idle: Notify,
    counters: Counters,
}

impl CatalogExecution {
    pub(crate) fn new() -> Self {
        Self {
            admission: Arc::new(Semaphore::new(MAX_ADMITTED_REQUESTS)),
            bytes: Arc::new(Semaphore::new(MAX_QUEUED_BYTES)),
            executing: Arc::new(Semaphore::new(MAX_EXECUTING)),
            shutting_down: AtomicBool::new(false),
            idle: Notify::new(),
            counters: Counters::default(),
        }
    }
}

/// A readable view of the boundary's state, for tests and diagnostics.  The
/// deployment has no metrics endpoint, so this struct and the shutdown log
/// line are the whole exposure.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CatalogExecutionSnapshot {
    pub queued: usize,
    pub executing: usize,
    pub queued_bytes: usize,
    pub waiting_producers: usize,
    pub admitted: u64,
    pub completed: u64,
    pub failed: u64,
    pub panicked: u64,
    pub rejected: u64,
    pub saturated: u64,
    pub queue_wait_micros_total: u64,
    pub queue_wait_micros_max: u64,
    pub execution_micros_total: u64,
    pub execution_micros_max: u64,
}

impl CatalogExecutionSnapshot {
    /// Requests that were accepted and have settled one way or another.
    pub fn settled(&self) -> u64 {
        self.completed + self.failed + self.panicked + self.rejected
    }
}

/// Capacity held for one request, taken before the caller builds the owned
/// inputs it will submit.
///
/// While this value is alive the request is in the `Preparing` state: the
/// caller owns it, no SQL has been submitted, and dropping it releases every
/// permit.  Submitting moves ownership to the service.
pub struct CatalogReservation {
    catalog: Arc<Catalog>,
    admission: Option<OwnedSemaphorePermit>,
    bytes: Option<OwnedSemaphorePermit>,
    input_bytes: usize,
    reserved_at: Instant,
}

impl std::fmt::Debug for CatalogReservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CatalogReservation")
            .field("input_bytes", &self.input_bytes)
            .finish_non_exhaustive()
    }
}

impl Drop for CatalogReservation {
    fn drop(&mut self) {
        // Only reached when the caller was cancelled before dispatch, or
        // never submitted: the counters unwind with the permits.
        if self.admission.is_some() {
            self.catalog
                .execution
                .counters
                .queued
                .fetch_sub(1, Ordering::Relaxed);
            self.catalog
                .execution
                .counters
                .queued_bytes
                .fetch_sub(self.input_bytes, Ordering::Relaxed);
            self.catalog.execution.idle.notify_waiters();
        }
    }
}

impl Catalog {
    /// Reserve capacity for one request before building its owned inputs.
    ///
    /// `input_bytes` is the caller's estimate of the owned arguments it is
    /// about to construct.  Reserving first is the point: a caller that
    /// copied a megabyte and then waited for a permit would have already
    /// spent the memory the budget exists to bound.
    ///
    /// Small requests may wait for capacity under the waiting-producer bound;
    /// larger ones return `Saturated` immediately rather than parking with
    /// their inputs in hand.
    pub async fn reserve_execution(
        self: &Arc<Self>,
        input_bytes: usize,
    ) -> Result<CatalogReservation, CatalogExecError> {
        if input_bytes > MAX_REQUEST_BYTES {
            return Err(CatalogExecError::TooLarge {
                bytes: input_bytes,
                limit: MAX_REQUEST_BYTES,
            });
        }
        let execution = &self.execution;
        if execution.shutting_down.load(Ordering::Acquire) {
            return Err(CatalogExecError::ShuttingDown);
        }
        let may_wait = input_bytes <= SMALL_REQUEST_BYTES;
        let admission = self.acquire(&execution.admission, 1, may_wait).await?;
        let bytes = self
            .acquire(&execution.bytes, input_bytes as u32, may_wait)
            .await?;
        // Re-check: shutdown may have started while this producer waited.
        if execution.shutting_down.load(Ordering::Acquire) {
            return Err(CatalogExecError::ShuttingDown);
        }
        execution.counters.queued.fetch_add(1, Ordering::Relaxed);
        execution
            .counters
            .queued_bytes
            .fetch_add(input_bytes, Ordering::Relaxed);
        execution.counters.admitted.fetch_add(1, Ordering::Relaxed);
        Ok(CatalogReservation {
            catalog: self.clone(),
            admission: Some(admission),
            bytes: Some(bytes),
            input_bytes,
            reserved_at: Instant::now(),
        })
    }

    /// Reserve and submit a connection job in one step, for callers whose
    /// inputs are already owned and small.
    pub async fn execute<T, F>(
        self: &Arc<Self>,
        input_bytes: usize,
        job: F,
    ) -> Result<T, CatalogExecError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> CatalogResult<T> + Send + 'static,
    {
        self.reserve_execution(input_bytes)
            .await?
            .execute(job)
            .await
    }

    /// Reserve and submit one existing catalogue operation in one step.  See
    /// [`CatalogReservation::operation`] for what a job may do.
    pub async fn execute_operation<T, F>(
        self: &Arc<Self>,
        input_bytes: usize,
        job: F,
    ) -> Result<T, CatalogExecError>
    where
        T: Send + 'static,
        F: FnOnce(&Catalog) -> CatalogResult<T> + Send + 'static,
    {
        self.reserve_execution(input_bytes)
            .await?
            .operation(job)
            .await
    }

    /// Reserve and submit a `BEGIN IMMEDIATE` transaction job in one step.
    pub async fn execute_transaction<T, F>(
        self: &Arc<Self>,
        input_bytes: usize,
        job: F,
    ) -> Result<T, CatalogExecError>
    where
        T: Send + 'static,
        F: FnOnce(&Transaction<'_>) -> CatalogResult<T> + Send + 'static,
    {
        self.reserve_execution(input_bytes)
            .await?
            .transaction(job)
            .await
    }

    async fn acquire(
        &self,
        semaphore: &Arc<Semaphore>,
        permits: u32,
        may_wait: bool,
    ) -> Result<OwnedSemaphorePermit, CatalogExecError> {
        let execution = &self.execution;
        match semaphore.clone().try_acquire_many_owned(permits) {
            Ok(permit) => return Ok(permit),
            Err(tokio::sync::TryAcquireError::Closed) => {
                return Err(CatalogExecError::ShuttingDown)
            }
            Err(tokio::sync::TryAcquireError::NoPermits) => {}
        }
        if !may_wait {
            execution.counters.saturated.fetch_add(1, Ordering::Relaxed);
            return Err(CatalogExecError::Saturated);
        }
        let waiting = execution
            .counters
            .waiting_producers
            .fetch_add(1, Ordering::Relaxed);
        if waiting >= MAX_WAITING_PRODUCERS {
            execution
                .counters
                .waiting_producers
                .fetch_sub(1, Ordering::Relaxed);
            execution.counters.saturated.fetch_add(1, Ordering::Relaxed);
            return Err(CatalogExecError::Saturated);
        }
        let acquired = semaphore.clone().acquire_many_owned(permits).await;
        execution
            .counters
            .waiting_producers
            .fetch_sub(1, Ordering::Relaxed);
        acquired.map_err(|_| CatalogExecError::ShuttingDown)
    }

    /// A readable view of admission and lifecycle counters.
    pub fn execution_snapshot(&self) -> CatalogExecutionSnapshot {
        let counters = &self.execution.counters;
        CatalogExecutionSnapshot {
            queued: counters.queued.load(Ordering::Relaxed),
            executing: counters.executing.load(Ordering::Relaxed),
            queued_bytes: counters.queued_bytes.load(Ordering::Relaxed),
            waiting_producers: counters.waiting_producers.load(Ordering::Relaxed),
            admitted: counters.admitted.load(Ordering::Relaxed),
            completed: counters.completed.load(Ordering::Relaxed),
            failed: counters.failed.load(Ordering::Relaxed),
            panicked: counters.panicked.load(Ordering::Relaxed),
            rejected: counters.rejected.load(Ordering::Relaxed),
            saturated: counters.saturated.load(Ordering::Relaxed),
            queue_wait_micros_total: counters.queue_wait_micros_total.load(Ordering::Relaxed),
            queue_wait_micros_max: counters.queue_wait_micros_max.load(Ordering::Relaxed),
            execution_micros_total: counters.execution_micros_total.load(Ordering::Relaxed),
            execution_micros_max: counters.execution_micros_max.load(Ordering::Relaxed),
        }
    }

    /// Stop admission, settle accepted work, then close SQLite.
    ///
    /// Producers waiting for capacity and requests that have not started are
    /// rejected with `ShuttingDown`; requests already executing keep the
    /// connection until their transaction and their completion hook finish,
    /// because interrupting a transaction here would leave exactly the
    /// uncertain outcome the lifecycle rules exist to avoid.  A committed
    /// request whose completion cannot run — a process killed rather than
    /// shut down — is recovered on restart the way it always was: the TEMP
    /// reservation table is gone with the process, so no stale edit
    /// reservation survives, and durable work is reconciled by its own
    /// receipt.
    pub async fn shutdown(&self) {
        self.execution.shutting_down.store(true, Ordering::Release);
        self.execution.admission.close();
        self.execution.bytes.close();
        self.execution.executing.close();
        loop {
            let idle = self.execution.idle.notified();
            let snapshot = self.execution_snapshot();
            if snapshot.queued == 0 && snapshot.executing == 0 {
                break;
            }
            idle.await;
        }
        self.close_connection();
    }

    fn finish(&self, outcome: &CatalogOutcome<'_>) {
        let counters = &self.execution.counters;
        match outcome {
            CatalogOutcome::Committed => counters.completed.fetch_add(1, Ordering::Relaxed),
            CatalogOutcome::Failed(_) => counters.failed.fetch_add(1, Ordering::Relaxed),
            CatalogOutcome::Panicked => counters.panicked.fetch_add(1, Ordering::Relaxed),
            CatalogOutcome::Rejected => counters.rejected.fetch_add(1, Ordering::Relaxed),
        };
    }
}

fn record_max(slot: &AtomicU64, value: u64) {
    let mut current = slot.load(Ordering::Relaxed);
    while value > current {
        match slot.compare_exchange_weak(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(observed) => current = observed,
        }
    }
}

impl CatalogReservation {
    /// The declared input size this reservation covers.
    pub fn input_bytes(&self) -> usize {
        self.input_bytes
    }

    /// Submit a job that borrows the connection, mirroring `with_connection`.
    pub async fn execute<T, F>(self, job: F) -> Result<T, CatalogExecError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> CatalogResult<T> + Send + 'static,
    {
        self.submit(Work::Connection(Box::new(job)), None).await
    }

    /// Submit a job that runs inside one `BEGIN IMMEDIATE` transaction,
    /// mirroring the synchronous `immediate` helper.  The whole transaction
    /// is one request; it is never split across queued statements.
    pub async fn transaction<T, F>(self, job: F) -> Result<T, CatalogExecError>
    where
        T: Send + 'static,
        F: FnOnce(&Transaction<'_>) -> CatalogResult<T> + Send + 'static,
    {
        self.submit(Work::Transaction(Box::new(job)), None).await
    }

    /// Submit one existing catalogue operation — a named `Catalog` method —
    /// to run on a blocking thread.
    ///
    /// Every such method already opens and commits exactly one `IMMEDIATE`
    /// transaction of its own, so a method call *is* the atomic unit
    /// `transaction` asks callers to preserve; it simply takes the connection
    /// itself rather than being handed one.  That is also why a job may not
    /// receive the connection here: a closure that held the connection and
    /// then called a catalogue method would deadlock on the same
    /// non-reentrant mutex, which is the hazard `document/store.rs` had at the
    /// one place it did that.
    ///
    /// The alternative — splitting each of the 142 catalogue methods into an
    /// `&Transaction` body plus two wrappers — buys nothing here, because the
    /// body would still be run as one transaction by one job, and it would
    /// rewrite the whole catalogue module for a migration that only needs to
    /// move the *call* off the Tokio worker.
    ///
    /// A closure that calls several methods gets several transactions, exactly
    /// as the synchronous caller did; it must therefore not be used to make a
    /// sequence atomic that was not atomic before.
    pub async fn operation<T, F>(self, job: F) -> Result<T, CatalogExecError>
    where
        T: Send + 'static,
        F: FnOnce(&Catalog) -> CatalogResult<T> + Send + 'static,
    {
        self.submit(Work::Operation(Box::new(job)), None).await
    }

    /// Submit an operation job with a service-owned completion hook.
    pub async fn operation_with_completion<T, F, C>(
        self,
        job: F,
        completion: C,
    ) -> Result<T, CatalogExecError>
    where
        T: Send + 'static,
        F: FnOnce(&Catalog) -> CatalogResult<T> + Send + 'static,
        C: CatalogCompletion,
    {
        self.submit(Work::Operation(Box::new(job)), Some(Box::new(completion)))
            .await
    }

    /// Submit a connection job with a service-owned completion hook.
    pub async fn execute_with_completion<T, F, C>(
        self,
        job: F,
        completion: C,
    ) -> Result<T, CatalogExecError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> CatalogResult<T> + Send + 'static,
        C: CatalogCompletion,
    {
        self.submit(Work::Connection(Box::new(job)), Some(Box::new(completion)))
            .await
    }

    /// Submit a transaction job with a service-owned completion hook.
    pub async fn transaction_with_completion<T, F, C>(
        self,
        job: F,
        completion: C,
    ) -> Result<T, CatalogExecError>
    where
        T: Send + 'static,
        F: FnOnce(&Transaction<'_>) -> CatalogResult<T> + Send + 'static,
        C: CatalogCompletion,
    {
        self.submit(Work::Transaction(Box::new(job)), Some(Box::new(completion)))
            .await
    }

    async fn submit<T>(
        mut self,
        work: Work<T>,
        completion: Option<Box<dyn CatalogCompletion>>,
    ) -> Result<T, CatalogExecError>
    where
        T: Send + 'static,
    {
        let catalog = self.catalog.clone();
        let input_bytes = self.input_bytes;
        let reserved_at = self.reserved_at;

        // Waiting for an executing slot is still the `Queued` state: nothing
        // has been dispatched.  The reservation stays armed across this await
        // so a caller cancelled here — dropping this future — releases its
        // permits and its share of the counters through `Drop`, and runs no
        // SQL.
        let executing = match catalog.execution.executing.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => {
                catalog.finish(&CatalogOutcome::Rejected);
                if let Some(completion) = completion {
                    // The hook may need the connection, which an executing
                    // job still owns during shutdown, so it settles on a
                    // blocking thread rather than on this runtime worker.
                    let catalog = catalog.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        run_completion(&catalog, completion, CatalogOutcome::Rejected);
                    })
                    .await;
                }
                return Err(CatalogExecError::ShuttingDown);
            }
        };

        // Past this point there is no await before dispatch, so ownership can
        // move from the reservation to the service.  Count the request as
        // executing before it stops counting as queued, so a concurrent
        // shutdown drain never sees an accepted request in neither state.
        catalog
            .execution
            .counters
            .executing
            .fetch_add(1, Ordering::Relaxed);
        let admission = self.admission.take();
        let bytes = self.bytes.take();
        catalog
            .execution
            .counters
            .queued
            .fetch_sub(1, Ordering::Relaxed);

        // From here the service owns the request.  Dropping the join handle
        // does not cancel a blocking task, which is exactly the contract: a
        // caller that goes away cannot interrupt or roll back SQL, and the
        // job is never re-run because a response channel closed.
        let handle = tokio::task::spawn_blocking(move || {
            let queue_wait = reserved_at.elapsed().as_micros() as u64;
            let counters = &catalog.execution.counters;
            counters
                .queue_wait_micros_total
                .fetch_add(queue_wait, Ordering::Relaxed);
            record_max(&counters.queue_wait_micros_max, queue_wait);

            let outcome_and_result = if catalog.execution.shutting_down.load(Ordering::Acquire) {
                // Dispatched but not started when shutdown began: reject it
                // without running any SQL.
                (None, Err(CatalogExecError::ShuttingDown))
            } else {
                let started = Instant::now();
                let result = std::panic::catch_unwind(AssertUnwindSafe(|| work.run(&catalog)));
                let elapsed = started.elapsed().as_micros() as u64;
                counters
                    .execution_micros_total
                    .fetch_add(elapsed, Ordering::Relaxed);
                record_max(&counters.execution_micros_max, elapsed);
                match result {
                    Ok(Ok(value)) => (Some(Ok(value)), Ok(())),
                    Ok(Err(err)) => (Some(Err(err)), Ok(())),
                    Err(_) => (None, Err(CatalogExecError::Panicked)),
                }
            };

            let (value, rejected) = outcome_and_result;
            let outcome = match (&value, &rejected) {
                (Some(Ok(_)), _) => CatalogOutcome::Committed,
                (Some(Err(err)), _) => CatalogOutcome::Failed(err),
                (None, Err(CatalogExecError::Panicked)) => CatalogOutcome::Panicked,
                (None, _) => CatalogOutcome::Rejected,
            };
            catalog.finish(&outcome);
            if let Some(completion) = completion {
                run_completion(&catalog, completion, outcome);
            }
            catalog
                .execution
                .counters
                .executing
                .fetch_sub(1, Ordering::Relaxed);
            Self::release(&catalog, input_bytes, admission, bytes);
            drop(executing);
            catalog.execution.idle.notify_waiters();
            match value {
                Some(Ok(value)) => Ok(value),
                Some(Err(err)) => Err(CatalogExecError::Catalog(err)),
                None => Err(rejected.err().unwrap_or(CatalogExecError::Panicked)),
            }
        });

        match handle.await {
            Ok(result) => result,
            // The blocking closure catches job panics itself, so a join error
            // here means the runtime is going away.
            Err(_) => Err(CatalogExecError::ShuttingDown),
        }
    }

    fn release(
        catalog: &Arc<Catalog>,
        input_bytes: usize,
        admission: Option<OwnedSemaphorePermit>,
        bytes: Option<OwnedSemaphorePermit>,
    ) {
        catalog
            .execution
            .counters
            .queued_bytes
            .fetch_sub(input_bytes, Ordering::Relaxed);
        drop(bytes);
        drop(admission);
    }
}

/// A completion hook is service-owned cleanup: a panic in one must not take
/// down the blocking worker or the connection with it.
fn run_completion(
    catalog: &Arc<Catalog>,
    completion: Box<dyn CatalogCompletion>,
    outcome: CatalogOutcome<'_>,
) {
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        if let Ok(mut guard) = catalog.lock_connection() {
            completion.complete(outcome, &mut guard);
        }
    }));
}

type ConnectionJob<T> = Box<dyn FnOnce(&mut Connection) -> CatalogResult<T> + Send>;
type TransactionJob<T> = Box<dyn for<'a> FnOnce(&Transaction<'a>) -> CatalogResult<T> + Send>;
type OperationJob<T> = Box<dyn FnOnce(&Catalog) -> CatalogResult<T> + Send>;

enum Work<T> {
    Connection(ConnectionJob<T>),
    Transaction(TransactionJob<T>),
    Operation(OperationJob<T>),
}

impl<T> Work<T> {
    fn run(self, catalog: &Catalog) -> CatalogResult<T> {
        match self {
            Self::Connection(job) => catalog.with_connection(job),
            Self::Transaction(job) => catalog.immediate(job),
            Self::Operation(job) => job(catalog),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::catalog::tests::{account, document};
    use rusqlite::OptionalExtension;
    use std::sync::mpsc;

    fn catalog() -> Arc<Catalog> {
        let catalog = Arc::new(Catalog::open_in_memory().unwrap());
        catalog.upsert_account(&account()).unwrap();
        catalog.create_document(&document()).unwrap();
        catalog
    }

    /// Reduce the executing budget to one permit for the rest of the test.
    /// Two jobs cannot both sit inside SQL anyway — the connection serialises
    /// them — so a one-slot budget is what makes "queued but not started"
    /// observable without a race.
    fn narrow_executing_budget(catalog: &Arc<Catalog>) {
        for _ in 1..MAX_EXECUTING {
            catalog
                .execution
                .executing
                .clone()
                .try_acquire_owned()
                .unwrap()
                .forget();
        }
    }

    fn pending_reservation(catalog: &Catalog, storage_id: &str) -> Option<i64> {
        catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT pending_bytes FROM room_edit_reservations WHERE storage_id=?1",
                        [storage_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(CatalogError::from)
            })
            .unwrap()
    }

    // A job that parks on a channel stands in for a slow query or a SQLite
    // busy wait: the failure this boundary exists to prevent is that wait
    // happening on a Tokio worker.
    #[tokio::test]
    async fn blocked_sql_does_not_stop_the_runtime() {
        let catalog = catalog();
        let (release, blocked) = mpsc::channel::<()>();
        let ticks = Arc::new(AtomicUsize::new(0));
        let heartbeat = {
            let ticks = ticks.clone();
            tokio::spawn(async move {
                for _ in 0..100 {
                    ticks.fetch_add(1, Ordering::Relaxed);
                    tokio::task::yield_now().await;
                }
                // Only once the runtime has demonstrably made progress does
                // the blocked job get to finish.
                release.send(()).unwrap();
            })
        };
        let value = catalog
            .execute(0, move |connection| {
                blocked.recv().unwrap();
                connection
                    .query_row("SELECT 7", [], |row| row.get::<_, i64>(0))
                    .map_err(CatalogError::from)
            })
            .await
            .unwrap();
        heartbeat.await.unwrap();
        assert_eq!(value, 7);
        assert_eq!(ticks.load(Ordering::Relaxed), 100);
        let snapshot = catalog.execution_snapshot();
        assert_eq!(snapshot.completed, 1);
        assert_eq!(snapshot.queued, 0);
        assert_eq!(snapshot.executing, 0);
        assert_eq!(snapshot.queued_bytes, 0);
    }

    #[tokio::test]
    async fn saturation_is_typed_and_budgets_stay_finite() {
        let catalog = catalog();
        assert!(matches!(
            catalog.reserve_execution(MAX_REQUEST_BYTES + 1).await,
            Err(CatalogExecError::TooLarge { .. })
        ));

        // Hold every request permit in the Preparing state.
        let mut held = Vec::new();
        for _ in 0..MAX_ADMITTED_REQUESTS {
            held.push(catalog.reserve_execution(1).await.unwrap());
        }
        let snapshot = catalog.execution_snapshot();
        assert_eq!(snapshot.queued, MAX_ADMITTED_REQUESTS);
        assert_eq!(snapshot.queued_bytes, MAX_ADMITTED_REQUESTS);
        assert_eq!(snapshot.executing, 0);
        assert_eq!(snapshot.waiting_producers, 0);

        // A large request must not park with its inputs in hand.
        assert!(matches!(
            catalog.reserve_execution(SMALL_REQUEST_BYTES + 1).await,
            Err(CatalogExecError::Saturated)
        ));

        // A small descriptor may wait, and is counted while it does.
        let waiter = {
            let catalog = catalog.clone();
            tokio::spawn(async move { catalog.reserve_execution(8).await.map(|_| ()) })
        };
        tokio::task::yield_now().await;
        assert_eq!(catalog.execution_snapshot().waiting_producers, 1);
        held.pop();
        assert!(waiter.await.unwrap().is_ok());

        let snapshot = catalog.execution_snapshot();
        assert_eq!(snapshot.waiting_producers, 0);
        assert!(snapshot.saturated >= 1);
        drop(held);
        let snapshot = catalog.execution_snapshot();
        assert_eq!(snapshot.queued, 0);
        assert_eq!(snapshot.queued_bytes, 0);
    }

    #[tokio::test]
    async fn cancellation_before_dispatch_releases_permits_and_runs_no_sql() {
        let catalog = catalog();
        let reservation = catalog.reserve_execution(64).await.unwrap();
        assert_eq!(catalog.execution_snapshot().queued, 1);
        assert_eq!(catalog.execution_snapshot().queued_bytes, 64);
        drop(reservation);
        let snapshot = catalog.execution_snapshot();
        assert_eq!(snapshot.queued, 0);
        assert_eq!(snapshot.queued_bytes, 0);
        assert_eq!(snapshot.admitted, 1);
        assert_eq!(snapshot.settled(), 0);

        // The same holds for a caller dropped while queued behind a full
        // executing budget: it never reaches SQL.
        narrow_executing_budget(&catalog);
        let (started, mut running) = tokio::sync::mpsc::unbounded_channel::<()>();
        let (release, blocked) = mpsc::channel::<()>();
        let occupied = {
            let catalog = catalog.clone();
            tokio::spawn(async move {
                catalog
                    .execute(0, move |_| {
                        started.send(()).unwrap();
                        blocked.recv().unwrap();
                        Ok(())
                    })
                    .await
            })
        };
        running.recv().await.unwrap();

        let cancelled = {
            let catalog = catalog.clone();
            tokio::spawn(async move {
                catalog
                    .execute(0, |_| -> CatalogResult<()> {
                        unreachable!("a cancelled request must not reach SQL")
                    })
                    .await
            })
        };
        tokio::task::yield_now().await;
        assert_eq!(catalog.execution_snapshot().queued, 1);
        cancelled.abort();
        let _ = cancelled.await;
        assert_eq!(catalog.execution_snapshot().queued, 0);

        release.send(()).unwrap();
        occupied.await.unwrap().unwrap();
        let snapshot = catalog.execution_snapshot();
        assert_eq!(snapshot.completed, 1);
        assert_eq!(snapshot.queued_bytes, 0);
        assert_eq!(snapshot.executing, 0);
    }

    #[tokio::test]
    async fn cancellation_during_sql_neither_interrupts_nor_rolls_back() {
        let catalog = catalog();
        let (started, mut running) = tokio::sync::mpsc::unbounded_channel::<()>();
        let (release, blocked) = mpsc::channel::<()>();
        let (completions, mut completed) = tokio::sync::mpsc::unbounded_channel();

        let caller = {
            let catalog = catalog.clone();
            tokio::spawn(async move {
                let reservation = catalog.reserve_execution(0).await.unwrap();
                reservation
                    .transaction_with_completion(
                        move |tx| {
                            started.send(()).unwrap();
                            blocked.recv().unwrap();
                            tx.execute(
                                "INSERT INTO room_edit_reservations(storage_id,pending_bytes) \
                                 VALUES('storage-1',11)",
                                [],
                            )
                            .map_err(CatalogError::from)?;
                            Ok(())
                        },
                        move |outcome: CatalogOutcome<'_>, _: &mut Connection| {
                            completions
                                .send(matches!(outcome, CatalogOutcome::Committed))
                                .unwrap();
                        },
                    )
                    .await
            })
        };

        running.recv().await.unwrap();
        // The caller disappears while SQL owns the transaction.
        caller.abort();
        release.send(()).unwrap();

        // The service still commits and still runs its completion hook.
        assert!(completed.recv().await.unwrap());
        assert_eq!(pending_reservation(&catalog, "storage-1"), Some(11));
        let snapshot = catalog.execution_snapshot();
        assert_eq!(snapshot.completed, 1);
        assert_eq!(snapshot.queued, 0);
        assert_eq!(snapshot.executing, 0);
        assert_eq!(snapshot.queued_bytes, 0);
        assert!(snapshot.execution_micros_total >= snapshot.execution_micros_max);
    }

    #[tokio::test]
    async fn cancellation_before_completion_still_settles_the_hook() {
        let catalog = catalog();
        let (committed, mut after_commit) = tokio::sync::mpsc::unbounded_channel::<()>();
        let (release_hook, hook_blocked) = mpsc::channel::<()>();
        let (completions, mut completed) = tokio::sync::mpsc::unbounded_channel();

        let caller = {
            let catalog = catalog.clone();
            tokio::spawn(async move {
                catalog
                    .reserve_execution(0)
                    .await
                    .unwrap()
                    .transaction_with_completion(
                        move |tx| {
                            tx.execute(
                                "INSERT INTO room_edit_reservations(storage_id,pending_bytes) \
                                 VALUES('storage-1',5)",
                                [],
                            )
                            .map_err(CatalogError::from)?;
                            committed.send(()).unwrap();
                            Ok(())
                        },
                        move |outcome: CatalogOutcome<'_>, connection: &mut Connection| {
                            hook_blocked.recv().unwrap();
                            // Reconciliation shape: the hook owns releasing
                            // exactly the reservation this request created.
                            connection
                                .execute(
                                    "DELETE FROM room_edit_reservations WHERE storage_id=?1",
                                    ["storage-1"],
                                )
                                .unwrap();
                            completions
                                .send(matches!(outcome, CatalogOutcome::Committed))
                                .unwrap();
                        },
                    )
                    .await
            })
        };

        after_commit.recv().await.unwrap();
        // Cancelled after commit and before the completion hook has run.
        caller.abort();
        release_hook.send(()).unwrap();
        assert!(completed.recv().await.unwrap());
        assert_eq!(pending_reservation(&catalog, "storage-1"), None);
        let snapshot = catalog.execution_snapshot();
        assert_eq!(snapshot.completed, 1);
        assert_eq!(snapshot.executing, 0);
        assert_eq!(snapshot.queued_bytes, 0);
    }

    #[tokio::test]
    async fn shutdown_settles_executing_work_and_rejects_the_queue() {
        let catalog = catalog();
        catalog.reserve_room_edit("doc", 1, -1, -1).unwrap();
        let (started, mut running) = tokio::sync::mpsc::unbounded_channel::<()>();

        // Fill the executing budget, so the next request is genuinely queued.
        narrow_executing_budget(&catalog);
        let (release, blocked) = mpsc::channel::<()>();
        let executing = {
            let catalog = catalog.clone();
            tokio::spawn(async move {
                catalog
                    .execute(0, move |connection| {
                        started.send(()).unwrap();
                        blocked.recv().unwrap();
                        connection
                            .execute(
                                "UPDATE room_edit_reservations SET pending_bytes=pending_bytes+1 \
                                 WHERE storage_id='storage-1'",
                                [],
                            )
                            .map_err(CatalogError::from)
                    })
                    .await
            })
        };
        running.recv().await.unwrap();

        // A request holding admission but not yet dispatched.
        let queued = catalog.reserve_execution(0).await.unwrap();
        let queued = {
            let (rejections, mut rejected) = tokio::sync::mpsc::unbounded_channel();
            tokio::spawn(async move {
                let outcome = queued
                    .execute_with_completion(
                        |_: &mut Connection| Ok(()),
                        move |outcome: CatalogOutcome<'_>, _: &mut Connection| {
                            rejections
                                .send(matches!(outcome, CatalogOutcome::Rejected))
                                .unwrap();
                        },
                    )
                    .await;
                (outcome, rejected.recv().await)
            })
        };

        tokio::task::yield_now().await;
        assert_eq!(catalog.execution_snapshot().queued, 1);

        let shutdown = {
            let catalog = catalog.clone();
            tokio::spawn(async move { catalog.shutdown().await })
        };
        // Shutdown cannot finish while a transaction owns the connection.
        tokio::task::yield_now().await;
        assert!(!shutdown.is_finished());

        release.send(()).unwrap();
        assert_eq!(executing.await.unwrap().unwrap(), 1);

        // Queued but not started: rejected with a typed error, and its
        // service-owned completion still runs.
        let (queued_result, hook) = queued.await.unwrap();
        assert!(matches!(queued_result, Err(CatalogExecError::ShuttingDown)));
        assert_eq!(hook, Some(true));

        shutdown.await.unwrap();

        // Admission is closed and SQLite is closed behind it.
        assert!(matches!(
            catalog.reserve_execution(0).await,
            Err(CatalogExecError::ShuttingDown)
        ));
        assert!(matches!(
            catalog.schema_version(),
            Err(CatalogError::Closed)
        ));
        let snapshot = catalog.execution_snapshot();
        assert_eq!(snapshot.completed, 1);
        assert_eq!(snapshot.rejected, 1);
        assert_eq!(snapshot.queued, 0);
        assert_eq!(snapshot.executing, 0);
    }

    // A worker failure must not be a catalogue failure: the connection owns
    // the process-local TEMP reservation table, and losing it would silently
    // drop live edit quota.
    #[tokio::test]
    async fn a_job_panic_leaves_the_connection_and_reservations_usable() {
        let catalog = catalog();
        catalog.reserve_room_edit("doc", 42, -1, -1).unwrap();
        assert_eq!(pending_reservation(&catalog, "storage-1"), Some(42));

        let panicked = catalog
            .execute::<(), _>(0, |_| panic!("job panicked deliberately"))
            .await;
        assert!(matches!(panicked, Err(CatalogExecError::Panicked)));

        // A transaction that panics mid-way rolls back through rusqlite's
        // own guard rather than leaving partial state behind.
        let in_transaction = catalog
            .execute_transaction::<(), _>(0, |tx| {
                tx.execute(
                    "INSERT INTO room_edit_reservations(storage_id,pending_bytes) \
                     VALUES('storage-2',9)",
                    [],
                )
                .map_err(CatalogError::from)?;
                panic!("transaction panicked deliberately");
            })
            .await;
        assert!(matches!(in_transaction, Err(CatalogExecError::Panicked)));
        assert_eq!(pending_reservation(&catalog, "storage-2"), None);

        // The reservation survives, synchronously and asynchronously.
        assert_eq!(pending_reservation(&catalog, "storage-1"), Some(42));
        let seen = catalog
            .execute(0, |connection| {
                connection
                    .query_row(
                        "SELECT pending_bytes FROM room_edit_reservations WHERE storage_id=?1",
                        ["storage-1"],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(CatalogError::from)
            })
            .await
            .unwrap();
        assert_eq!(seen, 42);
        assert_eq!(catalog.reserve_room_edit("doc", 43, -1, -1).unwrap(), 42);

        let snapshot = catalog.execution_snapshot();
        assert_eq!(snapshot.panicked, 2);
        assert_eq!(snapshot.completed, 1);
        assert_eq!(snapshot.queued_bytes, 0);
    }

    #[tokio::test]
    async fn a_failed_transaction_reports_its_error_to_the_hook() {
        let catalog = catalog();
        let (completions, mut completed) = tokio::sync::mpsc::unbounded_channel();
        let result = catalog
            .reserve_execution(0)
            .await
            .unwrap()
            .transaction_with_completion(
                |_: &Transaction<'_>| Err::<(), _>(CatalogError::Conflict("no".into())),
                move |outcome: CatalogOutcome<'_>, _: &mut Connection| {
                    completions
                        .send(matches!(outcome, CatalogOutcome::Failed(_)))
                        .unwrap();
                },
            )
            .await;
        assert!(matches!(
            result,
            Err(CatalogExecError::Catalog(CatalogError::Conflict(_)))
        ));
        assert!(completed.recv().await.unwrap());
        assert_eq!(catalog.execution_snapshot().failed, 1);
    }
    // The migrated storage callers, exercised through the boundary they now
    // use. These are about the callers, not the machinery above: what they
    // assert is that a caller that used to take the connection on a runtime
    // worker no longer does, and that shutdown settles its work in the right
    // order.

    /// A journal publication parked inside SQL must not stop the runtime.
    /// This is the acceptance criterion applied to a real caller: the ticks
    /// are counted before the blocked job is released, so the assertion is on
    /// progress, not on elapsed time.
    #[tokio::test]
    async fn a_blocked_journal_job_does_not_stop_unrelated_tasks() {
        let catalog = catalog();
        let store = crate::storage::journal::JournalStore::new(catalog.clone());
        store.initialize("deployment", "generation").unwrap();
        let (release, blocked) = mpsc::channel::<()>();
        let holder = {
            let catalog = catalog.clone();
            tokio::spawn(async move {
                catalog
                    .execute(0, move |connection| {
                        let tx = connection
                            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                            .map_err(CatalogError::from)?;
                        let _ = blocked.recv();
                        tx.rollback().map_err(CatalogError::from)?;
                        Ok(())
                    })
                    .await
                    .unwrap();
            })
        };
        // A second caller that needs the same connection. It waits on a
        // blocking thread, not on this runtime.
        let waiting = {
            let store = crate::storage::journal::JournalStore::new(catalog.clone());
            tokio::spawn(async move { store.state_async().await })
        };
        let ticks = Arc::new(AtomicUsize::new(0));
        {
            let ticks = ticks.clone();
            tokio::spawn(async move {
                for _ in 0..100 {
                    ticks.fetch_add(1, Ordering::Relaxed);
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        }
        assert_eq!(ticks.load(Ordering::Relaxed), 100);
        release.send(()).unwrap();
        holder.await.unwrap();
        assert_eq!(
            waiting.await.unwrap().unwrap().writer_generation,
            "generation"
        );
    }

    /// Shutdown while journal work is queued and executing: the executing
    /// transaction keeps the connection until it commits, the queued request
    /// is rejected before any SQL runs, and SQLite closes only afterwards.
    #[tokio::test]
    async fn shutdown_settles_executing_journal_work_and_rejects_the_queue() {
        let catalog = catalog();
        let store = crate::storage::journal::JournalStore::new(catalog.clone());
        store.initialize("deployment", "generation").unwrap();
        narrow_executing_budget(&catalog);
        let (release, blocked) = mpsc::channel::<()>();
        let (started, executing) = tokio::sync::oneshot::channel();
        let committing = {
            let catalog = catalog.clone();
            tokio::spawn(async move {
                catalog
                    .execute(0, move |connection| {
                        let _ = started.send(());
                        let _ = blocked.recv();
                        connection
                            .execute(
                                "UPDATE journal_state SET last_operation_id=?1 WHERE id=1",
                                ["settled"],
                            )
                            .map_err(CatalogError::from)?;
                        Ok(())
                    })
                    .await
            })
        };
        executing.await.unwrap();
        // Queued behind the one executing permit, so shutdown reaches it
        // before it starts.
        let queued = {
            let store = crate::storage::journal::JournalStore::new(catalog.clone());
            tokio::spawn(async move {
                store
                    .retire_storage_async("storage-1".to_string(), 10)
                    .await
            })
        };
        tokio::task::yield_now().await;
        let shutdown = {
            let catalog = catalog.clone();
            tokio::spawn(async move { catalog.shutdown().await })
        };
        // The drain cannot finish while the transaction holds the connection.
        tokio::task::yield_now().await;
        assert!(!shutdown.is_finished());
        release.send(()).unwrap();
        committing.await.unwrap().unwrap();
        assert!(matches!(
            queued.await.unwrap(),
            Err(crate::storage::journal::JournalError::Catalog(
                CatalogError::Closed
            ))
        ));
        shutdown.await.unwrap();
        // The executing work committed before SQLite closed, and every path
        // now reports closure.
        assert!(matches!(store.state(), Err(_)));
        assert!(matches!(
            catalog.reserve_execution(0).await,
            Err(CatalogExecError::ShuttingDown)
        ));
    }
}
