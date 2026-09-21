//! The log: what the server actually is.
//!
//! "The server stores and forwards source bytes. It reads only their headers
//! to do so. It interprets their contents only on demand, and whatever it
//! computes from them is a cache that can be thrown away at any moment and
//! rebuilt from the log." -- SPEC-server-is-a-log §1.
//!
//! Five pieces, and the boundaries between them are the specification's own:
//!
//! * [`frame`] is how a flushed row carries the batches that went into it
//!   (§4.2.1). It knows nothing about Loro.
//! * [`budget`] is the one process-wide memory budget every decoded document
//!   is admitted against (§9.2). It knows nothing about documents.
//! * [`admission`] is the other half of that bound: how many decoded
//!   documents may be worked on at once (§9.3). Memory and CPU run out
//!   separately, so they are counted separately.
//! * [`sequencer`] owns one document's buffer, counter, subscribers and cache
//!   entry, and is the only thing that decides an order (§4.1, §5, §6, §7).
//! * [`Registry`] holds the resident sequencers and evicts them.
//!
//! Compaction (§8.4) and the in-process background worker (§8.6) live in
//! [`crate::storage::worker`], beside the blob store they write to.

pub mod admission;
pub mod budget;
pub mod frame;
#[cfg(test)]
mod recovery;
pub mod sequencer;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::Arc;

use uuid::Uuid;

pub use budget::{Budget, Busy};
pub use frame::Batch;
pub use sequencer::{
    Command, CommandError, Evidence, FlushReason, Head, Ingested, Joined, LogState, PreparedSource,
    Role, Sequencer, SequencerError,
};

use crate::config::Configuration;
use crate::storage::blob::BlobStore;
use crate::storage::postgres::PostgresCatalog;

/// The name server-authored source is written under. Stable for the life of
/// the deployment and kept in `server_runtime_state` (§7.3), so a label
/// written by one process and read by the next names one author.
pub const DEPLOYMENT_PEER_STATE: &str = "deployment.peer";

/// Every resident sequencer.
///
/// Residency here is cheap: a sequencer is a counter, a vector and a buffer,
/// and holds a decoded document only if something asked it to. So the
/// registry bounds the number of sequencers generously and lets the memory
/// budget bound the expensive half.
pub struct Registry {
    catalog: Arc<PostgresCatalog>,
    blobs: Arc<dyn BlobStore>,
    config: Arc<Configuration>,
    budget: Arc<Budget>,
    deployment_peer_key: String,
    /// The background worker every sequencer this registry admits asks for
    /// compaction (§8.4). Filled by [`Registry::compacts_through`] once the
    /// worker exists, which cannot be before this registry does: the worker
    /// is built from it. Shared by `Arc` rather than copied at admission, so
    /// a sequencer admitted during startup -- before the worker is wired --
    /// still reaches it afterwards.
    compaction: Arc<std::sync::OnceLock<crate::storage::worker::Handle>>,
    resident: tokio::sync::Mutex<HashMap<Uuid, Arc<Sequencer>>>,
    /// One slot per document currently being admitted, so a slow head read
    /// stalls only callers of the same document.
    admitting: tokio::sync::Mutex<HashMap<Uuid, Admission>>,
}

/// The slot one caller fills and the rest of that document's callers wait
/// on: the outer mutex is the queue, the `Option` is "has anybody built it
/// yet".
type Admission = Arc<tokio::sync::Mutex<Option<Arc<Sequencer>>>>;

impl Registry {
    pub fn new(
        catalog: Arc<PostgresCatalog>,
        blobs: Arc<dyn BlobStore>,
        config: Arc<Configuration>,
        deployment_peer_key: String,
    ) -> Arc<Self> {
        let budget = Budget::new(config.memory_budget_bytes, config.cache_expansion);
        let registry = Arc::new(Self {
            catalog,
            blobs,
            config,
            budget,
            deployment_peer_key,
            compaction: Arc::new(std::sync::OnceLock::new()),
            resident: tokio::sync::Mutex::new(HashMap::new()),
            admitting: tokio::sync::Mutex::new(HashMap::new()),
        });
        // The budget cannot be told this at construction, because it is
        // built first and the registry is what it will be asking. Weakly,
        // so the budget the registry owns does not keep the registry alive.
        registry
            .budget
            .evicts_through(Arc::downgrade(&registry) as std::sync::Weak<dyn budget::Evict>);
        registry
    }

    pub fn budget(&self) -> &Arc<Budget> {
        &self.budget
    }

    /// Names the worker a flush asks for compaction (§8.4). Called once, by
    /// whoever built both; a second call is ignored, because the first
    /// handle is already in the hands of every sequencer admitted since.
    pub fn compacts_through(&self, handle: crate::storage::worker::Handle) {
        let _ = self.compaction.set(handle);
    }

    /// The sequencer for this document, admitting it if it is not resident.
    pub async fn get(&self, document_id: Uuid, slug: &str) -> sequencer::Result<Arc<Sequencer>> {
        if let Some(existing) = self.resident.lock().await.get(&document_id) {
            return Ok(existing.clone());
        }
        let slot = {
            let mut admitting = self.admitting.lock().await;
            admitting
                .entry(document_id)
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(None)))
                .clone()
        };
        let mut admitted = slot.lock().await;
        if let Some(existing) = admitted.as_ref() {
            return Ok(existing.clone());
        }
        if let Some(existing) = self.resident.lock().await.get(&document_id) {
            *admitted = Some(existing.clone());
            return Ok(existing.clone());
        }
        let sequencer = Arc::new(
            Sequencer::admit(
                document_id,
                slug.to_string(),
                self.catalog.clone(),
                self.blobs.clone(),
                self.config.clone(),
                self.budget.clone(),
                self.deployment_peer_key.clone(),
                self.compaction.clone(),
            )
            .await?,
        );
        self.resident
            .lock()
            .await
            .insert(document_id, sequencer.clone());
        *admitted = Some(sequencer.clone());
        self.admitting.lock().await.remove(&document_id);
        Ok(sequencer)
    }

    /// The sequencer for this document if it is already resident, and no
    /// admission if it is not. What a sweep and a cost report ask.
    pub async fn resident(&self, document_id: Uuid) -> Option<Arc<Sequencer>> {
        self.resident.lock().await.get(&document_id).cloned()
    }

    pub async fn all(&self) -> Vec<Arc<Sequencer>> {
        self.resident.lock().await.values().cloned().collect()
    }

    /// Flushes what is due and retires what nobody is using.
    ///
    /// This is the whole of the periodic work: there is no room sweep, no
    /// per-socket tick and no job poll. A deployment with nothing open issues
    /// no queries beyond the lease connection, which is what §14.2's last
    /// test holds.
    pub async fn housekeep(&self) {
        use futures_util::stream::StreamExt as _;

        let resident = self.all().await;
        // Which documents owe a row, decided before any of them is written.
        // `flush_due` reads in-memory state, though its lock may wait for
        // another operation on that document.
        let mut due = Vec::new();
        for sequencer in &resident {
            if let Some(reason) = sequencer.flush_due().await {
                due.push((sequencer.clone(), reason));
            }
        }
        // Then write them concurrently, up to what the pool can serve.
        //
        // This pass used to write one row at a time for the whole
        // deployment, which made one document's durable acknowledgement wait
        // on every other document that happened to be due in the same
        // second: a deployment-wide serialization between documents that
        // share nothing, and the first thing that saturates as the number of
        // simultaneously edited documents grows. Per-document ordering is
        // untouched -- each sequencer still takes its own transaction gate,
        // and one pass asks each sequencer at most once. The bound limits
        // this sweep's demand; other pool consumers can still cause waits
        // or acquisition timeouts. See docs/postgres-capacity.md.
        let concurrency = self.catalog.flush_concurrency();
        futures_util::stream::iter(due)
            .for_each_concurrent(concurrency, |(sequencer, reason)| async move {
                if let Err(error) = sequencer.flush(reason).await {
                    ::log::warn!("could not flush {}: {error}", sequencer.slug);
                }
            })
            .await;
        // A compaction ask the worker queue dropped has nothing else to fire
        // it again once editing stops, so the sweep re-asks. Rate limited
        // inside, and no query either way, so it stays on this task rather
        // than joining the concurrent half.
        for sequencer in &resident {
            sequencer.recheck_compaction().await;
        }
        self.retire_fenced().await;
        self.retire_idle().await;
    }

    /// §10: a sequencer that has lost the writer lease already refuses
    /// ingest and commands and has closed its own sockets (`Sequencer::
    /// close_fenced`, reached from `flush`). What is left to do here is drop
    /// it from `resident`, so the next `get` for this document does not
    /// return the same stale instance and instead re-admits from storage --
    /// which, for the process that now actually holds the lease, reads a log
    /// this process could not write to any more.
    ///
    /// Checked every housekeeping pass rather than only right after a flush,
    /// because a sequencer can also be fenced by compaction's own flush
    /// (`storage::worker::compact` calls `Sequencer::flush` directly, before
    /// this registry ever sees the error).
    async fn retire_fenced(&self) {
        let mut fenced = Vec::new();
        for sequencer in self.all().await {
            if sequencer.fenced().await.is_some() {
                fenced.push(sequencer.document_id);
            }
        }
        if fenced.is_empty() {
            return;
        }
        let mut resident = self.resident.lock().await;
        for document_id in fenced {
            resident.remove(&document_id);
        }
    }

    /// Drops sequencers nobody is subscribed to and whose buffer is empty.
    /// A dropped sequencer costs one head read to admit again.
    async fn retire_idle(&self) {
        let mut idle = Vec::new();
        for sequencer in self.all().await {
            let state = sequencer.log_state().await;
            if state.subscribers == 0 && state.buffered == 0 {
                idle.push(sequencer.document_id);
            }
        }
        if idle.is_empty() {
            return;
        }
        let mut resident = self.resident.lock().await;
        for document_id in idle {
            // Re-check under the lock: somebody may have joined since.
            let still_idle = match resident.get(&document_id) {
                Some(sequencer) => Arc::strong_count(sequencer) == 1,
                None => false,
            };
            if still_idle {
                resident.remove(&document_id);
            }
        }
    }

    /// Flushes every buffer and ends every sequencer. Shutdown, and nothing
    /// else: the crash contract of §6.4 covers the case where this does not
    /// run.
    pub async fn shutdown(&self) {
        for sequencer in self.all().await {
            if let Err(error) = sequencer.flush(FlushReason::Shutdown).await {
                ::log::warn!("could not flush {} at shutdown: {error}", sequencer.slug);
            }
        }
        self.resident.lock().await.clear();
    }

    /// Frees memory by dropping the coldest cache entries, which is what
    /// §9.2 does when a reservation cannot be met. Reached through
    /// [`budget::Evict`], from inside `Budget::reserve`. Entries with subscribers
    /// are fair game: under the opaque log nothing depends on an entry
    /// staying resident.
    /// Nothing here may AWAIT a sequencer's lock. This runs from inside
    /// `Budget::reserve`, which runs from `build`, which its callers run
    /// with that lock already held -- so an await would deadlock the task
    /// that asked for the memory against itself. Every step uses
    /// `try_lock` and skips what is busy, which is also the right policy:
    /// a sequencer somebody is using is not a cold entry.
    pub async fn evict_caches(&self, wanted: u64) -> u64 {
        let mut candidates: Vec<(usize, Arc<Sequencer>)> = Vec::new();
        for sequencer in self.all().await {
            if let Some(subscribers) = sequencer.try_subscribers() {
                candidates.push((subscribers, sequencer));
            }
        }
        // Cold first: an entry nobody is subscribed to is the cheapest to
        // lose, and the one most likely not to be wanted again at once.
        candidates.sort_by_key(|(subscribers, _)| *subscribers);
        let mut freed: u64 = 0;
        for (_, sequencer) in candidates {
            let Some(log_bytes) = sequencer.try_evict() else {
                continue;
            };
            freed = freed.saturating_add(budget::estimate(log_bytes, self.budget.expansion()));
            // Counted one by one here rather than once per pass, because
            // what an operator needs to see is entries lost, not passes run:
            // a deployment where every read evicts somebody else's cache
            // shows it in this number and nowhere else (§9.2).
            self.budget.note_evicted(1);
            if freed >= wanted {
                break;
            }
        }
        freed
    }
}

impl budget::Evict for Registry {
    fn evict(&self, wanted: u64) -> futures_util::future::BoxFuture<'_, u64> {
        Box::pin(self.evict_caches(wanted))
    }
}
