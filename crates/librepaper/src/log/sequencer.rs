//! The sequencer: one per resident document, and the only thing that decides
//! the order of anything.
//!
//! It owns the buffer, the sequence counter, the log vector, the subscriber
//! list and -- when there is one -- the cache entry (SPEC-server-is-a-log
//! §4.1). Everything else asks it.
//!
//! ## Why a lock and not a mailbox
//!
//! §4.1 describes an actor with a message queue. What the rest of the
//! specification actually depends on is narrower, and is stated in §7 step 5:
//! while a semantic command runs, "the sequencer processes no other message".
//! A fair `tokio::sync::Mutex` held across a command gives exactly that
//! ordering, in FIFO, and lets every caller keep its own return type instead
//! of routing it through a reply channel and a universal message enum.
//!
//! The one place where holding the lock would be wrong is an ordinary flush:
//! §10 requires relay to continue while PostgreSQL is unavailable. So a flush
//! snapshots the buffer under the lock, releases it for the round trip, and
//! takes it again to retire what committed. That snapshot is §4.1's "flush in
//! progress", and the count of batches it covers is the whole of its state.
//!
//! ## What it never does
//!
//! It never decides whether an update is allowed by its content. Ingest is a
//! header decode, a gap check, an append and a relay (§5). Meaning is read
//! out of the cache, on demand, and the cache can be thrown away at any
//! moment and rebuilt from the log.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::future::BoxFuture;
use loro::{ExportMode, Frontiers, LoroDoc, VersionVector};
use serde_json::json;
use uuid::Uuid;

use super::budget::{estimate, Budget, Busy, Reservation};
use super::frame;
use crate::config::Configuration;
use crate::room::outgoing::{Outgoing, Sender};
use crate::storage::blob::BlobStore;
use crate::storage::postgres::{self, Authority, FlushRow, PostgresCatalog};

/// The maximum size of one update as it arrives, derived from the log quota.
/// One update on the wire may be as large as the log quota, since anything
/// larger could never be admitted. The two expansion factors in budget.rs
/// are measurements applied to the log bytes, not bounds of their own.
pub fn max_update_bytes(log_quota_bytes: usize) -> usize {
    log_quota_bytes
}

/// The most one document's pending charge can come to.
///
/// [`BUFFER_CEILING_BYTES`] is the ordinary ceiling, but an empty buffer
/// always admits one update whatever it weighs -- otherwise a single
/// maximum-size update would be refused forever the moment its framing
/// pushed it one byte past the ceiling. So the true per-document maximum
/// is the larger of the two.
pub fn max_pending_charge(log_quota_bytes: usize) -> usize {
    let ceiling = BUFFER_CEILING_BYTES;
    let single = max_update_bytes(log_quota_bytes) + frame::BATCH_HEADER_BYTES + frame::MAX_PEER_KEY;
    if single > ceiling {
        single
    } else {
        ceiling
    }
}

/// The largest row an ordinary flush can write: one version byte over a
/// document's whole pending charge.
pub fn max_row_bytes(log_quota_bytes: usize) -> usize {
    frame::ROW_HEADER_BYTES + max_pending_charge(log_quota_bytes)
}

/// A buffer non-empty for this long flushes even while somebody is still
/// typing, so the log is never further behind the screen than this.
pub const FLUSH_MAX_AGE: Duration = Duration::from_secs(30);
/// No batch for this long flushes, so somebody who stops typing is durable
/// shortly afterwards rather than at the max age.
pub const FLUSH_QUIET: Duration = Duration::from_secs(5);
/// A trigger, not a cap: one large update flushes alone rather than being
/// refused for exceeding it.
pub const FLUSH_TRIGGER_BYTES: usize = 1024 * 1024;
/// With PostgreSQL unavailable the buffer grows to this before ingest is
/// refused retryably (§10).
///
/// Measured against the buffer's CHARGE -- payload plus this batch's framing
/// -- rather than payload alone, so that the row a flush will write is
/// bounded by it too. A buffer of very many very small updates is mostly
/// framing (`frame::overhead` per batch), and a ceiling that ignored it
/// bounded the payload while letting the row grow several times past it.
pub const BUFFER_CEILING_BYTES: usize = FLUSH_TRIGGER_BYTES * 4;
/// At most one `source-changed` per document per second (§4.5).
pub const SOURCE_CHANGED_INTERVAL: Duration = Duration::from_secs(1);
/// How long a build may run before the document is marked unreadable (§9.3).
pub const BUILD_DEADLINE: Duration = Duration::from_secs(10);
/// How long a caller waits for memory before it answers `busy` (§9.2).
pub const RESERVE_PATIENCE: Duration = Duration::from_millis(250);
/// How often the housekeeping sweep may re-ask for a compaction that is
/// still due. `Handle::ask` coalesces full-queue wake-ups and the worker's
/// durable rescan recovers any task that could not enter the queue. But a
/// large compaction takes far longer than a housekeeping tick, so re-asking
/// every tick would push dozens of duplicate `Task::Compact` entries at a
/// 256-slot queue. A minute is long enough that a compaction in progress has
/// normally finished or cleared the counters, and short enough that a missed
/// wake-up costs a minute rather than a restart.
pub const COMPACTION_RECHECK_INTERVAL: Duration = Duration::from_secs(60);

/// What a subscriber may do, which decides what it is sent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    /// Receives relayed batches. Blind review depends on nobody else doing.
    Editor,
    /// Receives `source-changed {digest}` and nothing of the CRDT (§2.1).
    Reader,
}

/// What snapshot a compaction exports: full history or shallow state only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotMode {
    /// A full snapshot with complete history.
    Full,
    /// A shallow snapshot keeping the state at the frontier and no history before it.
    Shallow,
}

struct Subscriber {
    tx: Sender,
    role: Role,
    /// Which peer's acknowledgements this socket is owed.
    peer_key: String,
}

/// One batch in the buffer: what arrived, from whom, what it covers, and the
/// deployment-wide reservation that admitted it.
///
/// Not `Clone`: the reservation is not, on purpose, because a copy of these
/// bytes is a second allocation that owes a second charge. The charge lives
/// here rather than in a counter beside the buffer so that it follows the
/// bytes exactly -- `Inner::retire` drains a prefix and those reservations
/// drop with it, a failed flush drains nothing and they all survive, and a
/// sequencer that is dropped while something else still holds an `Arc` to it
/// keeps paying until that last reference goes.
struct Pending {
    peer_key: String,
    client_seq: i64,
    bytes: Vec<u8>,
    end: VersionVector,
    /// What this batch's payload and framing cost the deployment. Released by
    /// `Drop`, so there is no path -- error, panic, cancellation -- on which
    /// removing the batch forgets to give the bytes back.
    charge: super::pending::Reservation,
}

/// A decoded document, held for as long as the budget allows. Dropping it
/// releases the reservation, which is why the two live in one struct.
struct Cache {
    doc: LoroDoc,
    reservation: Reservation,
}

/// Why a flush was asked for. Carried so a log line can say it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlushReason {
    MaxAge,
    Quiet,
    Bytes,
    Barrier,
    LastSubscriberLeft,
    Eviction,
    Shutdown,
    /// §10: somebody's access was withdrawn while their typing was still in
    /// the buffer. Distinct from `LastSubscriberLeft` because it fires
    /// whether or not anyone else is still connected: the person whose work
    /// this is has no way back in to reconcile it.
    AuthorityRevoked,
}

/// What ingest decided.
#[derive(Clone, Debug)]
pub enum Ingested {
    /// Appended and relayed.
    Accepted,
    /// The header's start vector is not covered. The client exports from the
    /// vector returned and sends that instead (§5 step 4).
    Gap { vector: Vec<u8> },
    /// Not a well-formed Loro update, checksum included. The socket is
    /// closed and the client reconciles on reconnect.
    Invalid(String),
    /// Refused for a reason the client can retry: the log quota, a buffer
    /// that has grown because PostgreSQL is unavailable, or the deployment's
    /// pending-source ceiling.
    Retryable(Retry),
    /// Refused for a reason retrying will not change.
    Refused(&'static str),
}

/// Everything a refused-but-retryable answer has to carry for the client to
/// actually recover from it.
///
/// A string was not enough, and the gap was not theoretical: the socket
/// handler turned every refusal into one generic `error` frame, the browser
/// showed a toast, and nothing anywhere resent the update. An editor who
/// stopped typing after a single refused keystroke kept that keystroke only
/// in their own browser until they typed again or the socket happened to
/// drop. So the refusal now says, in fields rather than in prose:
///
/// * `reason`, a stable code the client can branch on without parsing
///   English, and
/// * `vector`, the head as it stands. A refusal never merges the batch into
///   `head_vector` (every path returns before that merge), so re-exporting
///   from this vector is exactly the refused work plus anything since --
///   which is the same recovery `Gap` already asks for, just later.
///
/// `pressure` separates the two kinds of retryable refusal. A rate-limit or
/// quota refusal is about this client or this document and is what
/// `MAX_CONSECUTIVE_REFUSALS` exists to stop; back pressure is about the
/// deployment, and closing sockets over it turns a slow database into a
/// reconnect storm against the same slow database.
#[derive(Clone, Debug)]
pub struct Retry {
    pub why: &'static str,
    pub reason: &'static str,
    pub pressure: bool,
    pub vector: Vec<u8>,
}

impl std::fmt::Display for Retry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.why)
    }
}

/// What a join is answered with (§6.2).
#[derive(Debug)]
pub struct Joined {
    /// The head vector: what the reply plus the buffer leaves the client at.
    pub vector: Vec<u8>,
    /// The durable log vector: what has actually reached storage. This can
    /// lag `vector` while the reply includes the in-memory buffer.
    pub durable_vector: Vec<u8>,
    /// The compaction base, when the client does not cover it. Sent by
    /// reference above the inline limit, which is the caller's decision.
    pub base: Option<Vec<u8>>,
    /// Rows the client's vector does not cover, then the buffer. Handed to
    /// the client as one `importBatch`, which is why they stay separate: a
    /// Loro update is a framed blob and two of them concatenated are not one.
    pub batches: Vec<Vec<u8>>,
    /// Whether any row was uncovered, which is what makes the reply
    /// `doc-rows` rather than `doc-state`.
    pub from_rows: bool,
}

/// What a semantic command is evaluated against (§7 step 2). Everything here
/// is the head as it stands, and nothing can move while the caller holds it:
/// the sequencer processes no other message meanwhile.
pub struct Head<'a> {
    doc: &'a LoroDoc,
    rules_owner: &'a Configuration,
    pub vector: VersionVector,
    pub frontier: Frontiers,
    pub projection: Arc<librepaper_document_core::Projected>,
    /// What `prepare` reserves a fork against, and what it is reserved
    /// against (§9.2: "temporary forks, each reserving the parent's
    /// estimate"). Computed once by `Sequencer::command` from the same
    /// `resident_estimate` a build or `with_fork_at` uses, so every path
    /// that forks the resident document is judged by the same number.
    budget: Arc<Budget>,
    estimate: u64,
}

impl Head<'_> {
    /// The document itself, for a command that has to read more than the
    /// projection: an anchor to resolve, a branch to diff.
    pub fn doc(&self) -> &LoroDoc {
        self.doc
    }

    pub fn digest(&self) -> String {
        self.projection.projection.digest()
    }

    /// §7.3: a command that produces source prepares it on a fork, so a
    /// failed transaction cannot have changed the document anybody is
    /// looking at. The edit runs against `draft`; what comes back is the
    /// batch to put in the row, with the identity it leaves behind.
    pub fn prepare(
        &self,
        client_seq: i64,
        edit: impl FnOnce(&LoroDoc) -> std::result::Result<(), String>,
    ) -> std::result::Result<Option<PreparedSource>, String> {
        // §9.2: a fork reserves the parent's estimate before it is taken,
        // the same as `with_fork_at` and `build` do. `prepare` runs
        // synchronously, with the sequencer's lock held and inside
        // `Command::evaluate`'s own synchronous signature, so this cannot
        // await the patient `Budget::reserve` the way those two do; every
        // caller in this crate converts `prepare`'s `Err(String)` straight
        // into `CommandError::Conflict` (there is nowhere else in that
        // signature to put a distinct busy variant), so failing over that
        // patience window would just make an operation that could have
        // succeeded a moment later report the wrong kind of failure to the
        // caller. `try_reserve` answers at once instead: room now, or not.
        let _reservation = self.budget.try_reserve_now(self.estimate).map_err(|_| {
            "this deployment has no memory to prepare that edit right now; retry".to_string()
        })?;
        let draft = self.doc.fork();
        edit(&draft)?;
        draft.commit();
        let batch = draft
            .export(ExportMode::Updates {
                from: std::borrow::Cow::Borrowed(&self.vector),
            })
            .map_err(|error| error.to_string())?;
        // `Updates` export is never a zero-length `Vec<u8>`, even when there
        // are no ops to carry: the format writes its header and encoding
        // envelope regardless, and that alone is dozens of bytes. `batch`
        // being non-empty says nothing about whether the fork actually
        // diverged from `self.vector`; `ImportBlobMetadata::change_num` is
        // what the format itself uses to say "this covers zero changes",
        // and that is what a no-op edit -- one whose closure decided there
        // was nothing to do -- must be judged by, or every idempotent retry
        // of a source-producing command would re-export an empty-but-non-
        // empty-bytes batch and grow the log on every replay.
        let header =
            LoroDoc::decode_import_blob_meta(&batch, true).map_err(|error| error.to_string())?;
        if header.change_num == 0 {
            return Ok(None);
        }
        let projected = librepaper_document_core::project(&draft, &self.rules_owner.paths());
        Ok(Some(PreparedSource {
            batch,
            end: header.partial_end_vv,
            after_frontier: draft.oplog_frontiers(),
            after_digest: projected.projection.digest(),
            after_projection: Arc::new(projected),
            client_seq,
            // Moved out of the local, not released at the end of `prepare`:
            // the fork's memory is `draft`, and `draft` outlives `prepare`
            // only through the export above -- but the reservation is meant
            // to cover the fork itself, which is why it is taken before
            // `self.doc.fork()` and must not be dropped before whatever
            // holds `batch`/`after_projection` (the actual resident cost
            // after import) is done with them. Held on `PreparedSource` and
            // released by its own `Drop`, whether the command later
            // succeeds or its transaction rolls back and the value is
            // simply discarded (§7 step 5).
            _reservation,
        }))
    }
}

/// Source a command produced, prepared transaction-locally (§7.3).
#[derive(Debug)]
pub struct PreparedSource {
    pub batch: Vec<u8>,
    end: VersionVector,
    pub after_frontier: Frontiers,
    pub after_digest: String,
    pub after_projection: Arc<librepaper_document_core::Projected>,
    client_seq: i64,
    /// §9.2's fork reservation, released when this is dropped (§7 step 5,
    /// success or failure alike).
    _reservation: Reservation,
}

/// What state a command's rows are written against (§7 step 4).
#[derive(Clone, Debug)]
pub struct Evidence {
    /// The row that made the head durable: the row this command's flush
    /// wrote, or the last row when the buffer was empty and nothing was
    /// produced.
    pub source_sequence: i64,
    pub vector: Vec<u8>,
    /// The head frontier this command was evaluated against.
    pub before_frontier: Vec<u8>,
    pub before_digest: String,
    /// Where the source this command produced leaves the document, when it
    /// produced any.
    pub after_frontier: Option<Vec<u8>>,
    pub after_digest: Option<String>,
}

/// Which rung of access a command needs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rung {
    /// May leave a comment, a reply or a suggestion. Also satisfied by an
    /// editor, and by anyone at all on a document whose ownership mode is
    /// `open`.
    Commenter,
    /// May change what the document says.
    Editor,
}

/// One semantic command: anything that writes relational state whose meaning
/// depends on the source (§7).
///
/// Two halves, because they run in two places. `evaluate` is synchronous
/// against the head with the sequencer locked; `transact` runs inside the
/// fenced transaction, after the flush row that made that head durable.
pub trait Command {
    type Output: Send;

    /// A name for logs and refusals.
    fn name(&self) -> &'static str;

    /// What the caller must be allowed to do, checked at the commit boundary
    /// in the same transaction as the write.
    ///
    /// It is per-command and not per-document, because the two rungs write
    /// the same tables: a commenter may leave a comment on a document they
    /// cannot edit, and the comment lands in `annotations` exactly as an
    /// editor's would. A single editor check here would refuse every
    /// commenter, and a single commenter check would let one restore a
    /// document. The default is the stricter of the two, so a command that
    /// forgets to say is refused rather than admitted.
    fn authority(&self) -> Rung {
        Rung::Editor
    }

    /// Step 0: has this command already happened?
    ///
    /// §7.2 promises that "a retry with the same `request_id` finds the row
    /// and returns its result". That cannot be checked in `transact`, which
    /// is where the `document_labels` row is written and read: by the time
    /// `transact` runs, the sequencer has already prepared a second copy of
    /// the edit and inserted the log row carrying it. The retry would return
    /// the right label and still have grown the log by a duplicate batch.
    ///
    /// So a command that writes a retry record answers here instead, before
    /// anything is prepared and before any row is written. It returns the
    /// recorded result, and the sequencer stops.
    ///
    /// The default is "no record, go ahead", which is right for every
    /// command that is idempotent by primary key instead: a create carries
    /// a client UUID and a transition is conditional on a version, and
    /// neither needs this (§7.2).
    fn replay(&mut self) -> BoxFuture<'_, std::result::Result<Option<Self::Output>, CommandError>> {
        Box::pin(async { Ok(None) })
    }

    /// Step 1a: the relational state `evaluate` needs, read under the
    /// sequencer lock.
    ///
    /// `evaluate` is synchronous (§7 step 2) and so cannot reach Postgres,
    /// which pushes any row a command needs out to its caller, where it is
    /// read before the lock is taken and can be stale by the time `transact`
    /// runs. That is a real hole for a command whose prepared source depends
    /// on rows other commands write: two decisions on the same proposal,
    /// each prepared from a snapshot taken before the other landed, both
    /// conclude there is nothing to merge while the second still completes
    /// the review. Rereading here closes it, because the lock is held from
    /// this point through `transact` and no other command runs meanwhile.
    ///
    /// The default is "nothing to read", which is right for every command
    /// whose evaluation depends only on head and on what the client sent.
    fn load(&mut self) -> BoxFuture<'_, std::result::Result<(), CommandError>> {
        Box::pin(async { Ok(()) })
    }

    /// Step 2 and step 3. Read what the command needs from the head, check
    /// its preconditions, and -- for a source-producing command -- prepare
    /// the batch with [`Head::prepare`]. A failed precondition is an error
    /// here and nothing is written.
    fn evaluate(
        &mut self,
        head: &Head<'_>,
    ) -> std::result::Result<Option<PreparedSource>, CommandError>;

    /// Step 4. The command's own rows, in the transaction that wrote the
    /// row naming the state it was evaluated against.
    fn transact<'a>(
        &'a mut self,
        tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        evidence: &'a Evidence,
    ) -> BoxFuture<'a, std::result::Result<Self::Output, CommandError>>;
}

/// Why a command did not happen. A conflict is the caller's to resolve and
/// carries what it needs to do so.
#[derive(Debug)]
pub enum CommandError {
    /// A precondition failed at head. Nothing was written.
    Conflict(String),
    /// A rendered selection was made against a projection that is no longer
    /// the head one (§7.1). The current digest comes back with it.
    StaleSelection {
        digest: String,
    },
    Storage(postgres::Error),
    Sequencer(SequencerError),
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict(why) => formatter.write_str(why),
            Self::StaleSelection { .. } => {
                formatter.write_str("the document moved under that selection")
            }
            Self::Storage(error) => error.fmt(formatter),
            Self::Sequencer(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for CommandError {}

impl From<postgres::Error> for CommandError {
    fn from(error: postgres::Error) -> Self {
        Self::Storage(error)
    }
}

impl From<sqlx::Error> for CommandError {
    fn from(error: sqlx::Error) -> Self {
        Self::Storage(postgres::Error::from(error))
    }
}

impl From<SequencerError> for CommandError {
    fn from(error: SequencerError) -> Self {
        Self::Sequencer(error)
    }
}

#[derive(Debug)]
pub enum SequencerError {
    /// The document cannot be read: a build breached a bound (§9.3). Ingest
    /// and flush continue; projections and commands answer 503.
    Unreadable(String),
    /// The memory budget had no room. Retryable.
    Busy,
    Storage(postgres::Error),
    Loro(String),
    /// The fence was lost; this sequencer must stop and be re-admitted.
    Fenced(String),
}

impl std::fmt::Display for SequencerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable(why) => write!(formatter, "this document cannot be read: {why}"),
            Self::Busy => formatter.write_str("this deployment is at its memory budget; retry"),
            Self::Storage(error) => error.fmt(formatter),
            Self::Loro(error) => formatter.write_str(error),
            Self::Fenced(why) => write!(formatter, "writer ownership lost: {why}"),
        }
    }
}

impl std::error::Error for SequencerError {}

impl From<postgres::Error> for SequencerError {
    fn from(error: postgres::Error) -> Self {
        Self::Storage(error)
    }
}

pub type Result<T> = std::result::Result<T, SequencerError>;

/// What the sequencer asks of storage: the log reads and writes of §5 and
/// §6, and the base blob a build needs. Nothing about ingest, the flush
/// triggers or the buffer is here, because none of that is a storage
/// question (§14.2 tests those against a fake that never has these methods
/// called).
///
/// This exists so `Sequencer` can hold `Arc<dyn LogCatalog>` instead of
/// `Arc<PostgresCatalog>`. `admit` is still the only production caller and
/// still takes a concrete `PostgresCatalog`; the trait object is an internal
/// seam, not a new public surface.
pub(crate) trait LogCatalog: Send + Sync {
    fn log_coverage(
        &self,
        document_id: Uuid,
    ) -> BoxFuture<'_, postgres::Result<Vec<postgres::RowCoverage>>>;
    fn log_rows(
        &self,
        document_id: Uuid,
        after: i64,
        through: Option<i64>,
    ) -> BoxFuture<'_, postgres::Result<Vec<postgres::LogRow>>>;
    fn flush_log_row<'a>(
        &'a self,
        document_id: Uuid,
        row: FlushRow<'a>,
        scratch: super::pending::Reservation,
    ) -> BoxFuture<'a, postgres::Result<i64>>;
    fn persistence_connection(
        &self,
        scratch: super::pending::Reservation,
    ) -> BoxFuture<'_, postgres::Result<postgres::PersistenceConnection>>;
    fn log_sequence(&self, document_id: Uuid) -> BoxFuture<'_, postgres::Result<i64>>;
    fn begin_document_command<'a>(
        &'a self,
        connection: &'a mut postgres::PersistenceConnection,
        document_id: Uuid,
        authority: &'a Authority,
        rung: Rung,
    ) -> BoxFuture<'a, postgres::Result<sqlx::Transaction<'a, sqlx::Postgres>>>;
    fn insert_log_row<'a>(
        &'a self,
        tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        document_id: Uuid,
        row: FlushRow<'a>,
    ) -> BoxFuture<'a, postgres::Result<i64>>;
    /// The compaction base, read through the object store (§4.3). Returns a
    /// plain string error because that is all any caller here does with it.
    /// The base expands in memory while it is being sent, and the budget is
    /// what bounds that expansion.
    fn read_base(
        &self,
        document_id: Uuid,
        blobs: Arc<dyn BlobStore>,
        expanded_limit: u64,
    ) -> BoxFuture<'_, std::result::Result<Vec<u8>, String>>;
    /// §8.4's two compaction triggers, as rows and as bytes.
    ///
    /// Asked of storage rather than hard-coded here so that the flush
    /// trigger and the startup scan of §8.6 cross the same line. They are
    /// two halves of one rule: an operator who lowers the threshold and
    /// moves only one of them gets a scan that enqueues a document every
    /// restart and a flush that never asks, or the reverse.
    fn compaction_thresholds(&self) -> (i64, i64);
}

impl LogCatalog for PostgresCatalog {
    fn log_coverage(
        &self,
        document_id: Uuid,
    ) -> BoxFuture<'_, postgres::Result<Vec<postgres::RowCoverage>>> {
        Box::pin(self.log_coverage(document_id))
    }

    fn log_rows(
        &self,
        document_id: Uuid,
        after: i64,
        through: Option<i64>,
    ) -> BoxFuture<'_, postgres::Result<Vec<postgres::LogRow>>> {
        Box::pin(self.log_rows(document_id, after, through))
    }

    fn flush_log_row<'a>(
        &'a self,
        document_id: Uuid,
        row: FlushRow<'a>,
        scratch: super::pending::Reservation,
    ) -> BoxFuture<'a, postgres::Result<i64>> {
        Box::pin(self.flush_log_row_charged(document_id, row, scratch))
    }

    fn persistence_connection(
        &self,
        scratch: super::pending::Reservation,
    ) -> BoxFuture<'_, postgres::Result<postgres::PersistenceConnection>> {
        Box::pin(self.persistence_connection(scratch))
    }

    fn log_sequence(&self, document_id: Uuid) -> BoxFuture<'_, postgres::Result<i64>> {
        Box::pin(self.log_sequence(document_id))
    }

    fn begin_document_command<'a>(
        &'a self,
        connection: &'a mut postgres::PersistenceConnection,
        document_id: Uuid,
        authority: &'a Authority,
        rung: Rung,
    ) -> BoxFuture<'a, postgres::Result<sqlx::Transaction<'a, sqlx::Postgres>>> {
        Box::pin(async move {
            let mut tx = connection.begin().await?;
            self.check_document_command(&mut tx, document_id, authority, rung)
                .await?;
            Ok(tx)
        })
    }

    fn insert_log_row<'a>(
        &'a self,
        tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        document_id: Uuid,
        row: FlushRow<'a>,
    ) -> BoxFuture<'a, postgres::Result<i64>> {
        Box::pin(self.insert_log_row(tx, document_id, row))
    }

    fn read_base(
        &self,
        document_id: Uuid,
        blobs: Arc<dyn BlobStore>,
        expanded_limit: u64,
    ) -> BoxFuture<'_, std::result::Result<Vec<u8>, String>> {
        // `CollaborationStorage` wants an owned `Arc<PostgresCatalog>`.
        // Cloning one is cheap: the pool inside is itself reference counted.
        Box::pin(async move {
            crate::storage::collaboration::CollaborationStorage::new(Arc::new(self.clone()), blobs)
                .read_document_base(document_id, expanded_limit)
                .await
                .map_err(|error| error.to_string())
        })
    }

    fn compaction_thresholds(&self) -> (i64, i64) {
        self.compaction_thresholds()
    }
}

/// Everything the sequencer owns, behind one lock.
struct Inner {
    /// The sequence the next flush will write.
    next_sequence: i64,
    /// What the base plus every row covers.
    log_vector: VersionVector,
    /// The log vector plus the buffer. Never stored.
    head_vector: VersionVector,
    buffer: Vec<Pending>,
    /// The payload bytes in the buffer. What the resident estimate is taken
    /// from, because framing is not part of what a decoded document costs.
    buffer_bytes: usize,
    /// What the buffer is charged against the deployment pending pool:
    /// payload plus framing, which is also exactly the row those batches
    /// would encode to minus its one version byte. The per-document ceiling
    /// and the flush's scratch reservation are both taken from this.
    charged_bytes: usize,
    oldest_arrival: Option<Instant>,
    last_arrival: Option<Instant>,
    subscribers: HashMap<u64, Subscriber>,
    cache: Option<Cache>,
    /// The projection of the cache at its current version, so a poll that
    /// changes nothing costs nothing. Cleared whenever the cache moves.
    projection: Option<Arc<librepaper_document_core::Projected>>,
    /// Why this document cannot be read, when it cannot (§9.3).
    unreadable: Option<String>,
    /// Set when this process has lost the writer lease (§10): "this
    /// sequencer must stop and be re-admitted". Distinct from `unreadable`,
    /// which is about THIS DOCUMENT's own resources -- a build that breached
    /// a bound, where retrying later may work once the log is smaller or the
    /// budget is freed. `fenced` is about THIS PROCESS: another writer holds
    /// the log now, so nothing this process does to this document again can
    /// be correct, no matter what is retried, and a client that reconnects
    /// (through the load balancer, to whichever process holds the lease)
    /// reaches a sequencer that is not fenced. Conflating them would tell a
    /// client "retry me" for a condition retrying this process can never
    /// fix.
    fenced: Option<String>,
    /// When `source-changed` last went out, for the coalescing in §4.5.
    last_source_changed: Option<Instant>,
    /// Whether a digest is owed because one was suppressed or could not be
    /// computed without a cache.
    source_changed_owed: bool,
    /// When the housekeeping sweep last asked for this document's
    /// compaction, so a retry is rate limited rather than repeated every
    /// tick. Deliberately a timestamp and not an "already asked" flag: an
    /// ask that the worker queue dropped would leave such a flag set with
    /// nothing ever clearing it, and the retry would never fire.
    last_compaction_ask: Option<Instant>,
    /// What the base weighs, and what every row after it weighs. The two
    /// together are the log, which is what the quota and the resident
    /// estimate are both taken from.
    base_bytes: u64,
    row_bytes: u64,
    /// How many rows stand above the base, mirroring
    /// `documents.uncompacted_update_count`. The row half of §8.4's
    /// compaction trigger, as `row_bytes` is the byte half; kept here rather
    /// than derived from `next_sequence` because the sequence counter never
    /// resets and the base's `through` is not held in this struct.
    uncompacted_count: i64,
    /// What the base covers, so a join can tell a client that already holds
    /// it from one that needs it.
    base_vector: VersionVector,
    has_base: bool,
    /// What each principal has left of its update allowance (§5 step 2).
    /// Keyed by `ingest`'s `principal_key` and not by its `peer_key`: the
    /// bound is on whoever is behind the socket, so one person with four
    /// tabs open, or one reconnecting in a loop, is still one person.
    rate: HashMap<String, Bucket>,
}

/// §9.1's per-principal bound, as the token bucket the spec asks for.
///
/// A bucket and not a counter reset on a wall-clock boundary, which is what
/// this was before: a boundary hands the whole allowance back at once, so a
/// caller spending all of it just before the turn and all of it again just
/// after gets twice the bound inside a fraction of a second -- and a client
/// stuck in a resend loop, the workload this bound exists for, is exactly
/// the one that keeps hitting the boundary.
struct Bucket {
    /// Fractional so the refill is smooth: rounding to whole tokens would
    /// make the effective rate depend on how often the principal calls.
    tokens: f64,
    /// Monotonic, unlike the `now_unix()` this used to read: a clock step
    /// backwards would otherwise freeze a bucket or hand out a free refill.
    updated: Instant,
}

impl Bucket {
    /// A bucket nobody has spent from is full, which is why a principal with
    /// no entry and a principal at capacity are the same thing. See where
    /// `ingest` drops the full ones.
    fn full(capacity: f64, now: Instant) -> Self {
        Self {
            tokens: capacity,
            updated: now,
        }
    }

    fn is_full(&self, capacity: f64) -> bool {
        self.tokens >= capacity
    }

    /// Credits whatever the elapsed time is worth, then reports whether a
    /// whole token was there to spend.
    fn take(&mut self, capacity: f64, per_second: f64, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.updated).as_secs_f64();
        self.tokens = (self.tokens + elapsed * per_second).min(capacity);
        self.updated = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

impl Inner {
    /// Refuses a caller when this process no longer holds the writer lease.
    ///
    /// The two states are kept apart deliberately and both are answered from
    /// here rather than at each caller: `fenced` means somebody else is
    /// writing this document and reconnecting reaches them (§10), while
    /// `unreadable` means this document breached a resource bound and cannot
    /// be built (§9.3) -- ingest and flush continue through the second and
    /// not through the first.
    fn writable(&self) -> Result<()> {
        match &self.fenced {
            Some(why) => Err(SequencerError::Fenced(why.clone())),
            None => Ok(()),
        }
    }

    /// The same, for a caller that also needs the document built.
    fn readable(&self) -> Result<()> {
        self.writable()?;
        match &self.unreadable {
            Some(why) => Err(SequencerError::Unreadable(why.clone())),
            None => Ok(()),
        }
    }

    /// Marks this sequencer fenced and closes every socket on it.
    ///
    /// Idempotent: the second call finds no subscribers to close. Both
    /// callers -- `Sequencer::close_fenced`, which holds the lock, and
    /// `Entry::fence`, which owns the whole `Inner` -- had the same three
    /// lines, and the message a browser reads on the close frame was written
    /// out twice.
    fn fence(&mut self, why: String) {
        self.fenced = Some(why.clone());
        let payload = format!("fenced: {why}; reconnect");
        for (_, subscriber) in self.subscribers.drain() {
            let _ = subscriber.tx.force_close(payload.clone());
        }
    }

    /// What the first `take` batches are charged, which is also what they
    /// encode to once the row's version byte is added.
    fn charge_of(&self, take: usize) -> usize {
        self.buffer[..take]
            .iter()
            .map(|pending| pending.charge.bytes() as usize)
            .sum()
    }

    /// The row those batches make, encoded straight out of the buffer.
    ///
    /// Not through a `Vec<Batch>`: that intermediate copied every payload,
    /// which on a backed-up document is megabytes of transient allocation
    /// nothing accounted for and nothing needed. The lock is held while this
    /// runs -- it is a memcpy of at most one document's buffer, not I/O, and
    /// the round trip it feeds is still made with the lock released.
    fn encode_prefix(&self, take: usize) -> Vec<u8> {
        let mut row = frame::begin(self.charge_of(take));
        self.append_prefix(&mut row, take);
        row
    }

    fn append_prefix(&self, row: &mut Vec<u8>, take: usize) {
        for pending in &self.buffer[..take] {
            frame::append(row, &pending.peer_key, pending.client_seq, &pending.bytes);
        }
    }

    /// Who is owed an acknowledgement for the first `take` batches, and up to
    /// which of their own sequence numbers.
    ///
    /// Separate from the row because acknowledgement needs no payload at all,
    /// only attribution -- which is what lets the row be encoded once, under
    /// the lock, and dropped as soon as it is written, while the (tiny) ack
    /// list survives the round trip.
    fn ack_prefix(&self, take: usize) -> Vec<(String, i64)> {
        self.buffer[..take]
            .iter()
            .map(|pending| (pending.peer_key.clone(), pending.client_seq))
            .collect()
    }

    /// The vector the log would cover if the first `take` batches flushed.
    fn vector_through(&self, take: usize) -> VersionVector {
        let mut vector = self.log_vector.clone();
        for pending in &self.buffer[..take] {
            vector.merge(&pending.end);
        }
        vector
    }

    fn log_bytes(&self) -> u64 {
        self.base_bytes.saturating_add(self.row_bytes)
    }

    fn resident_estimate(&self, expansion: u64) -> u64 {
        estimate(
            self.log_bytes().saturating_add(self.buffer_bytes as u64),
            expansion,
        )
    }

    fn retire(&mut self, take: usize, written: i64, vector: VersionVector, row_bytes: usize) {
        self.next_sequence = written + 1;
        self.log_vector = vector;
        // Draining is what releases the pending charge: each `Pending` owns
        // its own reservation and gives it back as it drops. Only the
        // committed prefix goes, so edits that arrived while the row was
        // being written stay buffered and stay charged.
        self.buffer.drain(..take);
        self.buffer_bytes = self.buffer.iter().map(|pending| pending.bytes.len()).sum();
        self.charged_bytes = self.charge_of(self.buffer.len());
        self.row_bytes = self.row_bytes.saturating_add(row_bytes as u64);
        self.uncompacted_count = self.uncompacted_count.saturating_add(1);
        self.oldest_arrival = (!self.buffer.is_empty()).then(Instant::now);
    }

    /// §8.4's opening sentence: whether this document's backlog has reached
    /// either trigger.
    fn compaction_due(&self, thresholds: (i64, i64)) -> bool {
        let (count, bytes) = thresholds;
        self.uncompacted_count >= count || self.row_bytes >= bytes.max(0) as u64
    }
}

/// One document's log, live.
pub struct Sequencer {
    pub document_id: Uuid,
    pub slug: String,
    owner_id: Uuid,
    ledger: Arc<super::ledger::StorageLedger>,
    inner: tokio::sync::Mutex<Inner>,
    catalog: Arc<dyn LogCatalog>,
    blobs: Arc<dyn BlobStore>,
    config: Arc<Configuration>,
    budget: Arc<Budget>,
    /// The deployment's one pending-source authority, shared with every other
    /// sequencer this registry admitted (SPEC-frugal §2).
    pending: Arc<super::pending::PendingBudget>,
    /// Only one flush or command transaction at a time per document.
    transaction: tokio::sync::Mutex<()>,
    /// The peer key server-authored source is written under (§7.3).
    deployment_peer_key: String,
    /// Whether `ingest` refuses a batch whose start vector is not covered
    /// (§5 step 4). Every ingest call reads this, in production and in a
    /// test alike -- it is always `true` in production, because nothing
    /// calls `set_gap_check_enforced(false)` outside
    /// SPEC-server-is-a-log §14.1 item 1's negative control, which exists
    /// to show what the check prevents by turning off the one production
    /// path that enforces it, rather than by a `#[cfg(test)]` shortcut that
    /// would let a test see something the running server cannot.
    gap_check: std::sync::atomic::AtomicBool,
    /// When this sequencer was built, as the origin `rate_clock` counts
    /// from.
    created: Instant,
    /// What `ingest`'s token buckets read as "now", in nanoseconds after
    /// `created`, or zero for the live monotonic clock. Every ingest reads
    /// it, in production and in a test alike, and in production it is always
    /// zero because the only thing that writes it is `set_rate_clock` --
    /// the same shape, and for the same reason, as `gap_check` above.
    ///
    /// It exists because the two claims worth making about a bucket are that
    /// time passing refills it and that time NOT passing does not, and the
    /// only other way to make either of them is to sleep through real
    /// seconds and hope the machine was not busy.
    rate_clock: std::sync::atomic::AtomicU64,
    /// Where §8.4's "the sequencer schedules compaction on the in-process
    /// worker" goes.
    ///
    /// A cell rather than a field because the worker is built FROM the
    /// registry (it needs to reach the sequencers it compacts) and so cannot
    /// exist when the registry that admits this sequencer is built. The same
    /// shape as `Budget::evicts_through`, and for the same reason. Shared
    /// with the registry, so a sequencer admitted before the worker was
    /// wired still finds it afterwards rather than being deaf for the life
    /// of the process.
    ///
    /// Empty in the tests that build a sequencer with no deployment around
    /// it, which is the honest reading of "nothing is listening".
    compaction: Arc<std::sync::OnceLock<crate::storage::worker::Handle>>,
}

impl Sequencer {
    /// Admits a document: reads the head of its log, and nothing else. No
    /// document is built, because typing does not need one (§1).
    // Every argument is a distinct collaborator this sequencer keeps for
    // life, the same reason `from_parts` below carries the same allow;
    // grouping them into a struct would name the same list twice.
    #[allow(clippy::too_many_arguments)]
    pub async fn admit(
        document_id: Uuid,
        slug: String,
        catalog: Arc<PostgresCatalog>,
        blobs: Arc<dyn BlobStore>,
        config: Arc<Configuration>,
        budget: Arc<Budget>,
        pending: Arc<super::pending::PendingBudget>,
        deployment_peer_key: String,
        ledger: Arc<super::ledger::StorageLedger>,
        compaction: Arc<std::sync::OnceLock<crate::storage::worker::Handle>>,
    ) -> Result<Self> {
        // Admission's own two reads, made against the concrete catalogue.
        // They happen before the upcast because they happen only here:
        // putting them in `LogCatalog` put two methods in the trait that
        // every test double had to answer and nothing else ever called.
        let head = catalog.log_head(document_id).await?;
        let base = catalog.log_base(document_id).await?;
        ledger.refresh(&catalog, head.owner_id).await?;
        // Upcast once, here: every other method sees only the questions and
        // writes in `LogCatalog`, not the concrete catalog, which is what
        // lets `from_parts` build a sequencer without one (§14.2).
        let catalog: Arc<dyn LogCatalog> = catalog;
        let log_vector = decode_vector(&head.vector)?;
        let base_vector = match &base {
            Some(base) => decode_vector(&base.vector)?,
            None => VersionVector::default(),
        };
        Ok(Self::from_parts(
            document_id,
            slug,
            head.owner_id,
            catalog,
            blobs,
            config,
            budget,
            pending,
            deployment_peer_key,
            ledger,
            head.update_sequence + 1,
            log_vector,
            base.as_ref()
                .map_or(0, |base| base.snapshot_bytes.max(0) as u64),
            head.uncompacted_bytes.max(0) as u64,
            head.uncompacted_count.max(0),
            base_vector,
            base.is_some(),
            compaction,
        ))
    }

    /// Assembles a sequencer from state `admit` already read, doing no I/O
    /// of its own. `admit` is the only production caller; §14.2's ingest,
    /// flush-trigger, buffer and join tests are the other one, and construct
    /// a sequencer this way with a fake `LogCatalog` so exercising them
    /// never needs PostgreSQL.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_parts(
        document_id: Uuid,
        slug: String,
        owner_id: Uuid,
        catalog: Arc<dyn LogCatalog>,
        blobs: Arc<dyn BlobStore>,
        config: Arc<Configuration>,
        budget: Arc<Budget>,
        pending: Arc<super::pending::PendingBudget>,
        deployment_peer_key: String,
        ledger: Arc<super::ledger::StorageLedger>,
        next_sequence: i64,
        log_vector: VersionVector,
        base_bytes: u64,
        row_bytes: u64,
        uncompacted_count: i64,
        base_vector: VersionVector,
        has_base: bool,
        compaction: Arc<std::sync::OnceLock<crate::storage::worker::Handle>>,
    ) -> Self {
        Self {
            document_id,
            slug,
            owner_id,
            ledger,
            inner: tokio::sync::Mutex::new(Inner {
                next_sequence,
                head_vector: log_vector.clone(),
                log_vector,
                buffer: Vec::new(),
                buffer_bytes: 0,
                charged_bytes: 0,
                oldest_arrival: None,
                last_arrival: None,
                subscribers: HashMap::new(),
                cache: None,
                projection: None,
                unreadable: None,
                fenced: None,
                last_source_changed: None,
                source_changed_owed: false,
                last_compaction_ask: None,
                base_bytes,
                row_bytes,
                uncompacted_count,
                base_vector,
                has_base,
                rate: HashMap::new(),
            }),
            catalog,
            blobs,
            config,
            budget,
            pending,
            transaction: tokio::sync::Mutex::new(()),
            deployment_peer_key,
            gap_check: std::sync::atomic::AtomicBool::new(true),
            created: Instant::now(),
            rate_clock: std::sync::atomic::AtomicU64::new(0),
            compaction,
        }
    }

    /// Turns the gap check in `ingest` on or off. See the field doc on
    /// `gap_check`: this is the seam SPEC-server-is-a-log §14.1 item 1's
    /// negative control uses, and the only caller of it anywhere in this
    /// crate is that test.
    #[cfg(test)]
    pub(crate) fn set_gap_check_enforced(&self, enforced: bool) {
        self.gap_check
            .store(enforced, std::sync::atomic::Ordering::Relaxed);
    }

    /// What the rate buckets call "now". See the field doc on `rate_clock`.
    fn rate_now(&self) -> Instant {
        match self.rate_clock.load(std::sync::atomic::Ordering::Relaxed) {
            0 => Instant::now(),
            pinned => self.created + Duration::from_nanos(pinned),
        }
    }

    /// Puts that "now" at a fixed point after this sequencer was built, so a
    /// test can hold it still or step it. `Duration::ZERO` hands the buckets
    /// back to the live clock, which is where production leaves them.
    #[cfg(test)]
    pub(crate) fn set_rate_clock(&self, since_creation: Duration) {
        self.rate_clock.store(
            since_creation.as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    // -- section 5: the typing path -------------------------------------

    /// Header check, gap check, append, relay, opportunistic cache import.
    ///
    /// Nothing here looks at what the bytes say.
    pub async fn ingest(
        &self,
        socket: u64,
        peer_key: &str,
        principal_key: &str,
        client_seq: i64,
        bytes: Vec<u8>,
    ) -> Ingested {
        if bytes.is_empty() {
            return Ingested::Invalid("an update with no bytes".into());
        }
        if bytes.len() > max_update_bytes(self.config.log_quota_bytes) {
            return Ingested::Refused("that update is larger than this deployment accepts");
        }
        // A peer key wider than the frame can carry would encode a length
        // `frame::decode` refuses, so the row would be written and then be
        // unreadable by replay -- silent loss, discovered only on the next
        // cold build. Refused here instead, where it is one comparison, and
        // permanently: nothing about retrying makes the key shorter.
        if peer_key.len() > frame::MAX_PEER_KEY {
            return Ingested::Refused("that editor's identity is too long to record");
        }
        let header = match LoroDoc::decode_import_blob_meta(&bytes, true) {
            Ok(header) => header,
            Err(error) => return Ingested::Invalid(error.to_string()),
        };

        let mut inner = self.inner.lock().await;

        // §10: a fenced sequencer has lost the writer lease. Nothing it
        // appends could ever reach the durable log again, so ingest is
        // refused rather than buffered; the socket that sent this is closed
        // separately, by whatever marked `fenced` (see `close_fenced`).
        if inner.fenced.is_some() {
            return Ingested::Refused("this server no longer holds this document; reconnect");
        }

        // A batch that carries no changes is not work, and appending it
        // would put a row in the log for nothing.
        //
        // This is not hypothetical and it is not rare: §6.2 step 6 has every
        // client export `Updates { from: vector }` and send it as catch-up
        // on every single join, and a client that was already up to date
        // exports exactly this -- a well-formed batch of 22 bytes holding
        // zero changes, because a Loro `Updates` export always writes its
        // header and envelope. One reconnect would otherwise cost one row,
        // which is the same size as a minute of somebody typing (§5.2).
        //
        // It is acknowledged rather than dropped silently. The client is
        // holding it in its unacknowledged map, where this acknowledgement
        // retires transmission bookkeeping; it does not claim that the
        // document's durable vector advanced. Nothing is relayed, because
        // there is nothing to relay.
        if header.change_num == 0 {
            acknowledge_peer(&mut inner, peer_key, client_seq);
            return Ingested::Accepted;
        }

        // §5 step 2: the sender is within its per-principal rate. This is
        // the one bound of the three that is about a person rather than
        // about bytes, and it is the one that stops a client stuck in a
        // resend loop from filling the buffer faster than the flush drains
        // it. A refusal is retryable: the bucket refills continuously, so
        // waiting admits them again.
        //
        // Keyed on `principal_key` and never on `peer_key`: a peer key names
        // one connection, so charging it would hand a fresh allowance to
        // every reconnect -- and reconnecting is precisely what the client
        // this bound exists for does. What the caller passes as the
        // principal is what decides how much a rotation is worth, and
        // `run_socket` chooses it there.
        let capacity = self.config.session.updates_per_minute.max(0) as f64;
        let per_second = capacity / 60.0;
        let rate_now = self.rate_now();
        if !inner.rate.contains_key(principal_key) {
            // Admitting a principal is the only place this map can grow, so
            // it is the only place the growth has to be paid for. A bucket
            // that has refilled to capacity is indistinguishable from no
            // entry at all -- both start the next update from
            // `Bucket::full` -- so those entries hold no state and are
            // dropped here instead of by a timer. The pass is over one entry
            // per principal that has ever touched THIS document, and it runs
            // only when a new one arrives, never on the typing path.
            inner.rate.retain(|_, bucket| !bucket.is_full(capacity));
        }
        let bucket = inner
            .rate
            .entry(principal_key.to_string())
            .or_insert_with(|| Bucket::full(capacity, rate_now));
        if !bucket.take(capacity, per_second, rate_now) {
            return retryable(
                &inner,
                "too many updates; slow down and they will be accepted",
                "rate",
                false,
            );
        }

        if inner.log_bytes() >= self.config.log_quota_bytes as u64 {
            return retryable(
                &inner,
                "this document's log is at its quota and is waiting to be compacted",
                "log_quota",
                false,
            );
        }
        // What this batch will cost the deployment if it is accepted: its
        // payload and the framing the row will spend on it.
        let charge = bytes.len().saturating_add(frame::overhead(peer_key));
        if let Some(reason) = self.ledger.would_exceed(
            self.owner_id,
            charge as i64,
            self.config.storage.per_owner,
            self.config.storage.total,
        ) {
            return retryable(&inner, "this account's storage is full", reason, false);
        }
        // The per-document ceiling, on the charge rather than the payload.
        // An empty buffer always admits one update whatever it weighs
        // (max_pending_charge), or a single maximum-size update would be
        // refused forever by its own framing.
        if !inner.buffer.is_empty()
            && inner.charged_bytes.saturating_add(charge) > BUFFER_CEILING_BYTES
        {
            return retryable(
                &inner,
                "sync_delayed: this server cannot save work right now",
                "document_buffer",
                true,
            );
        }
        if self.gap_check.load(std::sync::atomic::Ordering::Relaxed)
            && !covers(&inner.head_vector, &header.partial_start_vv)
        {
            return Ingested::Gap {
                vector: inner.head_vector.encode(),
            };
        }

        // SPEC-frugal §2: the deployment-wide ceiling, taken BEFORE the
        // update is accepted into the document. Order matters and is the
        // whole of what makes this a memory bound rather than a memory
        // report: the charge is held first, and only a charge that was
        // granted goes on to move the head, append to the buffer, import
        // into the cache or relay to anybody.
        //
        // A refusal here leaves this document exactly as it was. The rate
        // token spent above is not given back, which is deliberate: the
        // allowance bounds attempts, and an attempt was made.
        let charge = match self.pending.try_retain(charge as u64) {
            Ok(charge) => charge,
            Err(_) => {
                return retryable(
                    &inner,
                    "sync_delayed: this deployment is holding all the unsaved work it can",
                    "pending_budget",
                    true,
                )
            }
        };

        // A warm document grows synchronously below. Extend its reservation
        // before importing; if the budget cannot cover the growth, evict
        // this cache and leave the accepted update in the buffer for a
        // properly reserved cold build later.
        let wanted = estimate(
            inner
                .log_bytes()
                .saturating_add(inner.buffer_bytes as u64)
                .saturating_add(bytes.len() as u64),
            self.budget.expansion(),
        );
        let cache_growth_failed = inner
            .cache
            .as_mut()
            .is_some_and(|cache| cache.reservation.try_grow_to(wanted).is_err());
        if cache_growth_failed {
            inner.cache = None;
            inner.projection = None;
        }

        inner.head_vector.merge(&header.partial_end_vv);
        let now = Instant::now();
        if inner.oldest_arrival.is_none() {
            inner.oldest_arrival = Some(now);
        }
        inner.last_arrival = Some(now);
        inner.buffer_bytes = inner.buffer_bytes.saturating_add(bytes.len());
        inner.charged_bytes = inner.charged_bytes.saturating_add(charge.bytes() as usize);
        if let Some(cache) = &inner.cache {
            // Opportunistic: a warm entry stays at head so a projection is
            // free. A failure is not the sender's problem -- the bytes are
            // accepted and about to be buffered -- so the entry is dropped
            // and rebuilt rather than the update refused.
            if cache.doc.import(&bytes).is_err() {
                inner.cache = None;
            }
            inner.projection = None;
        }
        relay(&mut inner, Some(socket), &bytes);
        // Moved into the buffer rather than cloned into it. `ingest` owns
        // these bytes; keeping a second copy alive only until the end of the
        // call doubled what a keystroke costs, and the import and the relay
        // above are the only readers that needed the original. Both run
        // first, under the same lock, so nothing observes a different order.
        inner.buffer.push(Pending {
            peer_key: peer_key.to_string(),
            client_seq,
            bytes,
            end: header.partial_end_vv,
            // Moved, not copied: the charge taken above now belongs to these
            // bytes and is released when they are retired or the sequencer
            // holding them is dropped.
            charge,
        });
        self.note_source_changed(&mut inner);
        Ingested::Accepted
    }

    /// Whether a flush is due, and why. The sequencer keeps no timer of its
    /// own: a document nobody is touching should cost nothing, and the
    /// caller's housekeeping sweep already runs.
    pub async fn flush_due(&self) -> Option<FlushReason> {
        let inner = self.inner.lock().await;
        if inner.buffer.is_empty() {
            return None;
        }
        if inner.buffer_bytes >= FLUSH_TRIGGER_BYTES {
            return Some(FlushReason::Bytes);
        }
        if inner
            .oldest_arrival
            .is_some_and(|at| at.elapsed() >= FLUSH_MAX_AGE)
        {
            return Some(FlushReason::MaxAge);
        }
        if inner
            .last_arrival
            .is_some_and(|at| at.elapsed() >= FLUSH_QUIET)
        {
            return Some(FlushReason::Quiet);
        }
        None
    }

    /// Re-ask for a compaction that is still due.
    ///
    /// `flush` and `command_reporting_replay` ask the moment the counters
    /// cross. A full worker queue coalesces the wake-up into the worker's
    /// durable rescan, so a document that is resident but no longer edited
    /// still gets recovered without relying on another edit or a restart.
    /// This is the event that is left: the sweep itself, rate limited by
    /// `COMPACTION_RECHECK_INTERVAL`.
    ///
    /// One lock and an integer compare, no query, so §14.2's idle gate is
    /// untouched (`idle_deployment_issues_no_queries_beyond_the_lease_keepalive`).
    pub async fn recheck_compaction(&self) {
        let mut inner = self.inner.lock().await;
        if !inner.compaction_due(self.catalog.compaction_thresholds()) {
            return;
        }
        if inner
            .last_compaction_ask
            .is_some_and(|at| at.elapsed() < COMPACTION_RECHECK_INTERVAL)
        {
            return;
        }
        // Stamped only when there is a worker to ask. Before the handle is
        // wired there is nothing to rate limit, and stamping anyway would
        // make the first real sweep after startup wait out the interval.
        let Some(handle) = self.compaction.get() else {
            return;
        };
        inner.last_compaction_ask = Some(Instant::now());
        drop(inner);
        handle.ask(crate::storage::worker::Task::Compact(self.document_id));
    }

    /// §5.1. One transaction writing one row, then acknowledgements.
    ///
    /// The buffer is snapshotted under the lock and the lock released for the
    /// round trip, so relay continues while the row is written. Returns the
    /// sequence written, or `None` when there was nothing to write.
    pub async fn flush(&self, reason: FlushReason) -> Result<Option<i64>> {
        let _transaction = self.transaction.lock().await;
        let guaranteed_scratch = if matches!(
            reason,
            FlushReason::Shutdown | FlushReason::Barrier | FlushReason::AuthorityRevoked
        ) {
            Some(
                self.pending
                    .reserve_scratch(super::pending::scratch_for(max_row_bytes(self.config.log_quota_bytes) as u64))
                    .await
                    .map_err(|_| SequencerError::Busy)?,
            )
        } else {
            None
        };
        let (take, acknowledged, encoded, mut scratch, vector, expected, identity) = {
            let inner = self.inner.lock().await;
            // §10: once fenced, this sequencer's view of the log can never
            // be made durable again by writing more to it. Fail fast rather
            // than round-tripping to PostgreSQL only to be told the same
            // thing again.
            inner.writable()?;
            let take = inner.buffer.len();
            if take == 0 {
                return Ok(None);
            }
            // SPEC-frugal §2: the row about to be built, and the copy the
            // driver will make of it on the way to the wire, are reserved
            // BEFORE either exists. The size is known exactly rather than
            // estimated: a row is one version byte over the buffer's own
            // charge, which is what `charge_of` already holds.
            //
            // A refusal here is a deferral and nothing more. The buffer is
            // untouched, its charges stand, and the next housekeeping pass
            // -- one second later -- asks again. It cannot be a deadlock:
            // scratch is a separate pool that `ingest` never spends, and the
            // configuration is refused unless it can hold one maximum-size
            // row (`Configuration::validate_pending`).
            let row_bytes = frame::ROW_HEADER_BYTES.saturating_add(inner.charge_of(take)) as u64;
            let scratch = match guaranteed_scratch.map(Ok).unwrap_or_else(|| {
                self.pending
                    .try_scratch(super::pending::scratch_for(row_bytes))
            }) {
                Ok(scratch) => scratch,
                Err(_) => {
                    ::log::warn!(
                        "deferring the flush of {}: no persistence scratch right now",
                        self.slug
                    );
                    return Err(SequencerError::Busy);
                }
            };
            (
                take,
                inner.ack_prefix(take),
                inner.encode_prefix(take),
                scratch,
                inner.vector_through(take),
                inner.next_sequence - 1,
                self.identity_of(&inner),
            )
        };

        let driver_scratch =
            scratch.split(super::pending::driver_scratch_for(encoded.len() as u64));
        let written = match self
            .catalog
            .flush_log_row(
                self.document_id,
                FlushRow {
                    expected_update_sequence: expected,
                    update_bytes: &encoded,
                    vector: &vector.encode(),
                    source_format: identity.as_ref().map(|(format, _)| format.as_str()),
                    main_path: identity.as_ref().map(|(_, main)| main.as_str()),
                },
                driver_scratch,
            )
            .await
        {
            Ok(sequence) => sequence,
            Err(error) => match self
                .resolve_ambiguous_flush(expected, &vector.encode(), error)
                .await
            {
                Ok(sequence) => sequence,
                Err(SequencerError::Fenced(why)) => {
                    self.close_fenced(why.clone()).await;
                    return Err(SequencerError::Fenced(why));
                }
                Err(other) => return Err(other),
            },
        };

        let row_bytes = encoded.len();
        // The row has reached storage and nothing reads these bytes again.
        // Dropped here rather than at the end of the function so the scratch
        // is back in the pool before the acknowledgements go out.
        drop(encoded);
        drop(scratch);
        let mut inner = self.inner.lock().await;
        inner.retire(take, written, vector, row_bytes);
        self.ledger.charge(self.owner_id, row_bytes as i64);
        let compaction_due = inner.compaction_due(self.catalog.compaction_thresholds());
        acknowledge(&mut inner, &acknowledged);
        notify_durable(&mut inner);
        // A digest the coalescing window suppressed goes out on the flush,
        // which is the floor §4.5 promises a reader.
        if inner.source_changed_owed {
            self.emit_source_changed(&mut inner);
        }
        ::log::debug!(
            "flushed {} batches for {} as row {written} ({reason:?})",
            acknowledged.len(),
            self.slug
        );
        drop(inner);

        // §8.4's first sentence. This is the only thing that schedules
        // compaction during ordinary editing: the scan of §8.6 runs once, at
        // startup, and nothing polls afterwards. Without it a log grows
        // until §9.1's quota refuses updates with "waiting to be compacted"
        // and nothing ever compacts it, which is a document that can never
        // be written to again.
        //
        // Asked here rather than from the housekeeping sweep so that no
        // clock is involved: the moment a row lands is the moment the
        // counters can cross, and a deployment with nothing resident asks
        // nothing because nothing flushes. The queue is bounded and drops
        // what it cannot take, which is why this is safe to ask on every
        // flush once over the line: the durable counters still say so, and
        // the next flush -- or the next startup scan -- asks again.
        if compaction_due {
            if let Some(handle) = self.compaction.get() {
                handle.ask(crate::storage::worker::Task::Compact(self.document_id));
            }
        }
        Ok(Some(written))
    }

    /// §5.1, ambiguous outcome. If the database response was lost the row may
    /// have committed anyway, and writing it again would duplicate it.
    async fn resolve_ambiguous_flush(
        &self,
        expected: i64,
        attempted: &[u8],
        error: postgres::Error,
    ) -> Result<i64> {
        let current = match self.catalog.log_sequence(self.document_id).await {
            Ok(current) => current,
            // The re-read failed too, so nothing is known. Retry is the safe
            // answer: the sequence check in the flush refuses a duplicate.
            Err(_) => return Err(error.into()),
        };
        if current == expected {
            // Nothing was written. Retry later with the same buffer.
            return Err(error.into());
        }
        if current != expected + 1 {
            return Err(SequencerError::Fenced(format!(
                "the log is at {current} and this server expected {expected}"
            )));
        }
        // The sequence advanced by exactly one -- but that alone does NOT
        // mean it advanced because of THIS write. A semantic command's
        // transaction writes a row the same way, and if its own commit
        // response was lost the sequencer's `next_sequence` is a row behind
        // reality; the next flush then computes the same `expected`, is
        // refused, lands here, sees `expected + 1`, and would conclude its
        // own write had committed. It would then retire batches that are
        // nowhere in the log and acknowledge them to their authors as
        // durable, which is silent loss of somebody's work from a single
        // dropped network response.
        //
        // So the row itself is asked. Its `vector` is exactly what this
        // flush tried to write, and it is written once and never updated,
        // so comparing them is decisive.
        let rows = match self
            .catalog
            .log_rows(self.document_id, expected, Some(expected + 1))
            .await
        {
            Ok(rows) => rows,
            Err(_) => return Err(error.into()),
        };
        match rows.first() {
            Some(row) if row.vector == attempted => Ok(expected + 1),
            Some(_) => Err(SequencerError::Fenced(
                "another writer holds this document's log".into(),
            )),
            None => Err(error.into()),
        }
    }

    /// The format and main path this flush stamps on the document row, when a
    /// warm cache can say them without a build. A cold document leaves them
    /// alone: the row still holds what the last projection said.
    fn identity_of(&self, inner: &Inner) -> Option<(String, String)> {
        let projection = inner.projection.as_ref()?;
        let main = projection.projection.main.clone();
        if main.is_empty() {
            return None;
        }
        let format = crate::document::render::document_format(&main)?;
        Some((format.to_string(), main))
    }

    // -- section 6: join -------------------------------------------------

    /// Registers a subscriber and computes its reply in one step, so no batch
    /// falls between the reply and the first relay (§6.2).
    pub async fn join(
        &self,
        socket: u64,
        role: Role,
        peer_key: &str,
        tx: Sender,
        client_vector: Option<&[u8]>,
    ) -> Result<Joined> {
        let client = match client_vector {
            Some(bytes) if !bytes.is_empty() => decode_vector(bytes)?,
            _ => VersionVector::default(),
        };
        // Neither step needs a document (§6.2). Reading the coverage is I/O
        // and cannot be done under `inner`, so this holds the TRANSACTION
        // gate for the whole join instead: no flush and no command can
        // interleave, which is what makes the coverage read still true when
        // the reply is assembled.
        //
        // An earlier version took no gate and reasoned that a row written
        // meanwhile would reach this subscriber by relay, because
        // registration and the reply happen together under `inner`. That
        // was wrong, and wrong in the direction that loses data. A flush
        // DRAINS the batches it writes out of the buffer. So a row flushed
        // between the coverage read and the lock is in neither place: not
        // in the stale coverage list this join scans, and no longer in the
        // buffer the reply carries. The client would be handed a `vector`
        // that claims those operations and none of their bytes, and §6.2
        // step 6 has it trust that vector as the baseline for its own
        // catch-up export -- so it would believe itself synced and never
        // ask again. Nothing would resend them.
        //
        // Ingest is deliberately NOT blocked by this gate: a batch that
        // arrives during a join stays in the buffer and the reply carries
        // it, which is correct, and typing must not wait on somebody else's
        // reconnect.
        let _gate = self.transaction.lock().await;
        let coverage = self.catalog.log_coverage(self.document_id).await?;

        // Step 2: the earliest row the client does not cover, scanning
        // stored vectors backwards from the head.
        let mut first_uncovered: Option<i64> = None;
        for row in coverage.iter().rev() {
            if covers(&client, &decode_vector(&row.vector)?) {
                break;
            }
            first_uncovered = Some(row.update_sequence);
        }

        let mut inner = self.inner.lock().await;
        inner.writable()?;
        inner.subscribers.insert(
            socket,
            Subscriber {
                tx,
                role,
                peer_key: peer_key.to_string(),
            },
        );
        let vector = inner.head_vector.encode();
        let durable_vector = inner.log_vector.encode();
        // A document with no base is covered by definition: there is nothing
        // under the rows. A base whose vector is empty is a different thing
        // and must NOT read as covered -- every vector covers the empty one,
        // so the test would pass for every client and a cold join would be
        // answered with rows alone, on top of a base it never received. That
        // cannot happen through `activate_log_base`, which writes the vector
        // compaction proved (§8.4 step 4), but the failure mode is silent
        // data loss and the guard costs one comparison.
        let base_covered = match (inner.has_base, inner.base_vector.is_empty()) {
            (false, _) => true,
            (true, true) => false,
            (true, false) => covers(&client, &inner.base_vector),
        };
        let buffer: Vec<Vec<u8>> = inner
            .buffer
            .iter()
            .map(|pending| pending.bytes.clone())
            .collect();
        let base_bytes = inner.base_bytes;
        drop(inner);

        let mut batches: Vec<Vec<u8>> = Vec::new();
        // A client that does not cover the base is sent every row, because
        // the base it is about to import ends before the first of them.
        let from = if base_covered {
            first_uncovered
        } else {
            coverage.first().map(|row| row.update_sequence)
        };
        if let Some(from) = from {
            for row in self
                .catalog
                .log_rows(self.document_id, from - 1, None)
                .await?
            {
                for batch in frame::decode(&row.update_bytes)
                    .map_err(|error| SequencerError::Loro(error.to_string()))?
                {
                    batches.push(batch.bytes);
                }
            }
        }
        batches.extend(buffer);

        let base = if base_covered {
            None
        } else {
            let estimate = super::budget::estimate(
                base_bytes,
                super::budget::BUILD_TRANSIENT_EXPANSION,
            );
            let transient_reservation = self.budget.reserve(estimate, RESERVE_PATIENCE).await.map_err(|Busy| SequencerError::Busy)?;
            let base_result = self.read_base(transient_reservation.bytes()).await?;
            drop(transient_reservation);
            Some(base_result)
        };
        Ok(Joined {
            vector,
            durable_vector,
            base,
            batches,
            from_rows: base_covered && first_uncovered.is_some(),
        })
    }

    /// Whether this was the last subscriber, which is one of the flush
    /// triggers in §5 step 6.
    pub async fn unsubscribe(&self, socket: u64) -> bool {
        let mut inner = self.inner.lock().await;
        inner.subscribers.remove(&socket);
        inner.subscribers.is_empty()
    }

    pub async fn subscribers(&self) -> usize {
        self.inner.lock().await.subscribers.len()
    }

    pub async fn editors(&self) -> usize {
        self.inner
            .lock()
            .await
            .subscribers
            .values()
            .filter(|subscriber| subscriber.role == Role::Editor)
            .count()
    }

    /// Sends one frame to every subscriber of a role, except one.
    pub async fn broadcast(
        &self,
        role: Option<Role>,
        skip: Option<u64>,
        payload: &serde_json::Value,
    ) {
        let text = payload.to_string();
        let mut inner = self.inner.lock().await;
        fan_out(&mut inner, false, |id, subscriber| {
            let wanted = Some(id) != skip && role.is_none_or(|wanted| wanted == subscriber.role);
            wanted.then(|| Outgoing::shared_text(text.clone()))
        });
    }

    // -- section 7: semantic commands ------------------------------------

    /// §7, all five steps.
    ///
    /// The lock is held from step 1 to step 5, so nothing the command was
    /// evaluated against can move underneath it: the state the row captured
    /// is the state the command saw, because no ingest interleaved.
    pub async fn command<C: Command>(
        &self,
        authority: &Authority,
        command: &mut C,
    ) -> std::result::Result<C::Output, CommandError> {
        self.command_reporting_replay(authority, command)
            .await
            .map(|(output, _)| output)
    }

    /// [`Self::command`], and also whether §7.2's retry record answered it
    /// instead of a fresh commit.
    ///
    /// Callers that speak to a client over a request/response protocol want
    /// this: the whole point of the retry contract is that a client whose
    /// response was lost can ask again and be told its first attempt
    /// landed. A caller told only the result cannot tell that apart from
    /// having just made a second change, and the MCP surface used to say
    /// `"replay": false` unconditionally, which is a lie on exactly the
    /// request the contract exists for.
    pub async fn command_reporting_replay<C: Command>(
        &self,
        authority: &Authority,
        command: &mut C,
    ) -> std::result::Result<(C::Output, bool), CommandError> {
        let _transaction = self.transaction.lock().await;

        // 0. Has this already happened? §7.2's retry record is checked
        //    before the lock is taken and before anything is prepared,
        //    because preparing a second copy of the edit and writing the row
        //    that carries it is exactly what a retry must not do.
        if let Some(recorded) = command.replay().await? {
            return Ok((recorded, true));
        }

        // 1a. Whatever relational state the synchronous `evaluate` will need,
        //     read now that the lock is held rather than by the caller before
        //     it was.
        command.load().await?;

        let mut inner = self.inner.lock().await;

        // §10: a fenced sequencer refuses commands the same as ingest, for
        // the same reason -- nothing it writes now can become durable.
        if let Some(why) = &inner.fenced {
            return Err(CommandError::Sequencer(SequencerError::Fenced(why.clone())));
        }

        // 1. Materialize head.
        if let Some(why) = &inner.unreadable {
            return Err(CommandError::Sequencer(SequencerError::Unreadable(
                why.clone(),
            )));
        }
        if inner.cache.is_none() {
            let cache = self.build(&mut inner).await?;
            inner.cache = Some(cache);
        }
        if inner.projection.is_none() {
            let projected = {
                let cache = inner.cache.as_ref().expect("a cache was just built");
                Arc::new(librepaper_document_core::project(
                    &cache.doc,
                    &self.config.paths(),
                ))
            };
            inner.projection = Some(projected);
        }

        // 2 and 3. Evaluate against head, and prepare any source.
        //
        // The estimate is `inner`'s, computed before the cache reference is
        // taken below, on the same formula `build` and `with_fork_at` use:
        // a fork of this document is expected to cost what the resident
        // document itself is estimated to cost (§9.2).
        let estimate = inner.resident_estimate(self.budget.expansion());
        let prepared = {
            let cache = inner.cache.as_ref().expect("a cache is resident");
            let head = Head {
                doc: &cache.doc,
                rules_owner: &self.config,
                vector: inner.head_vector.clone(),
                frontier: cache.doc.oplog_frontiers(),
                projection: inner.projection.clone().expect("a projection is resident"),
                budget: self.budget.clone(),
                estimate,
            };
            let prepared = command.evaluate(&head)?;
            (prepared, head.frontier.encode(), head.digest())
        };
        let (prepared, before_frontier, before_digest) = prepared;

        // 3. Assemble the row: the buffer as it stands, plus the prepared
        //    batch. The buffer itself is not modified.
        let take = inner.buffer.len();
        let mut acknowledged = inner.ack_prefix(take);
        let mut vector = inner.vector_through(take);
        // SPEC-frugal §2: what this row will weigh, before it exists. The
        // buffered prefix is charged already and its size is known exactly;
        // the prepared batch is not in the buffer and so has to be added,
        // framing and all.
        let mut row_bytes = frame::ROW_HEADER_BYTES.saturating_add(inner.charge_of(take));
        if let Some(source) = &prepared {
            vector.merge(&source.end);
            acknowledged.push((self.deployment_peer_key.clone(), source.client_seq));
            row_bytes = row_bytes
                .saturating_add(frame::overhead(&self.deployment_peer_key))
                .saturating_add(source.batch.len());
        }
        // Reserved before the row is encoded, and refused through §7's
        // ordinary precondition failure -- which runs before the transaction
        // is opened, so nothing is rolled back and no effect was committed.
        // `Conflict` is what every other temporary refusal inside a command
        // uses (`Head::prepare`'s own memory refusal included), and it is
        // what the caller already turns into a retryable answer.
        let scratch = self
            .pending
            .try_scratch(super::pending::scratch_for(row_bytes as u64))
            .map_err(|_| {
                CommandError::Conflict(
                    "this deployment is holding all the unsaved work it can; retry".to_string(),
                )
            })?;
        let expected = inner.next_sequence - 1;
        let identity = self.identity_of(&inner);

        // The connection owns scratch through commit, rollback or cancellation.
        let mut connection = self.catalog.persistence_connection(scratch).await?;
        // 4. Transact.
        let mut tx = self
            .catalog
            .begin_document_command(
                &mut connection,
                self.document_id,
                authority,
                command.authority(),
            )
            .await?;
        // The row itself, built once. The buffered prefix is encoded straight
        // out of the buffer, the prepared batch appended to it: no
        // intermediate `Vec<Batch>` copying every payload, and -- unlike
        // before -- no second `frame::encode` of the same batches after the
        // commit just to learn the row's length. The length is `encoded.len()`
        // and was already known from the reservation above.
        let encoded = if acknowledged.is_empty() {
            Vec::new()
        } else {
            frame::encode_slices(
                inner.buffer[..take]
                    .iter()
                    .map(|pending| {
                        (
                            pending.peer_key.as_str(),
                            pending.client_seq,
                            pending.bytes.as_slice(),
                        )
                    })
                    .chain(prepared.iter().map(|source| {
                        (
                            self.deployment_peer_key.as_str(),
                            source.client_seq,
                            source.batch.as_slice(),
                        )
                    })),
            )
        };
        let row_bytes = encoded.len();
        let source_sequence = if acknowledged.is_empty() {
            expected
        } else {
            let written = self
                .catalog
                .insert_log_row(
                    &mut tx,
                    self.document_id,
                    FlushRow {
                        expected_update_sequence: expected,
                        update_bytes: &encoded,
                        vector: &vector.encode(),
                        source_format: identity.as_ref().map(|(format, _)| format.as_str()),
                        main_path: identity.as_ref().map(|(_, main)| main.as_str()),
                    },
                )
                .await?;
            written
        };
        let evidence = Evidence {
            source_sequence,
            vector: vector.encode(),
            before_frontier,
            before_digest,
            after_frontier: prepared
                .as_ref()
                .map(|source| source.after_frontier.encode()),
            after_digest: prepared.as_ref().map(|source| source.after_digest.clone()),
        };
        let outcome = command.transact(&mut tx, &evidence).await;
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                // 5, the failure half. Nothing between step 1 and here
                // changed the buffer or the cache, so there is nothing to put
                // back: the prepared batch is dropped with the transaction.
                let _ = tx.rollback().await;
                return Err(error);
            }
        };
        tx.commit().await?;
        // The row has reached storage and nothing reads these bytes again.
        // Released here rather than at the end of the call, so the scratch is
        // back in the pool before the relay and the acknowledgements. On
        // every other exit -- a precondition failure, a rollback, a
        // cancelled future -- the same bytes are released by `Drop` instead.
        drop(encoded);
        connection.complete();
        drop(connection);

        // 5. Retire the flushed batches, install the prepared batch, relay
        //    it, and acknowledge.
        //
        // `row_bytes` is the length of the row written above, kept from the
        // one encoding rather than recomputed. It used to re-encode the whole
        // batch list here purely to measure it -- a second full copy of every
        // buffered payload, allocated after the transaction had already
        // committed, for a number the first encoding knew.
        let wrote_a_row = !acknowledged.is_empty();
        if wrote_a_row {
            inner.retire(take, source_sequence, vector, row_bytes);
        }
        if let Some(source) = &prepared {
            inner.head_vector.merge(&source.end);
            if let Some(cache) = &inner.cache {
                if cache.doc.import(&source.batch).is_err() {
                    inner.cache = None;
                }
            }
            inner.projection = if inner.cache.is_some() {
                Some(source.after_projection.clone())
            } else {
                None
            };
            relay(&mut inner, None, &source.batch);
            self.note_source_changed(&mut inner);
        }
        acknowledge(&mut inner, &acknowledged);
        if wrote_a_row {
            notify_durable(&mut inner);
        }
        let compaction_due = inner.compaction_due(self.catalog.compaction_thresholds());
        drop(inner);

        // §8.4, the same trigger `flush` carries and for the same reason: a
        // command's row is a row. A document edited only by commands -- an
        // agent patching it, a restore, a proposal being merged -- may never
        // reach an ordinary flush, and its backlog would then cross the
        // threshold with nothing watching.
        if compaction_due {
            if let Some(handle) = self.compaction.get() {
                handle.ask(crate::storage::worker::Task::Compact(self.document_id));
            }
        }
        Ok((outcome, false))
    }

    // -- sections 4.3 and 9: the cache ----------------------------------

    /// The projection at head, but only when a cache is already resident,
    /// and never building one. §4.5 already draws this line for a reader's
    /// digest -- "computed only when a warm cache exists, and otherwise
    /// deferred" -- and anything else on a periodic pass that only wants to
    /// know whether the document moved (comment re-anchoring, for one) needs
    /// the same restraint: a document nobody has looked at recently should
    /// not have a cache built for it just to answer "did it change".
    pub async fn projection_if_warm(&self) -> Option<Arc<librepaper_document_core::Projected>> {
        let mut inner = self.inner.lock().await;
        if inner.fenced.is_some() || inner.unreadable.is_some() || inner.cache.is_none() {
            return None;
        }
        if let Some(projection) = &inner.projection {
            return Some(projection.clone());
        }
        let projected = {
            let cache = inner.cache.as_ref().expect("checked above");
            Arc::new(librepaper_document_core::project(
                &cache.doc,
                &self.config.paths(),
            ))
        };
        inner.projection = Some(projected.clone());
        Some(projected)
    }

    /// The projection at head, building a cache entry if there is not one.
    pub async fn projection(&self) -> Result<Arc<librepaper_document_core::Projected>> {
        let mut inner = self.inner.lock().await;
        inner.readable()?;
        if let Some(projection) = &inner.projection {
            return Ok(projection.clone());
        }
        if inner.cache.is_none() {
            let cache = self.build(&mut inner).await?;
            inner.cache = Some(cache);
        }
        let projected = {
            let cache = inner.cache.as_ref().expect("a cache was just built");
            Arc::new(librepaper_document_core::project(
                &cache.doc,
                &self.config.paths(),
            ))
        };
        inner.projection = Some(projected.clone());
        Ok(projected)
    }

    /// The projection at a named frontier: a fork of the entry, discarded
    /// after use (§4.3).
    pub async fn projection_at(
        &self,
        frontier: &Frontiers,
    ) -> Result<librepaper_document_core::Projected> {
        self.with_fork_at(frontier, |fork| {
            librepaper_document_core::project(fork, &self.config.paths())
        })
        .await
    }

    /// A document as it stood at a cut, for as long as the closure runs.
    /// The fork reserves the parent's estimate and is discarded after use.
    pub async fn with_fork_at<T>(
        &self,
        frontier: &Frontiers,
        read: impl FnOnce(&LoroDoc) -> T,
    ) -> Result<T> {
        let mut inner = self.inner.lock().await;
        inner.readable()?;
        if inner.cache.is_none() {
            let cache = self.build(&mut inner).await?;
            inner.cache = Some(cache);
        }
        let estimate = inner.resident_estimate(self.budget.expansion());
        let _fork = self
            .budget
            .reserve(estimate, RESERVE_PATIENCE)
            .await
            .map_err(|Busy| SequencerError::Busy)?;
        let cache = inner.cache.as_ref().expect("a cache was just built");
        let fork = cache
            .doc
            .fork_at(frontier)
            .map_err(|error| SequencerError::Loro(error.to_string()))?;
        Ok(read(&fork))
    }

    /// The head document *and* its projection, under one acquisition of the
    /// lock.
    ///
    /// A caller that wanted both asked for them separately, and the two
    /// answers were about two different moments: an edit landing between the
    /// `projection()` call and the `with_head` call gave a caller a digest
    /// naming one revision and text from another, which is exactly the
    /// mismatch a stale-selection check exists to catch and would instead
    /// have caused. Holding the lock across both also means the projection
    /// is built once and cached, rather than recomputed per comment page.
    pub async fn with_projected_head<T>(
        &self,
        read: impl FnOnce(&LoroDoc, &librepaper_document_core::Projected) -> T,
    ) -> Result<T> {
        let mut inner = self.inner.lock().await;
        inner.readable()?;
        if inner.cache.is_none() {
            let cache = self.build(&mut inner).await?;
            inner.cache = Some(cache);
        }
        if inner.projection.is_none() {
            let cache = inner.cache.as_ref().expect("a cache was just built");
            inner.projection = Some(Arc::new(librepaper_document_core::project(
                &cache.doc,
                &self.config.paths(),
            )));
        }
        let projected = inner.projection.clone().expect("just built");
        let cache = inner.cache.as_ref().expect("a cache was just built");
        Ok(read(&cache.doc, &projected))
    }

    /// The head document, for a caller that has to read more than the
    /// projection and is not writing anything.
    pub async fn with_head<T>(&self, read: impl FnOnce(&LoroDoc) -> T) -> Result<T> {
        let mut inner = self.inner.lock().await;
        inner.readable()?;
        if inner.cache.is_none() {
            let cache = self.build(&mut inner).await?;
            inner.cache = Some(cache);
        }
        let cache = inner.cache.as_ref().expect("a cache was just built");
        Ok(read(&cache.doc))
    }

    /// Builds an entry: the base, then every row, then the buffer.
    ///
    /// The Loro work is synchronous and cannot be interrupted once started,
    /// so the reservation is taken before it begins and the deadline is a
    /// signal to stop scheduling rather than a way to reclaim a thread
    /// (§9.2, §9.3).
    async fn build(&self, inner: &mut Inner) -> Result<Cache> {
        let estimate = inner.resident_estimate(self.budget.expansion());
        let reservation = self
            .budget
            .reserve(estimate, RESERVE_PATIENCE)
            .await
            .map_err(|Busy| SequencerError::Busy)?;
        // A cold build costs more than the resident figure while it is
        // actually running (`BUILD_TRANSIENT_EXPANSION` in budget.rs
        // documents the measurement). This second reservation covers that
        // peak; it is a local and is never moved into `Cache`, so it drops
        // -- and gives its bytes back -- the moment `build` returns, which
        // is before the caller puts the entry in `inner.cache`. What
        // outlives `build` is the resident-only reservation above.
        let transient_estimate = super::budget::estimate(
            inner.log_bytes().saturating_add(inner.buffer_bytes as u64),
            super::budget::BUILD_TRANSIENT_EXPANSION,
        );
        let transient_reservation = self
            .budget
            .reserve(transient_estimate, RESERVE_PATIENCE)
            .await
            .map_err(|Busy| SequencerError::Busy)?;
        let base = if inner.has_base {
            Some(self.read_base(reservation.bytes() + transient_reservation.bytes()).await?)
        } else {
            None
        };
        let rows = self.catalog.log_rows(self.document_id, 0, None).await?;
        let mut batches: Vec<Vec<u8>> = Vec::new();
        for row in rows {
            for batch in frame::decode(&row.update_bytes)
                .map_err(|error| SequencerError::Loro(error.to_string()))?
            {
                batches.push(batch.bytes);
            }
        }
        for pending in &inner.buffer {
            batches.push(pending.bytes.clone());
        }
        // §9.3: the import below is uninterruptible CPU work, so the
        // permit is taken here rather than at the top of `build`. The reads
        // above are I/O and waiting on them holds nothing back.
        let _turn = super::admission::heavy().await;
        let started = Instant::now();
        let built = tokio::task::spawn_blocking(move || -> std::result::Result<LoroDoc, String> {
            let doc = crate::document::session::new_doc();
            if let Some(base) = base {
                doc.import(&base).map_err(|error| error.to_string())?;
            }
            // One at a time, NOT `import_batch`, which is what this used to
            // call and what §4.2.1's framing was written for.
            //
            // Measured 2026-09-20 (`batched_import_cost_probe`): importing a
            // 1 MiB document as a single batch takes 1.5 ms, and adding ONE
            // further 100-byte update to the same `import_batch` call takes
            // 3.8 s. The cliff is in the call shape, not the bytes: 64 more
            // updates cost nothing beyond it, and the identical batches
            // imported one at a time take 1.8 ms and stay flat. It also
            // dominated memory -- the batched call peaked at 400 MB of RSS
            // for a 1 MiB document, against a §9.2 estimate of 6 MB.
            //
            // The log is causally ordered, because §5 refuses a batch whose
            // dependencies are not already in `head_vector`, so replaying in
            // row order needs no buffering of pending imports. That is the
            // only thing `import_batch` was giving us here.
            for batch in &batches {
                doc.import(batch).map_err(|error| error.to_string())?;
            }
            Ok(doc)
        })
        .await
        .map_err(|error| SequencerError::Loro(error.to_string()))?;
        let doc = match built {
            Ok(doc) => doc,
            Err(error) => {
                inner.unreadable = Some(error.clone());
                return Err(SequencerError::Unreadable(error));
            }
        };
        if started.elapsed() > BUILD_DEADLINE {
            let why = format!(
                "reading it took {}s, past the {}s this deployment allows",
                started.elapsed().as_secs(),
                BUILD_DEADLINE.as_secs()
            );
            inner.unreadable = Some(why.clone());
            return Err(SequencerError::Unreadable(why));
        }
        Ok(Cache { doc, reservation })
    }

    /// Drops the decoded document and gives its memory back. Typing is
    /// unaffected: nothing on the ingest path needs one, which is what lets
    /// §9.2 evict an entry that still has subscribers.
    pub async fn drop_cache(&self) {
        let mut inner = self.inner.lock().await;
        inner.cache = None;
        inner.projection = None;
    }

    /// What this sequencer is worth evicting, and the eviction itself, but
    /// only if its lock is free right now.
    ///
    /// Eviction is reached from inside `Budget::reserve`, and `reserve` is
    /// called from `build`, which its callers run with `inner` ALREADY
    /// LOCKED. An eviction pass that awaited `inner` would therefore
    /// deadlock against the very task that asked for the memory:
    /// `tokio::sync::Mutex` is not reentrant, the task would never
    /// resolve, the lock would never be released, and the reservation its
    /// cache already holds would never be returned -- one stuck document
    /// permanently eating the process budget, and every later ingest,
    /// join and flush on it blocked behind the same lock.
    ///
    /// `try_lock` makes that structurally impossible. Skipping a busy
    /// sequencer is also the right answer on its own terms: a sequencer
    /// whose lock is held is a sequencer somebody is using, and §9.2 asks
    /// for the COLDEST entries first.
    pub(super) fn try_evict(&self) -> Option<u64> {
        let mut inner = self.inner.try_lock().ok()?;
        inner.cache.as_ref()?;
        let bytes = inner.log_bytes();
        inner.cache = None;
        inner.projection = None;
        Some(bytes)
    }

    /// Whether anybody is subscribed, for eviction's cold-first ordering.
    /// `None` when the lock is busy, for the reason in [`Self::try_evict`].
    pub(super) fn try_subscribers(&self) -> Option<usize> {
        self.inner
            .try_lock()
            .ok()
            .map(|inner| inner.subscribers.len())
    }

    pub async fn is_warm(&self) -> bool {
        self.inner.lock().await.cache.is_some()
    }

    pub async fn unreadable(&self) -> Option<String> {
        self.inner.lock().await.unreadable.clone()
    }

    /// Clears the unreadable mark. An operator does this after raising a
    /// limit; it also happens naturally after an editor trims the document.
    pub async fn recover(&self) {
        self.inner.lock().await.unreadable = None;
    }

    /// Whether this sequencer has lost the writer lease (§10). What
    /// `Registry::housekeep` polls to know which sequencers to drop from
    /// `resident`, so the next `get` re-admits from storage instead of
    /// reaching this stale instance again.
    pub async fn fenced(&self) -> Option<String> {
        self.inner.lock().await.fenced.clone()
    }

    /// §10: "refuse ingest and commands, close sockets; a new owner loads
    /// sequencers from the log". Marks the sequencer so `ingest` and
    /// `command` refuse from here on, then closes every subscriber's socket
    /// directly -- there is no need to wait for that socket's own next
    /// message to discover the fence, and a client left open would sit
    /// believing itself synced against a process that can no longer make
    /// anything durable for it.
    ///
    /// Idempotent: called from `flush`, which may be retried by
    /// `Registry::housekeep` before the sequencer is dropped from
    /// `resident`, and calling it twice just re-sends `Close` to whatever
    /// subscribers (normally none, the second time) remain.
    async fn close_fenced(&self, why: String) {
        self.inner.lock().await.fence(why);
    }

    /// The log as it stands, for compaction and for the cost report.
    pub async fn log_state(&self) -> LogState {
        let inner = self.inner.lock().await;
        LogState {
            next_sequence: inner.next_sequence,
            log_vector: inner.log_vector.encode(),
            head_vector: inner.head_vector.encode(),
            buffered: inner.buffer.len(),
            buffered_charge: inner.charged_bytes,
            log_bytes: inner.log_bytes(),
            subscribers: inner.subscribers.len(),
            warm: inner.cache.is_some(),
        }
    }

    /// §8.4 step 2: an entry whose `oplog_vv()` equals the log vector -- a
    /// warm entry at head with an empty buffer -- and the snapshot exported
    /// from it. An entry that is ahead is not used; the caller waits for the
    /// next opportunity.
    pub async fn snapshot_at_log_vector(&self, mode: SnapshotMode) -> Result<Option<(i64, Vec<u8>, Vec<u8>, usize)>> {
        let mut inner = self.inner.lock().await;
        inner.writable()?;
        if !inner.buffer.is_empty() {
            return Ok(None);
        }
        if let Some(why) = &inner.unreadable {
            return Err(SequencerError::Unreadable(why.clone()));
        }
        if inner.cache.is_none() {
            let cache = self.build(&mut inner).await?;
            inner.cache = Some(cache);
        }
        let through = inner.next_sequence - 1;
        let log_vector = inner.log_vector.clone();
        let cache = inner.cache.as_ref().expect("a cache was just built");
        if cache.doc.oplog_vv() != log_vector {
            return Ok(None);
        }
        let changes = cache.doc.len_changes();
        // §9.3 counts a compaction export against the same bound as a
        // build: it is the same uninterruptible Loro work over the same
        // decoded document, and a deployment compacting four documents at
        // once has no cores left for anything else. `inner` is still held,
        // which is what keeps the cloned handle at the vector just read:
        // nothing can import into this entry until this returns.
        let doc = cache.doc.clone();
        let _turn = super::admission::heavy().await;
        let snapshot = tokio::task::spawn_blocking(move || {
            match mode {
                SnapshotMode::Full => doc.export(ExportMode::Snapshot),
                SnapshotMode::Shallow => {
                    doc.export(ExportMode::ShallowSnapshot(std::borrow::Cow::Owned(
                        doc.oplog_frontiers(),
                    )))
                }
            }
        })
        .await
        .map_err(|error| SequencerError::Loro(error.to_string()))?
        .map_err(|error| SequencerError::Loro(error.to_string()))?;
        Ok(Some((through, snapshot, log_vector.encode(), changes)))
    }

    /// §8.4 step 5's lock. Everything that reads the base and the rows
    /// together is held off until the returned guard is dropped.
    ///
    /// Activating a base is two writes that must look like one: the
    /// catalogue's transaction (new base row, rows `<= through` deleted,
    /// counters recomputed) and this sequencer's own copy of what the base
    /// covers and weighs. Between them the two disagree, and both readers
    /// that mix them break on exactly that disagreement:
    ///
    /// * `join` reads the row coverage from storage, then this sequencer's
    ///   `has_base`/`base_vector`, then the rows. A base activated in
    ///   between leaves it reading a coverage list whose rows are gone and a
    ///   `has_base` that is still false, so it answers a fresh client with
    ///   the head vector, no base and no rows -- the client believes itself
    ///   synced and never asks again. This is the same loss the comment in
    ///   `join` already describes for a concurrent flush.
    /// * `build` reads `has_base`, then the base, then the rows, and would
    ///   cache a document missing everything the deleted rows held.
    ///
    /// So both locks are taken, in the order every other writer takes them
    /// (`transaction`, then `inner`): `join` and `command` are held off by
    /// the first, `build` and every other cache path by the second. Nothing
    /// under this guard may await anything that wants either lock again, and
    /// in particular nothing may reach `Budget::reserve` -- `build` runs with
    /// `inner` held and evicts through it, which is the deadlock shape
    /// `try_evict` documents.
    ///
    /// The blob is deliberately NOT written under this: it is compression
    /// and an object-store round trip over as much as 32 MiB, and typing
    /// would stall for the whole of it. A base nothing points at yet is
    /// inert, so the caller writes it first and activates it here.
    pub(crate) async fn compaction_gate(&self) -> CompactionGate<'_> {
        let transaction = self.transaction.lock().await;
        let inner = self.inner.lock().await;
        CompactionGate {
            _transaction: transaction,
            inner,
            slug: &self.slug,
            owner_id: self.owner_id,
            ledger: self.ledger.clone(),
        }
    }

    async fn read_base(&self, expanded_limit: u64) -> Result<Vec<u8>> {
        self.catalog
            .read_base(self.document_id, self.blobs.clone(), expanded_limit)
            .await
            .map_err(SequencerError::Loro)
    }
}

/// Both of a sequencer's locks, held for the whole of §8.4 step 5. See
/// [`Sequencer::compaction_gate`].
pub(crate) struct CompactionGate<'a> {
    _transaction: tokio::sync::MutexGuard<'a, ()>,
    inner: tokio::sync::MutexGuard<'a, Inner>,
    slug: &'a str,
    owner_id: Uuid,
    ledger: Arc<super::ledger::StorageLedger>,
}

impl CompactionGate<'_> {
    /// Moves the sequencer's accounting onto the base the caller just
    /// activated, before either lock is released.
    ///
    /// `row_bytes` and `uncompacted_count` are NOT zeroed here. Compaction
    /// is not atomic with the log
    /// (§8.4): it flushes, takes `through`, builds and proves a snapshot,
    /// writes a blob, and only then activates the base. Typing continues
    /// throughout by design, so rows above `through` can and do land in the
    /// meantime. `activate_log_base` deletes only rows `<= through` and
    /// recomputes `documents.uncompacted_update_bytes` in SQL from what is
    /// left, which is the only place that count can be read correctly --
    /// this sequencer has no way to tell, from in here, which of the rows
    /// landed after `through` was taken. The activation transaction returns
    /// both recomputed counters to the caller, which passes them here rather
    /// than doing a separate post-commit read or assuming zero. Getting this wrong would
    /// under-report `log_bytes()` from this moment on, compounding across
    /// compactions, which under-enforces the §9.1 log quota and reserves too
    /// little for a build under the §9.2 budget.
    pub(crate) fn note_compacted(
        &mut self,
        through: i64,
        vector: &[u8],
        base_bytes: u64,
        row_bytes: u64,
        uncompacted_count: i64,
    ) {
        let before = self.inner.base_bytes.saturating_add(self.inner.row_bytes);
        let after = base_bytes.saturating_add(row_bytes);
        if let Ok(vector) = decode_vector(vector) {
            self.inner.base_vector = vector;
        }
        self.inner.has_base = true;
        self.inner.base_bytes = base_bytes;
        self.inner.row_bytes = row_bytes;
        self.inner.uncompacted_count = uncompacted_count;
        self.ledger.charge(
            self.owner_id,
            after as i64 - before as i64,
        );
        ::log::debug!("{} compacted through {through}", self.slug);
    }

    /// Prevents this instance from serving state when an activation may
    /// have committed but its durable result cannot be read back.
    pub(crate) fn fence(&mut self, why: String) {
        self.inner.fence(why);
    }

    /// Rebuilds the resident document from a shallow base, dropping the cache
    /// and closing every subscriber so it rejoins from that base.
    ///
    /// The resident document still holds the history the base no longer has,
    /// so it is dropped and rebuilt from the shallow base, and every subscriber
    /// rejoins from that base.
    pub(crate) fn reset_after_trim(&mut self, why: &str) {
        self.inner.cache = None;
        self.inner.projection = None;
        let payload = format!("{why}; reconnect");
        for (_, subscriber) in self.inner.subscribers.drain() {
            let _ = subscriber.tx.force_close(payload.clone());
        }
    }
}

impl Sequencer {
    fn note_source_changed(&self, inner: &mut Inner) {
        let due = inner
            .last_source_changed
            .is_none_or(|at| at.elapsed() >= SOURCE_CHANGED_INTERVAL);
        if due {
            self.emit_source_changed(inner);
        } else {
            inner.source_changed_owed = true;
        }
    }

    /// §4.5: a reader is told the digest, never the bytes. The digest is
    /// computed only from a cache that is already resident, and is `null`
    /// otherwise -- §5 and §4.3 keep the typing path header-only, so `ingest`
    /// must never build a `LoroDoc` to fill this in.
    ///
    /// Deviation from the spec text, stated deliberately: §4.5 says a reader
    /// "otherwise receives it on flush", and this notifies on the ordinary
    /// throttled path as well. The reason for deferring was the digest
    /// computation, and when the document is cold there is no longer one to
    /// defer. Deferring instead means never: per §9.2 an evicted, reader-only
    /// document is the steady state, so the old early return left exactly the
    /// readers who most need the notification with none, on ingest and on
    /// flush alike. `note_source_changed`'s one-per-second coalescing still
    /// bounds the frame rate, and the first refetch a notified reader makes
    /// warms the cache, so later frames carry a real digest.
    ///
    /// §9.2 lists "projection output buffers" as one of the four things the
    /// budget accounts for, and this runs on ordinary typing traffic
    /// (throttled to once a second by `note_source_changed`, but still: no
    /// semantic command gates it). It takes no reservation of its own on
    /// purpose. The `map` below only ever calls `project` on `cache.doc`,
    /// and a cache only exists holding the reservation `build` took for it
    /// (`Cache::reservation`); when there is no cache nothing is projected
    /// at all. So the buffer this allocates is a transient
    /// view derived from, and bounded above by, a document whose own
    /// resident cost the budget already charged -- there is no path here
    /// that manufactures projection output the budget has not already
    /// admitted. A separate reservation would double-count that cost rather
    /// than add a real one.
    fn emit_source_changed(&self, inner: &mut Inner) {
        let digest: Option<String> = inner.cache.as_ref().map(|cache| {
            librepaper_document_core::project(&cache.doc, &self.config.paths())
                .projection
                .digest()
        });
        // Set unconditionally: keeping this on the success path alone meant a
        // cold document never engaged the one-per-second throttle, so every
        // ingest called in and bailed.
        inner.last_source_changed = Some(Instant::now());
        inner.source_changed_owed = false;
        let payload = json!({"type": "source-changed", "digest": digest}).to_string();
        fan_out(inner, false, |_, subscriber| {
            (subscriber.role == Role::Reader).then(|| Outgoing::shared_text(payload.clone()))
        });
    }
}

/// What the log looks like from outside, for compaction, eviction and the
/// cost report.
#[derive(Clone, Debug)]
pub struct LogState {
    pub next_sequence: i64,
    pub log_vector: Vec<u8>,
    /// The log vector plus the buffer: what this document would be at if
    /// every buffered batch were written. A refused update is never merged
    /// into it, which is what lets a test say "nothing was accepted" by
    /// comparing this across the refusal.
    pub head_vector: Vec<u8>,
    pub buffered: usize,
    /// What the buffer costs the deployment pending pool: payload and
    /// framing (SPEC-frugal §2).
    pub buffered_charge: usize,
    pub log_bytes: u64,
    pub subscribers: usize,
    pub warm: bool,
}

/// §4.5, "a subscriber whose queue overflows is closed and reconnects", and
/// §10, "subscriber too slow: closed; reconnects and reconciles".
///
/// Every subscriber removal in this file has the same cause: the queue would
/// not take the frame. Dropping the subscription alone is not what the spec
/// asks for, and it is the worst outcome available -- the socket stays open
/// and goes on answering pings while it receives no relay and no
/// acknowledgements, so the person sees a connected editor that has quietly
/// stopped collaborating. Nothing on the socket side would notice either:
/// §12 deleted the per-socket housekeeping tick that used to re-check room
/// membership and cut the transport, so the close has to originate here.
///
/// The close goes through `force_close`, which bypasses the queue budget and
/// lands in the slot reserved for it, so termination is guaranteed rather
/// than best-effort: the frame that stops a drowning subscriber must not have
/// to win admission against the traffic that drowned it.
///
/// Guaranteed is not instant. The close is FIFO behind whatever real backlog
/// the subscriber has already accepted, and it reaches the wire only once the
/// writer has worked through it -- each of those items bounded in turn by
/// `SOCKET_WRITE_TIMEOUT`, so the wait is bounded, not open-ended.
fn drop_slow(inner: &mut Inner, closed: Vec<u64>) {
    for id in closed {
        let Some(subscriber) = inner.subscribers.remove(&id) else {
            continue;
        };
        let _ = subscriber.tx.force_close("subscriber too slow; reconnect");
    }
}

/// Sends one frame to every subscriber `frame` produces one for, and closes
/// the ones whose queue is full.
///
/// Every fan-out in this module had this loop, and each had its own copy of
/// the slow-peer collection and the `drop_slow` that goes with it. Missing
/// either leaves a peer that has stopped reading holding a socket for good,
/// which is exactly the failure a bounded queue exists to prevent.
///
/// `durable` picks the send: a frame the peer must not miss -- an
/// acknowledgement, a durability announcement -- goes through
/// `try_send_durable`, which is admitted past the ordinary queue budget.
fn fan_out(inner: &mut Inner, durable: bool, frame: impl Fn(u64, &Subscriber) -> Option<Outgoing>) {
    let mut closed = Vec::new();
    for (id, subscriber) in inner.subscribers.iter() {
        let Some(outgoing) = frame(*id, subscriber) else {
            continue;
        };
        let sent = if durable {
            subscriber.tx.try_send_durable(outgoing)
        } else {
            subscriber.tx.try_send(outgoing)
        };
        if sent.is_err() {
            closed.push(*id);
        }
    }
    drop_slow(inner, closed);
}

/// Copies one frame to every editor but the sender.
fn relay(inner: &mut Inner, skip: Option<u64>, bytes: &[u8]) {
    use base64::Engine as _;
    let payload = json!({
        "type": "doc-update",
        "update": base64::engine::general_purpose::STANDARD.encode(bytes),
    })
    .to_string();
    fan_out(inner, false, |id, subscriber| {
        (Some(id) != skip && subscriber.role == Role::Editor)
            .then(|| Outgoing::shared_text(payload.clone()))
    });
}

/// `doc-ack {upTo}` to one peer, for a batch that never reached the buffer
/// because it held no changes. Every socket that peer has open is told, the
/// same as an ordinary acknowledgement: the map being cleared is the
/// person's, not the socket's.
fn acknowledge_peer(inner: &mut Inner, peer_key: &str, up_to: i64) {
    let payload = json!({"type": "doc-ack", "upTo": up_to}).to_string();
    let mut closed = Vec::new();
    for (id, subscriber) in inner.subscribers.iter() {
        if subscriber.peer_key != peer_key {
            continue;
        }
        if subscriber
            .tx
            .try_send_durable(Outgoing::Text(payload.clone()))
            .is_err()
        {
            closed.push(*id);
        }
    }
    drop_slow(inner, closed);
}

/// `doc-ack {upTo}` to each peer whose batches were in the row, where `upTo`
/// is that peer's highest `client_seq` in it. Acknowledgements cover a
/// contiguous prefix of a peer's session, and not because a refusal ends the
/// session: `Retryable` and `Refused` leave the socket open (only
/// `MAX_CONSECUTIVE_REFUSALS` of them closes it), so the peer goes on
/// sending. It holds because a refused batch is never merged into
/// `head_vector` -- `ingest` returns before that merge -- so the peer's next
/// batch starts from a vector the head does not cover and comes back `Gap`
/// rather than being appended past the hole. The peer recovers by
/// re-exporting from the vector the gap reports, under a new `client_seq`,
/// and it is that batch a row can hold (§5.1).
fn acknowledge(inner: &mut Inner, acknowledged: &[(String, i64)]) {
    let highest = ack_targets(
        acknowledged
            .iter()
            .map(|(peer_key, client_seq)| (peer_key.as_str(), *client_seq)),
    );
    fan_out(inner, true, |_, subscriber| {
        let up_to = highest.get(subscriber.peer_key.as_str())?;
        Some(Outgoing::Text(
            json!({"type": "doc-ack", "upTo": up_to}).to_string(),
        ))
    });
}

/// Announces the vector through the row that has just committed. This is
/// separate from `doc-ack`: acknowledgements retire a sender's transmission
/// bookkeeping, while this frame confirms that the document's durable log
/// reached this vector. It goes to every editor, including the
/// editor whose batches were in the row; readers receive source projections
/// rather than source-log state.
fn notify_durable(inner: &mut Inner) {
    use base64::Engine as _;
    let payload = json!({
        "type": "doc-durable",
        "vector": base64::engine::general_purpose::STANDARD.encode(inner.log_vector.encode()),
    })
    .to_string();
    fan_out(inner, true, |_, subscriber| {
        (subscriber.role == Role::Editor).then(|| Outgoing::shared_text(payload.clone()))
    });
}

/// Each peer's highest `client_seq` among the batches: what a single
/// `doc-ack` names for that peer. A row commonly holds several batches from
/// the same session, and one `upTo` covers all of them, which is the whole
/// of what "acknowledgements name a contiguous prefix" means (§5.1, §14.2).
/// Pulled out of `acknowledge` so that claim can be checked without a
/// subscriber or a database: nothing here is I/O.
/// Takes attribution alone, not whole batches: acknowledgement never needs a
/// payload, which is what lets a flush drop the encoded row -- and give its
/// scratch back -- before the acknowledgements go out.
pub(crate) fn ack_targets<'a>(
    acknowledged: impl IntoIterator<Item = (&'a str, i64)>,
) -> HashMap<&'a str, i64> {
    let mut highest: HashMap<&str, i64> = HashMap::new();
    for (peer_key, client_seq) in acknowledged {
        let entry = highest.entry(peer_key).or_insert(i64::MIN);
        *entry = (*entry).max(client_seq);
    }
    highest
}

/// A retryable refusal, with the head vector the client should re-export
/// from.
///
/// Every caller is a path that returns BEFORE merging the batch into
/// `head_vector`, which is what makes that vector the right answer: exporting
/// from it resends exactly the refused work. Taking it here, in one place,
/// is also how that invariant stays true -- a future refusal added after the
/// merge would have to reach past this helper to get the vector wrong.
fn retryable(inner: &Inner, why: &'static str, reason: &'static str, pressure: bool) -> Ingested {
    Ingested::Retryable(Retry {
        why,
        reason,
        pressure,
        vector: inner.head_vector.encode(),
    })
}

fn covers(holder: &VersionVector, wanted: &VersionVector) -> bool {
    matches!(
        holder.partial_cmp(wanted),
        Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
    )
}

fn decode_vector(bytes: &[u8]) -> Result<VersionVector> {
    if bytes.is_empty() {
        return Ok(VersionVector::default());
    }
    VersionVector::decode(bytes).map_err(|error| SequencerError::Loro(error.to_string()))
}

/// §9.2: "temporary forks, each reserving the parent's estimate." Exercised
/// here, against a bare `Head` and a `Budget`, rather than through
/// `Sequencer::command`, because `command` needs a real
/// `sqlx::Transaction` to reach `Head::prepare` at all (§7 steps 2 to 4 run
/// in one call) -- there is no fake for that, and adding one would end up
/// testing the transaction plumbing instead of the reservation. Building a
/// `Head` by hand is possible only from inside this module, because its
/// non-`pub` fields are module-private; that is also why this lives here
/// and not in `log::tests`, which is a sibling module of `sequencer`, not a
/// descendant of it.
#[cfg(test)]
mod prepare_budget_tests {
    use super::*;

    fn head_for<'a>(
        doc: &'a LoroDoc,
        config: &'a Configuration,
        budget: &Arc<Budget>,
        estimate: u64,
    ) -> Head<'a> {
        Head {
            doc,
            rules_owner: config,
            vector: doc.oplog_vv(),
            frontier: doc.oplog_frontiers(),
            projection: Arc::new(librepaper_document_core::project(doc, &config.paths())),
            budget: budget.clone(),
            estimate,
        }
    }

    #[test]
    fn prepare_reserves_the_parents_estimate_and_releases_it_only_when_the_prepared_source_drops() {
        let config = Configuration::default();
        let doc = crate::document::session::new_doc();
        let budget = Budget::new(1000, 1);
        // Reserve everything but exactly the fork's estimate first, so the
        // assertion below proves `prepare` asked for `estimate` (not some
        // other number, and not nothing).
        let other = budget
            .try_reserve(900)
            .expect("room for the other reservation");
        let head = head_for(&doc, &config, &budget, 100);

        let prepared = head
            .prepare(1, |draft| {
                crate::document::session::put_text(draft, "a.txt", "hello");
                Ok(())
            })
            .expect("the edit itself does not fail")
            .expect("a real edit produces a batch, so prepare returns Some");
        assert_eq!(
            budget.used(),
            1000,
            "the fork's reservation (100) sits on top of the unrelated one (900)"
        );
        drop(prepared);
        assert_eq!(
            budget.used(),
            900,
            "dropping the prepared source releases exactly the fork's reservation, \
             not before it is dropped and not more than it took"
        );
        drop(other);
    }

    #[test]
    fn prepare_refuses_rather_than_forking_when_there_is_no_room_for_the_estimate() {
        let config = Configuration::default();
        let doc = crate::document::session::new_doc();
        let budget = Budget::new(100, 1);
        let _held = budget
            .try_reserve(100)
            .expect("room for the only reservation");
        let head = head_for(&doc, &config, &budget, 50);

        let error = head
            .prepare(1, |draft| {
                crate::document::session::put_text(draft, "a.txt", "hello");
                Ok(())
            })
            .expect_err("no room is left for the fork's reservation");
        assert!(
            error.contains("memory"),
            "the refusal should say why, not just that it failed: {error}"
        );
    }

    #[test]
    fn a_no_op_edit_still_reserves_and_releases_rather_than_skipping_the_budget() {
        // §9.2's reservation covers the fork itself, which exists (and
        // costs memory) whether or not the edit inside it turns out to
        // change anything. An idempotent retry of a source-producing
        // command is exactly this case (see the `Head::prepare` doc
        // comment on `ImportBlobMetadata::change_num`), and it must not be
        // a free way to fork the document.
        let config = Configuration::default();
        let doc = crate::document::session::new_doc();
        let budget = Budget::new(1000, 1);
        let head = head_for(&doc, &config, &budget, 100);

        let prepared = head
            .prepare(1, |_draft| Ok(()))
            .expect("a no-op edit is not an error");
        assert!(
            prepared.is_none(),
            "nothing changed, so there is no batch to carry"
        );
        assert_eq!(
            budget.used(),
            0,
            "the reservation taken for the fork was released when it dropped at the end of \
             prepare, along with the fork itself"
        );
    }
}
