//! Forge links: the GitHub repository a project is connected to, and what the
//! last sync left behind.
//!
//! The row changes on connect, pull, push, export and disconnect. A write
//! from the browser carries the revision it read, and the row moves to the
//! next revision only if that revision is still the stored one. A source
//! change writes the link in the same transaction as the change, through
//! `put_forge_link` on the transaction's connection.

use serde::Serializer;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{Error, PostgresCatalog, Result};

const LINK_COLUMNS: &str = "document_id, provider, repository_id, repository, branch, folder, \
     connected_by, synced_commit, synced_map, export, revision, last_pull_at, last_push_at";

#[derive(Clone, Debug, serde::Serialize, sqlx::FromRow)]
pub struct ForgeLink {
    pub document_id: Uuid,
    pub provider: String,
    pub repository_id: i64,
    pub repository: String,
    pub branch: String,
    pub folder: String,
    pub connected_by: Option<Uuid>,
    pub synced_commit: Option<String>,
    pub synced_map: serde_json::Value,
    pub export: Option<serde_json::Value>,
    pub revision: i64,
    #[serde(serialize_with = "serialize_optional_time")]
    pub last_pull_at: Option<OffsetDateTime>,
    #[serde(serialize_with = "serialize_optional_time")]
    pub last_push_at: Option<OffsetDateTime>,
}

#[derive(Clone, Debug)]
pub struct ForgeLinkWrite {
    pub provider: String,
    pub repository_id: i64,
    pub repository: String,
    pub branch: String,
    pub folder: String,
    /// Set on create only; an update keeps the stored value.
    pub connected_by: Option<Uuid>,
    pub synced_commit: Option<String>,
    pub synced_map: serde_json::Value,
    pub export: Option<serde_json::Value>,
    /// Sets last_pull_at to now().
    pub pulled: bool,
    /// Sets last_push_at to now().
    pub pushed: bool,
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
    pub async fn forge_link(&self, document_id: Uuid) -> Result<Option<ForgeLink>> {
        let sql = format!("SELECT {LINK_COLUMNS} FROM forge_links WHERE document_id=$1");
        sqlx::query_as::<_, ForgeLink>(&sql)
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
    pub async fn put_forge_link(
        connection: &mut sqlx::PgConnection,
        document_id: Uuid,
        expected_revision: Option<i64>,
        write: &ForgeLinkWrite,
    ) -> Result<Option<ForgeLink>> {
        let sql = match expected_revision {
            None => format!(
                "INSERT INTO forge_links (document_id, provider, repository_id, repository, \
                 branch, folder, connected_by, synced_commit, synced_map, export, \
                 last_pull_at, last_push_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, \
                 CASE WHEN $11 THEN now() END, CASE WHEN $12 THEN now() END) \
                 ON CONFLICT (document_id) DO NOTHING \
                 RETURNING {LINK_COLUMNS}"
            ),
            Some(_) => format!(
                "UPDATE forge_links SET \
                 provider=$2, repository_id=$3, repository=$4, branch=$5, folder=$6, \
                 synced_commit=$8, synced_map=$9, export=$10, \
                 last_pull_at=CASE WHEN $11 THEN now() ELSE last_pull_at END, \
                 last_push_at=CASE WHEN $12 THEN now() ELSE last_push_at END, \
                 revision=revision + 1, updated_at=now() \
                 WHERE document_id=$1 AND revision=$13 \
                 RETURNING {LINK_COLUMNS}"
            ),
        };
        let query = sqlx::query_as::<_, ForgeLink>(&sql)
            .bind(document_id)
            .bind(write.provider.clone())
            .bind(write.repository_id)
            .bind(write.repository.clone())
            .bind(write.branch.clone())
            .bind(write.folder.clone())
            .bind(write.connected_by)
            .bind(write.synced_commit.clone())
            .bind(write.synced_map.clone())
            .bind(write.export.clone())
            .bind(write.pulled)
            .bind(write.pushed);
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

    /// `put_forge_link` in a transaction of its own.
    pub async fn put_forge_link_now(
        &self,
        document_id: Uuid,
        expected_revision: Option<i64>,
        write: &ForgeLinkWrite,
    ) -> Result<Option<ForgeLink>> {
        let mut tx = self.begin_metered().await?;
        let link = Self::put_forge_link(&mut tx, document_id, expected_revision, write).await?;
        tx.commit().await?;
        Ok(link)
    }

    /// Forgets the link. Answers whether there was one to forget.
    pub async fn delete_forge_link(&self, document_id: Uuid) -> Result<bool> {
        let removed = sqlx::query("DELETE FROM forge_links WHERE document_id=$1")
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

    use super::{ForgeLinkWrite, PostgresCatalog};
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
                handle: format!("forge-test-{}", Uuid::now_v7()),
                display_name: "Forge test".into(),
                email: None,
            })
            .await
            .unwrap()
            .id
    }

    async fn document(catalog: &PostgresCatalog, owner_id: Uuid) -> Uuid {
        catalog
            .create_document(NewDocument {
                slug: format!("forge-test-{}", Uuid::now_v7()),
                owner_id,
                owner_session_generation: None,
                ownership_mode: "owned".into(),
                title: "Forge test".into(),
                source_format: "markdown".into(),
                main_path: "main.md".into(),
            })
            .await
            .unwrap()
            .id
    }

    fn write(connected_by: Option<Uuid>) -> ForgeLinkWrite {
        ForgeLinkWrite {
            provider: "github".into(),
            repository_id: 123,
            repository: "owner/name".into(),
            branch: "main".into(),
            folder: "paper".into(),
            connected_by,
            synced_commit: Some("a".repeat(40)),
            synced_map: serde_json::json!({
                "paper.qmd": { "blob": "b".repeat(40), "hash": "c".repeat(64) }
            }),
            export: None,
            pulled: false,
            pushed: false,
        }
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn link_is_created_and_read_back() {
        let catalog = test_catalog().await;
        let owner = account(&catalog).await;
        let doc = document(&catalog, owner).await;

        assert!(catalog.forge_link(doc).await.unwrap().is_none());
        let created = catalog
            .put_forge_link_now(doc, None, &write(Some(owner)))
            .await
            .unwrap()
            .expect("the first create writes the row");
        assert_eq!(created.revision, 1);
        assert_eq!(created.repository, "owner/name");
        assert_eq!(created.folder, "paper");
        assert_eq!(created.connected_by, Some(owner));
        assert!(created.last_pull_at.is_none());
        assert!(created.last_push_at.is_none());

        let read = catalog
            .forge_link(doc)
            .await
            .unwrap()
            .expect("the link reads back");
        assert_eq!(read.synced_commit, Some("a".repeat(40)));
        assert_eq!(read.synced_map, write(None).synced_map);
        assert_eq!(read.revision, 1);
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn second_create_answers_none_and_keeps_the_row() {
        let catalog = test_catalog().await;
        let owner = account(&catalog).await;
        let doc = document(&catalog, owner).await;

        catalog
            .put_forge_link_now(doc, None, &write(Some(owner)))
            .await
            .unwrap()
            .expect("the first create writes the row");
        let again = catalog
            .put_forge_link_now(doc, None, &write(None))
            .await
            .unwrap();
        assert!(
            again.is_none(),
            "a second create finds the row and writes nothing"
        );

        let read = catalog.forge_link(doc).await.unwrap().unwrap();
        assert_eq!(read.revision, 1);
        assert_eq!(read.repository, "owner/name");
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn update_with_the_stored_revision_moves_it_on_and_keeps_connected_by() {
        let catalog = test_catalog().await;
        let owner = account(&catalog).await;
        let other = account(&catalog).await;
        let doc = document(&catalog, owner).await;

        catalog
            .put_forge_link_now(doc, None, &write(Some(owner)))
            .await
            .unwrap()
            .expect("create");

        let mut next = write(Some(other));
        next.synced_commit = Some("d".repeat(40));
        let updated = catalog
            .put_forge_link_now(doc, Some(1), &next)
            .await
            .unwrap()
            .expect("a matching revision updates the row");
        assert_eq!(updated.revision, 2);
        assert_eq!(updated.synced_commit, Some("d".repeat(40)));
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
            .put_forge_link_now(doc, None, &write(Some(owner)))
            .await
            .unwrap()
            .expect("create");
        catalog
            .put_forge_link_now(doc, Some(1), &write(None))
            .await
            .unwrap()
            .expect("the first update at revision 1 lands");

        let mut stale = write(None);
        stale.synced_commit = Some("e".repeat(40));
        let refused = catalog
            .put_forge_link_now(doc, Some(1), &stale)
            .await
            .unwrap();
        assert!(refused.is_none(), "revision 1 is no longer stored");

        let read = catalog.forge_link(doc).await.unwrap().unwrap();
        assert_eq!(read.revision, 2);
        assert_eq!(read.synced_commit, Some("a".repeat(40)));
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn pull_and_push_stamp_only_their_own_time() {
        let catalog = test_catalog().await;
        let owner = account(&catalog).await;
        let doc = document(&catalog, owner).await;

        catalog
            .put_forge_link_now(doc, None, &write(Some(owner)))
            .await
            .unwrap()
            .expect("create");

        let mut pulled = write(None);
        pulled.pulled = true;
        let after_pull = catalog
            .put_forge_link_now(doc, Some(1), &pulled)
            .await
            .unwrap()
            .expect("pull");
        assert!(
            after_pull.last_pull_at.is_some(),
            "a pull sets last_pull_at"
        );
        assert!(after_pull.last_push_at.is_none());

        let pull_time = after_pull.last_pull_at;
        let after_push = catalog
            .put_forge_link_now(doc, Some(2), &write(None))
            .await
            .unwrap()
            .expect("push without a pull");
        assert_eq!(
            after_push.last_pull_at, pull_time,
            "a write that is not a pull leaves last_pull_at alone"
        );
        assert!(after_push.last_push_at.is_none());
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn delete_answers_whether_a_link_was_removed() {
        let catalog = test_catalog().await;
        let owner = account(&catalog).await;
        let doc = document(&catalog, owner).await;

        catalog
            .put_forge_link_now(doc, None, &write(Some(owner)))
            .await
            .unwrap()
            .expect("create");
        assert!(catalog.delete_forge_link(doc).await.unwrap());
        assert!(!catalog.delete_forge_link(doc).await.unwrap());
        assert!(catalog.forge_link(doc).await.unwrap().is_none());
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn deleting_the_document_removes_its_link() {
        let catalog = test_catalog().await;
        let owner = account(&catalog).await;
        let doc = document(&catalog, owner).await;

        catalog
            .put_forge_link_now(doc, None, &write(Some(owner)))
            .await
            .unwrap()
            .expect("create");
        sqlx::query("DELETE FROM documents WHERE id=$1")
            .bind(doc)
            .execute(catalog.pool())
            .await
            .expect("delete the document");
        assert!(catalog.forge_link(doc).await.unwrap().is_none());
    }
}
