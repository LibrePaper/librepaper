#![allow(clippy::items_after_test_module)]

//! The resident-memory estimate `RoomSet` admission is decided on.
//!
//! This is an admission measure, not an allocator reading and not the storage
//! quota: it says roughly how much of the deployment's memory budget a room
//! occupies while it is cached, so a room cannot bypass the budget by holding
//! its bulk in structures the session byte count ignores. Every component is
//! added with saturating arithmetic, and a component that cannot be measured
//! counts as `usize::MAX` so an unmeasurable room is refused rather than
//! silently admitted.

use std::ops::{Deref, DerefMut};

use serde::{Serialize, Serializer};

use super::RoomState;
use crate::document::session;

/// A value whose serialized size the admission estimate caches, and which
/// forgets that size the moment anybody takes it mutably.
///
/// The alternative -- an `invalidate()` call beside every mutation -- is a
/// list that has to stay complete forever across every future edit to comment
/// and manifest handling; one missed call is an estimate that silently
/// undercounts a room and lets it stay resident past the budget. Routing
/// mutation through `DerefMut` makes the invariant structural instead: the
/// borrow checker will not let a caller change the value without passing
/// through the point that clears the cache. The cost is that a `&mut` borrow
/// that does not actually mutate still clears it, which only costs one
/// re-measure.
pub struct Measured<T> {
    value: T,
    /// The serialized length of `value`, or `None` when it must be measured
    /// again. `Some(usize::MAX)` records that measuring it failed, which is
    /// the estimate's conservative answer and holds until the next mutation.
    bytes: Option<usize>,
}

impl<T> Measured<T> {
    pub fn new(value: T) -> Self {
        Self { value, bytes: None }
    }
}

impl<T: Serialize> Measured<T> {
    /// The serialized length, measured at most once per mutation. A value
    /// that cannot be serialized counts as `usize::MAX`, the same refusal the
    /// estimate has always made, and that answer is cached too: nothing about
    /// the value has changed, so measuring it again would fail again.
    fn bytes(&mut self) -> usize {
        match self.bytes {
            Some(bytes) => bytes,
            None => {
                note_serialization();
                let bytes = serde_json::to_vec(&self.value)
                    .map(|bytes| bytes.len())
                    .unwrap_or(usize::MAX);
                self.bytes = Some(bytes);
                bytes
            }
        }
    }
}

impl<T> Deref for Measured<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T> DerefMut for Measured<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.bytes = None;
        &mut self.value
    }
}

impl<T: Serialize> Serialize for Measured<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.value.serialize(serializer)
    }
}

/// Kept so `for item in &state.comments` still reads the way it did; the
/// iterator borrows, so it cannot invalidate.
impl<'a, T> IntoIterator for &'a Measured<T>
where
    &'a T: IntoIterator,
{
    type Item = <&'a T as IntoIterator>::Item;
    type IntoIter = <&'a T as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        (&self.value).into_iter()
    }
}

/// The mutable counterpart, which invalidates like any other `&mut` borrow.
impl<'a, T> IntoIterator for &'a mut Measured<T>
where
    &'a mut T: IntoIterator,
{
    type Item = <&'a mut T as IntoIterator>::Item;
    type IntoIter = <&'a mut T as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.bytes = None;
        (&mut self.value).into_iter()
    }
}

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
        // Comments (with their replies) and the retained manifest are the two
        // components whose measurement is proportional to how much a busy
        // document holds, so they are the two that are cached across scans.
        // Input assets are counted from map cardinality, which costs
        // nothing to recompute and so cannot go stale.
        let comments = self.comments.bytes();
        let manifest = self.manifest.bytes();
        // The retained checkpoint tree is one entry per path, like the asset
        // map above, and is counted the same way: from cardinality, which
        // cannot go stale.
        let checkpoint_tree = self
            .session
            .checkpoint_tree
            .as_ref()
            .map_or(0, |tree| tree.files.len())
            * std::mem::size_of::<(String, crate::document::history::TreeEntry)>();
        session
            .saturating_add(comments)
            .saturating_add(manifest)
            .saturating_add(checkpoint_tree)
            .saturating_add(self.session.asset_sizes.len() * std::mem::size_of::<(String, i64)>())
    }
}
