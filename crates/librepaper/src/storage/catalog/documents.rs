//! V2 document metadata and ownership boundaries.
//!
//! Physical bytes are admitted and settled by object operations.  This module
//! only creates and updates the authoritative document row; it never keeps a
//! second reservation or accounting ledger.

use super::*;

fn normalized_title(title: &str) -> String {
    unicode_normalization::UnicodeNormalization::nfc(title.trim())
        .collect::<String>()
        .to_lowercase()
}

impl Catalog {
    fn unique_project_title_in_tx(
        tx: &Transaction<'_>,
        slug: &str,
        title: &str,
        owner_id: Option<&str>,
        _owner_key: &str,
    ) -> CatalogResult<()> {
        let owner_id = owner_id
            .ok_or_else(|| CatalogError::Invalid("v2 documents require an owner account".into()))?;
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM documents
                 WHERE owner_id=?1 AND title_key=?2 AND slug<>?3 AND status<>'deleting')",
                params![owner_id, normalized_title(title), slug],
                |row| row.get(0),
            )
            .map_err(CatalogError::from)?;
        if exists {
            return Err(CatalogError::Conflict(
                "A project with this name already exists. Choose a different name.".into(),
            ));
        }
        Ok(())
    }

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

    pub fn document_results_metadata(
        &self,
        slug: &str,
    ) -> CatalogResult<crate::results::DocumentMetadata> {
        self.with_connection(|connection| {
            let source_format: String = connection
                .query_row(
                    "SELECT d.source_format FROM documents d JOIN accounts a ON a.id=d.owner_id
                     WHERE d.slug=?1 AND a.status='active'",
                    [slug],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            let execution_engine = crate::results::ExecutionEngine::parse(&source_format)
                .map_err(CatalogError::Invalid)?;
            let draft_format = crate::results::DraftFormat::parse(&source_format)
                .map_err(CatalogError::Invalid)?;
            Ok(crate::results::DocumentMetadata {
                execution_engine,
                draft_format,
            })
        })
    }

    pub(super) fn admit_upload_in_tx(
        _tx: &Transaction<'_>,
        _owner_id: Option<&str>,
        _owner_key: &str,
        _uploads_limit: usize,
    ) -> CatalogResult<()> {
        Err(CatalogError::Invalid(
            "upload admission is part of the v2 operation transaction".into(),
        ))
    }

    pub fn admit_document_upload(&self, _slug: &str, _uploads_limit: usize) -> CatalogResult<()> {
        Err(CatalogError::Invalid(
            "upload admission is part of the v2 operation transaction".into(),
        ))
    }

    pub fn create_document(&self, document: &NewDocument) -> CatalogResult<Document> {
        self.validate_document_input(document)?;
        let owner_id = document
            .owner_id
            .as_deref()
            .ok_or_else(|| CatalogError::Invalid("v2 documents require an owner account".into()))?;
        let created_at = document
            .created_at
            .parse::<i64>()
            .unwrap_or_else(|_| super::unix_millis());
        let status = match document.status.as_str() {
            "creating" | "active" | "deleting" => document.status.as_str(),
            _ => return Err(CatalogError::Invalid("invalid document status".into())),
        };
        if !matches!(
            document.source_format.as_str(),
            "markdown" | "html" | "typst" | "latex" | "quarto"
        ) {
            return Err(CatalogError::Invalid("invalid source format".into()));
        }
        self.immediate(|tx| {
            Self::unique_project_title_in_tx(
                tx,
                &document.slug,
                &document.title,
                Some(owner_id),
                "",
            )?;
            self.validate_owner_in_tx(tx, Some(owner_id))?;
            Self::insert_document_in_tx(tx, document, created_at, status)?;
            Self::document_in_tx(tx, &document.slug)
        })
    }

    pub fn create_document_admitted(
        &self,
        document: &NewDocument,
        owner_limit: i64,
        total_limit: i64,
        documents_limit: usize,
        _uploads_limit: usize,
    ) -> CatalogResult<Document> {
        if owner_limit < 0 || total_limit < 0 || documents_limit == 0 {
            return Err(CatalogError::Invalid("invalid document limits".into()));
        }
        // A document row cannot carry an unbacked reservation.  Callers must
        // allocate objects through the operation admission protocol first.
        if document.size != 0 || document.counted_size != 0 || document.maintenance_reserved != 0 {
            return Err(CatalogError::Invalid(
                "document bytes require v2 object admission".into(),
            ));
        }
        self.create_document(document)
    }

    pub fn replace_document_admitted(
        &self,
        document: &NewDocument,
        _owner_limit: i64,
        _total_limit: i64,
        _uploads_limit: usize,
    ) -> CatalogResult<Document> {
        self.validate_document_input(document)?;
        if document.size != 0 || document.counted_size != 0 || document.maintenance_reserved != 0 {
            return Err(CatalogError::Invalid(
                "document bytes require v2 object settlement".into(),
            ));
        }
        self.immediate(|tx| {
            let old = Self::document_in_tx(tx, &document.slug)?;
            if old.owner_id != document.owner_id {
                return Err(CatalogError::Conflict(
                    "replacement cannot change document ownership".into(),
                ));
            }
            Self::unique_project_title_in_tx(
                tx,
                &document.slug,
                &document.title,
                old.owner_id.as_deref(),
                "",
            )?;
            tx.execute(
                "UPDATE documents SET title=?1,title_key=?2,updated_at=?3,
                 source_format=?4,main_path=?5 WHERE id=?6 AND status<>'deleting'",
                params![
                    document.title,
                    normalized_title(&document.title),
                    super::unix_millis(),
                    document.source_format,
                    document.main,
                    document.storage_id
                ],
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
        let owner_id = owner_id
            .ok_or_else(|| CatalogError::Invalid("v2 documents require an owner account".into()))?;
        let active: bool = tx
            .query_row(
                "SELECT status='active' FROM accounts WHERE id=?1",
                [owner_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(CatalogError::from)?
            .unwrap_or(false);
        if !active {
            return Err(CatalogError::Conflict("owner account is not active".into()));
        }
        Ok(())
    }

    pub(super) fn insert_document_in_tx(
        tx: &Transaction<'_>,
        document: &NewDocument,
        created_at: i64,
        status: &str,
    ) -> CatalogResult<()> {
        let changed = tx
            .execute(
                "INSERT INTO documents
                 (id,slug,owner_id,ownership_mode,title,title_key,status,created_at,
                  updated_at,source_format,main_path)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8,?9,?10)",
                params![
                    document.storage_id,
                    document.slug,
                    document.owner_id.as_deref(),
                    if document.example { "example" } else { "owned" },
                    document.title,
                    normalized_title(&document.title),
                    status,
                    created_at,
                    document.source_format,
                    document.main,
                ],
            )
            .map_err(CatalogError::from)?;
        if changed != 1 {
            return Err(CatalogError::Conflict("document already exists".into()));
        }
        tx.execute(
            "UPDATE accounts SET document_count=document_count+1 WHERE id=?1",
            [document.owner_id.as_deref().unwrap_or_default()],
        )
        .map_err(CatalogError::from)?;
        tx.execute(
            "UPDATE server_state SET document_count=document_count+1,
             catalog_revision=catalog_revision+1,updated_at=max(updated_at,?1)
             WHERE id=1",
            [created_at],
        )
        .map_err(CatalogError::from)?;
        Ok(())
    }

    pub fn reserve_maintenance(
        &self,
        _job_id: &str,
        _slug: &str,
        _bytes: i64,
        _limit: i64,
        _now: i64,
    ) -> CatalogResult<Document> {
        Err(CatalogError::Invalid(
            "maintenance reservations were removed; use v2 object admission".into(),
        ))
    }

    pub fn release_maintenance(
        &self,
        _job_id: &str,
        _slug: &str,
        _bytes: i64,
        _now: i64,
    ) -> CatalogResult<Document> {
        Err(CatalogError::Invalid(
            "maintenance reservations were removed; use v2 object settlement".into(),
        ))
    }

    pub fn transfer_ownership(
        &self,
        slug: &str,
        owner_id: &str,
        owner_limit: i64,
    ) -> CatalogResult<Document> {
        if owner_id.is_empty() || owner_limit < 0 {
            return Err(CatalogError::Invalid("invalid ownership transfer".into()));
        }
        self.immediate(|tx| self.transfer_ownership_in_tx(tx, slug, owner_id, owner_limit))
    }

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

    pub fn transfer_ownership_authorized_with_generation(
        &self,
        slug: &str,
        caller_id: Option<&str>,
        _caller_owner_key: &str,
        caller_generation: Option<&str>,
        new_owner_id: &str,
        owner_limit: i64,
    ) -> CatalogResult<Document> {
        let caller_id = caller_id.ok_or(CatalogError::NotFound)?;
        let generation = caller_generation.ok_or(CatalogError::NotFound)?;
        self.immediate(|tx| {
            let owner: String = tx
                .query_row(
                    "SELECT owner_id FROM documents WHERE slug=?1 AND status='active'",
                    [slug],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if owner != caller_id {
                return Err(CatalogError::refused(
                    CatalogRefusal::ActorRights,
                    "caller is not document owner",
                ));
            }
            let valid: bool = tx
                .query_row(
                    "SELECT status='active' AND session_generation=?2 FROM accounts WHERE id=?1",
                    params![caller_id, generation],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if !valid {
                return Err(CatalogError::refused(
                    CatalogRefusal::ActorRights,
                    "caller session changed",
                ));
            }
            self.transfer_ownership_in_tx(tx, slug, new_owner_id, owner_limit)
        })
    }

    fn transfer_ownership_in_tx(
        &self,
        tx: &Transaction<'_>,
        slug: &str,
        new_owner_id: &str,
        owner_limit: i64,
    ) -> CatalogResult<Document> {
        self.validate_owner_in_tx(tx, Some(new_owner_id))?;
        let (doc_id, old_owner, stored, reserved): (String, String, i64, i64) = tx
            .query_row(
                "SELECT id,owner_id,stored_bytes,reserved_bytes
                 FROM documents WHERE slug=?1 AND status='active'",
                [slug],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(CatalogError::from)?;
        if old_owner == new_owner_id {
            return Self::document_in_tx(tx, slug);
        }
        let target_bytes: i64 = tx
            .query_row(
                "SELECT stored_bytes+reserved_bytes FROM accounts WHERE id=?1",
                [new_owner_id],
                |row| row.get(0),
            )
            .map_err(CatalogError::from)?;
        let incoming = stored
            .checked_add(reserved)
            .and_then(|value| target_bytes.checked_add(value))
            .ok_or_else(|| CatalogError::Invalid("owner accounting overflow".into()))?;
        if incoming > owner_limit {
            return Err(CatalogError::refused(
                CatalogRefusal::OwnerBytes,
                "owner storage quota exceeded",
            ));
        }
        let document = Self::document_in_tx(tx, slug)?;
        Self::unique_project_title_in_tx(tx, slug, &document.title, Some(new_owner_id), "")?;
        tx.execute(
            "UPDATE documents SET owner_id=?1,updated_at=max(updated_at,?2) WHERE id=?3",
            params![new_owner_id, super::unix_millis(), doc_id],
        )
        .map_err(CatalogError::from)?;
        tx.execute(
            "UPDATE accounts SET document_count=document_count-1 WHERE id=?1 AND document_count>=1",
            [old_owner.as_str()],
        )
        .map_err(CatalogError::from)?;
        tx.execute(
            "UPDATE accounts SET document_count=document_count+1 WHERE id=?1",
            [new_owner_id],
        )
        .map_err(CatalogError::from)?;
        Self::document_in_tx(tx, slug)
    }

    pub(super) fn validate_document_input(&self, document: &NewDocument) -> CatalogResult<()> {
        if document.slug.is_empty() || document.storage_id.is_empty() {
            return Err(CatalogError::Invalid(
                "slug and storage_id are required".into(),
            ));
        }
        if document.owner_id.is_none() || !document.owner_key.is_empty() {
            return Err(CatalogError::Invalid(
                "v2 documents require an owner account and no owner key".into(),
            ));
        }
        if document.title.is_empty() || document.title.len() > 4096 {
            return Err(CatalogError::Invalid("invalid document title".into()));
        }
        if document.size < 0 || document.counted_size < 0 || document.maintenance_reserved != 0 {
            return Err(CatalogError::Invalid("invalid document accounting".into()));
        }
        Ok(())
    }

    pub fn document(&self, slug: &str) -> CatalogResult<Option<Document>> {
        self.with_connection(|connection| {
            connection
                .query_row(Self::DOCUMENT_SELECT, [slug], Self::read_document)
                .optional()
                .map_err(CatalogError::from)
        })
    }

    pub fn slug_by_storage_id(&self, storage_id: &str) -> CatalogResult<Option<String>> {
        if storage_id.is_empty() {
            return Err(CatalogError::Invalid("storage identity is empty".into()));
        }
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT d.slug FROM documents d JOIN accounts a ON a.id=d.owner_id
                     WHERE d.id=?1 AND a.status='active'",
                    [storage_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)
        })
    }

    pub fn documents_page(
        &self,
        cursor: Option<(&str, &str)>,
        limit: u32,
    ) -> CatalogResult<Vec<Document>> {
        let limit = i64::from(limit.clamp(1, 200));
        let cursor_at = cursor.and_then(|(value, _)| value.parse::<i64>().ok());
        let cursor_slug = cursor.map(|(_, slug)| slug);
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(&format!("{} WHERE d.status='active' AND (?1 IS NULL OR d.updated_at<?1 OR (d.updated_at=?1 AND d.slug<?2)) ORDER BY d.updated_at DESC,d.slug DESC LIMIT ?3", Self::DOCUMENT_SELECT))
                .map_err(CatalogError::from)?;
            let mut rows = statement
                .query(params![cursor_at, cursor_slug, limit])
                .map_err(CatalogError::from)?;
            let mut result = Vec::new();
            while let Some(row) = rows.next().map_err(CatalogError::from)? {
                result.push(Self::read_document(row).map_err(CatalogError::from)?);
            }
            Ok(result)
        })
    }

    pub fn update_document(&self, document: &Document) -> CatalogResult<Document> {
        if document.title.is_empty() || document.title.len() > 4096 {
            return Err(CatalogError::Invalid("invalid document metadata".into()));
        }
        self.immediate(|tx| {
            let previous = Self::document_in_tx(tx, &document.slug)?;
            if previous.title != document.title {
                Self::unique_project_title_in_tx(
                    tx,
                    &document.slug,
                    &document.title,
                    previous.owner_id.as_deref(),
                    "",
                )?;
            }
            let changed = tx
                .execute(
                    "UPDATE documents SET title=?1,title_key=?2,updated_at=?3,
                     status=?4,source_format=?5,main_path=?6
                     WHERE id=?7 AND status<>'deleting'",
                    params![
                        document.title,
                        normalized_title(&document.title),
                        super::unix_millis(),
                        document.status,
                        document.source_format,
                        document.main,
                        document.storage_id,
                    ],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::NotFound);
            }
            Self::document_in_tx(tx, &document.slug)
        })
    }

    pub fn record_document_measurement(
        &self,
        _slug: &str,
        _measured_size: i64,
        _sha: Option<&str>,
        _updated_at: Option<&str>,
        _format: &str,
        _main: &str,
    ) -> CatalogResult<Document> {
        Err(CatalogError::Invalid(
            "measurements must settle v2 object allocations".into(),
        ))
    }

    pub fn totals(&self) -> CatalogResult<(i64, i64)> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT stored_bytes+reserved_bytes,document_count FROM server_state WHERE id=1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(CatalogError::from)
        })
    }

    pub(super) const DOCUMENT_SELECT: &'static str =
        "SELECT d.slug,d.id,d.title,'',CAST(d.created_at AS TEXT),COALESCE(CAST(d.published_at AS TEXT),''),CAST(d.updated_at AS TEXT),
                d.ownership_mode='example','',d.owner_id,d.status,d.stored_bytes,d.stored_bytes+d.reserved_bytes,
                d.reserved_bytes,d.next_annotation_seq,d.last_checkpoint_at,NULL,COALESCE(d.publication_id,''),d.source_format,d.main_path
         FROM documents d JOIN accounts a ON a.id=d.owner_id AND a.status='active'";

    pub(super) fn document_in_tx(tx: &Transaction<'_>, slug: &str) -> CatalogResult<Document> {
        tx.query_row(
            &format!("{} WHERE slug=?1", Self::DOCUMENT_SELECT),
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
