//! Favourites, and what each person opened when.
//!
//! Both are one person's private notes about a document rather than anything
//! the document itself knows, so nothing here is visible to the owner of a
//! project somebody else has starred, and nothing here grants any access: a
//! mark on a document whose share link was since revoked simply stops being
//! reachable, because the listing that reads these marks joins them to the
//! documents the caller may already see.

use time::OffsetDateTime;
use uuid::Uuid;

use super::{Error, PostgresCatalog, Result};

#[cfg(test)]
#[path = "listing_regression_tests.rs"]
mod listing_regression_tests;

#[derive(Clone, Debug)]
pub struct MarkRecord {
    pub document_id: Uuid,
    pub favorited_at: Option<OffsetDateTime>,
    pub opened_at: Option<OffsetDateTime>,
}

/// The two numbers the listing prints beside a project's name.
#[derive(Clone, Debug)]
pub struct CountRecord {
    pub document_id: Uuid,
    pub comments: i64,
    pub open: i64,
    pub file_count: Option<i32>,
}

impl PostgresCatalog {
    /// Star a document, or take the star off it. Returns whether it is starred
    /// afterwards, which is what the caller was asking for and saves it
    /// reading the row back to find out.
    pub async fn set_favorite(
        &self,
        account_id: Uuid,
        document_id: Uuid,
        on: bool,
    ) -> Result<bool> {
        if on {
            sqlx::query!(
                "INSERT INTO document_marks(account_id,document_id,favorited_at)
                 VALUES($1,$2,now())
                 ON CONFLICT(account_id,document_id) DO UPDATE SET favorited_at=now()",
                account_id,
                document_id,
            )
            .execute(&self.pool)
            .await?;
            return Ok(true);
        }
        // Keep the pair of writes together.  In particular, do not briefly
        // clear a never-opened favourite: the CHECK quite deliberately rejects
        // that intermediate row.  The row lock also makes this serialize with
        // mark_opened when both operations race on an existing mark.
        let mut tx = self.begin_metered().await?;
        let _ = sqlx::query!(
            "SELECT account_id FROM document_marks
             WHERE account_id=$1 AND document_id=$2 FOR UPDATE",
            account_id,
            document_id,
        )
        .fetch_optional(&mut *tx)
        .await?;
        sqlx::query!(
            "DELETE FROM document_marks
             WHERE account_id=$1 AND document_id=$2
               AND favorited_at IS NOT NULL AND opened_at IS NULL",
            account_id,
            document_id,
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!(
            "UPDATE document_marks SET favorited_at=NULL
             WHERE account_id=$1 AND document_id=$2 AND opened_at IS NOT NULL",
            account_id,
            document_id,
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(false)
    }

    /// Note that this person has just opened this document. Called on every
    /// open, so it is one upsert and never a read first.
    pub async fn mark_opened(&self, account_id: Uuid, document_id: Uuid) -> Result<()> {
        sqlx::query!(
            "INSERT INTO document_marks(account_id,document_id,opened_at)
             VALUES($1,$2,now())
             ON CONFLICT(account_id,document_id) DO UPDATE SET opened_at=now()",
            account_id,
            document_id,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Every mark this person holds on the documents named. The listing asks
    /// for one page of documents at a time and decorates it with this, rather
    /// than joining the marks into the listing query: the marks are the
    /// caller's and the documents are not, and keeping the two apart is what
    /// stops a mark from ever widening what the listing returns.
    pub async fn marks_for_documents(
        &self,
        account_id: Uuid,
        document_ids: &[Uuid],
    ) -> Result<Vec<MarkRecord>> {
        if document_ids.is_empty() {
            return Ok(Vec::new());
        }
        sqlx::query_as!(
            MarkRecord,
            "SELECT document_id,favorited_at,opened_at FROM document_marks
             WHERE account_id=$1 AND document_id=ANY($2)",
            account_id,
            document_ids,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    /// What the listing says about each project besides its name: how many
    /// comments it has collected, and how many versions it holds.
    ///
    /// One query per fact for a whole page of projects, rather than the
    /// request per project the landing page used to make. The comments are an
    /// aggregate over `annotations`, and so, now, is `file_count`: there is
    /// no per-version row to read a stored count off any more (§8.3 dropped
    /// `document_versions`), a named moment is a `document_labels` row
    /// (`storage/postgres/labels.rs`), and this field is that count.
    ///
    /// A document with no labels yields `None` rather than zero: the `LEFT
    /// JOIN` produces no row for it, same as a document with no comments
    /// produces no row in the comments aggregate, and the listing draws both
    /// absences the same way.
    pub async fn listing_counts(&self, document_ids: &[Uuid]) -> Result<Vec<CountRecord>> {
        if document_ids.is_empty() {
            return Ok(Vec::new());
        }
        sqlx::query_as!(
            CountRecord,
            r#"SELECT d.id AS "document_id!",
                      COALESCE(a.comments, 0) AS "comments!",
                      COALESCE(a.open, 0) AS "open!",
                      l.file_count
               FROM documents d
               LEFT JOIN (
                   SELECT document_id, count(*)::int4 AS file_count
                   FROM document_labels
                   WHERE document_id = ANY($1)
                   GROUP BY document_id
               ) l ON l.document_id = d.id
               LEFT JOIN (
                   SELECT document_id,
                          count(*) AS comments,
                          count(*) FILTER (WHERE resolved_at IS NULL) AS open
                   FROM annotations
                   WHERE document_id = ANY($1) AND kind = 'comment'
                   GROUP BY document_id
               ) a ON a.document_id = d.id
               WHERE d.id = ANY($1)"#,
            document_ids,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }
}
