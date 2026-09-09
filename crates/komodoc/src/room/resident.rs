//! The resident-memory estimate `RoomSet` admission is decided on.
//!
//! This is an admission measure, not an allocator reading and not the storage
//! quota: it says roughly how much of the deployment's memory budget a room
//! occupies while it is cached, so a room cannot bypass the budget by holding
//! its bulk in structures the session byte count ignores. Every component is
//! added with saturating arithmetic, and a component that cannot be measured
//! counts as `usize::MAX` so an unmeasurable room is refused rather than
//! silently admitted.

use super::RoomState;
use crate::document::session;

/// Counters the tests use to prove that an unchanged room is estimated
/// without re-encoding the document or re-serializing its metadata.
///
/// Thread-local rather than global: a `#[tokio::test]` runs its whole body on
/// one thread, so a counting test sees only its own work even while the rest
/// of the suite estimates other rooms in parallel.
#[cfg(test)]
pub(super) mod counters {
    use std::cell::Cell;

    thread_local! {
        /// Full `encode_state` calls made by the estimate.
        static ENCODES: Cell<usize> = const { Cell::new(0) };
        /// `serde_json` passes over the comments or the manifest.
        static SERIALIZATIONS: Cell<usize> = const { Cell::new(0) };
    }

    pub fn reset() {
        ENCODES.with(|count| count.set(0));
        SERIALIZATIONS.with(|count| count.set(0));
    }

    pub fn encodes() -> usize {
        ENCODES.with(Cell::get)
    }

    pub fn serializations() -> usize {
        SERIALIZATIONS.with(Cell::get)
    }

    pub fn note_encode() {
        ENCODES.with(|count| count.set(count.get() + 1));
    }

    pub fn note_serialization() {
        SERIALIZATIONS.with(|count| count.set(count.get() + 1));
    }
}

#[cfg(test)]
pub(super) use counters::{note_encode, note_serialization};

#[cfg(not(test))]
pub(super) fn note_encode() {}

#[cfg(not(test))]
pub(super) fn note_serialization() {}

impl RoomState {
    /// The estimate itself, with the state lock already held.
    pub(super) fn resident_estimate(&mut self) -> usize {
        let generation = self.session.generation;
        let session = match self.session.encoded_size {
            Some((encoded, size)) if encoded == generation => size.max(0) as usize,
            _ => {
                note_encode();
                let size = session::encode_state(&self.session.doc).len();
                self.session.note_encoded_len(generation, size);
                size
            }
        };
        let comments = serde_json::to_vec(&self.comments)
            .map(|bytes| bytes.len())
            .unwrap_or(usize::MAX);
        note_serialization();
        let manifest = serde_json::to_vec(&self.manifest)
            .map(|bytes| bytes.len())
            .unwrap_or(usize::MAX);
        note_serialization();
        session
            .saturating_add(comments)
            .saturating_add(manifest)
            .saturating_add(self.session.asset_sizes.len() * std::mem::size_of::<(String, i64)>())
            .saturating_add(
                self.session.rendering_sizes.len() * std::mem::size_of::<(String, i64)>(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Configuration;
    use crate::document::history::Checkpoint;
    use crate::room::{Comment, RoomSet};
    use crate::storage::blob::FsStore;

    use std::sync::Arc;

    /// A room carrying the profiling workload: a thousand comments with
    /// kibibyte bodies and sixty-four checkpoints, which is what a long-lived
    /// reviewed document looks like by the time admission starts scanning it.
    pub(super) async fn loaded_room() -> (tempfile::TempDir, Arc<crate::room::Room>) {
        let directory = tempfile::tempdir().unwrap();
        let rooms = RoomSet::new(
            Arc::new(FsStore::new(directory.path())),
            Arc::new(Configuration::default()),
        );
        let room = rooms.get("doc").await;
        {
            let mut state = room.state.lock().await;
            for seq in 0..1000 {
                state.comments.push(Comment {
                    id: format!("comment-{seq}"),
                    seq,
                    body: "x".repeat(1024),
                    creator: "reviewer".into(),
                    created: "2026-01-01T00:00:00Z".into(),
                    ..Default::default()
                });
            }
            for seq in 0..64 {
                state.manifest.checkpoints.push(Checkpoint {
                    sha: format!("event-{seq}"),
                    at: "2026-01-01T00:00:00Z".into(),
                    by: "editor".into(),
                    why: "quiet".into(),
                    ..Default::default()
                });
            }
        }
        (directory, room)
    }

    /// Not a pass/fail assertion: run it with
    /// `cargo test -p komodoc --lib -- --ignored --nocapture
    /// room::resident::tests::profile_unchanged_resident_metadata`
    /// and read the printed counts. It exists so the cost this track was
    /// gated on can be reproduced rather than remembered.
    #[tokio::test]
    #[ignore = "profiling workload; run with --ignored --nocapture"]
    async fn profile_unchanged_resident_metadata() {
        let (_directory, room) = loaded_room().await;
        counters::reset();
        let started = std::time::Instant::now();
        let mut bytes = 0usize;
        for _ in 0..200 {
            bytes = room.resident_bytes().await;
        }
        let elapsed = started.elapsed();
        println!(
            "200 unchanged estimates: {elapsed:?}, {} encodes, {} serializations, {bytes} bytes",
            counters::encodes(),
            counters::serializations(),
        );
    }
}
