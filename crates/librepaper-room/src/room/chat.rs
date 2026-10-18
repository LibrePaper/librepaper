//! Project chat history, as the room holds it.
//!
//! A room buffers the messages it receives and writes them in batches, so a
//! busy conversation costs one transaction per flush rather than one per
//! message. The buffer sits behind one mutex that is held across every
//! database call, which is what keeps a clear or a trim from racing a flush:
//! a flush that has not started sees the cleared buffer, and one that has
//! started finishes before the clear runs.

use serde_json::{json, Value};
use time::OffsetDateTime;
use uuid::Uuid;

use librepaper_engine::storage::postgres::ChatRecord;
#[cfg(test)]
use librepaper_engine::storage::postgres::CHAT_ROW_OVERHEAD_BYTES;

use super::{Room, WriteError};

/// Flush at once when this many messages are waiting.
pub const CHAT_FLUSH_ROWS: usize = 64;
/// Refuse new messages when this many are waiting, which means flushes fail.
pub const CHAT_BUFFER_MAX_ROWS: usize = 256;
/// How long the oldest waiting message may wait before the next pass writes it.
pub const CHAT_FLUSH_AFTER: std::time::Duration = std::time::Duration::from_secs(2);

/// Messages accepted but not yet written.
#[derive(Default)]
pub struct ChatBuffer {
    rows: Vec<ChatRecord>,
    /// When the oldest waiting message was queued.
    oldest: Option<tokio::time::Instant>,
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

    fn charged(&self) -> i64 {
        self.rows.iter().map(ChatRecord::charged_bytes).sum()
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
    /// Queues one message, or says why it cannot be kept. Accepting it
    /// reserves its bytes on the owner's ledger at once, so another room of
    /// the same owner cannot spend the same allowance before the flush. The
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
        if self
            .log
            .storage_would_exceed(record.charged_bytes())
            .is_some()
        {
            return Err("the project owner's storage is full");
        }
        self.log.charge_storage(record.charged_bytes());
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
    /// the next pass.
    pub async fn flush_chat(&self, force: bool) {
        let mut buffer = self.chat.lock().await;
        if buffer.rows.is_empty() || !(force || buffer.due()) {
            return;
        }
        self.write_chat(&mut buffer).await;
    }

    async fn write_chat(&self, buffer: &mut ChatBuffer) {
        match self
            .catalog
            .insert_chat_messages(self.document_id, &buffer.rows)
            .await
        {
            Ok(_) => {
                // The bytes were reserved when each message was accepted.
                let _ = buffer.discard();
            }
            Err(error) => {
                tracing::warn!("could not write the chat of {}: {error}", self.slug);
            }
        }
        self.chat_waiting
            .store(buffer.rows.len(), std::sync::atomic::Ordering::Relaxed);
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
    /// read is logged and the hello goes out with an empty history. A hello
    /// that is not a `hello` (an error reply) gets no chat keys. Errors when
    /// the hello cannot be queued within `timeout`.
    pub async fn send_hello_with_chat(
        &self,
        socket: u64,
        tx: &super::Sender,
        mut hello: Value,
        can_clear: bool,
        limit: i64,
        timeout: std::time::Duration,
    ) -> Result<(), ()> {
        let buffer = self.chat.lock().await;
        self.listen_chat(socket, tx.clone()).await;
        if hello["type"] == "hello" {
            let (messages, more) = match self.page_locked(&buffer, None, limit).await {
                Ok(page) => page,
                Err(error) => {
                    tracing::warn!("could not read the chat of {}: {error}", self.slug);
                    (Vec::new(), false)
                }
            };
            hello["chat"] = Value::Array(messages.iter().map(chat_json).collect());
            hello["chat_more"] = json!(more);
            hello["chat_can_clear"] = json!(can_clear);
        }
        tokio::time::timeout(timeout, tx.send(super::Outgoing::Text(hello.to_string())))
            .await
            .map_err(|_| ())?
            .map_err(|_| ())
    }

    /// Deletes every message, stored and waiting, gives the owner the bytes
    /// back and tells every listener, all under the chat lock. Returns the
    /// rows and bytes the database held.
    pub async fn clear_chat(&self) -> Result<(i64, i64), WriteError> {
        let mut buffer = self.chat.lock().await;
        let cleared = self.catalog.clear_chat(self.document_id).await?;
        let waiting = buffer.discard();
        self.chat_waiting
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.log.charge_storage(-cleared.1);
        self.log.charge_storage(-waiting);
        self.broadcast_chat(None, &json!({"type": "chat-cleared"}))
            .await;
        Ok(cleared)
    }

    /// The chat lock, for a caller that deletes chat inside its own
    /// transaction (trim). It calls [`ChatBuffer::discard`] once that commits,
    /// credits the bytes it returns and broadcasts `chat-cleared` before
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
}
