//! Documents: creating, replacing, transferring and reading the row that says
//! a document exists and who owns it.

use super::*;

impl Catalog {
    /// Check under the write transaction, including pending creations.
    fn unique_project_title_in_tx(
        tx: &Transaction<'_>,
        slug: &str,
        title: &str,
        owner_id: Option<&str>,
        owner_key: &str,
    ) -> CatalogResult<()> {
        let mut statement = tx
            .prepare(
                "SELECT title FROM documents WHERE slug <> ?1 AND status <> 'deleting'
             AND ((?2 IS NOT NULL AND owner_id = ?2)
                  OR (?2 IS NULL AND owner_id IS NULL AND owner_key = ?3))",
            )
            .map_err(CatalogError::from)?;
        let titles = statement
            .query_map(params![slug, owner_id, owner_key], |row| {
                row.get::<_, String>(0)
            })
            .map_err(CatalogError::from)?;
        let name = title.trim().to_lowercase();
        for other in titles {
            if other.map_err(CatalogError::from)?.trim().to_lowercase() == name {
                return Err(CatalogError::Conflict(
                    "A project with this name already exists. Choose a different name.".into(),
                ));
            }
        }
        Ok(())
    }

    /// Preflight a rename before an upload changes the project's source.
    pub fn check_project_title(&self, slug: &str, title: &str) -> CatalogResult<()> {
        self.immediate(|tx| {
            let document = Self::document_in_tx(tx, slug)?;
            if title.is_empty() || title == document.title {
                return Ok(());
            }
            Self::unique_project_title_in_tx(
                tx,
                slug,
                title,
                document.owner_id.as_deref(),
                &document.owner_key,
            )
        })
    }

    /// Read the explicit result metadata.  Migration 18 backfills this row
    /// for every legacy document, so callers do not need to guess from a
    /// missing value.  The old `source_format` column remains authoritative
    /// for compatibility routes and is deliberately not renamed.
    pub fn document_results_metadata(
        &self,
        slug: &str,
    ) -> CatalogResult<crate::results::DocumentMetadata> {
        self.with_connection(|connection| {
            let (engine, draft): (String, String) = connection
                .query_row(
                    "SELECT execution_engine, draft_format
                     FROM document_results_metadata WHERE slug = ?1",
                    [slug],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            let execution_engine =
                crate::results::ExecutionEngine::parse(&engine).map_err(CatalogError::Invalid)?;
            let draft_format =
                crate::results::DraftFormat::parse(&draft).map_err(CatalogError::Invalid)?;
            Ok(crate::results::DocumentMetadata {
                execution_engine,
                draft_format,
            })
        })
    }

    pub(super) fn admit_upload_in_tx(
        tx: &Transaction<'_>,
        owner_id: Option<&str>,
        owner_key: &str,
        uploads_limit: usize,
    ) -> CatalogResult<()> {
        let (owner_kind, owner_value) = match owner_id {
            Some(id) => ("id", id),
            None => ("key", owner_key),
        };
        let bucket: i64 = tx
            .query_row(
                "SELECT CAST(strftime('%s','now') AS INTEGER) / 3600",
                [],
                |row| row.get(0),
            )
            .map_err(CatalogError::from)?;
        // Keep the counter table bounded without making an admission scan or
        // delete an unbounded amount of historical state.  At most a small
        // batch is reclaimed per upload; the rolling query below only needs
        // the current and previous hour.
        tx.execute(
            "DELETE FROM upload_buckets
             WHERE (owner_kind,owner_value,bucket) IN (
                 SELECT owner_kind,owner_value,bucket FROM upload_buckets
                 WHERE owner_kind=?1 AND owner_value=?2 AND bucket<?3
                 ORDER BY bucket LIMIT 64
             )",
            params![owner_kind, owner_value, bucket.saturating_sub(1)],
        )
        .map_err(CatalogError::from)?;
        let recent: i64 = tx
            .query_row(
                "SELECT COALESCE(SUM(uploads), 0) FROM upload_buckets
                 WHERE owner_kind=?1 AND owner_value=?2 AND bucket>=?3",
                params![owner_kind, owner_value, bucket.saturating_sub(1)],
                |row| row.get(0),
            )
            .map_err(CatalogError::from)?;
        if recent >= uploads_limit as i64 {
            return Err(CatalogError::Conflict("owner upload rate exceeded".into()));
        }
        tx.execute(
            "INSERT INTO upload_buckets(owner_kind,owner_value,bucket,uploads)
             VALUES(?1,?2,?3,1)
             ON CONFLICT(owner_kind,owner_value,bucket)
             DO UPDATE SET uploads=uploads+1",
            params![owner_kind, owner_value, bucket],
        )
        .map_err(CatalogError::from)?;
        Ok(())
    }

    /// Admit an upload that edits an existing document.  The HTTP publish
    /// path applies replacements through the room rather than replacing the
    /// catalogue row, so it needs the same durable rolling-hour counter as
    /// the initial publication path.
    pub fn admit_document_upload(&self, slug: &str, uploads_limit: usize) -> CatalogResult<()> {
        self.immediate(|tx| {
            let (owner_id, owner_key): (Option<String>, String) = tx
                .query_row(
                    "SELECT owner_id,owner_key FROM documents
                     WHERE slug=?1 AND status='active'",
                    [slug],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            Self::admit_upload_in_tx(tx, owner_id.as_deref(), &owner_key, uploads_limit)
        })
    }

    pub fn create_document(&self, document: &NewDocument) -> CatalogResult<Document> {
        self.validate_document_input(document)?;
        self.immediate(|tx| {
            Self::unique_project_title_in_tx(
                tx,
                &document.slug,
                &document.title,
                document.owner_id.as_deref(),
                &document.owner_key,
            )?;
            if let Some(owner_id) = &document.owner_id {
                let status: Option<String> = tx
                    .query_row(
                        "SELECT status FROM accounts WHERE id = ?1",
                        [owner_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(CatalogError::from)?;
                if status.as_deref() != Some("active") {
                    return Err(CatalogError::Conflict("owner account is not active".into()));
                }
            }
            tx.execute(
                "INSERT INTO documents
                 (slug, storage_id, title, sha, created_at, published_at, updated_at,
                  example, owner_key, owner_id, status, size, counted_size,
                  maintenance_reserved, comment_seq, last_auto_checkpoint_at,
                  pending_publication, last_publication_id, source_format, main)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                         ?13, ?14, 0, ?15, NULL, '', ?16, ?17)",
                params![
                    document.slug,
                    document.storage_id,
                    document.title,
                    document.sha,
                    document.created_at,
                    document.published_at,
                    document.updated_at,
                    document.example as i64,
                    document.owner_key,
                    document.owner_id,
                    document.status,
                    document.size,
                    document.counted_size,
                    document.maintenance_reserved,
                    document.last_auto_checkpoint_at,
                    document.source_format,
                    document.main,
                ],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE totals SET bytes = bytes + ?1, documents = documents + 1 WHERE id = 1",
                [document.counted_size],
            )
            .map_err(CatalogError::from)?;
            Self::document_in_tx(tx, &document.slug)
        })
    }

    /// Creation admission and row insertion are one authoritative decision.
    /// The ordinary reservation is checked independently from maintenance
    /// borrowing, so maintenance capacity can never be consumed by uploads.
    pub fn create_document_admitted(
        &self,
        document: &NewDocument,
        owner_limit: i64,
        total_limit: i64,
        documents_limit: usize,
        uploads_limit: usize,
    ) -> CatalogResult<Document> {
        self.validate_document_input(document)?;
        if owner_limit < 0 || total_limit < 0 {
            return Err(CatalogError::Invalid("negative quota limit".into()));
        }
        self.immediate(|tx| {
            Self::unique_project_title_in_tx(
                tx,
                &document.slug,
                &document.title,
                document.owner_id.as_deref(),
                &document.owner_key,
            )?;
            self.validate_owner_in_tx(tx, document.owner_id.as_deref())?;
            let (owner_column, owner_value) = match document.owner_id.as_deref() {
                Some(id) => ("owner_id", id),
                None => ("owner_key", document.owner_key.as_str()),
            };
            let owner_documents: i64 = tx
                .query_row(
                    &format!("SELECT COUNT(*) FROM documents WHERE {owner_column}=?1"),
                    [owner_value],
                    |r| r.get(0),
                )
                .map_err(CatalogError::from)?;
            if owner_documents >= documents_limit as i64 {
                return Err(CatalogError::Conflict(
                    "owner document count quota exceeded".into(),
                ));
            }
            Self::admit_upload_in_tx(
                tx,
                document.owner_id.as_deref(),
                &document.owner_key,
                uploads_limit,
            )?;
            let owner_sql = if let Some(id) = document.owner_id.as_deref() {
                tx.query_row(
                    "SELECT COALESCE(SUM(admission_bytes),0)
                     FROM admission_documents WHERE owner_id = ?1",
                    [id],
                    |r| r.get(0),
                )
            } else {
                tx.query_row(
                    "SELECT COALESCE(SUM(admission_bytes),0)
                     FROM admission_documents WHERE owner_id IS NULL AND owner_key = ?1",
                    [&document.owner_key],
                    |r| r.get(0),
                )
            };
            let owner_bytes: i64 = owner_sql.map_err(CatalogError::from)?;
            let total_bytes: i64 = tx
                .query_row(
                    "SELECT COALESCE(SUM(admission_bytes),0) FROM admission_documents",
                    [],
                    |r| r.get(0),
                )
                .map_err(CatalogError::from)?;
            let ordinary = document.counted_size - document.maintenance_reserved;
            if owner_bytes.saturating_add(ordinary) > owner_limit {
                return Err(CatalogError::Conflict(
                    "owner storage quota exceeded".into(),
                ));
            }
            if total_bytes.saturating_add(ordinary) > total_limit {
                return Err(CatalogError::Conflict(
                    "deployment storage quota exceeded".into(),
                ));
            }
            self.insert_document_in_tx(tx, document)?;
            tx.execute(
                "UPDATE totals SET bytes = bytes + ?1, documents = documents + 1 WHERE id = 1",
                [document.counted_size],
            )
            .map_err(CatalogError::from)?;
            Self::document_in_tx(tx, &document.slug)
        })
    }

    /// Atomically admit a replacement against the authoritative document
    /// totals.  The old reservation is excluded from the owner sum and the
    /// new reservation is retained until reclamation; this prevents a
    /// replacement from either double-counting its own old bytes or briefly
    /// releasing capacity before its new objects are durable.
    pub fn replace_document_admitted(
        &self,
        document: &NewDocument,
        owner_limit: i64,
        total_limit: i64,
        uploads_limit: usize,
    ) -> CatalogResult<Document> {
        self.validate_document_input(document)?;
        if owner_limit < 0 || total_limit < 0 {
            return Err(CatalogError::Invalid("negative quota limit".into()));
        }
        self.immediate(|tx| {
            Self::unique_project_title_in_tx(
                tx, &document.slug, &document.title,
                document.owner_id.as_deref(), &document.owner_key,
            )?;
            let (old_owner_id, old_owner_key, old_counted, maintenance): (
                Option<String>,
                String,
                i64,
                i64,
            ) = tx
                .query_row(
                    "SELECT owner_id, owner_key, counted_size, maintenance_reserved
                     FROM documents WHERE slug = ?1 AND status = 'active'",
                    [&document.slug],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            if document.owner_id.as_deref() != old_owner_id.as_deref()
                || (!document.owner_key.is_empty() && document.owner_key != old_owner_key)
            {
                return Err(CatalogError::Conflict(
                    "replacement cannot change document ownership".into(),
                ));
            }
            Self::admit_upload_in_tx(tx, old_owner_id.as_deref(), &old_owner_key, uploads_limit)?;
            let new_counted = old_counted.max(document.size).max(document.counted_size);
            let live_bytes: i64 = tx.query_row(
                "SELECT admission_bytes-counted_size+maintenance_reserved FROM admission_documents WHERE slug=?1",
                [&document.slug], |row| row.get(0),
            )?;
            let owner_bytes: i64 = if let Some(owner_id) = old_owner_id.as_deref() {
                tx.query_row(
                    "SELECT COALESCE(SUM(admission_bytes),0)
                     FROM admission_documents WHERE owner_id = ?1 AND slug <> ?2",
                    params![owner_id, document.slug],
                    |row| row.get::<_, i64>(0),
                )
            } else {
                tx.query_row(
                    "SELECT COALESCE(SUM(admission_bytes),0)
                     FROM admission_documents WHERE owner_id IS NULL AND owner_key = ?1 AND slug <> ?2",
                    params![old_owner_key, document.slug],
                    |row| row.get::<_, i64>(0),
                )
            }
            .map_err(CatalogError::from)?
            .saturating_add(new_counted - maintenance).saturating_add(live_bytes);
            let total_bytes: i64 = tx
                .query_row(
                    "SELECT COALESCE(SUM(admission_bytes),0)
                     FROM admission_documents WHERE slug <> ?1",
                    [&document.slug],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(CatalogError::from)?
                .saturating_add(new_counted - maintenance).saturating_add(live_bytes);
            if owner_bytes > owner_limit {
                return Err(CatalogError::Conflict(
                    "owner storage quota exceeded".into(),
                ));
            }
            if total_bytes > total_limit {
                return Err(CatalogError::Conflict(
                    "deployment storage quota exceeded".into(),
                ));
            }
            tx.execute(
                "UPDATE documents SET title=?2, sha=?3, updated_at=?4, size=?5,
                        counted_size=?6, source_format=?7, main=?8
                 WHERE slug=?1 AND status='active'",
                params![
                    document.slug,
                    document.title,
                    document.sha,
                    document.updated_at,
                    document.size,
                    new_counted,
                    document.source_format,
                    document.main,
                ],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE totals SET bytes = bytes + ?1 - ?2 WHERE id = 1",
                params![new_counted, old_counted],
            )
            .map_err(CatalogError::from)?;
            Self::document_in_tx(tx, &document.slug)
        })
    }

    pub(super) fn validate_owner_in_tx(
        &self,
        tx: &Transaction<'_>,
        owner_id: Option<&str>,
    ) -> CatalogResult<()> {
        if let Some(id) = owner_id {
            let status: Option<String> = tx
                .query_row("SELECT status FROM accounts WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .optional()
                .map_err(CatalogError::from)?;
            if status.as_deref() != Some("active") {
                return Err(CatalogError::Conflict("owner account is not active".into()));
            }
        }
        Ok(())
    }

    pub(super) fn insert_document_in_tx(
        &self,
        tx: &Transaction<'_>,
        document: &NewDocument,
    ) -> CatalogResult<()> {
        tx.execute(
            "INSERT INTO documents
             (slug, storage_id, title, sha, created_at, published_at, updated_at,
              example, owner_key, owner_id, status, size, counted_size,
             maintenance_reserved, comment_seq, last_auto_checkpoint_at,
             pending_publication, last_publication_id, source_format, main)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                     ?13, ?14, 0, ?15, NULL, '', ?16, ?17)",
            params![
                document.slug,
                document.storage_id,
                document.title,
                document.sha,
                document.created_at,
                document.published_at,
                document.updated_at,
                document.example as i64,
                document.owner_key,
                document.owner_id,
                document.status,
                document.size,
                document.counted_size,
                document.maintenance_reserved,
                document.last_auto_checkpoint_at,
                document.source_format,
                document.main,
            ],
        )
        .map_err(CatalogError::from)?;
        Ok(())
    }

    /// Borrow deployment maintenance capacity for a bounded rewrite.
    pub fn reserve_maintenance(
        &self,
        job_id: &str,
        slug: &str,
        bytes: i64,
        limit: i64,
        now: i64,
    ) -> CatalogResult<Document> {
        if job_id.is_empty() || bytes <= 0 || limit < 0 {
            return Err(CatalogError::Invalid(
                "invalid maintenance reservation".into(),
            ));
        }
        self.immediate(|tx| {
            let used: i64 = tx
                .query_row(
                    "SELECT COALESCE(SUM(maintenance_reserved), 0) FROM documents",
                    [],
                    |r| r.get(0),
                )
                .map_err(CatalogError::from)?;
            let already: Option<(String, i64)> = tx
                .query_row(
                    "SELECT status, reserved_bytes FROM maintenance_jobs WHERE id = ?1",
                    [job_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?;
            if already.is_some() {
                return Self::document_in_tx(tx, slug);
            }
            if used.saturating_add(bytes) > limit {
                return Err(CatalogError::Conflict(
                    "maintenance reserve exhausted".into(),
                ));
            }
            let changed = tx
                .execute(
                    "UPDATE documents SET counted_size = counted_size + ?2,
                     maintenance_reserved = maintenance_reserved + ?2
                     WHERE slug = ?1 AND status IN ('creating', 'active')",
                    params![slug, bytes],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::NotFound);
            }
            tx.execute("UPDATE totals SET bytes = bytes + ?1 WHERE id = 1", [bytes])
                .map_err(CatalogError::from)?;
            tx.execute(
                "INSERT INTO maintenance_jobs(id,status,reserved_bytes,created_at,updated_at)
                 VALUES (?1,'active',?2,?3,?3)",
                params![job_id, bytes, now],
            )
            .map_err(CatalogError::from)?;
            Self::document_in_tx(tx, slug)
        })
    }

    pub fn release_maintenance(
        &self,
        job_id: &str,
        slug: &str,
        bytes: i64,
        now: i64,
    ) -> CatalogResult<Document> {
        if bytes < 0 {
            return Err(CatalogError::Invalid("negative maintenance release".into()));
        }
        self.immediate(|tx| {
            let (status, reserved): (String, i64) = tx
                .query_row(
                    "SELECT status,reserved_bytes FROM maintenance_jobs WHERE id = ?1",
                    [job_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            if status != "active" {
                return Self::document_in_tx(tx, slug);
            }
            let release = bytes.min(reserved);
            let changed = tx
                .execute(
                    "UPDATE documents SET counted_size = counted_size - ?2,
                     maintenance_reserved = maintenance_reserved - ?2
                     WHERE slug = ?1 AND maintenance_reserved >= ?2",
                    params![slug, release],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::Conflict(
                    "maintenance accounting mismatch".into(),
                ));
            }
            tx.execute(
                "UPDATE totals SET bytes = bytes - ?1 WHERE id = 1",
                [release],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE maintenance_jobs SET reserved_bytes = reserved_bytes - ?2,
                 status = CASE WHEN reserved_bytes - ?2 = 0 THEN 'released' ELSE status END,
                 updated_at = ?3 WHERE id = ?1",
                params![job_id, release, now],
            )
            .map_err(CatalogError::from)?;
            Self::document_in_tx(tx, slug)
        })
    }

    /// Transfer ownership without changing deployment totals.  Quota is
    /// checked against all lifecycle rows and the target must be active.
    pub fn transfer_ownership(
        &self,
        slug: &str,
        owner_id: &str,
        owner_limit: i64,
    ) -> CatalogResult<Document> {
        if owner_id.is_empty() || owner_limit < 0 {
            return Err(CatalogError::Invalid("invalid ownership transfer".into()));
        }
        self.immediate(|tx| {
            self.validate_owner_in_tx(tx, Some(owner_id))?;
            let (old_owner, owner_key, counted, maintenance): (Option<String>, String, i64, i64) = tx
                .query_row(
                    "SELECT owner_id,owner_key,admission_bytes,0 FROM admission_documents WHERE slug=?1",
                    [slug], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                ).optional().map_err(CatalogError::from)?.ok_or(CatalogError::NotFound)?;
            if old_owner.as_deref() == Some(owner_id) && owner_key.is_empty() {
                return Self::document_in_tx(tx, slug);
            }
            let target: i64 = tx.query_row(
                "SELECT COALESCE(SUM(admission_bytes),0) FROM admission_documents WHERE owner_id=?1 AND slug<>?2",
                params![owner_id, slug], |r| r.get(0)).map_err(CatalogError::from)?;
            if target.saturating_add(counted - maintenance) > owner_limit {
                return Err(CatalogError::Conflict("owner storage quota exceeded".into()));
            }
            let document = Self::document_in_tx(tx, slug)?;
            Self::unique_project_title_in_tx(tx, slug, &document.title, Some(owner_id), "")?;
            tx.execute("UPDATE documents SET owner_id=?2,owner_key='' WHERE slug=?1", params![slug, owner_id])
                .map_err(CatalogError::from)?;
            Self::document_in_tx(tx, slug)
        })
    }

    /// Authorize and transfer against the same row lock. This is the mutation
    /// entry point for HTTP handlers: an authorization result obtained before
    /// `BEGIN IMMEDIATE` is advisory only and must not be used to commit.
    pub fn transfer_ownership_authorized(
        &self,
        slug: &str,
        caller_id: Option<&str>,
        caller_owner_key: &str,
        new_owner_id: &str,
        owner_limit: i64,
    ) -> CatalogResult<Document> {
        self.transfer_ownership_authorized_with_generation(
            slug,
            caller_id,
            caller_owner_key,
            None,
            new_owner_id,
            owner_limit,
        )
    }

    /// Authorize and transfer while also checking the caller's current
    /// account generation under the ownership write lock.  The compatibility
    /// method above remains for administrative callers that already operate
    /// inside a trusted catalogue boundary.
    pub fn transfer_ownership_authorized_with_generation(
        &self,
        slug: &str,
        caller_id: Option<&str>,
        caller_owner_key: &str,
        caller_generation: Option<&str>,
        new_owner_id: &str,
        owner_limit: i64,
    ) -> CatalogResult<Document> {
        if new_owner_id.is_empty() || owner_limit < 0 {
            return Err(CatalogError::Invalid("invalid ownership transfer".into()));
        }
        self.immediate(|tx| {
            self.validate_owner_in_tx(tx, Some(new_owner_id))?;
            let (old_owner, owner_key, counted, maintenance, status):
                (Option<String>, String, i64, i64, String) = tx.query_row(
                    "SELECT owner_id,owner_key,admission_bytes,0,status FROM admission_documents WHERE slug=?1",
                    [slug], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
                ).optional().map_err(CatalogError::from)?.ok_or(CatalogError::NotFound)?;
            if status != "active" {
                return Err(CatalogError::NotFound);
            }
            let authorized = match old_owner.as_deref() {
                Some(owner) => caller_id.is_some_and(|id| {
                    !id.is_empty()
                        && id == owner
                        && caller_generation.is_none_or(|generation| {
                            !generation.is_empty()
                                && tx
                                    .query_row(
                                        "SELECT status='active' AND session_generation=?2 FROM accounts WHERE id=?1",
                                        params![id, generation],
                                        |row| row.get::<_, bool>(0),
                                    )
                                    .unwrap_or(false)
                        })
                }),
                None => !owner_key.is_empty() && !caller_owner_key.is_empty() && owner_key == caller_owner_key,
            };
            if !authorized {
                return Err(CatalogError::NotFound);
            }
            let target: i64 = tx.query_row(
                "SELECT COALESCE(SUM(admission_bytes),0) FROM admission_documents WHERE owner_id=?1 AND slug<>?2",
                params![new_owner_id, slug], |r| r.get(0),
            ).map_err(CatalogError::from)?;
            if target.saturating_add(counted - maintenance) > owner_limit {
                return Err(CatalogError::Conflict("owner storage quota exceeded".into()));
            }
            let document = Self::document_in_tx(tx, slug)?;
            Self::unique_project_title_in_tx(tx, slug, &document.title, Some(new_owner_id), "")?;
            tx.execute("UPDATE documents SET owner_id=?2,owner_key='' WHERE slug=?1", params![slug,new_owner_id]).map_err(CatalogError::from)?;
            tx.execute("DELETE FROM grants WHERE slug=?1 AND account_id=?2", params![slug,new_owner_id]).map_err(CatalogError::from)?;
            Self::document_in_tx(tx, slug)
        })
    }

    pub(super) fn validate_document_input(&self, document: &NewDocument) -> CatalogResult<()> {
        if document.slug.is_empty() || document.storage_id.is_empty() {
            return Err(CatalogError::Invalid(
                "slug and storage_id are required".into(),
            ));
        }
        if document.owner_id.is_some() && !document.owner_key.is_empty() {
            return Err(CatalogError::Invalid(
                "signed-in ownership must have an empty owner_key".into(),
            ));
        }
        if document.owner_id.is_none() && document.owner_key.is_empty() {
            return Err(CatalogError::Invalid(
                "visitor ownership needs an owner_key".into(),
            ));
        }
        if document.size < 0
            || document.counted_size < document.size
            || document.maintenance_reserved < 0
            || document.maintenance_reserved > document.counted_size
        {
            return Err(CatalogError::Invalid("invalid document accounting".into()));
        }
        Ok(())
    }

    pub fn document(&self, slug: &str) -> CatalogResult<Option<Document>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT slug, storage_id, title, sha, created_at, published_at,
                            updated_at, example, owner_key, owner_id, status, size,
                            counted_size, maintenance_reserved, comment_seq,
                            last_auto_checkpoint_at, pending_publication,
                            last_publication_id, source_format, main
                     FROM documents WHERE slug = ?1",
                    [slug],
                    Self::read_document,
                )
                .optional()
                .map_err(CatalogError::from)
        })
    }

    /// Resolve the public slug for a durable object-accounting owner. Journal
    /// objects are keyed by deployment/storage identity rather than slug, so
    /// their quota reservation needs this small authoritative lookup.
    pub fn slug_by_storage_id(&self, storage_id: &str) -> CatalogResult<Option<String>> {
        if storage_id.is_empty() {
            return Err(CatalogError::Invalid("storage identity is empty".into()));
        }
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT slug FROM documents WHERE storage_id = ?1",
                    [storage_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)
        })
    }

    /// Enumerate active catalogue rows in stable newest-first order.  This is
    /// intentionally a bounded-query primitive for the compatibility store;
    /// callers serving user listings should prefer `visible_documents`.
    pub fn documents(&self) -> CatalogResult<Vec<Document>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT slug, storage_id, title, sha, created_at, published_at,
                            updated_at, example, owner_key, owner_id, status, size,
                            counted_size, maintenance_reserved, comment_seq,
                            last_auto_checkpoint_at, pending_publication,
                            last_publication_id, source_format, main
                     FROM documents WHERE status = 'active' AND pending_publication IS NULL
                     ORDER BY updated_at DESC, slug DESC",
                )
                .map_err(CatalogError::from)?;
            let mut rows = statement.query([]).map_err(CatalogError::from)?;
            let mut documents = Vec::new();
            while let Some(row) = rows.next().map_err(CatalogError::from)? {
                documents.push(Self::read_document(row).map_err(CatalogError::from)?);
            }
            Ok(documents)
        })
    }

    /// Bounded catalogue enumeration for janitors and administrative pages.
    /// The cursor is `(updated_at, slug)` in the same descending order as
    /// `documents`, so callers never need an offset scan.
    pub fn documents_page(
        &self,
        cursor: Option<(&str, &str)>,
        limit: u32,
    ) -> CatalogResult<Vec<Document>> {
        let limit = i64::from(limit.clamp(1, 200));
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT slug, storage_id, title, sha, created_at, published_at,
                            updated_at, example, owner_key, owner_id, status, size,
                            counted_size, maintenance_reserved, comment_seq,
                            last_auto_checkpoint_at, pending_publication,
                            last_publication_id, source_format, main
                     FROM documents
                     WHERE status = 'active' AND pending_publication IS NULL AND
                           (?1 IS NULL OR updated_at < ?1 OR
                            (updated_at = ?1 AND slug < ?2))
                     ORDER BY updated_at DESC, slug DESC LIMIT ?3",
                )
                .map_err(CatalogError::from)?;
            let mut rows = statement
                .query(params![cursor.map(|v| v.0), cursor.map(|v| v.1), limit])
                .map_err(CatalogError::from)?;
            let mut result = Vec::new();
            while let Some(row) = rows.next().map_err(CatalogError::from)? {
                result.push(Self::read_document(row).map_err(CatalogError::from)?);
            }
            Ok(result)
        })
    }

    /// Update metadata after a successful object publication.  The measured
    /// size is reconciled in the same transaction, and the reservation delta
    /// is reflected exactly once in `totals`.
    pub fn update_document(&self, document: &Document) -> CatalogResult<Document> {
        if document.size < 0
            || document.counted_size < document.size
            || document.maintenance_reserved < 0
            || document.maintenance_reserved > document.counted_size
        {
            return Err(CatalogError::Invalid("invalid document accounting".into()));
        }
        self.immediate(|tx| {
            let previous = Self::document_in_tx(tx, &document.slug)?;
            if previous.title != document.title
                || previous.owner_id != document.owner_id
                || previous.owner_key != document.owner_key
            {
                Self::unique_project_title_in_tx(
                    tx,
                    &document.slug,
                    &document.title,
                    document.owner_id.as_deref(),
                    &document.owner_key,
                )?;
            }
            let old_counted: i64 = tx
                .query_row(
                    "SELECT counted_size FROM documents WHERE slug = ?1",
                    [&document.slug],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            tx.execute(
                "UPDATE documents SET storage_id = ?2, title = ?3, sha = ?4,
                    created_at = ?5, published_at = ?6, updated_at = ?7,
                    example = ?8, owner_key = ?9, owner_id = ?10, status = ?11,
                    size = ?12, counted_size = ?13, maintenance_reserved = ?14,
                    comment_seq = ?15, last_auto_checkpoint_at = ?16,
                    pending_publication = ?17, last_publication_id = ?18,
                    source_format = ?19, main = ?20 WHERE slug = ?1",
                params![
                    document.slug,
                    document.storage_id,
                    document.title,
                    document.sha,
                    document.created_at,
                    document.published_at,
                    document.updated_at,
                    document.example as i64,
                    document.owner_key,
                    document.owner_id,
                    document.status,
                    document.size,
                    document.counted_size,
                    document.maintenance_reserved,
                    document.comment_seq,
                    document.last_auto_checkpoint_at,
                    document.pending_publication,
                    document.last_publication_id,
                    document.source_format,
                    document.main,
                ],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE totals SET bytes = bytes + ?1 WHERE id = 1",
                [document.counted_size - old_counted],
            )
            .map_err(CatalogError::from)?;
            Self::document_in_tx(tx, &document.slug)
        })
    }

    /// Reconcile measured retained bytes without replacing reservation state.
    /// Ordinary object reservations live in `object_reservations`; maintenance
    /// borrowing lives in `maintenance_reserved`.  Both are preserved while
    /// the measured committed payload and metadata are advanced atomically.
    pub fn record_document_measurement(
        &self,
        slug: &str,
        measured_size: i64,
        sha: Option<&str>,
        updated_at: Option<&str>,
        format: &str,
        main: &str,
    ) -> CatalogResult<Document> {
        if measured_size < 0 {
            return Err(CatalogError::Invalid("negative measured size".into()));
        }
        self.immediate(|tx| {
            let (storage_id, old_counted, maintenance): (String, i64, i64) = tx
                .query_row(
                    "SELECT storage_id, counted_size, maintenance_reserved
                     FROM documents WHERE slug = ?1",
                    [slug],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            let ordinary_reserved: i64 = tx
                .query_row(
                    "SELECT COALESCE(SUM(MAX(new_bytes - old_bytes, 0)), 0)
                     FROM object_reservations WHERE storage_id = ?1",
                    [&storage_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            // The ledger is the authoritative lower bound.  Callers provide
            // the room's logical measurement, but immutable tree/session and
            // journal objects must never make `size` smaller than the bytes
            // actually attributable to this document.
            let ledger_size: i64 = tx
                .query_row(
                    "SELECT COALESCE(SUM(bytes),0) FROM object_accounting
                     WHERE storage_id=?1",
                    [&storage_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            let measured_size = measured_size.max(ledger_size);
            let new_counted = measured_size
                .saturating_add(ordinary_reserved)
                .saturating_add(maintenance);
            if new_counted > old_counted {
                return Err(CatalogError::Conflict(
                    "measured usage exceeds its reservation".into(),
                ));
            }
            let changed = tx
                .execute(
                    "UPDATE documents SET size = ?2, counted_size = ?3,
                        sha = CASE WHEN ?4 IS NULL THEN sha ELSE ?4 END,
                        updated_at = CASE WHEN ?5 IS NULL THEN updated_at ELSE ?5 END,
                        source_format = CASE WHEN ?6 = '' THEN source_format ELSE ?6 END,
                        main = CASE WHEN ?7 = '' THEN main ELSE ?7 END
                     WHERE slug = ?1",
                    params![
                        slug,
                        measured_size,
                        new_counted,
                        sha,
                        updated_at,
                        format,
                        main
                    ],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::NotFound);
            }
            tx.execute(
                "UPDATE totals SET bytes = bytes + ?1 WHERE id = 1",
                [new_counted - old_counted],
            )
            .map_err(CatalogError::from)?;
            Self::document_in_tx(tx, slug)
        })
    }

    /// Exact accounting totals, including creating and deleting rows.
    pub fn totals(&self) -> CatalogResult<(i64, i64)> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT bytes, documents FROM totals WHERE id = 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(CatalogError::from)
        })
    }

    pub(super) fn document_in_tx(tx: &Transaction<'_>, slug: &str) -> CatalogResult<Document> {
        tx.query_row(
            "SELECT slug, storage_id, title, sha, created_at, published_at,
                    updated_at, example, owner_key, owner_id, status, size,
                    counted_size, maintenance_reserved, comment_seq,
                    last_auto_checkpoint_at, pending_publication,
                    last_publication_id, source_format, main
             FROM documents WHERE slug = ?1",
            [slug],
            Self::read_document,
        )
        .map_err(CatalogError::from)
    }

    pub(super) fn read_document(row: &rusqlite::Row<'_>) -> rusqlite::Result<Document> {
        Ok(Document {
            slug: row.get(0)?,
            storage_id: row.get(1)?,
            title: row.get(2)?,
            sha: row.get(3)?,
            created_at: row.get(4)?,
            published_at: row.get(5)?,
            updated_at: row.get(6)?,
            example: row.get::<_, i64>(7)? != 0,
            owner_key: row.get(8)?,
            owner_id: row.get(9)?,
            status: row.get(10)?,
            size: row.get(11)?,
            counted_size: row.get(12)?,
            maintenance_reserved: row.get(13)?,
            comment_seq: row.get(14)?,
            last_auto_checkpoint_at: row.get(15)?,
            pending_publication: row.get(16)?,
            last_publication_id: row.get(17)?,
            source_format: row.get(18)?,
            main: row.get(19)?,
        })
    }
}
