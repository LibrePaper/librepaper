//! Who may see a document besides its owner: grants, links and the keyring
//! that seals a link's key at rest, pinned guests, and the visibility query
//! the landing page runs.

use super::*;

pub(super) type ActiveLinkRotation = (String, String, String, Option<String>, Option<String>);

/// Durable progress returned by one link-key rotation worker invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinkKeyRotationProgress {
    pub id: String,
    pub status: String,
    pub processed: u32,
    pub cursor_slug: Option<String>,
    pub cursor_role: Option<String>,
}

pub(super) fn link_key_id(key: &[u8; 32]) -> String {
    hex::encode(sha2::Sha256::digest(key))[..16].to_string()
}

pub(super) fn promote_link_key(keys: &mut Vec<(String, [u8; 32])>, id: String, key: [u8; 32]) {
    keys.retain(|(known, _)| known != &id);
    keys.insert(0, (id, key));
}

pub(super) fn open_link_envelope(
    key: &[u8; 32],
    storage_id: &str,
    role: &str,
    digest: &str,
    envelope: &[u8],
) -> CatalogResult<String> {
    if envelope.len() < 30 || (&envelope[..6] != b"KLINK1" && &envelope[..6] != b"KLINK2") {
        return Err(CatalogError::Invalid("invalid sealed link envelope".into()));
    }
    let (nonce_at, body_at) = if &envelope[..6] == b"KLINK2" {
        if envelope.len() < 46 {
            return Err(CatalogError::Invalid("invalid sealed link envelope".into()));
        }
        let key_id = link_key_id(key);
        if envelope[6..22] != key_id.as_bytes()[..16] {
            return Err(CatalogError::Invalid("sealed link key id mismatch".into()));
        }
        (22, 46)
    } else {
        (6, 30)
    };
    let aad = format!("librepaper-link-v1\0{storage_id}\0{role}\0{digest}");
    let plaintext = XChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| CatalogError::Invalid("invalid link sealing key".into()))?
        .decrypt(
            XNonce::from_slice(&envelope[nonce_at..body_at]),
            Payload {
                msg: &envelope[body_at..],
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| CatalogError::Invalid("sealed link authentication failed".into()))?;
    let plaintext = String::from_utf8(plaintext)
        .map_err(|_| CatalogError::Invalid("sealed link is not UTF-8".into()))?;
    if hex::encode(sha2::Sha256::digest(plaintext.as_bytes())) != digest {
        return Err(CatalogError::Invalid("sealed link digest mismatch".into()));
    }
    Ok(plaintext)
}

pub(super) fn seal_link_envelope(
    key: &[u8; 32],
    key_id: &str,
    storage_id: &str,
    role: &str,
    digest: &str,
    plaintext: &str,
) -> CatalogResult<Vec<u8>> {
    let nonce_bytes = crate::auth::random_bytes(24);
    let aad = format!("librepaper-link-v1\0{storage_id}\0{role}\0{digest}");
    let ciphertext = XChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| CatalogError::Invalid("invalid link sealing key".into()))?
        .encrypt(
            XNonce::from_slice(&nonce_bytes),
            Payload {
                msg: plaintext.as_bytes(),
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| CatalogError::Invalid("could not seal link key".into()))?;
    let mut envelope = b"KLINK2".to_vec();
    envelope.extend_from_slice(key_id.as_bytes());
    envelope.extend_from_slice(&nonce_bytes);
    envelope.extend_from_slice(&ciphertext);
    Ok(envelope)
}

pub(super) fn envelope_key_id(envelope: &[u8]) -> String {
    if envelope.starts_with(b"KLINK2") && envelope.len() >= 22 {
        std::str::from_utf8(&envelope[6..22])
            .map(str::to_owned)
            .unwrap_or_else(|_| "legacy".into())
    } else {
        "legacy".into()
    }
}

impl Catalog {
    /// Check a mutation actor against the live document and access rows.  This
    /// is deliberately evaluated inside the caller's write transaction so a
    /// link expiry, policy change, grant revocation, or account generation
    /// change cannot be bypassed by a stale route-level Viewer.
    pub(super) fn mutation_authorized_in_tx(
        tx: &rusqlite::Transaction<'_>,
        slug: &str,
        actor: MutationAuthority<'_>,
        role: &str,
    ) -> CatalogResult<bool> {
        if !Self::agent_execution_epoch_active_tx(tx, slug, actor.execution_epoch)? {
            return Ok(false);
        }
        let link_ok = !actor.link_hash.is_empty()
            && actor.policy_editor
            && tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM links l
                       JOIN documents d ON d.slug=l.slug
                         WHERE l.slug=?1 AND l.hash=?2 AND l.role='editor'
                         AND d.status='active' AND d.pending_publication IS NULL
                         AND (l.until='' OR unixepoch(l.until)>unixepoch('now')))",
                    params![slug, actor.link_hash],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(CatalogError::from)?;
        if actor.automation {
            // Automation is explicitly link bounded.  An owner's cached
            // cookie supplies attribution only and must not authorize a
            // mutation when the link is absent, expired, or read-only.
            if !actor.account_id.is_empty() {
                let account_live = tx
                    .query_row(
                        "SELECT status='active' AND session_generation=?2
                           FROM accounts WHERE id=?1",
                        params![actor.account_id, actor.generation],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(CatalogError::from)?;
                if !account_live {
                    return Ok(false);
                }
            }
            return Ok(role == "editor" && link_ok);
        }
        if actor.account_id.is_empty() {
            if actor.unowned_publisher && actor.policy_editor {
                return tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM documents
                           WHERE slug=?1 AND status='active' AND pending_publication IS NULL
                             AND owner_id IS NULL AND owner_key='example:' || slug)",
                        [slug],
                        |row| row.get::<_, bool>(0),
                    )
                    .map(|open| open || link_ok)
                    .map_err(CatalogError::from);
            }
            return tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM documents
                       WHERE slug=?1 AND status='active' AND pending_publication IS NULL
                         AND owner_id IS NULL AND owner_key<>'' AND owner_key=?2)",
                    params![slug, actor.owner_key],
                    |row| row.get::<_, bool>(0),
                )
                .map(|owner| owner || link_ok)
                .map_err(CatalogError::from);
        }
        tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM documents d
               JOIN accounts a ON a.id=?2
              WHERE d.slug=?1 AND d.status='active' AND d.pending_publication IS NULL
                AND a.status='active' AND a.session_generation=?3
                AND (d.owner_id=?2 OR (?4=1 AND EXISTS(
                    SELECT 1 FROM grants g WHERE g.slug=d.slug
                      AND g.account_id=?2 AND g.role='editor')) OR ?5=1))",
            params![
                slug,
                actor.account_id,
                actor.generation,
                actor.policy_editor,
                link_ok
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(CatalogError::from)
    }

    pub fn set_link_sealing_key(&self, key: &[u8]) -> CatalogResult<()> {
        let key: [u8; 32] = key
            .try_into()
            .map_err(|_| CatalogError::Invalid("link sealing key must be 32 bytes".into()))?;
        let mut keys = self
            .link_sealing_keys
            .write()
            .map_err(|_| CatalogError::Busy)?;
        let key_id = hex::encode(sha2::Sha256::digest(key));
        let key_id = key_id[..16].to_string();
        if let Some((_, existing)) = keys.first() {
            return if existing == &key {
                Ok(())
            } else {
                Err(CatalogError::Conflict("link sealing key changed".into()))
            };
        }
        keys.push((key_id.clone(), key));
        self.with_connection(|connection| {
            connection.execute("INSERT INTO link_keyring(key_id,status,created_at) VALUES(?1,'primary',unixepoch()) ON CONFLICT(key_id) DO UPDATE SET status='primary'",[key_id]).map_err(CatalogError::from)?;
            Ok(())
        })?;
        Ok(())
    }

    pub fn add_link_decryption_key(&self, key: &[u8]) -> CatalogResult<String> {
        let key: [u8; 32] = key
            .try_into()
            .map_err(|_| CatalogError::Invalid("link sealing key must be 32 bytes".into()))?;
        let digest = hex::encode(sha2::Sha256::digest(key));
        let id = digest[..16].to_string();
        let mut keys = self
            .link_sealing_keys
            .write()
            .map_err(|_| CatalogError::Busy)?;
        if !keys.iter().any(|(known, _)| known == &id) {
            keys.push((id.clone(), key));
        }
        Ok(id)
    }

    /// The catalogue's durable primary key id.  Local administrative tools
    /// use this rather than trusting the order of a possibly interrupted
    /// on-disk keyring: a destination may have been written to the ring just
    /// before the SQLite rotation row was created.
    pub fn link_keyring_primary_id(&self) -> CatalogResult<Option<String>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT key_id FROM link_keyring WHERE status='primary' ORDER BY created_at DESC, key_id DESC LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)
        })
    }

    pub fn seal_link_key(
        &self,
        storage_id: &str,
        role: &str,
        digest: &str,
        plaintext: &str,
    ) -> CatalogResult<Vec<u8>> {
        let keys = self
            .link_sealing_keys
            .read()
            .map_err(|_| CatalogError::Busy)?;
        let (key_id, key) = keys
            .first()
            .ok_or_else(|| CatalogError::Invalid("link sealing key is not configured".into()))?;
        let nonce_bytes = crate::auth::random_bytes(24);
        let nonce = XNonce::from_slice(&nonce_bytes);
        let aad = format!("librepaper-link-v1\0{storage_id}\0{role}\0{digest}");
        let cipher = XChaCha20Poly1305::new_from_slice(key)
            .map_err(|_| CatalogError::Invalid("invalid link sealing key".into()))?;
        let ciphertext = cipher
            .encrypt(
                nonce,
                Payload {
                    msg: plaintext.as_bytes(),
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| CatalogError::Invalid("could not seal link key".into()))?;
        let mut envelope = b"KLINK2".to_vec();
        envelope.extend_from_slice(key_id.as_bytes());
        envelope.extend_from_slice(&nonce_bytes);
        envelope.extend_from_slice(&ciphertext);
        Ok(envelope)
    }

    pub fn open_link_key(
        &self,
        storage_id: &str,
        role: &str,
        digest: &str,
        envelope: &[u8],
    ) -> CatalogResult<String> {
        let keys = self
            .link_sealing_keys
            .read()
            .map_err(|_| CatalogError::Busy)?;
        if keys.is_empty() {
            return Err(CatalogError::Invalid(
                "link sealing key is not configured".into(),
            ));
        }
        if envelope.len() < 30
            || (&envelope[..6] != b"KLINK1" && &envelope[..6] != b"KLINK2")
            || (&envelope[..6] == b"KLINK2" && envelope.len() < 46)
        {
            return Err(CatalogError::Invalid("invalid sealed link envelope".into()));
        }
        let aad = format!("librepaper-link-v1\0{storage_id}\0{role}\0{digest}");
        let (nonce_at, body_at) = if &envelope[..6] == b"KLINK2" {
            (22, 46)
        } else {
            (6, 30)
        };
        let wanted = (&envelope[..6] == b"KLINK2")
            .then(|| std::str::from_utf8(&envelope[6..22]).ok())
            .flatten();
        let plaintext = keys
            .iter()
            .filter(|(id, _)| wanted.is_none_or(|wanted| wanted == id))
            .find_map(|(_, key)| {
                XChaCha20Poly1305::new_from_slice(key)
                    .ok()?
                    .decrypt(
                        XNonce::from_slice(&envelope[nonce_at..body_at]),
                        Payload {
                            msg: &envelope[body_at..],
                            aad: aad.as_bytes(),
                        },
                    )
                    .ok()
            })
            .ok_or_else(|| CatalogError::Invalid("sealed link authentication failed".into()))?;
        let plaintext = String::from_utf8(plaintext)
            .map_err(|_| CatalogError::Invalid("sealed link is not UTF-8".into()))?;
        let actual = hex::encode(sha2::Sha256::digest(plaintext.as_bytes()));
        if actual != digest {
            return Err(CatalogError::Invalid("sealed link digest mismatch".into()));
        }
        Ok(plaintext)
    }

    /// Complete a link-key rotation, retaining the old key for decryption.
    ///
    /// The convenience API deliberately performs the work through the
    /// bounded worker below.  Each invocation commits at most 200 links, so
    /// a process death leaves a durable cursor rather than one giant SQLite
    /// transaction to replay.
    pub fn rotate_link_sealing_key(&self, new_key: &[u8]) -> CatalogResult<u32> {
        let mut processed: u32 = 0;
        loop {
            let progress = self.rotate_link_sealing_key_batch(new_key)?;
            processed = processed.saturating_add(progress.processed);
            if progress.status == "committed" {
                return Ok(processed);
            }
        }
    }

    /// Run one durable, lexicographically ordered rotation batch.  The
    /// `link_key_rotations` row is the recovery record: callers may stop after
    /// any successful batch and resume later with the same destination key.
    pub fn rotate_link_sealing_key_batch(
        &self,
        new_key: &[u8],
    ) -> CatalogResult<LinkKeyRotationProgress> {
        const BATCH_SIZE: i64 = 200;
        let new_key: [u8; 32] = new_key
            .try_into()
            .map_err(|_| CatalogError::Invalid("link sealing key must be 32 bytes".into()))?;
        let new_id = link_key_id(&new_key);
        let mut keys = self
            .link_sealing_keys
            .write()
            .map_err(|_| CatalogError::Busy)?;
        if keys.is_empty() {
            return Err(CatalogError::Invalid(
                "link sealing key is not configured".into(),
            ));
        }
        let current_id = keys[0].0.clone();
        if keys[0].1 == new_key {
            return Ok(LinkKeyRotationProgress {
                id: String::new(),
                status: "committed".into(),
                processed: 0,
                cursor_slug: None,
                cursor_role: None,
            });
        }
        let mut connection = self.lock_connection()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(CatalogError::from)?;
        let active: Option<ActiveLinkRotation> = tx
            .query_row(
                "SELECT id,from_key_id,to_key_id,cursor_slug,cursor_role
                 FROM link_key_rotations
                 WHERE status IN ('prepared','running')
                 ORDER BY created_at,id LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()
            .map_err(CatalogError::from)?;
        let (rotation_id, old_id, cursor_slug, cursor_role) = if let Some((
            id,
            from_id,
            to_id,
            cursor_slug,
            cursor_role,
        )) = active
        {
            if to_id != new_id {
                return Err(CatalogError::Conflict(
                    "a different link-key rotation is already running".into(),
                ));
            }
            (id, from_id, cursor_slug, cursor_role)
        } else {
            let all_new: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM links WHERE key_id <> ?1",
                    [&new_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if all_new == 0 {
                tx.execute(
                    "INSERT INTO link_keyring(key_id,status,created_at) VALUES(?1,'primary',unixepoch())
                     ON CONFLICT(key_id) DO UPDATE SET status='primary',retired_at=NULL",
                    [&new_id],
                )
                .map_err(CatalogError::from)?;
                tx.commit().map_err(CatalogError::from)?;
                if let Some(position) = keys.iter().position(|(id, _)| id == &new_id) {
                    let key = keys.remove(position);
                    keys.insert(0, key);
                } else {
                    keys.insert(0, (new_id.clone(), new_key));
                }
                return Ok(LinkKeyRotationProgress {
                    id: String::new(),
                    status: "committed".into(),
                    processed: 0,
                    cursor_slug: None,
                    cursor_role: None,
                });
            }
            let id = hex::encode(crate::auth::random_bytes(16));
            tx.execute(
                "INSERT INTO link_key_rotations
                 (id,from_key_id,to_key_id,status,created_at,updated_at)
                 VALUES(?1,?2,?3,'running',unixepoch(),unixepoch())",
                params![id, current_id, new_id],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "INSERT INTO link_keyring(key_id,status,created_at)
                 VALUES(?1,'decrypt',unixepoch())
                 ON CONFLICT(key_id) DO NOTHING",
                [&new_id],
            )
            .map_err(CatalogError::from)?;
            (id, current_id, None, None)
        };
        let old_key = keys
            .iter()
            .find(|(id, _)| id == &old_id)
            .map(|(_, key)| *key)
            .ok_or_else(|| {
                CatalogError::Invalid(format!(
                    "link rotation needs source key {old_id}, but it is not configured"
                ))
            })?;
        let mut statement = tx
            .prepare(
                "SELECT d.storage_id,l.slug,l.role,l.hash,l.sealed
                 FROM links l JOIN documents d ON d.slug=l.slug
                 WHERE l.key_id <> ?1
                   AND (?2 IS NULL OR l.slug > ?2 OR (l.slug = ?2 AND l.role > ?3))
                 ORDER BY l.slug,l.role LIMIT ?4",
            )
            .map_err(CatalogError::from)?;
        let rows = statement
            .query_map(
                params![
                    new_id,
                    cursor_slug.as_deref(),
                    cursor_role.as_deref(),
                    BATCH_SIZE
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                    ))
                },
            )
            .map_err(CatalogError::from)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(CatalogError::from)?;
        drop(statement);
        if rows.is_empty() {
            let remaining: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM links WHERE key_id <> ?1",
                    [&new_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if remaining != 0 {
                return Err(CatalogError::Conflict(
                    "link-key rotation cursor passed an unprocessed row".into(),
                ));
            }
            tx.execute(
                "UPDATE link_keyring SET status='decrypt',retired_at=NULL WHERE key_id=?1",
                [&old_id],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "INSERT INTO link_keyring(key_id,status,created_at) VALUES(?1,'primary',unixepoch())
                 ON CONFLICT(key_id) DO UPDATE SET status='primary',retired_at=NULL",
                [&new_id],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE link_key_rotations SET status='committed',updated_at=unixepoch()
                 WHERE id=?1",
                [&rotation_id],
            )
            .map_err(CatalogError::from)?;
            tx.commit().map_err(CatalogError::from)?;
            promote_link_key(&mut keys, new_id, new_key);
            return Ok(LinkKeyRotationProgress {
                id: rotation_id,
                status: "committed".into(),
                processed: 0,
                cursor_slug,
                cursor_role,
            });
        }
        for (storage_id, slug, role, digest, envelope) in &rows {
            let plaintext = open_link_envelope(&old_key, storage_id, role, digest, envelope)?;
            let next = seal_link_envelope(&new_key, &new_id, storage_id, role, digest, &plaintext)?;
            tx.execute(
                "UPDATE links SET sealed=?3,key_id=?4 WHERE slug=?1 AND role=?2 AND key_id <> ?4",
                params![slug, role, next, new_id],
            )
            .map_err(CatalogError::from)?;
        }
        let (last_slug, last_role) = rows
            .last()
            .map(|row| (row.1.as_str(), row.2.as_str()))
            .ok_or_else(|| CatalogError::Invalid("empty link-key rotation batch".into()))?;
        tx.execute(
            "UPDATE link_key_rotations SET cursor_slug=?2,cursor_role=?3,status='running',updated_at=unixepoch()
             WHERE id=?1",
            params![rotation_id, last_slug, last_role],
        )
        .map_err(CatalogError::from)?;
        tx.commit().map_err(CatalogError::from)?;
        Ok(LinkKeyRotationProgress {
            id: rotation_id,
            status: "running".into(),
            processed: rows.len() as u32,
            cursor_slug: Some(last_slug.to_string()),
            cursor_role: Some(last_role.to_string()),
        })
    }

    /// Update a document and only the access rows whose values changed.  This
    /// is the catalogue-backed Store mutation primitive: callers may stage a
    /// complete compatibility view, but the transaction never drops and
    /// recreates unrelated grants, links, or guest pins.
    pub fn update_document_access(
        &self,
        document: &Document,
        grants: &[Grant],
        links: &[Link],
        guests: &[Guest],
        actor: Option<(&str, &str, &str)>,
    ) -> CatalogResult<Document> {
        if document.size < 0
            || document.counted_size < document.size
            || document.maintenance_reserved < 0
            || document.maintenance_reserved > document.counted_size
        {
            return Err(CatalogError::Invalid("invalid document accounting".into()));
        }
        self.immediate(|tx| {
            if let Some((account_id, owner_key, generation)) = actor {
                let allowed: bool = if account_id.is_empty() {
                    tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM documents WHERE slug=?1
                         AND owner_id IS NULL AND owner_key=?2 AND status='active'
                         AND pending_publication IS NULL)",
                        params![document.slug, owner_key],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?
                } else {
                    tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM documents d JOIN accounts a ON a.id=d.owner_id
                         WHERE d.slug=?1 AND d.owner_id=?2 AND d.status='active'
                         AND d.pending_publication IS NULL AND a.status='active'
                         AND a.session_generation=?3)",
                        params![document.slug, account_id, generation],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?
                };
                if !allowed {
                    return Err(CatalogError::Conflict(
                        "actor ownership or session generation changed".into(),
                    ));
                }
            }
            let old_counted: i64 = tx
                .query_row(
                    "SELECT counted_size FROM documents WHERE slug = ?1 AND status = 'active'",
                    [&document.slug],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            if let Some(owner_id) = document.owner_id.as_deref() {
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
            let changed = tx
                .execute(
                    "UPDATE documents SET title=?2, sha=?3, updated_at=?4,
                            example=?5, owner_key=?6, owner_id=?7, size=?8,
                            counted_size=?9, source_format=?10, main=?11
                     WHERE slug=?1 AND status='active'",
                    params![
                        document.slug,
                        document.title,
                        document.sha,
                        document.updated_at,
                        document.example as i64,
                        document.owner_key,
                        document.owner_id,
                        document.size,
                        document.counted_size,
                        document.source_format,
                        document.main,
                    ],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::NotFound);
            }
            tx.execute(
                "UPDATE totals SET bytes = bytes + ?1 WHERE id = 1",
                [document.counted_size - old_counted],
            )
            .map_err(CatalogError::from)?;

            let desired_grants: HashSet<(&str, &str)> = grants
                .iter()
                .map(|grant| (grant.role.as_str(), grant.account_id.as_str()))
                .collect();
            let existing_grants: Vec<(String, String)> = {
                let mut statement = tx
                    .prepare("SELECT role, account_id FROM grants WHERE slug=?1")
                    .map_err(CatalogError::from)?;
                let rows = statement
                    .query_map([document.slug.as_str()], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })
                    .map_err(CatalogError::from)?;
                rows.collect::<Result<Vec<_>, _>>()
                    .map_err(CatalogError::from)?
            };
            for (role, account_id) in existing_grants {
                if !desired_grants.contains(&(role.as_str(), account_id.as_str())) {
                    tx.execute(
                        "DELETE FROM grants WHERE slug=?1 AND role=?2 AND account_id=?3",
                        params![document.slug, role, account_id],
                    )
                    .map_err(CatalogError::from)?;
                }
            }
            for grant in grants {
                let active: Option<String> = tx
                    .query_row(
                        "SELECT status FROM accounts WHERE id=?1",
                        [&grant.account_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(CatalogError::from)?;
                if active.as_deref() != Some("active") {
                    return Err(CatalogError::Conflict("grantee is not active".into()));
                }
                tx.execute(
                    "INSERT INTO grants(slug,role,account_id,since) VALUES(?1,?2,?3,?4)
                     ON CONFLICT(slug,role,account_id) DO UPDATE SET since=excluded.since",
                    params![document.slug, grant.role, grant.account_id, grant.since],
                )
                .map_err(CatalogError::from)?;
            }

            let desired_links: HashSet<&str> =
                links.iter().map(|link| link.role.as_str()).collect();
            let existing_links: Vec<String> = {
                let mut statement = tx
                    .prepare("SELECT role FROM links WHERE slug=?1")
                    .map_err(CatalogError::from)?;
                let rows = statement
                    .query_map([document.slug.as_str()], |row| row.get(0))
                    .map_err(CatalogError::from)?;
                rows.collect::<Result<Vec<_>, _>>()
                    .map_err(CatalogError::from)?
            };
            for role in existing_links {
                if !desired_links.contains(role.as_str()) {
                    tx.execute(
                        "DELETE FROM links WHERE slug=?1 AND role=?2",
                        params![document.slug, role],
                    )
                    .map_err(CatalogError::from)?;
                }
            }
            for link in links {
                if link.hash.is_empty() || link.sealed.is_empty() {
                    return Err(CatalogError::Invalid("invalid link".into()));
                }
                let key_id = envelope_key_id(&link.sealed);
                tx.execute(
                    "INSERT INTO links(slug,role,hash,sealed,key_id,label,budget,since,until)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
                     ON CONFLICT(slug,role) DO UPDATE SET hash=excluded.hash,
                       sealed=excluded.sealed,label=excluded.label,budget=excluded.budget,
                       since=excluded.since,until=excluded.until,key_id=excluded.key_id",
                    params![
                        document.slug,
                        link.role,
                        link.hash,
                        link.sealed,
                        key_id,
                        link.label,
                        link.budget,
                        link.since,
                        link.until,
                    ],
                )
                .map_err(CatalogError::from)?;
            }

            let desired_guests: HashSet<(&str, &str)> = guests
                .iter()
                .map(|guest| (guest.account_id.as_str(), guest.link_hash.as_str()))
                .collect();
            let existing_guests: Vec<(String, String)> = {
                let mut statement = tx
                    .prepare("SELECT account_id, link_hash FROM guests WHERE slug=?1")
                    .map_err(CatalogError::from)?;
                let rows = statement
                    .query_map([document.slug.as_str()], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })
                    .map_err(CatalogError::from)?;
                rows.collect::<Result<Vec<_>, _>>()
                    .map_err(CatalogError::from)?
            };
            for (account_id, link_hash) in existing_guests {
                if !desired_guests.contains(&(account_id.as_str(), link_hash.as_str())) {
                    tx.execute(
                        "DELETE FROM guests WHERE slug=?1 AND account_id=?2 AND link_hash=?3",
                        params![document.slug, account_id, link_hash],
                    )
                    .map_err(CatalogError::from)?;
                }
            }
            for guest in guests {
                let active: Option<String> = tx
                    .query_row(
                        "SELECT status FROM accounts WHERE id=?1",
                        [&guest.account_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(CatalogError::from)?;
                if active.as_deref() != Some("active") {
                    return Err(CatalogError::Conflict("guest account is not active".into()));
                }
                tx.execute(
                    "INSERT INTO guests(slug,account_id,since,link_hash) VALUES(?1,?2,?3,?4)
                     ON CONFLICT(slug,account_id,link_hash) DO UPDATE SET since=excluded.since",
                    params![
                        document.slug,
                        guest.account_id,
                        guest.since,
                        guest.link_hash
                    ],
                )
                .map_err(CatalogError::from)?;
            }
            Self::document_in_tx(tx, &document.slug)
        })
    }

    /// Return active documents visible to an owner, grant holder, guest pin,
    /// or the public examples branch.  Cursor ordering is stable and indexed.
    pub fn grant(
        &self,
        slug: &str,
        role: &str,
        account_id: &str,
        since: &str,
    ) -> CatalogResult<Grant> {
        if slug.is_empty() || role.is_empty() || account_id.is_empty() || since.is_empty() {
            return Err(CatalogError::Invalid("invalid grant".into()));
        }
        self.immediate(|tx| {
            let active: Option<String> = tx
                .query_row(
                    "SELECT status FROM accounts WHERE id=?1",
                    [account_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?;
            if active.as_deref() != Some("active") {
                return Err(CatalogError::Conflict("grantee is not active".into()));
            }
            let already: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM grants
                     WHERE slug=?1 AND account_id=?2)",
                    params![slug, account_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if !already {
                let documents: i64 = tx
                    .query_row(
                        "SELECT COUNT(DISTINCT slug) FROM grants WHERE account_id=?1",
                        [account_id],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                if documents >= MAX_RECIPIENT_DOCUMENTS {
                    return Err(CatalogError::Conflict(
                        "recipient document capacity exceeded".into(),
                    ));
                }
            }
            tx.execute(
                "INSERT INTO grants(slug,role,account_id,since) VALUES(?1,?2,?3,?4)
                        ON CONFLICT(slug,role,account_id) DO UPDATE SET since=excluded.since",
                params![slug, role, account_id, since],
            )
            .map_err(CatalogError::from)?;
            Ok(Grant {
                slug: slug.into(),
                role: role.into(),
                account_id: account_id.into(),
                since: since.into(),
            })
        })
    }

    pub fn revoke_grant(&self, slug: &str, role: &str, account_id: &str) -> CatalogResult<bool> {
        self.immediate(|tx| {
            let n = tx
                .execute(
                    "DELETE FROM grants WHERE slug=?1 AND role=?2 AND account_id=?3",
                    params![slug, role, account_id],
                )
                .map_err(CatalogError::from)?;
            Ok(n == 1)
        })
    }

    pub fn grants(
        &self,
        slug: &str,
        cursor: Option<(&str, &str)>,
        limit: u32,
    ) -> CatalogResult<Vec<Grant>> {
        let limit = i64::from(limit.clamp(1, 200));
        self.with_connection(|c| {
            let mut s = c
                .prepare(
                    "SELECT slug,role,account_id,since FROM grants
                WHERE slug=?1 AND (?2 IS NULL OR role>?2 OR (role=?2 AND account_id>?3))
                ORDER BY role,account_id LIMIT ?4",
                )
                .map_err(CatalogError::from)?;
            let mut rows = s
                .query(params![
                    slug,
                    cursor.map(|x| x.0),
                    cursor.map(|x| x.1),
                    limit
                ])
                .map_err(CatalogError::from)?;
            let mut out = Vec::new();
            while let Some(r) = rows.next().map_err(CatalogError::from)? {
                out.push(Grant {
                    slug: r.get(0).map_err(CatalogError::from)?,
                    role: r.get(1).map_err(CatalogError::from)?,
                    account_id: r.get(2).map_err(CatalogError::from)?,
                    since: r.get(3).map_err(CatalogError::from)?,
                });
            }
            Ok(out)
        })
    }

    pub fn put_link(&self, link: &Link) -> CatalogResult<Link> {
        if link.slug.is_empty()
            || link.role.is_empty()
            || link.hash.is_empty()
            || link.sealed.is_empty()
        {
            return Err(CatalogError::Invalid("invalid link".into()));
        }
        if link.budget.is_some_and(|n| n < 0) || (!link.until.is_empty() && link.until.len() < 10) {
            return Err(CatalogError::Invalid(
                "invalid link expiry or budget".into(),
            ));
        }
        self.immediate(|tx| {
            let valid_expiry: i64 = tx.query_row("SELECT (?1='' OR julianday(?1) IS NOT NULL)", [&link.until], |r| r.get(0)).map_err(CatalogError::from)?;
            if valid_expiry == 0 { return Err(CatalogError::Invalid("invalid link expiry".into())); }
            let key_id = envelope_key_id(&link.sealed);
            tx.execute("INSERT INTO links(slug,role,hash,sealed,key_id,label,budget,since,until)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
                ON CONFLICT(slug,role) DO UPDATE SET hash=excluded.hash,sealed=excluded.sealed,
                label=excluded.label,budget=excluded.budget,since=excluded.since,until=excluded.until,key_id=excluded.key_id",
                params![link.slug,link.role,link.hash,link.sealed,key_id,link.label,link.budget,link.since,link.until]).map_err(CatalogError::from)?;
            tx.execute("DELETE FROM guests WHERE slug=?1 AND link_hash<>?2", params![link.slug, link.hash]).map_err(CatalogError::from)?;
            Ok(link.clone())
        })
    }

    pub fn drop_link(&self, slug: &str, role: &str) -> CatalogResult<bool> {
        self.immediate(|tx| {
            let n = tx
                .execute(
                    "DELETE FROM links WHERE slug=?1 AND role=?2",
                    params![slug, role],
                )
                .map_err(CatalogError::from)?;
            Ok(n == 1)
        })
    }

    pub fn links(&self, slug: &str) -> CatalogResult<Vec<Link>> {
        self.with_connection(|c| {
            let mut s = c.prepare("SELECT slug,role,hash,sealed,label,budget,since,until FROM links WHERE slug=?1 ORDER BY role").map_err(CatalogError::from)?;
            let mut rows = s.query([slug]).map_err(CatalogError::from)?;
            let mut out = Vec::new();
            while let Some(r) = rows.next().map_err(CatalogError::from)? {
                out.push(Link { slug:r.get(0).map_err(CatalogError::from)?,role:r.get(1).map_err(CatalogError::from)?,hash:r.get(2).map_err(CatalogError::from)?,sealed:r.get(3).map_err(CatalogError::from)?,label:r.get(4).map_err(CatalogError::from)?,budget:r.get(5).map_err(CatalogError::from)?,since:r.get(6).map_err(CatalogError::from)?,until:r.get(7).map_err(CatalogError::from)? });
            }
            Ok(out)
        })
    }

    /// Pin a guest only when the account, document and link are still live.
    /// The operation is idempotent for the same `(slug, account, hash)`.
    pub fn pin_guest(&self, guest: &Guest) -> CatalogResult<Guest> {
        if guest.slug.is_empty() || guest.account_id.is_empty() || guest.link_hash.is_empty() {
            return Err(CatalogError::Invalid("invalid guest pin".into()));
        }
        self.immediate(|tx| {
            let status: Option<String> = tx.query_row("SELECT status FROM accounts WHERE id=?1", [&guest.account_id], |r| r.get(0)).optional().map_err(CatalogError::from)?;
            if status.as_deref() != Some("active") { return Err(CatalogError::Conflict("guest account is not active".into())); }
            let live: i64 = tx.query_row("SELECT COUNT(*) FROM documents d JOIN links l ON l.slug=d.slug
                WHERE d.slug=?1 AND d.status='active' AND l.hash=?2 AND
                (l.until='' OR (julianday(l.until) IS NOT NULL AND julianday(l.until)>julianday('now')))", params![guest.slug,guest.link_hash], |r| r.get(0)).map_err(CatalogError::from)?;
            if live != 1 { return Err(CatalogError::NotFound); }
            tx.execute("INSERT OR IGNORE INTO guests(slug,account_id,since,link_hash) VALUES(?1,?2,?3,?4)", params![guest.slug,guest.account_id,guest.since,guest.link_hash]).map_err(CatalogError::from)?;
            Ok(guest.clone())
        })
    }

    pub fn guests(&self, slug: &str, limit: u32) -> CatalogResult<Vec<Guest>> {
        let limit = i64::from(limit.clamp(1, 200));
        self.with_connection(|c| {
            let mut s = c.prepare("SELECT slug,account_id,since,link_hash FROM guests WHERE slug=?1 ORDER BY account_id,link_hash LIMIT ?2").map_err(CatalogError::from)?;
            let mut rows = s.query(params![slug,limit]).map_err(CatalogError::from)?;
            let mut out = Vec::new();
            while let Some(r) = rows.next().map_err(CatalogError::from)? { out.push(Guest {slug:r.get(0).map_err(CatalogError::from)?,account_id:r.get(1).map_err(CatalogError::from)?,since:r.get(2).map_err(CatalogError::from)?,link_hash:r.get(3).map_err(CatalogError::from)?}); }
            Ok(out)
        })
    }

    pub fn unpin_guest(
        &self,
        slug: &str,
        account_id: &str,
        link_hash: &str,
    ) -> CatalogResult<bool> {
        self.immediate(|tx| {
            Ok(tx
                .execute(
                    "DELETE FROM guests WHERE slug=?1 AND account_id=?2 AND link_hash=?3",
                    params![slug, account_id, link_hash],
                )
                .map_err(CatalogError::from)?
                == 1)
        })
    }

    pub fn visible_documents(
        &self,
        account_id: Option<&str>,
        owner_key: Option<&str>,
        cursor: Option<(&str, &str)>,
        limit: u32,
    ) -> CatalogResult<Vec<Document>> {
        self.visible_documents_with_examples(account_id, owner_key, cursor, limit, true)
    }

    /// Authorization-aware keyset listing with an explicit example switch.
    /// Each visibility source is queried through its covering index and the
    /// bounded pages are merged in memory.  Keeping the branches separate is
    /// important: a single OR/EXISTS query makes SQLite scan and sort the
    /// entire documents table before applying LIMIT.
    pub fn visible_documents_with_examples(
        &self,
        account_id: Option<&str>,
        owner_key: Option<&str>,
        cursor: Option<(&str, &str)>,
        limit: u32,
        include_examples: bool,
    ) -> CatalogResult<Vec<Document>> {
        let limit = limit.clamp(1, 200);
        let branch_limit = i64::from(limit.saturating_mul(4).min(800));
        self.with_connection(|connection| {
            let mut slugs = HashSet::new();
            let cursor_sql = " AND (?2 IS NULL OR d.updated_at < ?2 OR (d.updated_at = ?2 AND d.slug < ?3))";
            if let Some(account_id) = account_id {
                let sql = format!(
                    "SELECT d.slug FROM documents d WHERE d.status='active' AND d.pending_publication IS NULL AND d.owner_id=?1{cursor_sql} ORDER BY d.updated_at DESC,d.slug DESC LIMIT ?4"
                );
                let mut statement = connection.prepare(&sql).map_err(CatalogError::from)?;
                let mut rows = statement
                    .query(params![account_id, cursor.map(|c| c.0), cursor.map(|c| c.1), branch_limit])
                    .map_err(CatalogError::from)?;
                while let Some(row) = rows.next().map_err(CatalogError::from)? {
                    slugs.insert(row.get::<_, String>(0).map_err(CatalogError::from)?);
                }
                let sql = format!(
                    "SELECT d.slug FROM documents d
                     CROSS JOIN grants g ON g.slug=d.slug AND g.account_id=?1
                     WHERE d.status='active' AND d.pending_publication IS NULL{cursor_sql}
                     ORDER BY d.updated_at DESC,d.slug DESC LIMIT ?4"
                );
                let mut statement = connection.prepare(&sql).map_err(CatalogError::from)?;
                let mut rows = statement
                    .query(params![account_id, cursor.map(|c| c.0), cursor.map(|c| c.1), branch_limit])
                    .map_err(CatalogError::from)?;
                while let Some(row) = rows.next().map_err(CatalogError::from)? {
                    slugs.insert(row.get::<_, String>(0).map_err(CatalogError::from)?);
                }
                let sql = format!(
                    "SELECT d.slug FROM documents d
                     CROSS JOIN guests ge ON ge.slug=d.slug AND ge.account_id=?1
                     CROSS JOIN links l ON l.slug=d.slug AND l.hash=ge.link_hash
                     WHERE d.status='active' AND d.pending_publication IS NULL
                       AND (l.until='' OR (julianday(l.until) IS NOT NULL AND julianday(l.until)>julianday('now'))){cursor_sql}
                     ORDER BY d.updated_at DESC,d.slug DESC LIMIT ?4"
                );
                let mut statement = connection.prepare(&sql).map_err(CatalogError::from)?;
                let mut rows = statement
                    .query(params![account_id, cursor.map(|c| c.0), cursor.map(|c| c.1), branch_limit])
                    .map_err(CatalogError::from)?;
                while let Some(row) = rows.next().map_err(CatalogError::from)? {
                    slugs.insert(row.get::<_, String>(0).map_err(CatalogError::from)?);
                }
            }
            if let Some(owner_key) = owner_key {
                let sql = format!(
                    "SELECT d.slug FROM documents d WHERE d.status='active' AND d.pending_publication IS NULL AND d.owner_id IS NULL AND d.owner_key=?1{cursor_sql} ORDER BY d.updated_at DESC,d.slug DESC LIMIT ?4"
                );
                let mut statement = connection.prepare(&sql).map_err(CatalogError::from)?;
                let mut rows = statement
                    .query(params![owner_key, cursor.map(|c| c.0), cursor.map(|c| c.1), branch_limit])
                    .map_err(CatalogError::from)?;
                while let Some(row) = rows.next().map_err(CatalogError::from)? {
                    slugs.insert(row.get::<_, String>(0).map_err(CatalogError::from)?);
                }
            }
            if include_examples {
                let sql = format!(
                    "SELECT d.slug FROM documents d INDEXED BY documents_active_example_updated
                     WHERE d.status='active' AND d.pending_publication IS NULL
                       AND d.example=1{cursor_sql}
                     ORDER BY d.updated_at DESC,d.slug DESC LIMIT ?4"
                );
                let mut statement = connection.prepare(&sql).map_err(CatalogError::from)?;
                let mut rows = statement
                    .query(params!["", cursor.map(|c| c.0), cursor.map(|c| c.1), branch_limit])
                    .map_err(CatalogError::from)?;
                while let Some(row) = rows.next().map_err(CatalogError::from)? {
                    slugs.insert(row.get::<_, String>(0).map_err(CatalogError::from)?);
                }
                // A pre-contract seed may have marked an example only by its
                // reserved owner key. Keep that compatibility form, but as a
                // separate key-range branch so it cannot turn the normal
                // example query into an OR scan or temporary sort.
                let sql = format!(
                    "SELECT d.slug FROM documents d INDEXED BY documents_active_owner_key_updated
                     WHERE d.status='active' AND d.pending_publication IS NULL
                       AND d.owner_id IS NULL AND d.owner_key >= 'example:' AND d.owner_key < 'example;'{cursor_sql}
                     ORDER BY d.updated_at DESC,d.slug DESC LIMIT ?4"
                );
                let mut statement = connection.prepare(&sql).map_err(CatalogError::from)?;
                let mut rows = statement
                    .query(params!["", cursor.map(|c| c.0), cursor.map(|c| c.1), branch_limit])
                    .map_err(CatalogError::from)?;
                while let Some(row) = rows.next().map_err(CatalogError::from)? {
                    slugs.insert(row.get::<_, String>(0).map_err(CatalogError::from)?);
                }
            }
            let mut documents = Vec::with_capacity(slugs.len());
            for slug in slugs {
                if let Some(document) = connection
                    .query_row(
                        "SELECT slug, storage_id, title, sha, created_at, published_at,
                                updated_at, example, owner_key, owner_id, status, size,
                                counted_size, maintenance_reserved, comment_seq,
                                last_auto_checkpoint_at, pending_publication,
                                last_publication_id, source_format, main
                         FROM documents WHERE slug=?1",
                        [&slug],
                        Self::read_document,
                    )
                    .optional()
                    .map_err(CatalogError::from)?
                {
                    documents.push(document);
                }
            }
            documents.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then_with(|| b.slug.cmp(&a.slug)));
            documents.truncate(limit as usize);
            Ok(documents)
        })
    }
}
