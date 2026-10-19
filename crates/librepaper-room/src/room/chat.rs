//! Project chat history, as the room holds it.
//!
//! A room buffers the messages it receives and writes them in batches, so a
//! busy conversation costs one transaction per flush rather than one per
//! message. The buffer sits behind one mutex that is held across every
//! database call, which is what keeps a clear or a trim from racing a flush:
//! a flush that has not started sees the cleared buffer, and one that has
//! started finishes before the clear runs.
//!
//! A flush whose reply is lost can leave rows both stored and buffered. A
//! clear or trim therefore credits a buffered row only when the database did
//! not already hold it; the stored rows are credited through `chat_bytes`.
//!
//! The sequencer's storage ledger is only an estimate, so it does not decide
//! whether a message is kept. The room holds a reservation row in
//! PostgreSQL (`pending_log_reservations`, which every usage sum counts) and
//! admits a message only when the reservation covers the buffered rows plus
//! the new one. It asks for more in chunks. Writing a batch takes its charge
//! off the row in the same transaction. The row stays as headroom across
//! clears and trims, and a forced flush with nothing waiting deletes it.

use serde_json::{json, Value};
use time::OffsetDateTime;
use uuid::Uuid;

#[cfg(test)]
use librepaper_engine::storage::postgres::CHAT_ROW_OVERHEAD_BYTES;
use librepaper_engine::storage::postgres::{
    ChatCleared, ChatRecord, Error as CatalogError, CHAT_RESERVATION_CHUNK,
};

use super::{Room, WriteError};

/// Flush at once when this many messages are waiting.
pub const CHAT_FLUSH_ROWS: usize = 64;
/// Refuse new messages when this many are waiting, which means flushes fail.
pub const CHAT_BUFFER_MAX_ROWS: usize = 256;
/// How long the oldest waiting message may wait before the next pass writes it.
pub const CHAT_FLUSH_AFTER: std::time::Duration = std::time::Duration::from_secs(2);

/// Messages accepted but not yet written.
pub struct ChatBuffer {
    rows: Vec<ChatRecord>,
    /// When the oldest waiting message was queued.
    oldest: Option<tokio::time::Instant>,
    /// The id of this room's reservation row, kept for the room's life.
    reservation_key: Uuid,
    /// The bytes the reservation row holds, as last reported. It is never
    /// less than `charged()`, and the difference is the headroom.
    reserved: i64,
    /// False when a reservation call failed, because it may have committed
    /// and `reserved` may then be stale. The next use reads the row again.
    reserved_known: bool,
}

impl Default for ChatBuffer {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            oldest: None,
            reservation_key: Uuid::now_v7(),
            reserved: 0,
            reserved_known: true,
        }
    }
}

impl ChatBuffer {
    /// Drops everything waiting and returns the bytes it had reserved, which
    /// the caller must give back to the ledger. The caller holds the lock
    /// across whatever deleted the stored rows, so nothing can be written in
    /// between.
    #[must_use]
    pub fn discard(&mut self) -> i64 {
        let dropped = self.charged();
        self.rows.clear();
        self.oldest = None;
        dropped
    }

    /// The ids of the waiting messages.
    pub fn ids(&self) -> Vec<Uuid> {
        self.rows.iter().map(|row| row.id).collect()
    }

    /// Drops everything waiting and returns the bytes reserved by the rows
    /// whose ids are not in `stored`. Rows that are stored were already
    /// credited through `chat_bytes`, so crediting them again would refund
    /// them twice. Same locking rule as [`Self::discard`].
    #[must_use]
    pub fn discard_unstored(&mut self, stored: &[Uuid]) -> i64 {
        let dropped = self
            .rows
            .iter()
            .filter(|row| !stored.contains(&row.id))
            .map(ChatRecord::charged_bytes)
            .sum();
        self.rows.clear();
        self.oldest = None;
        dropped
    }

    fn charged(&self) -> i64 {
        self.rows.iter().map(ChatRecord::charged_bytes).sum()
    }

    fn holds_reservation(&self) -> bool {
        self.reserved > 0 || !self.reserved_known
    }

    fn due(&self) -> bool {
        self.rows.len() >= CHAT_FLUSH_ROWS
            || self
                .oldest
                .is_some_and(|since| since.elapsed() >= CHAT_FLUSH_AFTER)
    }
}

/// The wire object for one message. Hello, the history route and live
/// broadcasts all use it.
pub fn chat_json(record: &ChatRecord) -> Value {
    json!({
        "type": "chat",
        "id": record.id.to_string(),
        "text": record.body,
        "creator": record.author_label,
        "created": chat_timestamp(record.created_at),
    })
}

/// A message time in the same shape `util::timestamp` writes.
pub fn chat_timestamp(at: OffsetDateTime) -> String {
    librepaper_base::util::format_unix(at.unix_timestamp())
}

/// Merges stored rows with buffered ones into the newest `limit` messages,
/// oldest first. `before` keeps only buffered rows older than it, since the
/// stored page already did the same. The flag says whether older messages
/// exist: the database said so, or this merge dropped some.
pub fn merge_page(
    stored: Vec<ChatRecord>,
    stored_more: bool,
    buffered: &[ChatRecord],
    before: Option<Uuid>,
    limit: usize,
) -> (Vec<ChatRecord>, bool) {
    let mut rows = stored;
    rows.extend(
        buffered
            .iter()
            .filter(|row| before.is_none_or(|before| row.id < before))
            .cloned(),
    );
    rows.sort_by_key(|row| row.id);
    rows.dedup_by_key(|row| row.id);
    let dropped = rows.len().saturating_sub(limit);
    rows.drain(..dropped);
    (rows, stored_more || dropped > 0)
}

impl Room {
    /// Queues one message, or says why it cannot be kept. It is kept only when
    /// the PostgreSQL reservation covers it, which the room tops up first when
    /// the headroom is short, so another room of the same owner cannot spend
    /// the same allowance before the flush. The
    /// message goes to every other listener and, with `temp_id` added, back
    /// to the sender, before the lock is released, so a clear can never
    /// deliver `chat-cleared` ahead of a message it did not delete or after
    /// one it did.
    pub async fn enqueue_chat(
        &self,
        record: ChatRecord,
        sender: u64,
        temp_id: &str,
    ) -> Result<(), &'static str> {
        let mut buffer = self.chat.lock().await;
        if buffer.rows.len() >= CHAT_BUFFER_MAX_ROWS {
            return Err("chat is not being saved right now; try again shortly");
        }
        let charge = record.charged_bytes();
        if !buffer.reserved_known {
            match self
                .catalog
                .chat_reservation_bytes(self.document_id, buffer.reservation_key)
                .await
            {
                Ok(bytes) => {
                    buffer.reserved = bytes;
                    buffer.reserved_known = true;
                    self.note_chat_reservation(&buffer);
                }
                Err(error) => {
                    tracing::warn!(
                        "could not read the chat reservation of {}: {error}",
                        self.slug
                    );
                    return Err("chat is not being saved right now; try again shortly");
                }
            }
        }
        let headroom = buffer.reserved - buffer.charged();
        if headroom < charge {
            match self
                .catalog
                .reserve_chat_bytes(
                    self.document_id,
                    buffer.reservation_key,
                    CHAT_RESERVATION_CHUNK.max(charge),
                    charge - headroom,
                )
                .await
            {
                Ok(total) => {
                    buffer.reserved = total;
                    buffer.reserved_known = true;
                    self.note_chat_reservation(&buffer);
                }
                Err(CatalogError::Conflict(_)) => {
                    return Err("the project owner's storage is full");
                }
                Err(error) => {
                    buffer.reserved_known = false;
                    self.note_chat_reservation(&buffer);
                    tracing::warn!("could not reserve chat storage of {}: {error}", self.slug);
                    return Err("chat is not being saved right now; try again shortly");
                }
            }
        }
        self.log.charge_storage(charge);
        let message = chat_json(&record);
        let mut echo = message.clone();
        echo["temp_id"] = Value::String(temp_id.to_owned());
        if buffer.rows.is_empty() {
            buffer.oldest = Some(tokio::time::Instant::now());
        }
        buffer.rows.push(record);
        self.chat_waiting
            .store(buffer.rows.len(), std::sync::atomic::Ordering::Relaxed);
        self.broadcast_chat(Some(sender), &message).await;
        self.echo_chat(sender, &echo).await;
        if buffer.rows.len() >= CHAT_FLUSH_ROWS {
            self.write_chat(&mut buffer).await;
        }
        Ok(())
    }

    /// Writes the waiting messages when they are due, or whatever is waiting
    /// when forced. A failed write keeps the rows, and their reservation, for
    /// the next pass. A forced flush that leaves nothing waiting also gives
    /// the reservation back.
    pub async fn flush_chat(&self, force: bool) {
        let mut buffer = self.chat.lock().await;
        if !buffer.rows.is_empty() && (force || buffer.due()) {
            self.write_chat(&mut buffer).await;
        }
        if force && buffer.rows.is_empty() && (buffer.reserved > 0 || !buffer.reserved_known) {
            match self
                .catalog
                .release_chat_reservation(self.document_id, buffer.reservation_key)
                .await
            {
                Ok(()) => {
                    buffer.reserved = 0;
                    buffer.reserved_known = true;
                    self.note_chat_reservation(&buffer);
                }
                Err(error) => {
                    buffer.reserved_known = false;
                    self.note_chat_reservation(&buffer);
                    tracing::warn!(
                        "could not release the chat reservation of {}: {error}",
                        self.slug
                    );
                }
            }
        }
    }

    async fn write_chat(&self, buffer: &mut ChatBuffer) {
        match self
            .catalog
            .insert_chat_messages(self.document_id, &buffer.rows, Some(buffer.reservation_key))
            .await
        {
            Ok(inserted) => {
                // The ledger was charged when each message was accepted.
                let _ = buffer.discard();
                buffer.reserved = inserted.reserved;
                buffer.reserved_known = true;
                self.note_chat_reservation(buffer);
            }
            Err(error) => {
                buffer.reserved_known = false;
                self.note_chat_reservation(buffer);
                tracing::warn!("could not write the chat of {}: {error}", self.slug);
            }
        }
        self.chat_waiting
            .store(buffer.rows.len(), std::sync::atomic::Ordering::Relaxed);
    }

    fn note_chat_reservation(&self, buffer: &ChatBuffer) {
        self.chat_holds.store(
            buffer.holds_reservation(),
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// Whether the room may hold a chat reservation row, without taking the
    /// lock. A room that does must not be retired before it releases it.
    pub fn chat_holds_reservation(&self) -> bool {
        self.chat_holds.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The key of this room's reservation row.
    #[cfg(test)]
    pub async fn chat_reservation_key(&self) -> Uuid {
        self.chat.lock().await.reservation_key
    }

    /// Makes the room doubt its cached reservation, as a lost reply would.
    #[cfg(test)]
    pub async fn forget_chat_reservation(&self) {
        let mut buffer = self.chat.lock().await;
        buffer.reserved_known = false;
        self.note_chat_reservation(&buffer);
    }

    /// The reservation total the room believes it holds.
    #[cfg(test)]
    pub async fn chat_reserved(&self) -> i64 {
        self.chat.lock().await.reserved
    }

    /// How many messages are waiting to be written, without taking the lock.
    pub fn chat_waiting(&self) -> usize {
        self.chat_waiting.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// One page of chat, older than `before` when given, oldest first, with
    /// the messages still in the buffer merged in.
    pub async fn chat_page(
        &self,
        before: Option<Uuid>,
        limit: i64,
    ) -> Result<(Vec<ChatRecord>, bool), WriteError> {
        let buffer = self.chat.lock().await;
        self.page_locked(&buffer, before, limit).await
    }

    /// [`Self::chat_page`] for a caller that already holds the chat lock.
    async fn page_locked(
        &self,
        buffer: &ChatBuffer,
        before: Option<Uuid>,
        limit: i64,
    ) -> Result<(Vec<ChatRecord>, bool), WriteError> {
        let (stored, more) = self
            .catalog
            .chat_page(self.document_id, before, limit)
            .await?;
        Ok(merge_page(
            stored,
            more,
            &buffer.rows,
            before,
            limit.max(0) as usize,
        ))
    }

    /// Registers the socket as a chat listener, reads the newest `limit`
    /// messages, adds them to `hello` and queues it on the socket, all under
    /// the chat lock. Every chat event is sent under the same lock, so each
    /// one follows the hello and none can be lost or undone by it. A failed
    /// read is logged and the hello goes out with `chat_unavailable` set and
    /// no `chat` or `chat_more`, so the client keeps what it holds and asks
    /// again. The socket still receives live messages. A hello that is not a
    /// `hello` (an error reply) gets no chat keys. Returns false when the
    /// hello cannot be queued within `timeout`.
    pub async fn send_hello_with_chat(
        &self,
        socket: u64,
        tx: &super::Sender,
        mut hello: Value,
        can_clear: bool,
        limit: i64,
        timeout: std::time::Duration,
    ) -> bool {
        let buffer = self.chat.lock().await;
        self.listen_chat(socket, tx.clone()).await;
        if hello["type"] == "hello" {
            match self.page_locked(&buffer, None, limit).await {
                Ok((messages, more)) => {
                    hello["chat"] = Value::Array(messages.iter().map(chat_json).collect());
                    hello["chat_more"] = json!(more);
                }
                Err(error) => {
                    tracing::warn!("could not read the chat of {}: {error}", self.slug);
                    hello["chat_unavailable"] = json!(true);
                }
            }
            hello["chat_can_clear"] = json!(can_clear);
        }
        tokio::time::timeout(timeout, tx.send(super::Outgoing::Text(hello.to_string())))
            .await
            .is_ok_and(|sent| sent.is_ok())
    }

    /// Deletes every message, stored and waiting, gives the owner the bytes
    /// back and tells every listener, all under the chat lock. A waiting
    /// message that is also stored (a flush whose reply was lost) is credited
    /// once, through the stored bytes. Returns what the database held, or `None`
    /// and changes nothing when `owner` no longer owns the document.
    pub async fn clear_chat(&self, owner: Uuid) -> Result<Option<ChatCleared>, WriteError> {
        let mut buffer = self.chat.lock().await;
        let Some(cleared) = self
            .catalog
            .clear_chat(self.document_id, &buffer.ids(), owner)
            .await?
        else {
            return Ok(None);
        };
        let waiting = buffer.discard_unstored(&cleared.stored_buffered);
        self.chat_waiting
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.log.charge_storage(-cleared.bytes);
        self.log.charge_storage(-waiting);
        self.broadcast_chat(None, &json!({"type": "chat-cleared"}))
            .await;
        Ok(Some(cleared))
    }

    /// The chat lock, for a caller that deletes chat inside its own
    /// transaction (trim). It passes [`ChatBuffer::ids`] into that transaction,
    /// calls [`ChatBuffer::discard_unstored`] once it commits, credits the
    /// bytes it returns and broadcasts `chat-cleared` before
    /// dropping the guard.
    pub async fn chat_lock(&self) -> tokio::sync::MutexGuard<'_, ChatBuffer> {
        self.chat.lock().await
    }

    /// Records that the buffer was discarded through [`Self::chat_lock`].
    pub fn chat_discarded(&self) {
        self.chat_waiting
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }

    /// Anonymizes the waiting messages of an erased account. Stored ones are
    /// anonymized by the catalogue.
    pub async fn scrub_chat_author(&self, account_id: &str) {
        let Ok(account) = Uuid::parse_str(account_id) else {
            return;
        };
        let mut buffer = self.chat.lock().await;
        for row in buffer
            .rows
            .iter_mut()
            .filter(|row| row.author_account_id == Some(account))
        {
            row.author_account_id = None;
            row.author_label = "Deleted user".to_string();
        }
    }

    /// Sends a frame to every socket that may see chat, except one. A socket
    /// whose queue refuses the frame is closed, so it reconnects and reads
    /// the history again instead of going on with a hole in it.
    pub async fn broadcast_chat(&self, skip: Option<u64>, payload: &Value) {
        let text = payload.to_string();
        let peers = self.peers.lock().await;
        for (id, peer) in peers.iter() {
            if Some(*id) == skip {
                continue;
            }
            if let Some(tx) = &peer.chat_tx {
                if tx
                    .try_send(super::Outgoing::shared_text(text.clone()))
                    .is_err()
                {
                    let _ = tx.force_close("chat fell behind; reconnect");
                }
            }
        }
    }

    /// Sends a frame to one chat listener, closing it if it cannot take it.
    async fn echo_chat(&self, socket: u64, payload: &Value) {
        let peers = self.peers.lock().await;
        if let Some(tx) = peers.get(&socket).and_then(|peer| peer.chat_tx.as_ref()) {
            if tx
                .try_send(super::Outgoing::shared_text(payload.to_string()))
                .is_err()
            {
                let _ = tx.force_close("chat fell behind; reconnect");
            }
        }
    }

    /// Lets this socket receive chat frames. Only commenters and above call it.
    pub async fn listen_chat(&self, socket: u64, tx: super::Sender) {
        self.peers
            .lock()
            .await
            .entry(socket)
            .or_insert_with(super::Peer::default)
            .chat_tx = Some(tx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(n: u128, body: &str) -> ChatRecord {
        ChatRecord {
            id: Uuid::from_u128(n),
            author_account_id: None,
            author_label: "A".into(),
            body: body.into(),
            created_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn ids(rows: &[ChatRecord]) -> Vec<u128> {
        rows.iter().map(|row| row.id.as_u128()).collect()
    }

    #[test]
    fn discard_returns_the_bytes_it_dropped() {
        let mut buffer = ChatBuffer::default();
        buffer.rows.push(record(1, "abc"));
        buffer.rows.push(record(2, "de"));
        buffer.oldest = Some(tokio::time::Instant::now());
        let expected = buffer.charged();
        assert_eq!(expected, 3 + 2 + 2 * CHAT_ROW_OVERHEAD_BYTES);
        assert_eq!(buffer.discard(), expected);
        assert!(buffer.rows.is_empty());
        assert!(buffer.oldest.is_none());
        assert_eq!(buffer.discard(), 0);
    }

    #[test]
    fn discard_unstored_skips_rows_the_database_holds() {
        let mut buffer = ChatBuffer::default();
        buffer.rows.push(record(1, "abc"));
        buffer.rows.push(record(2, "de"));
        assert_eq!(buffer.ids(), vec![Uuid::from_u128(1), Uuid::from_u128(2)]);
        let dropped = buffer.discard_unstored(&[Uuid::from_u128(1)]);
        assert_eq!(dropped, 2 + CHAT_ROW_OVERHEAD_BYTES);
        assert!(buffer.rows.is_empty());
    }

    #[test]
    fn buffered_rows_follow_stored_ones_in_order() {
        let (rows, more) = merge_page(
            vec![record(1, "a"), record(2, "b")],
            false,
            &[record(3, "c"), record(4, "d")],
            None,
            50,
        );
        assert_eq!(ids(&rows), vec![1, 2, 3, 4]);
        assert!(!more);
    }

    #[test]
    fn before_hides_newer_buffered_rows() {
        let (rows, more) = merge_page(
            vec![record(1, "a")],
            true,
            &[record(2, "b"), record(5, "e")],
            Some(Uuid::from_u128(3)),
            50,
        );
        assert_eq!(ids(&rows), vec![1, 2]);
        assert!(more);
    }

    #[test]
    fn the_limit_keeps_the_newest_and_reports_more() {
        let (rows, more) = merge_page(
            vec![record(1, "a"), record(2, "b")],
            false,
            &[record(3, "c"), record(4, "d")],
            None,
            3,
        );
        assert_eq!(ids(&rows), vec![2, 3, 4]);
        assert!(more);
    }

    #[test]
    fn a_row_both_stored_and_buffered_appears_once() {
        let (rows, _) = merge_page(
            vec![record(1, "a"), record(2, "b")],
            false,
            &[record(2, "b")],
            None,
            50,
        );
        assert_eq!(ids(&rows), vec![1, 2]);
    }

    #[test]
    fn an_empty_buffer_leaves_the_stored_page_alone() {
        let (rows, more) = merge_page(vec![record(7, "x")], true, &[], None, 50);
        assert_eq!(ids(&rows), vec![7]);
        assert!(more);
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn clear_after_a_lost_flush_reply_refunds_each_row_once() {
        let Some(deployment) =
            super::super::label_replay_tests::deployment("chat-lost-reply-paper").await
        else {
            return;
        };
        let room = deployment.rooms.get(&deployment.slug).await.unwrap();
        // Lift the owner's figure so a double refund cannot hide at the floor.
        room.log().charge_storage(1_000_000);
        let before = room.log().owner_storage_bytes();
        let rows = vec![record(1, "abc"), record(2, "de")];
        let rows: Vec<ChatRecord> = rows
            .into_iter()
            .map(|mut row| {
                row.id = Uuid::now_v7();
                row.created_at = librepaper_engine::storage::postgres::chat_now();
                row
            })
            .collect();
        let charge: i64 = rows.iter().map(ChatRecord::charged_bytes).sum();
        for row in &rows {
            room.enqueue_chat(row.clone(), 1, "t").await.unwrap();
        }
        assert_eq!(room.log().owner_storage_bytes(), before + charge);
        // The flush committed, but its reply was lost, so the rows stay buffered.
        room.catalog()
            .insert_chat_messages(room.document_id, &rows, None)
            .await
            .unwrap();
        let cleared = room
            .clear_chat(deployment.account_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cleared.bytes, charge);
        assert_eq!(room.log().owner_storage_bytes(), before);
        assert_eq!(room.chat_waiting(), 0);
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn chat_reserves_in_postgres_and_a_forced_flush_gives_it_back() {
        let Some(deployment) =
            super::super::label_replay_tests::deployment("chat-reservation-paper").await
        else {
            return;
        };
        let room = deployment.rooms.get(&deployment.slug).await.unwrap();
        let held = || async {
            sqlx::query_scalar::<_, Option<i64>>(
                "SELECT sum(bytes)::bigint FROM pending_log_reservations WHERE document_id=$1",
            )
            .bind(room.document_id)
            .fetch_one(room.catalog().pool())
            .await
            .unwrap()
        };
        let mut row = record(1, "abc");
        row.id = Uuid::now_v7();
        row.created_at = librepaper_engine::storage::postgres::chat_now();
        let charge = row.charged_bytes();
        // Dropping the soft ledger to zero does not change what PostgreSQL holds.
        room.log().charge_storage(-room.log().owner_storage_bytes());
        room.enqueue_chat(row, 1, "t").await.unwrap();
        assert_eq!(held().await, Some(CHAT_RESERVATION_CHUNK));
        // A non-forced flush with nothing due writes nothing and keeps the row.
        room.flush_chat(false).await;
        assert_eq!(held().await, Some(CHAT_RESERVATION_CHUNK));
        room.flush_chat(true).await;
        assert_eq!(room.chat_waiting(), 0);
        assert_eq!(held().await, None);
        let stored: i64 = sqlx::query_scalar("SELECT chat_bytes FROM documents WHERE id=$1")
            .bind(room.document_id)
            .fetch_one(room.catalog().pool())
            .await
            .unwrap();
        assert_eq!(stored, charge);
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn a_lost_insert_reply_makes_the_room_reread_its_reservation() {
        let Some(deployment) =
            super::super::label_replay_tests::deployment("chat-lost-insert-paper").await
        else {
            return;
        };
        let room = deployment.rooms.get(&deployment.slug).await.unwrap();
        room.log().charge_storage(-room.log().owner_storage_bytes());
        let mut first = record(1, "abc");
        first.id = Uuid::now_v7();
        first.created_at = librepaper_engine::storage::postgres::chat_now();
        let first_charge = first.charged_bytes();
        room.enqueue_chat(first.clone(), 1, "t").await.unwrap();
        let key = room.chat_reservation_key().await;
        assert_eq!(room.chat_reserved().await, CHAT_RESERVATION_CHUNK);
        // The insert commits and takes its charge off the row, but the reply is lost.
        room.catalog()
            .insert_chat_messages(room.document_id, &[first], Some(key))
            .await
            .unwrap();
        room.forget_chat_reservation().await;
        assert!(room.chat_holds_reservation());
        room.clear_chat(deployment.account_id)
            .await
            .unwrap()
            .unwrap();
        let mut second = record(2, "de");
        second.id = Uuid::now_v7();
        second.created_at = librepaper_engine::storage::postgres::chat_now();
        room.enqueue_chat(second, 1, "t").await.unwrap();
        let row = room
            .catalog()
            .chat_reservation_bytes(room.document_id, key)
            .await
            .unwrap();
        assert_eq!(row, CHAT_RESERVATION_CHUNK - first_charge);
        assert_eq!(room.chat_reserved().await, row);
    }
}
