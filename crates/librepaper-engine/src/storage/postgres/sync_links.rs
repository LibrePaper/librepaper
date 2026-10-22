//! Sync links: the git remote a project is connected to, and what the last
//! sync left behind.
//!
//! The row changes on connect, pull, push and disconnect. A write from a
//! desktop carries the revision it read, and the row moves to the next
//! revision only if that revision is still the stored one. A source change
//! writes the link in the same transaction as the change, through
//! `put_sync_link` on the transaction's connection.

use serde::Serializer;
use sqlx::types::Json;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{Error, PostgresCatalog, Result};

const LINK_COLUMNS: &str = "document_id, revision, remote_kind, remote_label, connection_id, \
     branch, folder, checkpoint, synced_paths, connected_by, last_pull_at, last_pull_by, \
     last_push_at, last_push_by";

#[derive(Clone, Debug, serde::Serialize, sqlx::FromRow)]
pub struct SyncLink {
    pub document_id: Uuid,
    pub revision: i64,
    pub remote_kind: String,
    pub remote_label: String,
    pub connection_id: Uuid,
    pub branch: String,
    pub folder: String,
    pub checkpoint: Option<String>,
    pub synced_paths: Json<Vec<String>>,
    pub connected_by: Option<Uuid>,
    #[serde(serialize_with = "serialize_optional_time")]
    pub last_pull_at: Option<OffsetDateTime>,
    pub last_pull_by: Option<Uuid>,
    #[serde(serialize_with = "serialize_optional_time")]
    pub last_push_at: Option<OffsetDateTime>,
    pub last_push_by: Option<Uuid>,
}

#[derive(Clone, Debug)]
pub struct SyncLinkWrite {
    pub remote_kind: String,
    pub remote_label: String,
    pub connection_id: Uuid,
    pub branch: String,
    pub folder: String,
    pub checkpoint: Option<String>,
    pub synced_paths: Vec<String>,
    /// Set on create only; an update keeps the stored value.
    pub connected_by: Option<Uuid>,
    /// Some sets last_pull_at to now() and last_pull_by to this account.
    pub pulled_by: Option<Uuid>,
    /// Some sets last_push_at to now() and last_push_by to this account.
    pub pushed_by: Option<Uuid>,
}

/// Times go out as RFC 3339 text, the form the wire contract uses.
fn serialize_optional_time<S: Serializer>(
    value: &Option<OffsetDateTime>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    match value {
        Some(time) => {
            let text = time.format(&Rfc3339).map_err(serde::ser::Error::custom)?;
            serializer.serialize_some(&text)
        }
        None => serializer.serialize_none(),
    }
}

impl PostgresCatalog {
    /// The link on a document, or none if the document is not connected.
    pub async fn sync_link(&self, document_id: Uuid) -> Result<Option<SyncLink>> {
        let sql = format!("SELECT {LINK_COLUMNS} FROM sync_links WHERE document_id=$1");
        sqlx::query_as::<_, SyncLink>(&sql)
            .bind(document_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(Error::from)
    }

    /// Writes the row on an open connection, so a caller can do it inside its
    /// own transaction.
    ///
    /// `expected_revision` of none creates the row and answers none if one
    /// already exists. `Some(n)` updates the row only while its revision is
    /// n, moves the revision on, and answers none on a mismatch.
    pub async fn put_sync_link(
        connection: &mut sqlx::PgConnection,
        document_id: Uuid,
        expected_revision: Option<i64>,
        write: &SyncLinkWrite,
    ) -> Result<Option<SyncLink>> {
        let sql = match expected_revision {
            None => format!(
                "INSERT INTO sync_links (document_id, remote_kind, remote_label, connection_id, \
                 branch, folder, checkpoint, synced_paths, connected_by, \
                 last_pull_at, last_pull_by, last_push_at, last_push_by) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, \
                 CASE WHEN $10::uuid IS NOT NULL THEN now() END, $10, \
                 CASE WHEN $11::uuid IS NOT NULL THEN now() END, $11) \
                 ON CONFLICT (document_id) DO NOTHING \
                 RETURNING {LINK_COLUMNS}"
            ),
            Some(_) => format!(
                "UPDATE sync_links SET \
                 remote_kind=$2, remote_label=$3, connection_id=$4, branch=$5, folder=$6, \
                 checkpoint=$7, synced_paths=$8, \
                 last_pull_at=CASE WHEN $10::uuid IS NOT NULL THEN now() ELSE last_pull_at END, \
                 last_pull_by=COALESCE($10, last_pull_by), \
                 last_push_at=CASE WHEN $11::uuid IS NOT NULL THEN now() ELSE last_push_at END, \
                 last_push_by=COALESCE($11, last_push_by), \
                 revision=revision + 1, updated_at=now() \
                 WHERE document_id=$1 AND revision=$12 \
                 RETURNING {LINK_COLUMNS}"
            ),
        };
        let query = sqlx::query_as::<_, SyncLink>(&sql)
            .bind(document_id)
            .bind(write.remote_kind.clone())
            .bind(write.remote_label.clone())
            .bind(write.connection_id)
            .bind(write.branch.clone())
            .bind(write.folder.clone())
            .bind(write.checkpoint.clone())
            .bind(Json(write.synced_paths.clone()))
            .bind(write.connected_by)
            .bind(write.pulled_by)
            .bind(write.pushed_by);
        let link = match expected_revision {
            None => query.fetch_optional(&mut *connection).await?,
            Some(revision) => {
                query
                    .bind(revision)
                    .fetch_optional(&mut *connection)
                    .await?
            }
        };
        Ok(link)
    }

    /// `put_sync_link` in a transaction of its own.
    pub async fn put_sync_link_now(
        &self,
        document_id: Uuid,
        expected_revision: Option<i64>,
        write: &SyncLinkWrite,
    ) -> Result<Option<SyncLink>> {
        let mut tx = self.begin_metered().await?;
        let link = Self::put_sync_link(&mut tx, document_id, expected_revision, write).await?;
        tx.commit().await?;
        Ok(link)
    }

    /// Forgets the link. Answers whether there was one to forget.
    pub async fn delete_sync_link(&self, document_id: Uuid) -> Result<bool> {
        let removed = sqlx::query("DELETE FROM sync_links WHERE document_id=$1")
            .bind(document_id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(removed == 1)
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{PostgresCatalog, SyncLinkWrite};
    use crate::storage::postgres::{NewAccount, NewDocument, PostgresOptions};

    async fn test_catalog() -> PostgresCatalog {
        let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
            .expect("set LIBREPAPER_TEST_POSTGRES_URL to a throwaway PostgreSQL database");
        let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .expect("connect to PostgreSQL");
        catalog.migrate().await.expect("run PostgreSQL migrations");
        catalog
    }

    async fn account(catalog: &PostgresCatalog) -> Uuid {
        catalog
            .create_account(NewAccount {
                kind: "anonymous".into(),
                provider: None,
                provider_subject: None,
                handle: format!("sync-test-{}", Uuid::now_v7()),
                display_name: "Sync test".into(),
                email: None,
            })
            .await
            .unwrap()
            .id
    }

    async fn document(catalog: &PostgresCatalog, owner_id: Uuid) -> Uuid {
        catalog
            .create_document(NewDocument {
                slug: format!("sync-test-{}", Uuid::now_v7()),
                owner_id,
                owner_session_generation: None,
                ownership_mode: "owned".into(),
                title: "Sync test".into(),
                source_format: "markdown".into(),
                main_path: "main.md".into(),
            })
            .await
            .unwrap()
            .id
    }

    fn write(connected_by: Option<Uuid>) -> SyncLinkWrite {
        SyncLinkWrite {
            remote_kind: "network".into(),
            remote_label: "https://example.org/owner/name.git".into(),
            connection_id: Uuid::now_v7(),
            branch: "main".into(),
            folder: "paper".into(),
            checkpoint: Some("a".repeat(40)),
            synced_paths: vec!["paper.qmd".into(), "refs.bib".into()],
            connected_by,
            pulled_by: None,
            pushed_by: None,
        }
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn link_is_created_and_read_back() {
        let catalog = test_catalog().await;
        let owner = account(&catalog).await;
        let doc = document(&catalog, owner).await;

        assert!(catalog.sync_link(doc).await.unwrap().is_none());
        let created = catalog
            .put_sync_link_now(doc, None, &write(Some(owner)))
            .await
            .unwrap()
            .expect("the first create writes the row");
        assert_eq!(created.revision, 1);
        assert_eq!(created.remote_kind, "network");
        assert_eq!(created.folder, "paper");
        assert_eq!(created.connected_by, Some(owner));
        assert!(created.last_pull_at.is_none());
        assert!(created.last_push_at.is_none());

        let read = catalog
            .sync_link(doc)
            .await
            .unwrap()
            .expect("the link reads back");
        assert_eq!(read.checkpoint, Some("a".repeat(40)));
        assert_eq!(read.synced_paths.0, write(None).synced_paths);
        assert_eq!(read.revision, 1);
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn a_link_may_be_created_before_the_first_sync() {
        let catalog = test_catalog().await;
        let owner = account(&catalog).await;
        let doc = document(&catalog, owner).await;

        let mut first = write(Some(owner));
        first.checkpoint = None;
        first.synced_paths = Vec::new();
        let created = catalog
            .put_sync_link_now(doc, None, &first)
            .await
            .unwrap()
            .expect("create");
        assert!(created.checkpoint.is_none());
        assert!(created.synced_paths.0.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn second_create_answers_none_and_keeps_the_row() {
        let catalog = test_catalog().await;
        let owner = account(&catalog).await;
        let doc = document(&catalog, owner).await;

        catalog
            .put_sync_link_now(doc, None, &write(Some(owner)))
            .await
            .unwrap()
            .expect("the first create writes the row");
        let mut other = write(None);
        other.branch = "other".into();
        let again = catalog.put_sync_link_now(doc, None, &other).await.unwrap();
        assert!(
            again.is_none(),
            "a second create finds the row and writes nothing"
        );

        let read = catalog.sync_link(doc).await.unwrap().unwrap();
        assert_eq!(read.revision, 1);
        assert_eq!(read.branch, "main");
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn update_with_the_stored_revision_moves_it_on_and_keeps_connected_by() {
        let catalog = test_catalog().await;
        let owner = account(&catalog).await;
        let other = account(&catalog).await;
        let doc = document(&catalog, owner).await;

        catalog
            .put_sync_link_now(doc, None, &write(Some(owner)))
            .await
            .unwrap()
            .expect("create");

        let mut next = write(Some(other));
        next.checkpoint = Some("d".repeat(40));
        next.synced_paths = vec!["paper.qmd".into()];
        let updated = catalog
            .put_sync_link_now(doc, Some(1), &next)
            .await
            .unwrap()
            .expect("a matching revision updates the row");
        assert_eq!(updated.revision, 2);
        assert_eq!(updated.checkpoint, Some("d".repeat(40)));
        assert_eq!(updated.synced_paths.0, vec!["paper.qmd".to_string()]);
        assert_eq!(
            updated.connected_by,
            Some(owner),
            "an update keeps who connected the project"
        );
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn update_with_a_stale_revision_answers_none_and_changes_nothing() {
        let catalog = test_catalog().await;
        let owner = account(&catalog).await;
        let doc = document(&catalog, owner).await;

        catalog
            .put_sync_link_now(doc, None, &write(Some(owner)))
            .await
            .unwrap()
            .expect("create");
        catalog
            .put_sync_link_now(doc, Some(1), &write(None))
            .await
            .unwrap()
            .expect("the first update at revision 1 lands");

        let mut stale = write(None);
        stale.checkpoint = Some("e".repeat(40));
        let refused = catalog
            .put_sync_link_now(doc, Some(1), &stale)
            .await
            .unwrap();
        assert!(refused.is_none(), "revision 1 is no longer stored");

        let read = catalog.sync_link(doc).await.unwrap().unwrap();
        assert_eq!(read.revision, 2);
        assert_eq!(read.checkpoint, Some("a".repeat(40)));
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn pull_and_push_stamp_only_their_own_time_and_account() {
        let catalog = test_catalog().await;
        let owner = account(&catalog).await;
        let doc = document(&catalog, owner).await;

        catalog
            .put_sync_link_now(doc, None, &write(Some(owner)))
            .await
            .unwrap()
            .expect("create");

        let mut pulled = write(None);
        pulled.pulled_by = Some(owner);
        let after_pull = catalog
            .put_sync_link_now(doc, Some(1), &pulled)
            .await
            .unwrap()
            .expect("pull");
        assert!(
            after_pull.last_pull_at.is_some(),
            "a pull sets last_pull_at"
        );
        assert_eq!(after_pull.last_pull_by, Some(owner));
        assert!(after_pull.last_push_at.is_none());
        assert!(after_pull.last_push_by.is_none());

        let pull_time = after_pull.last_pull_at;
        let mut pushed = write(None);
        pushed.pushed_by = Some(owner);
        let after_push = catalog
            .put_sync_link_now(doc, Some(2), &pushed)
            .await
            .unwrap()
            .expect("push");
        assert_eq!(
            after_push.last_pull_at, pull_time,
            "a push leaves last_pull_at alone"
        );
        assert_eq!(after_push.last_pull_by, Some(owner));
        assert!(after_push.last_push_at.is_some());
        assert_eq!(after_push.last_push_by, Some(owner));

        let after_plain = catalog
            .put_sync_link_now(doc, Some(3), &write(None))
            .await
            .unwrap()
            .expect("plain update");
        assert_eq!(after_plain.last_pull_at, pull_time);
        assert_eq!(after_plain.last_push_by, Some(owner));
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn delete_answers_whether_a_link_was_removed() {
        let catalog = test_catalog().await;
        let owner = account(&catalog).await;
        let doc = document(&catalog, owner).await;

        catalog
            .put_sync_link_now(doc, None, &write(Some(owner)))
            .await
            .unwrap()
            .expect("create");
        assert!(catalog.delete_sync_link(doc).await.unwrap());
        assert!(!catalog.delete_sync_link(doc).await.unwrap());
        assert!(catalog.sync_link(doc).await.unwrap().is_none());
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn deleting_the_document_removes_its_link() {
        let catalog = test_catalog().await;
        let owner = account(&catalog).await;
        let doc = document(&catalog, owner).await;

        catalog
            .put_sync_link_now(doc, None, &write(Some(owner)))
            .await
            .unwrap()
            .expect("create");
        sqlx::query("DELETE FROM documents WHERE id=$1")
            .bind(doc)
            .execute(catalog.pool())
            .await
            .expect("delete the document");
        assert!(catalog.sync_link(doc).await.unwrap().is_none());
    }
}
