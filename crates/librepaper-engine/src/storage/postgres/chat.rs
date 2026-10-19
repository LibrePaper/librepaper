//! Project chat history.
//!
//! Rooms buffer messages and flush them here in batches. Every message charges
//! its body bytes plus a fixed row overhead to `documents.chat_bytes`, which
//! counts toward the owner's storage. The author label is not charged, so
//! anonymizing it never moves the counter.
//!
//! A room reserves owner storage in PostgreSQL before it accepts messages.
//! The reservation is a row in `pending_log_reservations`, which every usage
//! sum already counts. Inserting messages takes their charge off the row, and
//! the room deletes what is left when it has no more messages to hold.

use time::OffsetDateTime;
use uuid::Uuid;

use super::{Error, PostgresCatalog, Result};

/// The fixed per-row charge added to a message's body bytes.
pub const CHAT_ROW_OVERHEAD_BYTES: i64 = 64;

/// How much owner storage a room asks for at a time. One reservation covers
/// many messages, so a busy chat does not take the locks for every message.
pub const CHAT_RESERVATION_CHUNK: i64 = 64 * 1024;

/// What an insert charged and what its reservation row holds afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChatInserted {
    /// The bytes added to `documents.chat_bytes`.
    pub charged: i64,
    /// The bytes the reservation row still holds, or 0 if it is gone or the
    /// caller named none.
    pub reserved: i64,
}

/// One stored chat message. `id` is a UUIDv7 made at send time, so it orders
/// messages within a project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatRecord {
    pub id: Uuid,
    pub author_account_id: Option<Uuid>,
    pub author_label: String,
    pub body: String,
    pub created_at: OffsetDateTime,
}

/// The current time at the microsecond precision PostgreSQL stores, so a
/// buffered message and its stored row compare equal.
pub fn chat_now() -> OffsetDateTime {
    let now = OffsetDateTime::now_utc();
    now.replace_microsecond(now.microsecond()).unwrap_or(now)
}

impl ChatRecord {
    /// What this row charges the owner: body bytes plus the fixed overhead.
    pub fn charged_bytes(&self) -> i64 {
        self.body.len() as i64 + CHAT_ROW_OVERHEAD_BYTES
    }
}

#[derive(sqlx::FromRow)]
struct ChatRow {
    id: Uuid,
    author_account_id: Option<Uuid>,
    author_label: String,
    body: String,
    created_at: OffsetDateTime,
}

impl From<ChatRow> for ChatRecord {
    fn from(row: ChatRow) -> Self {
        Self {
            id: row.id,
            author_account_id: row.author_account_id,
            author_label: row.author_label,
            body: row.body,
            created_at: row.created_at,
        }
    }
}

impl PostgresCatalog {
    /// Inserts a batch in one transaction and adds its charge to
    /// documents.chat_bytes. The active author accounts are locked FOR SHARE
    /// first, in id order, so an erasure cannot start (it needs a conflicting
    /// row lock to mark the account 'erasing') until this batch commits, and
    /// its anonymizing update then sees every row. An author that is missing
    /// or already erasing is stored as NULL, with the label "Deleted user",
    /// instead of failing the batch.
    /// A row whose id is already stored is skipped and not charged again, so
    /// a retry after a lost reply is harmless. When `reservation` names a
    /// row, the charge comes off it in the same transaction, and the row is
    /// deleted when nothing is left. Returns the bytes charged and the bytes
    /// the reservation still holds.
    pub async fn insert_chat_messages(
        &self,
        document_id: Uuid,
        rows: &[ChatRecord],
        reservation: Option<Uuid>,
    ) -> Result<ChatInserted> {
        if rows.is_empty() {
            return Ok(ChatInserted {
                charged: 0,
                reserved: 0,
            });
        }
        let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
        let authors: Vec<Option<Uuid>> = rows.iter().map(|row| row.author_account_id).collect();
        let labels: Vec<String> = rows.iter().map(|row| row.author_label.clone()).collect();
        let bodies: Vec<String> = rows.iter().map(|row| row.body.clone()).collect();
        let created: Vec<OffsetDateTime> = rows.iter().map(|row| row.created_at).collect();
        let mut wanted: Vec<Uuid> = authors.iter().flatten().copied().collect();
        wanted.sort();
        wanted.dedup();
        let mut tx = self.begin_writer_transaction().await?;
        let active: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM accounts WHERE id = ANY($1) AND status='active' ORDER BY id FOR SHARE",
        )
        .bind(wanted)
        .fetch_all(&mut *tx)
        .await?;
        if reservation.is_some() {
            // Moving bytes from the reservation to chat_bytes keeps the
            // deployment sum steady only if no reservation or other chat
            // write reads the sums in between. This takes the locks the
            // reservation path takes, in its order, after the account locks.
            sqlx::query("SELECT id FROM documents WHERE id=$1 FOR UPDATE")
                .bind(document_id)
                .fetch_optional(&mut *tx)
                .await?;
            sqlx::query_scalar::<_, i64>(
                "SELECT bytes FROM storage_usage WHERE singleton FOR UPDATE",
            )
            .fetch_one(&mut *tx)
            .await?;
        }
        let inserted: Vec<i64> = sqlx::query_scalar(
            "INSERT INTO chat_messages (id,document_id,author_account_id,author_label,body,created_at) \
             SELECT m.id,$1,CASE WHEN m.author = ANY($7) THEN m.author ELSE NULL END,\
             CASE WHEN m.author IS NOT NULL AND NOT (m.author = ANY($7)) \
             THEN 'Deleted user' ELSE m.label END,m.body,m.created_at \
             FROM UNNEST($2::uuid[],$3::uuid[],$4::text[],$5::text[],$6::timestamptz[]) \
             AS m(id,author,label,body,created_at) \
             ON CONFLICT (id) DO NOTHING RETURNING octet_length(body)::bigint",
        )
        .bind(document_id)
        .bind(ids)
        .bind(authors)
        .bind(labels)
        .bind(bodies)
        .bind(created)
        .bind(active)
        .fetch_all(&mut *tx)
        .await?;
        let charge: i64 = inserted
            .iter()
            .map(|body| body + CHAT_ROW_OVERHEAD_BYTES)
            .sum();
        let updated = sqlx::query("UPDATE documents SET chat_bytes=chat_bytes+$2 WHERE id=$1")
            .bind(document_id)
            .bind(charge)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if updated != 1 {
            return Err(Error::NotFound);
        }
        let mut reserved = 0;
        if let Some(key) = reservation {
            let deleted = sqlx::query(
                "DELETE FROM pending_log_reservations \
                 WHERE reservation_id=$1 AND document_id=$2 AND bytes <= $3",
            )
            .bind(key)
            .bind(document_id)
            .bind(charge)
            .execute(&mut *tx)
            .await?
            .rows_affected();
            if deleted == 0 {
                reserved = sqlx::query_scalar::<_, i64>(
                    "UPDATE pending_log_reservations SET bytes=bytes-$3 \
                     WHERE reservation_id=$1 AND document_id=$2 RETURNING bytes",
                )
                .bind(key)
                .bind(document_id)
                .bind(charge)
                .fetch_optional(&mut *tx)
                .await?
                .unwrap_or(0);
            }
        }
        tx.commit().await?;
        Ok(ChatInserted {
            charged: charge,
            reserved,
        })
    }

    /// Adds up to `wanted` bytes to the reservation row `key` for this
    /// document, creating it if needed, and returns the row's new total. The
    /// locks and the owner and deployment limits are the ones
    /// `reserve_pending_log_bytes` uses. It grants what fits, and refuses
    /// when that is less than `minimum`.
    pub async fn reserve_chat_bytes(
        &self,
        document_id: Uuid,
        key: Uuid,
        wanted: i64,
        minimum: i64,
    ) -> Result<i64> {
        if wanted <= 0 || minimum <= 0 || minimum > wanted {
            return Err(Error::Invalid("invalid chat reservation".into()));
        }
        let mut tx = self.begin_writer_transaction().await?;
        let owner_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT owner_id FROM documents WHERE id=$1 AND status='active' \
             AND NOT EXISTS(SELECT 1 FROM moderated_projects WHERE document_id=$1) FOR UPDATE",
        )
        .bind(document_id)
        .fetch_optional(&mut *tx)
        .await?;
        let owner_id = owner_id.ok_or(Error::NotFound)?;
        let retained: i64 =
            sqlx::query_scalar("SELECT bytes FROM storage_usage WHERE singleton FOR UPDATE")
                .fetch_one(&mut *tx)
                .await?;
        let owner_usage = super::repository::owner_usage_bytes(&mut *tx, owner_id).await?;
        let epoch = self.writer_epoch()?;
        let existing = sqlx::query_as::<_, (Uuid, i64)>(
            "SELECT document_id,writer_epoch FROM pending_log_reservations \
             WHERE reservation_id=$1 FOR UPDATE",
        )
        .bind(key)
        .fetch_optional(&mut *tx)
        .await?;
        let existing = match existing {
            Some((existing_document, _)) if existing_document != document_id => {
                return Err(Error::Invalid(
                    "chat reservation identity was reused for another document".into(),
                ));
            }
            // A row from an earlier writer epoch belongs to a writer that is
            // gone. This writer holds the lease, so it replaces the row.
            Some((_, existing_epoch)) if existing_epoch != epoch => {
                sqlx::query("DELETE FROM pending_log_reservations WHERE reservation_id=$1")
                    .bind(key)
                    .execute(&mut *tx)
                    .await?;
                None
            }
            other => other,
        };
        let log_usage: i64 = sqlx::query_scalar(
            "SELECT COALESCE((SELECT sum(snapshot_bytes)::bigint FROM document_snapshots WHERE delete_after IS NULL),0) \
             + COALESCE((SELECT sum(uncompacted_update_bytes + chat_bytes)::bigint FROM documents),0)",
        )
        .fetch_one(&mut *tx)
        .await?;
        let reservations: i64 = sqlx::query_scalar(
            "SELECT COALESCE(sum(bytes),0)::bigint FROM pending_log_reservations",
        )
        .fetch_one(&mut *tx)
        .await?;
        let owner_room = self.policy.owner_bytes.saturating_sub(owner_usage);
        let deployment_room = self.policy.deployment_bytes.saturating_sub(
            retained
                .saturating_add(log_usage)
                .saturating_add(reservations),
        );
        let granted = wanted.min(owner_room).min(deployment_room);
        if granted < minimum {
            return Err(Error::Conflict(if owner_room < minimum {
                "account storage quota exceeded".into()
            } else {
                "deployment storage threshold exceeded".into()
            }));
        }
        let total: i64 = if existing.is_some() {
            sqlx::query_scalar(
                "UPDATE pending_log_reservations SET bytes=bytes+$2 \
                 WHERE reservation_id=$1 RETURNING bytes",
            )
            .bind(key)
            .bind(granted)
            .fetch_one(&mut *tx)
            .await?
        } else {
            sqlx::query(
                "INSERT INTO pending_log_reservations \
                 (reservation_id,document_id,writer_epoch,bytes,batch_digest) \
                 VALUES($1,$2,$3,$4,$5)",
            )
            .bind(key)
            .bind(document_id)
            .bind(epoch)
            .bind(granted)
            .bind([0u8; 32].as_slice())
            .execute(&mut *tx)
            .await?;
            granted
        };
        tx.commit().await?;
        Ok(total)
    }

    /// Deletes the chat reservation row `key`. A missing row is not an error.
    pub async fn release_chat_reservation(&self, document_id: Uuid, key: Uuid) -> Result<()> {
        let mut tx = self.begin_writer_transaction().await?;
        sqlx::query(
            "DELETE FROM pending_log_reservations WHERE reservation_id=$1 AND document_id=$2",
        )
        .bind(key)
        .bind(document_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The bytes the reservation row `key` holds, or 0 when it is missing.
    pub async fn chat_reservation_bytes(&self, document_id: Uuid, key: Uuid) -> Result<i64> {
        let bytes: Option<i64> = sqlx::query_scalar(
            "SELECT bytes FROM pending_log_reservations WHERE reservation_id=$1 AND document_id=$2",
        )
        .bind(key)
        .bind(document_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(bytes.unwrap_or(0))
    }

    /// Up to `limit` messages older than `before` (or the newest when None),
    /// returned oldest first, and whether older ones exist.
    pub async fn chat_page(
        &self,
        document_id: Uuid,
        before: Option<Uuid>,
        limit: i64,
    ) -> Result<(Vec<ChatRecord>, bool)> {
        let limit = limit.max(0);
        let mut rows: Vec<ChatRow> = sqlx::query_as(
            "SELECT id,author_account_id,author_label,body,created_at FROM chat_messages \
             WHERE document_id=$1 AND ($2::uuid IS NULL OR id < $2) \
             ORDER BY id DESC LIMIT $3",
        )
        .bind(document_id)
        .bind(before)
        .bind(limit + 1)
        .fetch_all(&self.pool)
        .await?;
        let more = rows.len() as i64 > limit;
        rows.truncate(limit as usize);
        rows.reverse();
        Ok((rows.into_iter().map(ChatRecord::from).collect(), more))
    }

    /// Deletes every message and zeroes chat_bytes. `buffered` is the ids of
    /// the messages the room still holds in memory. The document row is locked
    /// first. When `owner` no longer owns the document, nothing is deleted and
    /// this returns `None`.
    pub async fn clear_chat(
        &self,
        document_id: Uuid,
        buffered: &[Uuid],
        owner: Uuid,
    ) -> Result<Option<ChatCleared>> {
        let mut tx = self.begin_writer_transaction().await?;
        let current: Option<Uuid> =
            sqlx::query_scalar("SELECT owner_id FROM documents WHERE id=$1 FOR UPDATE")
                .bind(document_id)
                .fetch_optional(&mut *tx)
                .await?;
        if current != Some(owner) {
            tx.rollback().await?;
            return Ok(None);
        }
        let cleared = self
            .clear_chat_in_transaction(&mut tx, document_id, buffered)
            .await?;
        tx.commit().await?;
        Ok(Some(cleared))
    }

    /// Same as `clear_chat`, inside the caller's transaction. `bytes` is what
    /// `chat_bytes` held, which is what the owner is credited. `stored_buffered`
    /// names the buffered ids that were already stored, so the caller does not
    /// credit those a second time from the buffer.
    pub async fn clear_chat_in_transaction(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        document_id: Uuid,
        buffered: &[Uuid],
    ) -> Result<ChatCleared> {
        let stored_buffered: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM chat_messages WHERE document_id=$1 AND id = ANY($2)",
        )
        .bind(document_id)
        .bind(buffered.to_vec())
        .fetch_all(&mut **tx)
        .await?;
        let rows = sqlx::query("DELETE FROM chat_messages WHERE document_id=$1")
            .bind(document_id)
            .execute(&mut **tx)
            .await?
            .rows_affected() as i64;
        let bytes: Option<i64> = sqlx::query_scalar(
            "UPDATE documents d SET chat_bytes=0 \
             FROM (SELECT chat_bytes FROM documents WHERE id=$1 FOR UPDATE) old \
             WHERE d.id=$1 RETURNING old.chat_bytes",
        )
        .bind(document_id)
        .fetch_optional(&mut **tx)
        .await?;
        Ok(ChatCleared {
            rows,
            bytes: bytes.unwrap_or(0),
            stored_buffered,
        })
    }
}

/// What a clear removed from PostgreSQL.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChatCleared {
    pub rows: i64,
    pub bytes: i64,
    /// The buffered ids that were also stored, as after a flush whose reply
    /// was lost.
    pub stored_buffered: Vec<Uuid>,
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::super::{NewAccount, NewDocument, PostgresCatalog, PostgresOptions};
    use super::{ChatRecord, CHAT_RESERVATION_CHUNK, CHAT_ROW_OVERHEAD_BYTES};

    async fn catalog() -> PostgresCatalog {
        let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
            .expect("set LIBREPAPER_TEST_POSTGRES_URL to a throwaway PostgreSQL database");
        let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .expect("connect to PostgreSQL");
        catalog.migrate().await.expect("run PostgreSQL migrations");
        catalog
    }

    async fn account(catalog: &PostgresCatalog) -> super::super::AccountRecord {
        catalog
            .create_account(NewAccount {
                kind: "anonymous".into(),
                provider: None,
                provider_subject: None,
                handle: format!("chat-{}", Uuid::now_v7()),
                display_name: "Chat test".into(),
                email: None,
            })
            .await
            .unwrap()
    }

    async fn project(catalog: &PostgresCatalog, owner: &super::super::AccountRecord) -> Uuid {
        catalog
            .create_document(NewDocument {
                slug: format!("chat-{}", Uuid::now_v7()),
                owner_id: owner.id,
                owner_session_generation: Some(owner.session_generation),
                ownership_mode: "owned".into(),
                title: "Chat test".into(),
                source_format: "markdown".into(),
                main_path: "main.md".into(),
            })
            .await
            .unwrap()
            .id
    }

    fn message(author: Option<Uuid>, body: &str) -> ChatRecord {
        ChatRecord {
            id: Uuid::now_v7(),
            author_account_id: author,
            author_label: "Tester".into(),
            body: body.into(),
            created_at: super::chat_now(),
        }
    }

    async fn chat_bytes(catalog: &PostgresCatalog, document_id: Uuid) -> i64 {
        sqlx::query_scalar("SELECT chat_bytes FROM documents WHERE id=$1")
            .bind(document_id)
            .fetch_one(catalog.pool())
            .await
            .unwrap()
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn batch_insert_charges_chat_bytes_and_owner_usage() {
        let catalog = catalog().await;
        let _writer = catalog.claim_writer().await.unwrap();
        let owner = account(&catalog).await;
        let document = project(&catalog, &owner).await;
        let before = catalog.usage_bytes(Some(owner.id)).await.unwrap();
        let rows = vec![
            message(Some(owner.id), "hello"),
            message(Some(owner.id), "world!"),
        ];
        let charged = catalog
            .insert_chat_messages(document, &rows, None)
            .await
            .unwrap()
            .charged;
        assert_eq!(charged, 5 + 6 + 2 * CHAT_ROW_OVERHEAD_BYTES);
        assert_eq!(chat_bytes(&catalog, document).await, charged);
        let after = catalog.usage_bytes(Some(owner.id)).await.unwrap();
        assert_eq!(after - before, charged);
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn pages_run_oldest_first_with_before_and_more() {
        let catalog = catalog().await;
        let _writer = catalog.claim_writer().await.unwrap();
        let owner = account(&catalog).await;
        let document = project(&catalog, &owner).await;
        let rows: Vec<ChatRecord> = (0..5)
            .map(|n| message(Some(owner.id), &format!("m{n}")))
            .collect();
        catalog
            .insert_chat_messages(document, &rows, None)
            .await
            .unwrap();

        let (newest, more) = catalog.chat_page(document, None, 2).await.unwrap();
        assert_eq!(newest, rows[3..5].to_vec());
        assert!(more);
        let (middle, more) = catalog
            .chat_page(document, Some(newest[0].id), 2)
            .await
            .unwrap();
        assert_eq!(middle, rows[1..3].to_vec());
        assert!(more);
        let (oldest, more) = catalog
            .chat_page(document, Some(middle[0].id), 2)
            .await
            .unwrap();
        assert_eq!(oldest, rows[0..1].to_vec());
        assert!(!more);
        let (all, more) = catalog.chat_page(document, None, 5).await.unwrap();
        assert_eq!(all.len(), 5);
        assert!(!more);
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn clear_deletes_rows_and_returns_the_bytes() {
        let catalog = catalog().await;
        let _writer = catalog.claim_writer().await.unwrap();
        let owner = account(&catalog).await;
        let document = project(&catalog, &owner).await;
        let rows = vec![message(None, "one"), message(None, "two")];
        let charged = catalog
            .insert_chat_messages(document, &rows, None)
            .await
            .unwrap()
            .charged;
        let cleared = catalog
            .clear_chat(document, &[], owner.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((cleared.rows, cleared.bytes), (2, charged));
        assert!(cleared.stored_buffered.is_empty());
        assert_eq!(chat_bytes(&catalog, document).await, 0);
        let (left, more) = catalog.chat_page(document, None, 10).await.unwrap();
        assert!(left.is_empty());
        assert!(!more);
        assert_eq!(
            catalog.clear_chat(document, &[], owner.id).await.unwrap(),
            Some(super::ChatCleared::default())
        );
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn trim_deletes_chat_and_reports_it() {
        let catalog = catalog().await;
        let _writer = catalog.claim_writer().await.unwrap();
        let owner = account(&catalog).await;
        let document = project(&catalog, &owner).await;
        let rows = vec![message(Some(owner.id), "kept until trim")];
        let charged = catalog
            .insert_chat_messages(document, &rows, None)
            .await
            .unwrap()
            .charged;
        let mut tx = catalog.begin_writer_transaction().await.unwrap();
        let outcome = catalog
            .trim_retained_in_transaction(&mut tx, document, &[], &[], Some(owner.id))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert!(outcome.chat_cleared);
        assert_eq!(outcome.chat_messages, 1);
        assert_eq!(outcome.chat_bytes, charged);
        assert_eq!(chat_bytes(&catalog, document).await, 0);
        let (left, _) = catalog.chat_page(document, None, 10).await.unwrap();
        assert!(left.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn trim_by_a_non_owner_keeps_the_chat() {
        let catalog = catalog().await;
        let _writer = catalog.claim_writer().await.unwrap();
        let owner = account(&catalog).await;
        let editor = account(&catalog).await;
        let document = project(&catalog, &owner).await;
        let rows = vec![message(Some(owner.id), "survives an editor's trim")];
        let charged = catalog
            .insert_chat_messages(document, &rows, None)
            .await
            .unwrap()
            .charged;
        for trimmer in [None, Some(editor.id)] {
            let mut tx = catalog.begin_writer_transaction().await.unwrap();
            let outcome = catalog
                .trim_retained_in_transaction(&mut tx, document, &[], &[], trimmer)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            assert!(!outcome.chat_cleared);
            assert_eq!(outcome.chat_messages, 0);
            assert_eq!(outcome.chat_bytes, 0);
            assert!(outcome.chat_stored_buffered.is_empty());
            assert_eq!(chat_bytes(&catalog, document).await, charged);
            let (left, _) = catalog.chat_page(document, None, 10).await.unwrap();
            assert_eq!(left.len(), 1);
        }
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn erasure_anonymizes_messages_and_keeps_the_charge() {
        let catalog = catalog().await;
        let _writer = catalog.claim_writer().await.unwrap();
        let owner = account(&catalog).await;
        let author = account(&catalog).await;
        let document = project(&catalog, &owner).await;
        let rows = vec![message(Some(author.id), "from a guest")];
        let charged = catalog
            .insert_chat_messages(document, &rows, None)
            .await
            .unwrap()
            .charged;
        sqlx::query("UPDATE accounts SET status='erasing' WHERE id=$1")
            .bind(author.id)
            .execute(catalog.pool())
            .await
            .unwrap();
        assert!(catalog.finish_account_erasure(author.id).await.unwrap());
        let (page, _) = catalog.chat_page(document, None, 10).await.unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].author_account_id, None);
        assert_eq!(page[0].author_label, "Deleted user");
        assert_eq!(page[0].body, "from a guest");
        assert_eq!(chat_bytes(&catalog, document).await, charged);
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn a_missing_author_account_is_stored_as_null() {
        let catalog = catalog().await;
        let _writer = catalog.claim_writer().await.unwrap();
        let owner = account(&catalog).await;
        let document = project(&catalog, &owner).await;
        let rows = vec![message(Some(Uuid::now_v7()), "from nobody")];
        let charged = catalog
            .insert_chat_messages(document, &rows, None)
            .await
            .unwrap()
            .charged;
        let (page, _) = catalog.chat_page(document, None, 10).await.unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].author_account_id, None);
        assert_eq!(page[0].author_label, "Deleted user");
        assert_eq!(chat_bytes(&catalog, document).await, charged);
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn inserting_the_same_batch_twice_stores_and_charges_once() {
        let catalog = catalog().await;
        let _writer = catalog.claim_writer().await.unwrap();
        let owner = account(&catalog).await;
        let document = project(&catalog, &owner).await;
        let rows = vec![message(None, "one"), message(None, "two")];
        let first = catalog
            .insert_chat_messages(document, &rows, None)
            .await
            .unwrap()
            .charged;
        let second = catalog
            .insert_chat_messages(document, &rows, None)
            .await
            .unwrap()
            .charged;
        assert_eq!(first, 3 + 3 + 2 * super::CHAT_ROW_OVERHEAD_BYTES);
        assert_eq!(second, 0);
        let (page, _) = catalog.chat_page(document, None, 10).await.unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(chat_bytes(&catalog, document).await, first);
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn an_erasing_author_is_stored_anonymously_and_an_active_one_is_kept() {
        let catalog = catalog().await;
        let _writer = catalog.claim_writer().await.unwrap();
        let owner = account(&catalog).await;
        let erasing = account(&catalog).await;
        let document = project(&catalog, &owner).await;
        sqlx::query("UPDATE accounts SET status='erasing' WHERE id=$1")
            .bind(erasing.id)
            .execute(catalog.pool())
            .await
            .unwrap();
        let rows = vec![
            message(Some(erasing.id), "from the erased"),
            message(Some(owner.id), "from the owner"),
        ];
        catalog
            .insert_chat_messages(document, &rows, None)
            .await
            .unwrap();
        let (page, _) = catalog.chat_page(document, None, 10).await.unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].author_account_id, None);
        assert_eq!(page[0].author_label, "Deleted user");
        assert_eq!(page[1].author_account_id, Some(owner.id));
        assert_eq!(page[1].author_label, "Tester");
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn clear_reports_which_buffered_ids_were_already_stored() {
        let catalog = catalog().await;
        let _writer = catalog.claim_writer().await.unwrap();
        let owner = account(&catalog).await;
        let document = project(&catalog, &owner).await;
        let stored = message(None, "stored");
        let unstored = message(None, "waiting");
        catalog
            .insert_chat_messages(document, std::slice::from_ref(&stored), None)
            .await
            .unwrap();
        let cleared = catalog
            .clear_chat(document, &[stored.id, unstored.id], owner.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cleared.rows, 1);
        assert_eq!(cleared.stored_buffered, vec![stored.id]);
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn clear_by_a_non_owner_deletes_nothing() {
        let catalog = catalog().await;
        let _writer = catalog.claim_writer().await.unwrap();
        let owner = account(&catalog).await;
        let other = account(&catalog).await;
        let document = project(&catalog, &owner).await;
        let rows = vec![message(None, "one"), message(None, "two")];
        let charged = catalog
            .insert_chat_messages(document, &rows, None)
            .await
            .unwrap()
            .charged;
        let refused = catalog.clear_chat(document, &[], other.id).await.unwrap();
        assert_eq!(refused, None);
        assert_eq!(chat_bytes(&catalog, document).await, charged);
        let (left, _) = catalog.chat_page(document, None, 10).await.unwrap();
        assert_eq!(left.len(), 2);
    }

    async fn reservation_bytes(catalog: &PostgresCatalog, key: Uuid) -> Option<i64> {
        sqlx::query_scalar("SELECT bytes FROM pending_log_reservations WHERE reservation_id=$1")
            .bind(key)
            .fetch_optional(catalog.pool())
            .await
            .unwrap()
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn a_reservation_counts_toward_owner_usage() {
        let catalog = catalog().await;
        let _writer = catalog.claim_writer().await.unwrap();
        let owner = account(&catalog).await;
        let document = project(&catalog, &owner).await;
        let key = Uuid::now_v7();
        let before = catalog.usage_bytes(Some(owner.id)).await.unwrap();
        let total = catalog
            .reserve_chat_bytes(document, key, CHAT_RESERVATION_CHUNK, 10)
            .await
            .unwrap();
        assert_eq!(total, CHAT_RESERVATION_CHUNK);
        let after = catalog.usage_bytes(Some(owner.id)).await.unwrap();
        assert_eq!(after - before, CHAT_RESERVATION_CHUNK);
        let again = catalog
            .reserve_chat_bytes(document, key, 100, 10)
            .await
            .unwrap();
        assert_eq!(again, CHAT_RESERVATION_CHUNK + 100);
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn a_reservation_is_capped_by_the_owner_allowance() {
        let mut catalog = catalog().await;
        let _writer = catalog.claim_writer().await.unwrap();
        let owner = account(&catalog).await;
        let document = project(&catalog, &owner).await;
        let before = catalog.usage_bytes(Some(owner.id)).await.unwrap();
        catalog.policy.owner_bytes = before + 100;
        let key = Uuid::now_v7();
        let granted = catalog
            .reserve_chat_bytes(document, key, CHAT_RESERVATION_CHUNK, 40)
            .await
            .unwrap();
        assert_eq!(granted, 100);
        let refused = catalog
            .reserve_chat_bytes(document, key, CHAT_RESERVATION_CHUNK, 1)
            .await;
        assert!(matches!(refused, Err(super::Error::Conflict(_))));
        assert_eq!(reservation_bytes(&catalog, key).await, Some(100));
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn a_minimum_that_does_not_fit_is_refused() {
        let mut catalog = catalog().await;
        let _writer = catalog.claim_writer().await.unwrap();
        let owner = account(&catalog).await;
        let document = project(&catalog, &owner).await;
        let before = catalog.usage_bytes(Some(owner.id)).await.unwrap();
        catalog.policy.owner_bytes = before + 50;
        let key = Uuid::now_v7();
        let refused = catalog
            .reserve_chat_bytes(document, key, CHAT_RESERVATION_CHUNK, 51)
            .await;
        assert!(matches!(refused, Err(super::Error::Conflict(_))));
        assert_eq!(reservation_bytes(&catalog, key).await, None);
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn inserting_consumes_the_reservation_and_deletes_it_at_zero() {
        let catalog = catalog().await;
        let _writer = catalog.claim_writer().await.unwrap();
        let owner = account(&catalog).await;
        let document = project(&catalog, &owner).await;
        let rows = vec![message(Some(owner.id), "hello")];
        let charge = 5 + CHAT_ROW_OVERHEAD_BYTES;
        let key = Uuid::now_v7();
        catalog
            .reserve_chat_bytes(document, key, charge + 10, charge)
            .await
            .unwrap();
        let inserted = catalog
            .insert_chat_messages(document, &rows, Some(key))
            .await
            .unwrap();
        assert_eq!(inserted.charged, charge);
        assert_eq!(inserted.reserved, 10);
        assert_eq!(reservation_bytes(&catalog, key).await, Some(10));
        let more = vec![message(Some(owner.id), "hello and more")];
        let inserted = catalog
            .insert_chat_messages(document, &more, Some(key))
            .await
            .unwrap();
        assert_eq!(inserted.reserved, 0);
        assert_eq!(reservation_bytes(&catalog, key).await, None);
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn release_deletes_the_reservation_and_ignores_a_missing_one() {
        let catalog = catalog().await;
        let _writer = catalog.claim_writer().await.unwrap();
        let owner = account(&catalog).await;
        let document = project(&catalog, &owner).await;
        let key = Uuid::now_v7();
        catalog
            .reserve_chat_bytes(document, key, 100, 10)
            .await
            .unwrap();
        catalog
            .release_chat_reservation(document, key)
            .await
            .unwrap();
        assert_eq!(reservation_bytes(&catalog, key).await, None);
        catalog
            .release_chat_reservation(document, key)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn chat_reservation_bytes_reads_the_row_and_zero_when_missing() {
        let catalog = catalog().await;
        let _writer = catalog.claim_writer().await.unwrap();
        let owner = account(&catalog).await;
        let document = project(&catalog, &owner).await;
        let key = Uuid::now_v7();
        assert_eq!(
            catalog.chat_reservation_bytes(document, key).await.unwrap(),
            0
        );
        catalog
            .reserve_chat_bytes(document, key, 100, 10)
            .await
            .unwrap();
        assert_eq!(
            catalog.chat_reservation_bytes(document, key).await.unwrap(),
            100
        );
    }
}
