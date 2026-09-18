use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{new_id, Error, PostgresCatalog, Result};

#[derive(Clone, Debug)]
pub struct NewPublicationFile {
    pub path: String,
    pub storage_key: String,
    pub digest: [u8; 32],
    pub byte_length: i64,
    pub media_type: String,
}

#[derive(Clone, Debug)]
pub struct NewPublication {
    pub document_id: Uuid,
    pub source_version_id: Option<Uuid>,
    pub request_key: String,
    pub request_digest: [u8; 32],
    pub manifest_key: String,
    pub manifest_digest: [u8; 32],
    pub publisher_account_id: Option<Uuid>,
    pub publisher_label: String,
    pub expected_current_id: Option<Uuid>,
    pub files: Vec<NewPublicationFile>,
}

#[derive(Clone, Debug, FromRow)]
pub struct PublicationRecord {
    pub id: Uuid,
    pub document_id: Uuid,
    pub source_version_id: Option<Uuid>,
    pub request_key: String,
    pub request_digest: Vec<u8>,
    pub manifest_key: String,
    pub manifest_digest: Vec<u8>,
    pub publisher_account_id: Option<Uuid>,
    pub publisher_label: String,
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Debug, FromRow)]
pub struct PublicationFileRecord {
    pub publication_id: Uuid,
    pub path: String,
    pub storage_key: String,
    pub digest: Vec<u8>,
    pub byte_length: i64,
    pub media_type: String,
}

impl PostgresCatalog {
    pub async fn publication_by_request(
        &self,
        document_id: Uuid,
        request_key: &str,
    ) -> Result<Option<PublicationRecord>> {
        sqlx::query_as!(
            PublicationRecord,
            "SELECT id,document_id,source_version_id,request_key,request_digest,manifest_key,
                    manifest_digest,publisher_account_id,publisher_label,created_at
             FROM publications WHERE document_id=$1 AND request_key=$2",
            document_id,
            request_key,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn finalize_publication(&self, input: NewPublication) -> Result<PublicationRecord> {
        validate(&input)?;
        let mut tx = self.pool.begin().await?;
        let existing = sqlx::query_as!(
            PublicationRecord,
            "SELECT id,document_id,source_version_id,request_key,request_digest,manifest_key,
                    manifest_digest,publisher_account_id,publisher_label,created_at
             FROM publications WHERE document_id=$1 AND request_key=$2",
            input.document_id,
            input.request_key,
        )
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(existing) = existing {
            if existing.request_digest.as_slice() == input.request_digest {
                tx.commit().await?;
                return Ok(existing);
            }
            return Err(Error::Conflict(
                "publication request key was reused with different content".into(),
            ));
        }
        // The owner is read under the same lock that guards the current
        // publication, so the quota below is measured against the owner this
        // document still has when the row is written.
        let document = sqlx::query!(
            "SELECT current_publication_id,owner_id FROM documents
             WHERE id=$1 AND status='active' FOR UPDATE",
            input.document_id,
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(Error::NotFound)?;
        if document.current_publication_id != input.expected_current_id {
            return Err(Error::Conflict("current publication changed".into()));
        }
        if let Some(source_version_id) = input.source_version_id {
            let belongs = sqlx::query_scalar!(
                r#"SELECT EXISTS(SELECT 1 FROM document_versions WHERE id=$1 AND document_id=$2)
                   AS "exists!""#,
                source_version_id,
                input.document_id,
            )
            .fetch_one(&mut *tx)
            .await?;
            if !belongs {
                return Err(Error::Invalid(
                    "publication source version belongs to another document".into(),
                ));
            }
        }
        let incoming_bytes = input
            .files
            .iter()
            .try_fold(0_i64, |total, file| total.checked_add(file.byte_length))
            .ok_or_else(|| Error::Invalid("publication byte count overflow".into()))?;
        let owner_usage = super::repository::owner_usage_bytes(&mut *tx, document.owner_id).await?;
        let deployment_usage =
            sqlx::query_scalar!("SELECT bytes FROM storage_usage WHERE singleton FOR UPDATE",)
                .fetch_one(&mut *tx)
                .await?;
        if owner_usage.saturating_add(incoming_bytes) > self.policy.owner_bytes {
            return Err(Error::Conflict("account storage quota exceeded".into()));
        }
        if deployment_usage.saturating_add(incoming_bytes) > self.policy.deployment_bytes {
            return Err(Error::Conflict(
                "deployment storage threshold exceeded".into(),
            ));
        }
        let id = new_id();
        let row = sqlx::query_as!(
            PublicationRecord,
            "INSERT INTO publications(id,document_id,source_version_id,request_key,request_digest,
             manifest_key,manifest_digest,publisher_account_id,publisher_label)
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)
             RETURNING id,document_id,source_version_id,request_key,request_digest,manifest_key,
                       manifest_digest,publisher_account_id,publisher_label,created_at",
            id,
            input.document_id,
            input.source_version_id,
            input.request_key,
            input.request_digest.as_slice(),
            input.manifest_key,
            input.manifest_digest.as_slice(),
            input.publisher_account_id,
            input.publisher_label,
        )
        .fetch_one(&mut *tx)
        .await?;
        if !input.files.is_empty() {
            let paths: Vec<String> = input.files.iter().map(|file| file.path.clone()).collect();
            let keys: Vec<String> = input
                .files
                .iter()
                .map(|file| file.storage_key.clone())
                .collect();
            let digests: Vec<Vec<u8>> = input
                .files
                .iter()
                .map(|file| file.digest.to_vec())
                .collect();
            let lengths: Vec<i64> = input.files.iter().map(|file| file.byte_length).collect();
            let media_types: Vec<String> = input
                .files
                .iter()
                .map(|file| file.media_type.clone())
                .collect();
            sqlx::query!(
                "INSERT INTO publication_files(publication_id,path,storage_key,digest,byte_length,media_type)
                 SELECT $1,input.path,input.storage_key,input.digest,input.byte_length,input.media_type
                 FROM unnest($2::text[],$3::text[],$4::bytea[],$5::bigint[],$6::text[])
                   AS input(path,storage_key,digest,byte_length,media_type)",
                id,
                &paths,
                &keys,
                &digests,
                &lengths,
                &media_types,
            )
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query!(
            "UPDATE documents SET current_publication_id=$2,updated_at=now() WHERE id=$1",
            input.document_id,
            id,
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn current_publication(
        &self,
        document_id: Uuid,
    ) -> Result<Option<PublicationRecord>> {
        sqlx::query_as!(
            PublicationRecord,
            "SELECT p.id,p.document_id,p.source_version_id,p.request_key,p.request_digest,
                    p.manifest_key,p.manifest_digest,p.publisher_account_id,p.publisher_label,
                    p.created_at
             FROM documents d JOIN publications p ON p.id=d.current_publication_id WHERE d.id=$1",
            document_id,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn publication_files(
        &self,
        publication_id: Uuid,
    ) -> Result<Vec<PublicationFileRecord>> {
        sqlx::query_as!(
            PublicationFileRecord,
            "SELECT publication_id,path,storage_key,digest,byte_length,media_type
             FROM publication_files WHERE publication_id=$1 ORDER BY path",
            publication_id,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn publication_cleanup_keys(&self, publication_id: Uuid) -> Result<Vec<String>> {
        let current = sqlx::query_scalar!(
            r#"SELECT EXISTS(SELECT 1 FROM documents WHERE current_publication_id=$1)
               AS "exists!""#,
            publication_id,
        )
        .fetch_one(&self.pool)
        .await?;
        if current {
            return Err(Error::Conflict(
                "current publication cannot be cleaned up".into(),
            ));
        }
        // Only the blobs this publication is the last to name.
        //
        // A published page no longer copies the figures in it: it points at
        // the ones `document_assets` already holds, which are content-
        // addressed, never deleted, and shared with the live document. So
        // cleaning up a superseded publication must delete what it owned --
        // its rendering and its metadata -- and nothing it merely referred
        // to. Deleting a referenced key here would take the author's figure
        // out of the document they are still editing.
        sqlx::query_scalar!(
            r#"SELECT manifest_key AS "key!" FROM publications WHERE id=$1
               UNION ALL
               SELECT f.storage_key FROM publication_files f
               WHERE f.publication_id=$1
                 AND NOT EXISTS(SELECT 1 FROM document_assets a
                                WHERE a.storage_key=f.storage_key)
                 AND NOT EXISTS(SELECT 1 FROM publication_files o
                                WHERE o.storage_key=f.storage_key
                                  AND o.publication_id<>f.publication_id)"#,
            publication_id,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn finish_publication_cleanup(&self, publication_id: Uuid) -> Result<bool> {
        let removed = sqlx::query!(
            "DELETE FROM publications p WHERE p.id=$1
             AND NOT EXISTS(SELECT 1 FROM documents d WHERE d.current_publication_id=p.id)",
            publication_id,
        )
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(removed == 1)
    }
}

fn validate(input: &NewPublication) -> Result<()> {
    if input.request_key.is_empty()
        || input.request_key.len() > 200
        || input.manifest_key.is_empty()
        || input.publisher_label.is_empty()
        || input.files.is_empty()
        || input.files.len() > 4096
    {
        return Err(Error::Invalid("invalid publication".into()));
    }
    let mut paths = std::collections::HashSet::new();
    for file in &input.files {
        if file.path.is_empty()
            || file.storage_key.is_empty()
            || file.media_type.is_empty()
            || file.byte_length < 0
            || !paths.insert(&file.path)
        {
            return Err(Error::Invalid("invalid publication file".into()));
        }
    }
    Ok(())
}

impl PostgresCatalog {
    /// One publication by its own id, for the callers that have the id a
    /// reader was looking at rather than the request that made it.
    pub async fn publication(&self, id: Uuid) -> Result<Option<PublicationRecord>> {
        sqlx::query_as!(
            PublicationRecord,
            "SELECT id,document_id,source_version_id,request_key,request_digest,manifest_key,
                    manifest_digest,publisher_account_id,publisher_label,created_at
             FROM publications WHERE id=$1",
            id,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::from)
    }
}
