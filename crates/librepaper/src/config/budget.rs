//! The formulas that turn the log quota into memory and scratch bounds.
//!
//! They live beside `Configuration` because the configuration derives its
//! defaults and validates its limits from them; the log modules re-export
//! them under their old names.

use futures_util::future::BoxFuture;
use tokio::sync::Notify;

/// How much larger a decoded document is than the bytes it was loaded from.
///
/// §14.1 says the spike sets this. The spike has now been run
/// (`storage::postgres::benchmarks::typing_throughput_release_benchmark`,
/// twenty resident 1 MiB documents, real RSS, release build) and it
/// measured two figures rather than one, because a document's decoded cost
/// tracks its OPERATION COUNT as much as its byte count and the byte count
/// is all the estimate has to go on. Across a full ten-minute run and two
/// shorter ones:
///
/// | shape | factor |
/// |---|---|
/// | arrived as one upload | 1.66 to 2.34 |
/// | same bytes, 2000 accumulated edits | 3.24 to 3.43 |
///
/// The upload figure is the noisy one, which is expected: it is the smaller
/// delta of the two and so the one page-level allocator behaviour moves
/// most. The typed figure, which is the one that matters because it is
/// worse, held within six per cent across runs.
///
/// Six is kept. It is no longer a guess: it covers the worse measured shape
/// with room for the one thing the spike could not bound, which is a
/// document whose op history is far longer than two thousand edits. The
/// asymmetry is the reason to keep the margin -- reserving too much only
/// makes a read wait or answer `busy`, while reserving too little puts the
/// process out of memory, and §9.2 has no way to recover from the second.
///
/// Lowering it is a real option once somebody measures a long-lived
/// document: at 4 a 512 MiB deployment holds about half again as many
/// documents resident. Re-run the benchmark before changing the number,
/// and read `expansion_measurement` in its report rather than the single
/// `measured_expansion_factor`, which averages the two shapes.
pub const DEFAULT_EXPANSION: u64 = 6;

/// What a cold build costs in memory while it is running, as a multiple of
/// log bytes, reserved for the duration of the import and then released.
///
/// `DEFAULT_EXPANSION` is what a decoded document costs once it sits in the
/// cache. It is not what producing that document costs while the import is
/// in flight: `Sequencer::build` (sequencer.rs) imports row by row into a
/// fresh `LoroDoc`, and that process peaks before it settles there.
///
/// Measured 2026-09-20, release build, as peak RSS during a cold build
/// minus RSS before it began. That is the WHOLE cost of the build,
/// residency included, not the part standing above residency: the process
/// does not offer a way to separate the two, because the document being
/// built is what most of the peak is. Reserving this ON TOP OF the resident
/// estimate therefore over-reserves, by roughly the resident figure itself.
/// That is the direction to be wrong in, and it is only true while the
/// build runs.
///
/// | log size | measured peak, above pre-build RSS |
/// |---|---|
/// | 1 MiB | 3.2 MB |
/// | 2 MiB | 16 MB |
/// | 3.5 MiB | 27 MB |
///
/// The 1 MiB figure is the noisy one (small absolute numbers move a lot
/// relatively); the two larger ones agree at roughly 7 to 8 times log
/// bytes. Eight is kept, for the same reason `DEFAULT_EXPANSION` rounds up
/// rather than down: reserving too much only makes a build wait or answer
/// `busy`, reserving too little runs the process out of memory during
/// exactly the operation that cannot be interrupted (§9.3).
///
/// `Sequencer::build` reserves this on top of the resident estimate for the
/// duration of the import only, as a second reservation released the
/// moment the entry is cached -- the resident estimate is what the cache
/// keeps paying for afterwards, and the two must not be summed into one
/// long-lived reservation or every cached document would overpay for a cost
/// it no longer carries.
pub const BUILD_TRANSIENT_EXPANSION: u64 = 8;

/// What one document is expected to cost resident, from what its log weighs.
pub fn estimate(encoded_bytes: u64, expansion: u64) -> u64 {
    // A floor, because a document of three keystrokes still costs a Loro
    // document's fixed structures, and a reservation of nearly nothing would
    // let an unbounded number of them in.
    const FLOOR: u64 = 64 * 1024;
    encoded_bytes.saturating_mul(expansion).max(FLOOR)
}

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
    let single =
        max_update_bytes(log_quota_bytes) + BATCH_HEADER_BYTES + MAX_PEER_KEY;
    if single > ceiling {
        single
    } else {
        ceiling
    }
}

/// The largest row an ordinary flush can write: one version byte over a
/// document's whole pending charge.
pub fn max_row_bytes(log_quota_bytes: usize) -> usize {
    ROW_HEADER_BYTES + max_pending_charge(log_quota_bytes)
}

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

/// Room for small per-write allocations that are not the row itself: the
/// encoded version vector, the format and main-path strings stamped on the
/// document row, and the `FlushRow` around them. None of them scales with the
/// buffer; a fixed allowance is honest and keeps the estimate readable.
pub const SCRATCH_SLACK: u64 = 8 * 1024;

/// SQLx 0.8.6 copies the row into PgArguments and again into the
/// connection's Bind write buffer. Each Vec may grow to twice its needed
/// size. The connection guard holds this allowance until those buffers are
/// shrunk on success or destroyed on error/cancellation.
pub fn driver_scratch_for(row_bytes: u64) -> u64 {
    row_bytes.saturating_add(SCRATCH_SLACK).saturating_mul(4)
}

/// One exactly preallocated row plus both driver buffers and their capacity
/// growth. Small per-query fields are included in each driver's allowance.
pub fn scratch_for(row_bytes: u64) -> u64 {
    row_bytes.saturating_add(driver_scratch_for(row_bytes))
}

/// What one batch's header costs, on top of its payload:
/// `[peer_key_len u16][client_seq u64][bytes_len u32]`, and then the key
/// itself. Named rather than spelled `14` in three places, because the
/// pending accounting charges it -- a buffer of very many very small
/// updates is mostly this, and an estimate that ignored it would
/// under-reserve the row it is about to write by however many batches went
/// into it.
pub const BATCH_HEADER_BYTES: usize = 2 + 8 + 4;

/// The longest peer key a frame may claim: a peer key is an account id or a
/// deployment marker.
pub const MAX_PEER_KEY: usize = 256;

/// What the version byte costs. A row is this plus each batch's payload and
/// its framing overhead, exactly -- which is what lets a flush reserve its
/// scratch from the buffer's own accounting rather than by encoding first
/// and measuring afterwards.
pub const ROW_HEADER_BYTES: usize = 1;
