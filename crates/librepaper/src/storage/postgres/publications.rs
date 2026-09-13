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
        sqlx::query_as::<_, PublicationRecord>(
            "SELECT * FROM publications WHERE document_id=$1 AND request_key=$2",
        )
        .bind(document_id)
        .bind(request_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn finalize_publication(&self, input: NewPublication) -> Result<PublicationRecord> {
        validate(&input)?;
        let mut tx = self.pool.begin().await?;
        let existing = sqlx::query_as::<_, PublicationRecord>(
            "SELECT * FROM publications WHERE document_id=$1 AND request_key=$2",
        )
        .bind(input.document_id)
        .bind(&input.request_key)
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
        let current: Option<Uuid> = sqlx::query_scalar(
            "SELECT current_publication_id FROM documents WHERE id=$1 AND status='active' FOR UPDATE",
        ).bind(input.document_id).fetch_optional(&mut *tx).await?.ok_or(Error::NotFound)?;
        if current != input.expected_current_id {
            return Err(Error::Conflict("current publication changed".into()));
        }
        if let Some(source_version_id) = input.source_version_id {
            let belongs: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM document_versions WHERE id=$1 AND document_id=$2)",
            )
            .bind(source_version_id)
            .bind(input.document_id)
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
        let owner_usage: i64 = sqlx::query_scalar(
            "SELECT COALESCE(sum(bytes),0)::bigint FROM (
               SELECT a.byte_length bytes FROM document_assets a JOIN documents d ON d.id=a.document_id
                 WHERE d.owner_id=(SELECT owner_id FROM documents WHERE id=$1)
               UNION ALL SELECT v.archive_bytes FROM document_versions v JOIN documents d ON d.id=v.document_id
                 WHERE d.owner_id=(SELECT owner_id FROM documents WHERE id=$1)
               UNION ALL SELECT f.byte_length FROM publication_files f JOIN publications p ON p.id=f.publication_id
                 JOIN documents d ON d.id=p.document_id WHERE d.owner_id=(SELECT owner_id FROM documents WHERE id=$1)
             ) usage",
        )
        .bind(input.document_id)
        .fetch_one(&mut *tx)
        .await?;
        let deployment_usage: i64 = sqlx::query_scalar(
            "SELECT COALESCE(sum(bytes),0)::bigint FROM (
               SELECT byte_length bytes FROM document_assets
               UNION ALL SELECT archive_bytes FROM document_versions
               UNION ALL SELECT byte_length FROM publication_files
             ) usage",
        )
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
        let row = sqlx::query_as::<_, PublicationRecord>(
            "INSERT INTO publications(id,document_id,source_version_id,request_key,request_digest,
             manifest_key,manifest_digest,publisher_account_id,publisher_label)
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING *",
        )
        .bind(id)
        .bind(input.document_id)
        .bind(input.source_version_id)
        .bind(input.request_key)
        .bind(input.request_digest.as_slice())
        .bind(input.manifest_key)
        .bind(input.manifest_digest.as_slice())
        .bind(input.publisher_account_id)
        .bind(input.publisher_label)
        .fetch_one(&mut *tx)
        .await?;
        for file in input.files {
            sqlx::query("INSERT INTO publication_files(publication_id,path,storage_key,digest,byte_length,media_type) VALUES($1,$2,$3,$4,$5,$6)")
                .bind(id).bind(file.path).bind(file.storage_key).bind(file.digest.as_slice())
                .bind(file.byte_length).bind(file.media_type).execute(&mut *tx).await?;
        }
        sqlx::query("UPDATE documents SET current_publication_id=$2,updated_at=now() WHERE id=$1")
            .bind(input.document_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn current_publication(
        &self,
        document_id: Uuid,
    ) -> Result<Option<PublicationRecord>> {
        sqlx::query_as::<_,PublicationRecord>("SELECT p.* FROM documents d JOIN publications p ON p.id=d.current_publication_id WHERE d.id=$1")
            .bind(document_id).fetch_optional(&self.pool).await.map_err(Error::from)
    }

    pub async fn publication_files(
        &self,
        publication_id: Uuid,
    ) -> Result<Vec<PublicationFileRecord>> {
        sqlx::query_as::<_, PublicationFileRecord>(
            "SELECT * FROM publication_files WHERE publication_id=$1 ORDER BY path",
        )
        .bind(publication_id)
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn publication_cleanup_keys(&self, publication_id: Uuid) -> Result<Vec<String>> {
        let current: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM documents WHERE current_publication_id=$1)",
        )
        .bind(publication_id)
        .fetch_one(&self.pool)
        .await?;
        if current {
            return Err(Error::Conflict(
                "current publication cannot be cleaned up".into(),
            ));
        }
        sqlx::query_scalar(
            "SELECT manifest_key FROM publications WHERE id=$1
             UNION ALL SELECT storage_key FROM publication_files WHERE publication_id=$1",
        )
        .bind(publication_id)
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn finish_publication_cleanup(&self, publication_id: Uuid) -> Result<bool> {
        let removed = sqlx::query(
            "DELETE FROM publications p WHERE p.id=$1
             AND NOT EXISTS(SELECT 1 FROM documents d WHERE d.current_publication_id=p.id)",
        )
        .bind(publication_id)
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
