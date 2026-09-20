//! Ephemeral presence: who is here now, tracked with Loro's EphemeralStore.
//!
//! Each peer's presence is a LoroValue held in the store under a string key
//! (the peer ID). Entries are considered expired 30 seconds after last update;
//! call `remove_outdated` to purge them and notify subscribers. This is
//! transient metadata -- not stored with the document, not replayed to new
//! arrivals, only broadcast to live peers.

use loro::awareness::{EphemeralStore, EphemeralSubscriber, LocalEphemeralCallback};
use loro::{LoroValue, Subscription};
use std::collections::HashMap;

/// How long a peer stays listed after its last word, in milliseconds.
///
/// Thirty seconds is long enough to ride out a laptop lid closing for a moment
/// and short enough that somebody who has actually gone does not sit in the
/// list looking present. The browser expires peers on the same number, so a
/// collaborator disappears from both sides at roughly the same moment rather
/// than lingering on one.
const PRESENCE_TIMEOUT_MS: i64 = 30_000;

/// Ephemeral presence state for peers in a document room.
///
/// A key-value store of peer state with timestamp-based LWW (last-write-wins)
/// semantics and a 30-second timeout. Used for transient data like cursors,
/// selections, user names, colors. Presence is never persisted; it is broadcast
/// to live peers as it changes and discarded when peers disconnect.
#[derive(Clone)]
pub struct Presence {
    store: EphemeralStore,
}

impl Presence {
    /// Create a new presence store for a room.
    ///
    /// Entries expire 30 seconds after their last update. Expired entries are
    /// omitted from `encode`/`encode_all` and are purged when `remove_outdated`
    /// is called.
    pub fn new() -> Self {
        Self {
            store: EphemeralStore::new(PRESENCE_TIMEOUT_MS),
        }
    }

    /// Set this peer's local presence state.
    ///
    /// Stores the value and increments the peer's update timestamp. Triggers
    /// local update subscribers so changes can be broadcast to other peers.
    /// The value can be any LoroValue: strings, maps, lists, etc.
    ///
    /// # Arguments
    /// - `peer_id`: the peer's numeric ID (typically a socket or session ID)
    /// - `state`: the presence state to store and broadcast
    pub fn set_local(&self, peer_id: u64, state: impl Into<LoroValue>) {
        self.store.set(&peer_id.to_string(), state);
    }

    /// Encode this peer's state for sending to others.
    ///
    /// Returns encoded bytes in the EphemeralStore format (postcard). If the
    /// peer's state has expired, returns an empty vector.
    pub fn encode_peer(&self, peer_id: u64) -> Vec<u8> {
        self.store.encode(&peer_id.to_string())
    }

    /// Encode all non-expired peers' state for a full sync.
    ///
    /// Expired entries are omitted from the result. Call `remove_outdated` first
    /// if you want to purge them from storage as well.
    pub fn encode_all(&self) -> Vec<u8> {
        self.store.encode_all()
    }

    /// Apply an update received from a peer.
    ///
    /// Merges imported state using last-write-wins semantics: if the imported
    /// state is newer (by timestamp) than the stored state, it replaces it.
    /// Subscribers are notified of any changes.
    ///
    /// # Errors
    /// Returns an error if the bytes do not decode.
    pub fn apply(&self, data: &[u8]) -> Result<(), Box<str>> {
        self.store.apply(data)
    }

    /// Remove peers whose state has expired (past the 30-second timeout).
    ///
    /// This must be called periodically to actually purge expired entries from
    /// storage. Each call triggers a `Timeout` event for removed peers, notifying
    /// subscribers so the room can broadcast departure to other peers.
    pub fn remove_outdated(&self) {
        self.store.remove_outdated()
    }

    /// Get the presence state of a peer.
    pub fn get_peer_state(&self, peer_id: u64) -> Option<LoroValue> {
        self.store.get(&peer_id.to_string())
    }

    /// Get all currently-stored peer state.
    ///
    /// Returns a map from peer ID (as string) to LoroValue. Includes expired
    /// but not-yet-purged entries. Call `remove_outdated` first if you want to
    /// exclude those.
    pub fn all_states(&self) -> HashMap<String, LoroValue> {
        self.store
            .get_all_states()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Get all peer IDs currently in the store, in the order they appear.
    ///
    /// Includes expired but not-yet-purged entries. Parses the string keys as
    /// u64 peer IDs; skips any key that does not parse as a number (should not
    /// happen in normal operation).
    pub fn all_peers(&self) -> Vec<u64> {
        self.store
            .keys()
            .into_iter()
            .filter_map(|k| k.parse().ok())
            .collect()
    }

    /// Subscribe to local peer state changes.
    ///
    /// The callback receives encoded bytes ready to send to other peers whenever
    /// this peer's state changes (including deletion). The callback should return
    /// `true` to keep the subscription active, or `false` to unsubscribe.
    ///
    /// This is typically used by the room to broadcast presence updates to all
    /// other connected peers.
    pub fn subscribe_local_updates(&self, callback: LocalEphemeralCallback) -> Subscription {
        self.store.subscribe_local_updates(callback)
    }

    /// Subscribe to all state changes (local, import from peers, and timeout).
    ///
    /// The callback receives an `EphemeralStoreEvent` describing what changed:
    /// whether entries were added, updated, removed, and whether the change came
    /// from a local update, an import, or a timeout expiration. The callback
    /// should return `true` to keep the subscription active, or `false` to
    /// unsubscribe.
    pub fn subscribe(&self, callback: EphemeralSubscriber) -> Subscription {
        self.store.subscribe(callback)
    }
}

impl Default for Presence {
    fn default() -> Self {
        Self::new()
    }
}
