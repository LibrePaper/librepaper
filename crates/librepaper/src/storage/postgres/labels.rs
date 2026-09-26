//! Labels: named moments, and the retry record of every command that
//! produced source.
//!
//! One table for both, because they are the same row. A label somebody asked
//! for names a state by vector, frontier and projection digest; a restore, an
//! agent patch or a whole-project replacement names exactly the same three
//! things, plus the `request_id` that makes a retry return the first answer
//! instead of doing the work again (SPEC-server-is-a-log §7.2, §8.2).
//!
//! Nothing writes one on a timer. Every row here is a deliberate act.

use time::OffsetDateTime;
use uuid::Uuid;

use super::{Error, PostgresCatalog, Result};

/// A row to write. `sequence` is assigned here, under the document lock, so
/// two commands cannot claim one.
#[derive(Clone, Debug)]
pub struct NewLabel {
    pub id: Uuid,
    pub document_id: Uuid,
    /// The log row that made this state durable (§7 step 4).
    pub source_sequence: i64,
    pub vector: Vec<u8>,
    pub frontier: Vec<u8>,
    pub tree_digest: Option<[u8; 32]>,
    pub label: Option<String>,
    /// `label` for one somebody named, or the command's own name.
    pub reason: String,
    /// The idempotency key of a source-producing command, when there is one.
    pub request_id: Option<Uuid>,
    pub author_account_id: Option<Uuid>,
    pub author_label: String,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct LabelRecord {
    pub id: Uuid,
    pub document_id: Uuid,
    pub sequence: i64,
    pub source_sequence: i64,
    pub vector: Vec<u8>,
    pub frontier: Vec<u8>,
    pub tree_digest: Option<Vec<u8>>,
    pub label: Option<String>,
    pub reason: String,
    pub request_id: Option<Uuid>,
    pub author_account_id: Option<Uuid>,
    pub author_label: String,
    pub created_at: OffsetDateTime,
    pub archive_requested_at: Option<OffsetDateTime>,
    pub archive_key: Option<String>,
    pub archive_bytes: Option<i64>,
    pub archive_error: Option<String>,
}

const SELECT: &str = "SELECT id,document_id,sequence,source_sequence,vector,frontier,tree_digest,\
     label,reason,request_id,author_account_id,author_label,created_at,archive_requested_at,\
     archive_key,archive_bytes,archive_error FROM document_labels";

impl PostgresCatalog {
    /// Writes a label inside a transaction the caller owns, which is how it
    /// becomes atomic with the source it names (§7 step 4).
    ///
    /// A retry with the same `request_id` finds the existing row and returns
    /// it: `ON CONFLICT DO NOTHING RETURNING` yields nothing, and the select
    /// after it yields the first answer.
    pub async fn insert_label(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        input: &NewLabel,
    ) -> Result<LabelRecord> {
        if input.reason.is_empty() || input.author_label.len() > 500 {
            return Err(Error::Invalid("invalid label".into()));
        }
        if let Some(request_id) = input.request_id {
            if let Some(existing) = sqlx::query_as::<_, LabelRecord>(&format!(
                "{SELECT} WHERE document_id=$1 AND request_id=$2"
            ))
            .bind(input.document_id)
            .bind(request_id)
            .fetch_optional(&mut **tx)
            .await?
            {
                return Ok(existing);
            }
        }
        let sequence: i64 = sqlx::query_scalar(
            "SELECT COALESCE(max(sequence),0)+1 FROM document_labels WHERE document_id=$1",
        )
        .bind(input.document_id)
        .fetch_one(&mut **tx)
        .await?;
        sqlx::query_as::<_, LabelRecord>(
            "INSERT INTO document_labels\
             (id,document_id,sequence,source_sequence,vector,frontier,tree_digest,label,reason,\
              request_id,author_account_id,author_label)\
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)\
             RETURNING id,document_id,sequence,source_sequence,vector,frontier,tree_digest,label,\
              reason,request_id,author_account_id,author_label,created_at,archive_requested_at,\
              archive_key,archive_bytes,archive_error",
        )
        .bind(input.id)
        .bind(input.document_id)
        .bind(sequence)
        .bind(input.source_sequence)
        .bind(&input.vector)
        .bind(&input.frontier)
        .bind(input.tree_digest.as_ref().map(|digest| digest.as_slice()))
        .bind(&input.label)
        .bind(&input.reason)
        .bind(input.request_id)
        .bind(input.author_account_id)
        .bind(&input.author_label)
        .fetch_one(&mut **tx)
        .await
        .map_err(Error::from)
    }

    /// Finds the label a previous attempt at this request left behind, if
    /// any (§7.2). A plain, un-transacted read: this is what a command's
    /// `replay` step calls before the sequencer's lock is even taken, so
    /// there is no transaction to run it inside yet, and nothing here needs
    /// one -- a stale answer only means the retry does a little more work,
    /// never that it writes a wrong one, because `insert_label` still makes
    /// the same check again, atomically, inside the transaction that would
    /// write a second row.
    pub async fn label_by_request(
        &self,
        document_id: Uuid,
        request_id: Uuid,
    ) -> Result<Option<LabelRecord>> {
        sqlx::query_as::<_, LabelRecord>(&format!(
            "{SELECT} WHERE document_id=$1 AND request_id=$2"
        ))
        .bind(document_id)
        .bind(request_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn label(&self, document_id: Uuid, id: Uuid) -> Result<Option<LabelRecord>> {
        sqlx::query_as::<_, LabelRecord>(&format!("{SELECT} WHERE document_id=$1 AND id=$2"))
            .bind(document_id)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(Error::from)
    }

    /// One page of the timeline, newest first.
    pub async fn label_page(
        &self,
        document_id: Uuid,
        after: Option<i64>,
        limit: i64,
    ) -> Result<Vec<LabelRecord>> {
        if !(1..=200).contains(&limit) {
            return Err(Error::Invalid("label page limit must be 1..=200".into()));
        }
        sqlx::query_as::<_, LabelRecord>(&format!(
            "{SELECT} WHERE document_id=$1 AND sequence < COALESCE($2,9223372036854775807) \
             ORDER BY sequence DESC LIMIT $3"
        ))
        .bind(document_id)
        .bind(after)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    /// Renames one. Naming a moment does not move the document, so unlike
    /// every other write here this one needs no source and no fence beyond
    /// the writer epoch.
    pub async fn rename_label(
        &self,
        document_id: Uuid,
        id: Uuid,
        label: Option<&str>,
    ) -> Result<bool> {
        let mut tx = self.begin_writer_transaction().await?;
        let changed =
            sqlx::query("UPDATE document_labels SET label=$3 WHERE document_id=$1 AND id=$2")
                .bind(document_id)
                .bind(id)
                .bind(label)
                .execute(&mut *tx)
                .await?
                .rows_affected()
                == 1;
        tx.commit().await?;
        Ok(changed)
    }

    /// §8.5: asking for an archive is durable state, not a queued row. The
    /// startup scan finds a label with `archive_requested_at` and no
    /// `archive_key` and produces it.
    pub async fn request_label_archive(&self, document_id: Uuid, id: Uuid) -> Result<bool> {
        let mut tx = self.begin_writer_transaction().await?;
        let changed = sqlx::query(
            "UPDATE document_labels SET archive_requested_at=COALESCE(archive_requested_at,now()),\
                    archive_error=NULL
             WHERE document_id=$1 AND id=$2 AND archive_key IS NULL",
        )
        .bind(document_id)
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        tx.commit().await?;
        Ok(changed)
    }

    /// The archive landed. Key and weight move together so the storage
    /// trigger has a number for the object it is counting.
    pub async fn attach_label_archive(
        &self,
        id: Uuid,
        archive_key: &str,
        archive_bytes: i64,
    ) -> Result<bool> {
        if archive_key.is_empty() || archive_bytes < 0 {
            return Err(Error::Invalid("invalid label archive".into()));
        }
        let mut tx = self.begin_writer_transaction().await?;
        let changed = sqlx::query(
            "UPDATE document_labels SET archive_key=$2,archive_bytes=$3,archive_error=NULL \
             WHERE id=$1 AND archive_key IS NULL",
        )
        .bind(id)
        .bind(archive_key)
        .bind(archive_bytes)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        tx.commit().await?;
        Ok(changed)
    }

    /// It did not. `archive_requested_at` is left in place, so the startup
    /// scan tries again (§8.5).
    pub async fn note_label_archive_error(&self, id: Uuid, message: &str) -> Result<()> {
        let mut tx = self.begin_writer_transaction().await?;
        sqlx::query(
            "UPDATE document_labels SET archive_error=$2 WHERE id=$1 AND archive_key IS NULL",
        )
        .bind(id)
        .bind(message)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Every archive key the deployment holds, for the backup enumerator and
    /// the orphan sweeper (§8.5).
    pub async fn label_archive_keys(&self, after: Option<&str>, limit: i64) -> Result<Vec<String>> {
        if !(1..=1000).contains(&limit) {
            return Err(Error::Invalid(
                "archive key page limit must be 1..=1000".into(),
            ));
        }
        sqlx::query_scalar(
            "SELECT DISTINCT archive_key FROM document_labels \
             WHERE archive_key IS NOT NULL AND archive_key > COALESCE($1,'') \
             ORDER BY archive_key LIMIT $2",
        )
        .bind(after)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }
}

impl PostgresCatalog {
    /// One label by id alone, for the background worker: the startup scan
    /// returns ids, and the row itself says which document it belongs to.
    pub async fn label_by_id(&self, id: Uuid) -> Result<Option<LabelRecord>> {
        sqlx::query_as::<_, LabelRecord>(&format!("{SELECT} WHERE id=$1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(Error::from)
    }
}
