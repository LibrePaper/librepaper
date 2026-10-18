//! Project chat history.
//!
//! Rooms buffer messages and flush them here in batches. Every message charges
//! its body bytes plus a fixed row overhead to `documents.chat_bytes`, which
//! counts toward the owner's storage. The author label is not charged, so
//! anonymizing it never moves the counter.

use time::OffsetDateTime;
use uuid::Uuid;

use super::{Error, PostgresCatalog, Result};

/// The fixed per-row charge added to a message's body bytes.
pub const CHAT_ROW_OVERHEAD_BYTES: i64 = 64;

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
    /// documents.chat_bytes. An author account that no longer exists is stored
    /// as NULL, with the label "Deleted user", instead of failing the batch.
    /// A row whose id is already stored is skipped and not charged again, so
    /// a retry after a lost reply is harmless. Returns the bytes charged.
    pub async fn insert_chat_messages(
        &self,
        document_id: Uuid,
        rows: &[ChatRecord],
    ) -> Result<i64> {
        if rows.is_empty() {
            return Ok(0);
        }
        let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
        let authors: Vec<Option<Uuid>> = rows.iter().map(|row| row.author_account_id).collect();
        let labels: Vec<String> = rows.iter().map(|row| row.author_label.clone()).collect();
        let bodies: Vec<String> = rows.iter().map(|row| row.body.clone()).collect();
        let created: Vec<OffsetDateTime> = rows.iter().map(|row| row.created_at).collect();
        let mut tx = self.begin_writer_transaction().await?;
        let inserted: Vec<i64> = sqlx::query_scalar(
            "INSERT INTO chat_messages (id,document_id,author_account_id,author_label,body,created_at) \
             SELECT m.id,$1,(SELECT a.id FROM accounts a WHERE a.id=m.author),\
             CASE WHEN m.author IS NOT NULL AND NOT EXISTS (SELECT 1 FROM accounts a WHERE a.id=m.author) \
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
        tx.commit().await?;
        Ok(charge)
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

    /// Deletes every message and zeroes chat_bytes. Returns (rows, bytes).
    pub async fn clear_chat(&self, document_id: Uuid) -> Result<(i64, i64)> {
        let mut tx = self.begin_writer_transaction().await?;
        let cleared = self.clear_chat_in_transaction(&mut tx, document_id).await?;
        tx.commit().await?;
        Ok(cleared)
    }

    /// Same as `clear_chat`, inside the caller's transaction. The second
    /// figure is what `chat_bytes` held, which is what the owner is credited.
    pub async fn clear_chat_in_transaction(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        document_id: Uuid,
    ) -> Result<(i64, i64)> {
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
        Ok((rows, bytes.unwrap_or(0)))
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::super::{NewAccount, NewDocument, PostgresCatalog, PostgresOptions};
    use super::{ChatRecord, CHAT_ROW_OVERHEAD_BYTES};

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
        let charged = catalog.insert_chat_messages(document, &rows).await.unwrap();
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
        catalog.insert_chat_messages(document, &rows).await.unwrap();

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
        let charged = catalog.insert_chat_messages(document, &rows).await.unwrap();
        let cleared = catalog.clear_chat(document).await.unwrap();
        assert_eq!(cleared, (2, charged));
        assert_eq!(chat_bytes(&catalog, document).await, 0);
        let (left, more) = catalog.chat_page(document, None, 10).await.unwrap();
        assert!(left.is_empty());
        assert!(!more);
        assert_eq!(catalog.clear_chat(document).await.unwrap(), (0, 0));
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn trim_deletes_chat_and_reports_it() {
        let catalog = catalog().await;
        let _writer = catalog.claim_writer().await.unwrap();
        let owner = account(&catalog).await;
        let document = project(&catalog, &owner).await;
        let rows = vec![message(Some(owner.id), "kept until trim")];
        let charged = catalog.insert_chat_messages(document, &rows).await.unwrap();
        let mut tx = catalog.begin_writer_transaction().await.unwrap();
        let outcome = catalog
            .trim_retained_in_transaction(&mut tx, document, &[])
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(outcome.chat_messages, 1);
        assert_eq!(outcome.chat_bytes, charged);
        assert_eq!(chat_bytes(&catalog, document).await, 0);
        let (left, _) = catalog.chat_page(document, None, 10).await.unwrap();
        assert!(left.is_empty());
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
        let charged = catalog.insert_chat_messages(document, &rows).await.unwrap();
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
        let charged = catalog.insert_chat_messages(document, &rows).await.unwrap();
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
        let first = catalog.insert_chat_messages(document, &rows).await.unwrap();
        let second = catalog.insert_chat_messages(document, &rows).await.unwrap();
        assert_eq!(first, 3 + 3 + 2 * super::CHAT_ROW_OVERHEAD_BYTES);
        assert_eq!(second, 0);
        let (page, _) = catalog.chat_page(document, None, 10).await.unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(chat_bytes(&catalog, document).await, first);
    }
}
