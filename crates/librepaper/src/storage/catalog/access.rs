//! Who may see a document besides its owner: grants, links and the keyring
//! that seals a link's key at rest, pinned guests, and the visibility query
//! the landing page runs.

use super::*;

/// Durable progress returned by one link-key rotation worker invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinkKeyRotationProgress {
    pub id: String,
    pub status: String,
    pub processed: u32,
    pub cursor_document_id: Option<String>,
    pub cursor_link_id: Option<String>,
}

pub(super) fn link_key_id(key: &[u8; 32]) -> String {
    hex::encode(sha2::Sha256::digest(key))[..16].to_string()
}

pub(super) fn promote_link_key(keys: &mut Vec<(String, [u8; 32])>, id: String, key: [u8; 32]) {
    keys.retain(|(known, _)| known != &id);
    keys.insert(0, (id, key));
}

fn link_time(value: &str) -> CatalogResult<i64> {
    if value.is_empty() {
        return Ok(unix_millis());
    }
    if let Ok(value) = value.parse::<i64>() {
        return Ok(if value < 10_000_000_000 {
            value.saturating_mul(1_000)
        } else {
            value
        });
    }
    crate::util::parse_timestamp_millis(value).ok_or_else(|| {
        CatalogError::Invalid("link time must be Unix milliseconds or RFC3339".into())
    })
}

pub(super) fn open_link_envelope(
    key: &[u8; 32],
    storage_id: &str,
    role: &str,
    digest: &str,
    envelope: &[u8],
) -> CatalogResult<String> {
    let key_id = envelope_key_id(envelope)?;
    if key_id != link_key_id(key) {
        return Err(CatalogError::Invalid("sealed link key id mismatch".into()));
    }
    let (nonce_at, body_at) = (22, 46);
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

pub(super) fn envelope_key_id(envelope: &[u8]) -> CatalogResult<String> {
    if envelope.len() < 46 || !envelope.starts_with(b"KLINK2") {
        return Err(CatalogError::Invalid("invalid sealed link envelope".into()));
    }
    let id = &envelope[6..22];
    if !id
        .iter()
        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(CatalogError::Invalid("invalid sealed link key id".into()));
    }
    String::from_utf8(id.to_vec())
        .map_err(|_| CatalogError::Invalid("invalid sealed link key id".into()))
}

fn visibility_document(row: &rusqlite::Row<'_>) -> rusqlite::Result<Document> {
    Ok(Document {
        slug: row.get(0)?,
        storage_id: row.get(1)?,
        title: row.get(2)?,
        sha: String::new(),
        created_at: crate::util::format_unix_millis(row.get(3)?),
        published_at: row
            .get::<_, Option<i64>>(4)?
            .map_or_else(String::new, crate::util::format_unix_millis),
        updated_at: crate::util::format_unix_millis(row.get(5)?),
        example: row.get::<_, String>(6)? == "example",
        owner_key: String::new(),
        owner_id: row.get(7)?,
        status: row.get(8)?,
        size: row.get(9)?,
        counted_size: row
            .get::<_, i64>(9)?
            .checked_add(row.get(10)?)
            .ok_or(rusqlite::Error::InvalidQuery)?,
        maintenance_reserved: 0,
        comment_seq: 0,
        last_auto_checkpoint_at: 0,
        last_publication_id: String::new(),
        source_format: row.get(11)?,
        main: row.get(12)?,
    })
}

fn visibility_page(
    connection: &rusqlite::Connection,
    sql: &str,
    params: &[&dyn rusqlite::ToSql],
) -> CatalogResult<Vec<Document>> {
    let mut statement = connection.prepare(sql).map_err(CatalogError::from)?;
    let mut rows = statement
        .query(rusqlite::params_from_iter(params.iter().copied()))
        .map_err(CatalogError::from)?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().map_err(CatalogError::from)? {
        result.push(visibility_document(row).map_err(CatalogError::from)?);
    }
    Ok(result)
}

fn bump_document_updated_at(
    tx: &rusqlite::Transaction<'_>,
    document_id: &str,
) -> CatalogResult<()> {
    let current: i64 = tx
        .query_row(
            "SELECT updated_at FROM documents WHERE id=?1",
            [document_id],
            |row| row.get(0),
        )
        .map_err(CatalogError::from)?;
    let next = current
        .checked_add(1)
        .ok_or_else(|| CatalogError::Invalid("document timestamp overflow".into()))?
        .max(unix_millis());
    tx.execute(
        "UPDATE documents SET updated_at=?1 WHERE id=?2",
        params![next, document_id],
    )
    .map_err(CatalogError::from)?;
    Ok(())
}

impl Catalog {
    /// Recheck the non-secret account part of a publication display
    /// capability.  This is a read authorization rather than a mutation, but
    /// it has the same revocation boundary: an erased account, a rotated
    /// session, or a removed named-editor grant stops new document-origin
    /// responses immediately.
    pub fn display_account_authorized(
        &self,
        slug: &str,
        account_id: &str,
        generation_fingerprint: &str,
    ) -> CatalogResult<bool> {
        self.with_connection(|connection| {
            let account = connection
                .query_row(
                    "SELECT status, session_generation FROM accounts WHERE id=?1",
                    [account_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?;
            let Some((status, generation)) = account else {
                return Ok(false);
            };
            if status != "active"
                || hex::encode(sha2::Sha256::digest(generation.as_bytes()))
                    != generation_fingerprint
            {
                return Ok(false);
            }
            connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM documents d
                  WHERE d.slug=?1 AND d.status='active'
                    AND (d.owner_id=?2 OR EXISTS(
                        SELECT 1 FROM grants g WHERE g.document_id=d.id
                          AND g.account_id=?2 AND g.role='editor')))",
                    params![slug, account_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)
        })
    }

    /// Check a mutation actor against the live document and access rows.  This
    /// is deliberately evaluated inside the caller's write transaction so a
    /// link expiry, policy change, grant revocation, or account generation
    /// change cannot be bypassed by a stale route-level Viewer.
    pub(crate) fn mutation_authorized_in_tx(
        tx: &rusqlite::Transaction<'_>,
        slug: &str,
        actor: MutationAuthority<'_>,
        role: &str,
    ) -> CatalogResult<bool> {
        if !Self::agent_execution_epoch_active_tx(tx, slug, actor.execution_epoch)? {
            return Ok(false);
        }
        let required_role = match role {
            "reader" => "reader",
            "commenter" => "commenter",
            _ => "editor",
        };
        let now = unix_millis();
        let link_ok: bool = !actor.link_hash.is_empty()
            && tx
                .query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM links l JOIN documents d ON d.id=l.document_id
                        JOIN accounts owner ON owner.id=d.owner_id AND owner.status='active'
                        WHERE d.slug=?1 AND l.token_hash=?2 AND d.status='active'
                          AND (l.role=?3 OR l.role='editor' OR (?3='reader' AND l.role='commenter'))
                          AND (l.expires_at IS NULL OR l.expires_at>?4))",
                    params![slug, actor.link_hash, required_role, now],
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
            return Ok(matches!(role, "editor" | "commenter") && link_ok && actor.policy_editor);
        }
        if actor.account_id.is_empty() {
            if actor.unowned_publisher && actor.policy_editor {
                return tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM documents d JOIN accounts a ON a.id=d.owner_id
                           WHERE d.slug=?1 AND d.status='active' AND a.status='active'
                             AND ownership_mode IN ('open','example'))",
                        [slug],
                        |row| row.get::<_, bool>(0),
                    )
                    .map(|open| open || link_ok)
                    .map_err(CatalogError::from);
            }
            return Ok(link_ok);
        }
        tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM documents d
               JOIN accounts owner ON owner.id=d.owner_id AND owner.status='active'
               JOIN accounts a ON a.id=?2
              WHERE d.slug=?1 AND d.status IN ('active','creating')
                AND a.status='active' AND a.session_generation=?3
                AND (d.owner_id=?2 OR (?4=1 AND EXISTS(
                    SELECT 1 FROM grants g WHERE g.document_id=d.id
                      AND g.account_id=?2 AND (g.role=?5 OR g.role='editor'))) OR (?6=1 AND ?7=1)))",
            params![
                slug,
                actor.account_id,
                actor.generation,
                actor.policy_editor,
                required_role,
                link_ok,
                actor.policy_editor,
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(CatalogError::from)
    }

    pub fn set_link_sealing_key(&self, key: &[u8]) -> CatalogResult<()> {
        let key: [u8; 32] = key
            .try_into()
            .map_err(|_| CatalogError::Invalid("link sealing key must be 32 bytes".into()))?;
        let key_id = hex::encode(sha2::Sha256::digest(key))[..16].to_string();
        let durable: (String, String) = self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT active_link_key_id,keyring_json FROM server_state WHERE id=1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(CatalogError::from)
        })?;
        if durable.0 != "initial" && durable.0 != key_id {
            return Err(CatalogError::Conflict(
                "link sealing key changed outside the deployment initializer".into(),
            ));
        }
        let mut keys = self
            .link_sealing_keys
            .write()
            .map_err(|_| CatalogError::Busy)?;
        if let Some((_, existing)) = keys.first() {
            return if existing == &key {
                Ok(())
            } else {
                Err(CatalogError::Conflict("link sealing key changed".into()))
            };
        }
        let mut keyring: serde_json::Value = serde_json::from_str(&durable.1).map_err(|error| {
            CatalogError::Invalid(format!("invalid durable link keyring: {error}"))
        })?;
        let entries = keyring
            .get_mut("keys")
            .and_then(serde_json::Value::as_array_mut)
            .ok_or_else(|| CatalogError::Invalid("durable link keyring has no keys".into()))?;
        if !entries.iter().any(|entry| {
            entry.get("id").and_then(serde_json::Value::as_str) == Some(key_id.as_str())
        }) {
            entries.push(serde_json::json!({"id": key_id, "created_at": super::unix_millis()}));
        }
        let encoded = serde_json::to_string(&keyring)
            .map_err(|error| CatalogError::Invalid(error.to_string()))?;
        self.with_connection(|connection| {
            connection
                .execute(
                    "UPDATE server_state SET active_link_key_id=?1,keyring_json=?2,updated_at=max(updated_at,?3) WHERE id=1",
                    params![key_id, encoded, super::unix_millis()],
                )
                .map_err(CatalogError::from)?;
            Ok(())
        })?;
        keys.push((key_id, key));
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
        let durable: String = self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT keyring_json FROM server_state WHERE id=1",
                    [],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)
        })?;
        let mut keyring: serde_json::Value = serde_json::from_str(&durable).map_err(|error| {
            CatalogError::Invalid(format!("invalid durable link keyring: {error}"))
        })?;
        let entries = keyring
            .get_mut("keys")
            .and_then(serde_json::Value::as_array_mut)
            .ok_or_else(|| CatalogError::Invalid("durable link keyring has no keys".into()))?;
        if !entries
            .iter()
            .any(|entry| entry.get("id").and_then(serde_json::Value::as_str) == Some(id.as_str()))
        {
            entries.push(serde_json::json!({"id": id, "created_at": super::unix_millis()}));
        }
        let encoded = serde_json::to_string(&keyring)
            .map_err(|error| CatalogError::Invalid(error.to_string()))?;
        self.with_connection(|connection| {
            connection
                .execute(
                    "UPDATE server_state SET keyring_json=?1,updated_at=max(updated_at,?2) WHERE id=1",
                    params![encoded, super::unix_millis()],
                )
                .map_err(CatalogError::from)?;
            Ok(())
        })?;
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
                    "SELECT active_link_key_id FROM server_state WHERE id=1",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map(|value| value.filter(|id| id != "initial"))
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
        let wanted = envelope_key_id(envelope)?;
        let aad = format!("librepaper-link-v1\0{storage_id}\0{role}\0{digest}");
        let (nonce_at, body_at) = (22, 46);
        let plaintext = keys
            .iter()
            .filter(|(id, _)| wanted == *id)
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

    pub fn update_document_access(
        &self,
        document: &Document,
        grants: &[Grant],
        links: &[Link],
        _guests: &[Guest],
        actor: Option<(&str, &str, &str)>,
    ) -> CatalogResult<Document> {
        if document.slug.is_empty() || document.storage_id.is_empty() {
            return Err(CatalogError::Invalid("invalid document identity".into()));
        }
        if links.len() > 3 {
            return Err(CatalogError::Invalid(
                "a document may have at most three links".into(),
            ));
        }
        if grants.len() > 1_000 {
            return Err(CatalogError::Invalid("grant page is too large".into()));
        }
        let expected_updated_at = link_time(&document.updated_at)?;
        let slug = document.slug.clone();
        let keys = self
            .link_sealing_keys
            .read()
            .map_err(|_| CatalogError::Busy)?
            .clone();
        self.immediate(|tx| {
            let (document_id, current_updated_at, current_status, current_mode, owner_id):
                (String, i64, String, String, String) = tx
                .query_row(
                    "SELECT id,updated_at,status,ownership_mode,owner_id FROM documents WHERE slug=?1",
                    [&slug],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            if current_status != "active" {
                return Err(CatalogError::Conflict("document is not active".into()));
            }
            if document.storage_id != document_id
                || document.owner_id.as_deref() != Some(owner_id.as_str())
            {
                return Err(CatalogError::Conflict(
                    "document identity or owner changed while sharing".into(),
                ));
            }
            if current_updated_at != expected_updated_at {
                return Err(CatalogError::Conflict(
                    "document changed while sharing; reload and retry".into(),
                ));
            }
            let authorized = if let Some((account_id, owner_key, generation)) = actor {
                let owner_match = if account_id.is_empty() {
                    let digest = sha2::Sha256::digest(owner_key.as_bytes());
                    owner_id == format!("anonymous:{}", hex::encode(digest))
                } else {
                    account_id == owner_id
                };
                if account_id.is_empty() && owner_match {
                    tx.query_row(
                        "SELECT status='active' AND session_generation=?2
                         FROM accounts WHERE id=?1",
                        params![owner_id, generation],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(CatalogError::from)?
                } else {
                    owner_match
                        && Self::mutation_authorized_in_tx(
                            tx,
                            &slug,
                            MutationAuthority {
                                account_id,
                                owner_key,
                                generation,
                                link_hash: "",
                                policy_editor: true,
                                automation: false,
                                unowned_publisher: false,
                                execution_epoch: "",
                                agent_checkpoint: None,
                            },
                            "editor",
                        )?
                }
            } else {
                tx.query_row(
                    "SELECT a.status='active' AND d.status='active'
                     FROM documents d JOIN accounts a ON a.id=d.owner_id
                     WHERE d.id=?1",
                    [&document_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(CatalogError::from)?
            };
            if !authorized {
                return Err(CatalogError::Conflict(
                    "sharing authority is no longer valid".into(),
                ));
            }

            let mut roles = std::collections::HashSet::new();
            for link in links {
                if !matches!(link.role.as_str(), "reader" | "commenter" | "editor")
                    || link.hash.len() != 64
                    || !link.hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                    || !roles.insert(link.role.as_str())
                {
                    return Err(CatalogError::Invalid("invalid or duplicate link role".into()));
                }
                let expires_at = if link.until.is_empty() {
                    None
                } else {
                    Some(link_time(&link.until)?)
                };
                let created_at = link_time(&link.since)?;
                if expires_at.is_some_and(|until| until < created_at) {
                    return Err(CatalogError::Invalid("link expiry precedes creation".into()));
                }
                if keys.is_empty()
                    || !keys.iter().any(|(_, key)| {
                        open_link_envelope(
                            key,
                            &document_id,
                            &link.role,
                            &link.hash,
                            &link.sealed,
                        )
                        .is_ok()
                    })
                {
                    return Err(CatalogError::Invalid(
                        "sealed link authentication failed".into(),
                    ));
                }
                tx.execute(
                    "INSERT INTO links(document_id,id,role,token_hash,sealed_token,sealing_key_id,
                         label,budget,created_at,expires_at)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
                     ON CONFLICT(document_id,role) DO UPDATE SET
                       token_hash=excluded.token_hash,sealed_token=excluded.sealed_token,
                       sealing_key_id=excluded.sealing_key_id,label=excluded.label,
                       budget=excluded.budget,expires_at=excluded.expires_at,
                       credential_generation=credential_generation+
                         CASE WHEN token_hash<>excluded.token_hash THEN 1 ELSE 0 END",
                    params![
                        document_id,
                        format!("{}-{}", link.role, hex::encode(crate::auth::random_bytes(8))),
                        link.role,
                        link.hash,
                        link.sealed,
                        envelope_key_id(&link.sealed)?,
                        link.label,
                        link.budget,
                        created_at,
                        expires_at,
                    ],
                )
                .map_err(CatalogError::from)?;
            }
            if links.is_empty() {
                tx.execute("DELETE FROM links WHERE document_id=?1", [&document_id])
                    .map_err(CatalogError::from)?;
            } else {
                let placeholders = std::iter::repeat_n("?", links.len())
                    .collect::<Vec<_>>()
                    .join(",");
                let mut values: Vec<&dyn rusqlite::ToSql> = vec![&document_id];
                for link in links {
                    values.push(&link.role);
                }
                tx.execute(
                    &format!(
                        "DELETE FROM links WHERE document_id=?1 AND role NOT IN ({placeholders})"
                    ),
                    rusqlite::params_from_iter(values),
                )
                .map_err(CatalogError::from)?;
            }

            for grant in grants {
                if grant.slug != slug
                    || !matches!(grant.role.as_str(), "reader" | "commenter" | "editor")
                {
                    return Err(CatalogError::Invalid("invalid grant".into()));
                }
                let active: bool = tx
                    .query_row(
                        "SELECT status='active' FROM accounts WHERE id=?1",
                        [&grant.account_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(CatalogError::from)?
                    .unwrap_or(false);
                if !active {
                    return Err(CatalogError::Conflict("grantee is not active".into()));
                }
                let exists: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM grants
                         WHERE document_id=?1 AND account_id=?2)",
                        params![document_id, grant.account_id],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                if !exists {
                    let count: i64 = tx
                        .query_row(
                            "SELECT COUNT(DISTINCT document_id) FROM grants WHERE account_id=?1",
                            [&grant.account_id],
                            |row| row.get(0),
                        )
                        .map_err(CatalogError::from)?;
                    if count >= MAX_RECIPIENT_DOCUMENTS {
                        return Err(CatalogError::Conflict(
                            "recipient document capacity exceeded".into(),
                        ));
                    }
                }
                tx.execute(
                    "INSERT INTO grants(document_id,account_id,role,created_at)
                     VALUES(?1,?2,?3,?4)
                     ON CONFLICT(document_id,account_id) DO UPDATE SET
                       role=excluded.role,created_at=excluded.created_at",
                    params![document_id, grant.account_id, grant.role, link_time(&grant.since)?],
                )
                .map_err(CatalogError::from)?;
            }

            let now = unix_millis();
            let updated_at = now.max(current_updated_at.saturating_add(1));
            tx.execute(
                "UPDATE documents SET ownership_mode=?1,updated_at=?2
                 WHERE id=?3 AND updated_at=?4 AND status='active'",
                params![
                    if document.example { "example" } else { current_mode.as_str() },
                    updated_at,
                    document_id,
                    expected_updated_at
                ],
            )
            .map_err(CatalogError::from)?;
            if tx.changes() != 1 {
                return Err(CatalogError::Conflict(
                    "document changed while sharing; reload and retry".into(),
                ));
            }
            let mut result = document.clone();
            result.updated_at = updated_at.to_string();
            result.status = "active".into();
            result.owner_id = Some(owner_id);
            Ok(result)
        })
    }

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
            let document_id: String = tx.query_row("SELECT id FROM documents WHERE slug=?1", [slug], |row| row.get(0)).map_err(CatalogError::from)?;
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
                     WHERE document_id=?1 AND account_id=?2)",
                    params![document_id, account_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if !already {
                let documents: i64 = tx
                    .query_row(
                        "SELECT COUNT(DISTINCT document_id) FROM grants WHERE account_id=?1",
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
                "INSERT INTO grants(document_id,role,account_id,created_at) VALUES(?1,?2,?3,?4)
                        ON CONFLICT(document_id,account_id) DO UPDATE SET role=excluded.role,created_at=excluded.created_at",
                params![document_id, role, account_id, link_time(since)?],
            )
            .map_err(CatalogError::from)?;
            bump_document_updated_at(tx, &document_id)?;
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
            let document_id: String = tx
                .query_row(
                    "SELECT d.id FROM documents d JOIN accounts a ON a.id=d.owner_id
                            WHERE d.slug=?1 AND d.status='active' AND a.status='active'",
                    [slug],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            let n = tx
                .execute(
                    "DELETE FROM grants WHERE document_id=?1 AND role=?2 AND account_id=?3",
                    params![document_id, role, account_id],
                )
                .map_err(CatalogError::from)?;
            if n == 1 {
                bump_document_updated_at(tx, &document_id)?;
            }
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
                    "SELECT d.slug,g.role,g.account_id,g.created_at FROM grants g JOIN documents d ON d.id=g.document_id
                WHERE d.slug=?1 AND (?2 IS NULL OR g.role>?2 OR (g.role=?2 AND g.account_id>?3))
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
                    since: crate::util::format_unix_millis(r.get(3).map_err(CatalogError::from)?),
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
            || !matches!(link.role.as_str(), "reader" | "commenter" | "editor")
            || link.hash.len() != 64
            || !link.hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(CatalogError::Invalid("invalid link".into()));
        }
        if link.budget.is_some_and(|n| n < 0) {
            return Err(CatalogError::Invalid(
                "invalid link expiry or budget".into(),
            ));
        }
        let keys = self
            .link_sealing_keys
            .read()
            .map_err(|_| CatalogError::Busy)?
            .clone();
        self.immediate(|tx| {
            let (document_id, storage_id): (String, String) = tx.query_row("SELECT d.id,d.id FROM documents d JOIN accounts a ON a.id=d.owner_id
                WHERE d.slug=?1 AND d.status='active' AND a.status='active'", [&link.slug], |row| Ok((row.get(0)?, row.get(1)?))).map_err(CatalogError::from)?;
            if keys.is_empty()
                || !keys.iter().any(|(_, key)| {
                    open_link_envelope(
                        key,
                        &storage_id,
                        &link.role,
                        &link.hash,
                        &link.sealed,
                    )
                    .is_ok()
                })
            {
                return Err(CatalogError::Invalid(
                    "sealed link authentication failed".into(),
                ));
            }
            let key_id = envelope_key_id(&link.sealed)?;
            let created_at = link_time(&link.since)?;
            let expires_at = if link.until.is_empty() { None } else { Some(link_time(&link.until)?) };
            if expires_at.is_some_and(|expires_at| expires_at < created_at) { return Err(CatalogError::Invalid("link expiry precedes creation".into())); }
            tx.execute("INSERT INTO links(document_id,id,role,token_hash,sealed_token,sealing_key_id,label,budget,created_at,expires_at)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
                ON CONFLICT(document_id,role) DO UPDATE SET token_hash=excluded.token_hash,sealed_token=excluded.sealed_token,
                label=excluded.label,budget=excluded.budget,expires_at=excluded.expires_at,sealing_key_id=excluded.sealing_key_id,
                credential_generation=credential_generation+
                    CASE WHEN token_hash<>excluded.token_hash THEN 1 ELSE 0 END",
                params![document_id, format!("{}-{}", link.role, hex::encode(crate::auth::random_bytes(8))), link.role, link.hash, link.sealed, key_id, link.label, link.budget, created_at, expires_at]).map_err(CatalogError::from)?;
            bump_document_updated_at(tx, &document_id)?;
            Ok(link.clone())
        })
    }

    pub fn drop_link(&self, slug: &str, role: &str) -> CatalogResult<bool> {
        self.immediate(|tx| {
            let document_id: String = tx
                .query_row("SELECT id FROM documents WHERE slug=?1", [slug], |row| {
                    row.get(0)
                })
                .map_err(CatalogError::from)?;
            let n = tx
                .execute(
                    "DELETE FROM links WHERE document_id=?1 AND role=?2",
                    params![document_id, role],
                )
                .map_err(CatalogError::from)?;
            if n == 1 {
                bump_document_updated_at(tx, &document_id)?;
            }
            Ok(n == 1)
        })
    }

    pub fn links(&self, slug: &str) -> CatalogResult<Vec<Link>> {
        self.with_connection(|c| {
            let mut s = c.prepare("SELECT d.slug,l.role,l.token_hash,l.sealed_token,l.label,l.budget,l.created_at,l.expires_at FROM links l JOIN documents d ON d.id=l.document_id JOIN accounts a ON a.id=d.owner_id WHERE d.slug=?1 AND d.status='active' AND a.status='active' ORDER BY l.role").map_err(CatalogError::from)?;
            let mut rows = s.query([slug]).map_err(CatalogError::from)?;
            let mut out = Vec::new();
            while let Some(r) = rows.next().map_err(CatalogError::from)? {
                out.push(Link { slug:r.get(0).map_err(CatalogError::from)?,role:r.get(1).map_err(CatalogError::from)?,hash:r.get(2).map_err(CatalogError::from)?,sealed:r.get(3).map_err(CatalogError::from)?,label:r.get(4).map_err(CatalogError::from)?,budget:r.get(5).map_err(CatalogError::from)?,since:crate::util::format_unix_millis(r.get::<_, i64>(6).map_err(CatalogError::from)?),until:r.get::<_, Option<i64>>(7).map_err(CatalogError::from)?.map_or_else(String::new, crate::util::format_unix_millis) });
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
            let (document_id,link_id,generation): (String,String,i64) = tx.query_row("SELECT d.id,l.id,l.credential_generation FROM documents d JOIN accounts a ON a.id=d.owner_id JOIN links l ON l.document_id=d.id WHERE d.slug=?1 AND d.status='active' AND a.status='active' AND l.token_hash=?2 AND (l.expires_at IS NULL OR l.expires_at>?3)", params![guest.slug,guest.link_hash,unix_millis()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(CatalogError::from)?.ok_or(CatalogError::NotFound)?;
            let payload: String = tx.query_row("SELECT bookmarks_json FROM accounts WHERE id=?1", [&guest.account_id], |r| r.get(0)).map_err(CatalogError::from)?;
            let mut json: serde_json::Value = serde_json::from_str(&payload).map_err(|e| CatalogError::Invalid(format!("invalid bookmarks: {e}")))?;
            if json.get("version").and_then(serde_json::Value::as_i64) != Some(1) {
                return Err(CatalogError::Invalid("bookmarks_json has unsupported version".into()));
            }
            let items = json.get_mut("items").and_then(serde_json::Value::as_array_mut).ok_or_else(|| CatalogError::Invalid("bookmarks_json has invalid shape".into()))?;
            if items.len() > 1_000 {
                return Err(CatalogError::Invalid("bookmarks_json has too many items".into()));
            }
            items.retain(|item| item.get("document_id").and_then(serde_json::Value::as_str) != Some(document_id.as_str()));
            if items.len() >= 1_000 { return Err(CatalogError::refused(CatalogRefusal::Other,"bookmark limit exceeded")); }
            items.push(serde_json::json!({"document_id":document_id,"link_id":link_id,"credential_generation":generation,"pinned_at":guest.since}));
            tx.execute("UPDATE accounts SET bookmarks_json=?1 WHERE id=?2", params![serde_json::to_string(&json).map_err(|e| CatalogError::Invalid(e.to_string()))?,guest.account_id]).map_err(CatalogError::from)?;
            Ok(guest.clone())
        })
    }

    /// Resolve one account's live bookmark to the link that created it.  The
    /// v2 catalogue deliberately has no reverse guest index: callers ask for
    /// their own bookmark payload and this bounded join supplies the role for
    /// that account's listing row without scanning other accounts.
    pub fn bookmark_link(&self, slug: &str, account_id: &str) -> CatalogResult<Option<String>> {
        if slug.is_empty() || account_id.is_empty() {
            return Ok(None);
        }
        self.with_connection(|connection| {
            let document_id: Option<String> = connection
                .query_row(
                    "SELECT id FROM documents WHERE slug=?1 AND status='active'",
                    [slug],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?;
            let Some(document_id) = document_id else {
                return Ok(None);
            };
            let payload: String = connection
                .query_row(
                    "SELECT bookmarks_json FROM accounts WHERE id=?1 AND status='active'",
                    [account_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?
                .unwrap_or_else(|| r#"{"version":1,"items":[]}"#.into());
            let bookmarks: serde_json::Value = serde_json::from_str(&payload)
                .map_err(|error| CatalogError::Invalid(format!("invalid bookmarks: {error}")))?;
            if bookmarks.get("version").and_then(serde_json::Value::as_i64) != Some(1) {
                return Err(CatalogError::Invalid(
                    "bookmarks_json has unsupported version".into(),
                ));
            }
            let Some(items) = bookmarks.get("items").and_then(serde_json::Value::as_array) else {
                return Err(CatalogError::Invalid(
                    "bookmarks_json has invalid shape".into(),
                ));
            };
            if items.len() > 1_000 {
                return Err(CatalogError::Invalid(
                    "bookmarks_json has too many items".into(),
                ));
            }
            let now = unix_millis();
            for item in items {
                if item.get("document_id").and_then(serde_json::Value::as_str)
                    != Some(document_id.as_str())
                {
                    continue;
                }
                let Some(link_id) = item.get("link_id").and_then(serde_json::Value::as_str) else {
                    continue;
                };
                let Some(generation) = item
                    .get("credential_generation")
                    .and_then(serde_json::Value::as_i64)
                else {
                    continue;
                };
                return connection
                    .query_row(
                        "SELECT token_hash FROM links
                         WHERE id=?1 AND document_id=?2 AND credential_generation=?3
                           AND (expires_at IS NULL OR expires_at>?4)",
                        params![link_id, document_id, generation, now],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(CatalogError::from);
            }
            Ok(None)
        })
    }

    pub fn guests(&self, slug: &str, limit: u32) -> CatalogResult<Vec<Guest>> {
        let _ = (slug, limit);
        // v2 bookmarks belong to each account and are never an access index;
        // there is intentionally no durable guest table to enumerate.
        Ok(Vec::new())
    }

    pub fn unpin_guest(
        &self,
        slug: &str,
        account_id: &str,
        link_hash: &str,
    ) -> CatalogResult<bool> {
        self.immediate(|tx| {
            let document_id: Option<String> = tx
                .query_row("SELECT id FROM documents WHERE slug=?1", [slug], |r| {
                    r.get(0)
                })
                .optional()
                .map_err(CatalogError::from)?;
            let Some(document_id) = document_id else {
                return Ok(false);
            };
            let payload: String = tx
                .query_row(
                    "SELECT bookmarks_json FROM accounts WHERE id=?1",
                    [account_id],
                    |r| r.get(0),
                )
                .map_err(CatalogError::from)?;
            let mut json: serde_json::Value =
                serde_json::from_str(&payload).map_err(|e| CatalogError::Invalid(e.to_string()))?;
            if json.get("version").and_then(serde_json::Value::as_i64) != Some(1) {
                return Err(CatalogError::Invalid(
                    "bookmarks_json has unsupported version".into(),
                ));
            }
            let items = json
                .get_mut("items")
                .and_then(serde_json::Value::as_array_mut)
                .ok_or_else(|| CatalogError::Invalid("bookmarks_json has invalid shape".into()))?;
            if items.len() > 1_000 {
                return Err(CatalogError::Invalid(
                    "bookmarks_json has too many items".into(),
                ));
            }
            let before = items.len();
            let link_id: Option<String> = tx
                .query_row(
                    "SELECT id FROM links WHERE document_id=?1 AND token_hash=?2",
                    params![document_id, link_hash],
                    |r| r.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?;
            items.retain(|item| {
                item.get("link_id").and_then(serde_json::Value::as_str) != link_id.as_deref()
            });
            if items.len() == before {
                return Ok(false);
            }
            tx.execute(
                "UPDATE accounts SET bookmarks_json=?1 WHERE id=?2",
                params![
                    serde_json::to_string(&json)
                        .map_err(|e| CatalogError::Invalid(e.to_string()))?,
                    account_id
                ],
            )
            .map_err(CatalogError::from)?;
            Ok(true)
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
    /// Each visibility source is queried in a bounded page and the pages are
    /// merged in memory.  Bookmarks are read from the requesting account's
    /// JSON only; they are never turned into a cross-account guest index.
    pub fn visible_documents_with_examples(
        &self,
        account_id: Option<&str>,
        owner_key: Option<&str>,
        cursor: Option<(&str, &str)>,
        limit: u32,
        include_examples: bool,
    ) -> CatalogResult<Vec<Document>> {
        let limit = limit.clamp(1, 200);
        let cursor_time = cursor
            .map(|value| {
                crate::util::parse_timestamp_millis(value.0).ok_or_else(|| {
                    CatalogError::Invalid("visibility cursor has an invalid timestamp".into())
                })
            })
            .transpose()?;
        let cursor_slug = cursor.map(|value| value.1);
        let account = account_id.unwrap_or("");
        let _ = owner_key;
        let (documents, stale_bookmarks) = self.with_connection(|connection| {
            let page_limit = i64::from(limit);
            let mut documents = Vec::new();
            let account_active = if account.is_empty() {
                false
            } else {
                connection
                    .query_row(
                        "SELECT status='active' FROM accounts WHERE id=?1",
                        [account],
                        |row| row.get::<_, bool>(0),
                    )
                    .optional()
                    .map_err(CatalogError::from)?
                    .unwrap_or(false)
            };
            let mut page = |sql: &str, params: Vec<&dyn rusqlite::ToSql>| -> CatalogResult<()> {
                documents.extend(visibility_page(connection, sql, &params)?);
                Ok(())
            };
            if account_active {
                page(
                    "SELECT d.slug,d.id,d.title,d.created_at,d.published_at,d.updated_at,
                            d.ownership_mode,d.owner_id,d.status,d.stored_bytes,d.reserved_bytes,
                            d.source_format,d.main_path
                     FROM documents d JOIN accounts a ON a.id=d.owner_id AND a.status='active'
                     WHERE d.status='active' AND d.owner_id=?1
                       AND (?2 IS NULL OR d.updated_at<?2 OR (d.updated_at=?2 AND d.slug<?3))
                     ORDER BY d.updated_at DESC,d.slug DESC LIMIT ?4",
                    vec![&account, &cursor_time, &cursor_slug, &page_limit],
                )?;
                page(
                    "SELECT d.slug,d.id,d.title,d.created_at,d.published_at,d.updated_at,
                            d.ownership_mode,d.owner_id,d.status,d.stored_bytes,d.reserved_bytes,
                            d.source_format,d.main_path
                     FROM grants g JOIN documents d ON d.id=g.document_id
                       JOIN accounts a ON a.id=d.owner_id AND a.status='active'
                     WHERE d.status='active' AND g.account_id=?1
                       AND (?2 IS NULL OR d.updated_at<?2 OR (d.updated_at=?2 AND d.slug<?3))
                     ORDER BY d.updated_at DESC,d.slug DESC LIMIT ?4",
                    vec![&account, &cursor_time, &cursor_slug, &page_limit],
                )?;
            }
            page(
                "SELECT d.slug,d.id,d.title,d.created_at,d.published_at,d.updated_at,
                        d.ownership_mode,d.owner_id,d.status,d.stored_bytes,d.reserved_bytes,
                        d.source_format,d.main_path
                 FROM documents d JOIN accounts a ON a.id=d.owner_id AND a.status='active'
                 WHERE d.status='active' AND d.ownership_mode='open'
                   AND (?1 IS NULL OR d.updated_at<?1 OR (d.updated_at=?1 AND d.slug<?2))
                 ORDER BY d.updated_at DESC,d.slug DESC LIMIT ?3",
                vec![&cursor_time, &cursor_slug, &page_limit],
            )?;
            if include_examples {
                page(
                    "SELECT d.slug,d.id,d.title,d.created_at,d.published_at,d.updated_at,
                            d.ownership_mode,d.owner_id,d.status,d.stored_bytes,d.reserved_bytes,
                            d.source_format,d.main_path
                     FROM documents d JOIN accounts a ON a.id=d.owner_id AND a.status='active'
                     WHERE d.status='active' AND d.ownership_mode='example'
                       AND (?1 IS NULL OR d.updated_at<?1 OR (d.updated_at=?1 AND d.slug<?2))
                     ORDER BY d.updated_at DESC,d.slug DESC LIMIT ?3",
                    vec![&cursor_time, &cursor_slug, &page_limit],
                )?;
            }

            let mut stale = Vec::new();
            if account_active {
                let payload: Option<String> = connection
                    .query_row(
                        "SELECT bookmarks_json FROM accounts WHERE id=?1 AND status='active'",
                        [account],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(CatalogError::from)?;
                if let Some(payload) = payload {
                    let mut bookmarks: serde_json::Value = serde_json::from_str(&payload)
                        .map_err(|e| CatalogError::Invalid(format!("invalid bookmarks: {e}")))?;
                    if bookmarks.get("version").and_then(serde_json::Value::as_i64) != Some(1) {
                        return Err(CatalogError::Invalid("bookmarks_json has unsupported version".into()));
                    }
                    let items = bookmarks
                        .get_mut("items")
                        .and_then(serde_json::Value::as_array_mut)
                        .ok_or_else(|| CatalogError::Invalid("bookmarks_json has invalid shape".into()))?;
                    if items.len() > 1_000 {
                        return Err(CatalogError::Invalid("bookmarks_json has too many items".into()));
                    }
                    let mut retained = Vec::with_capacity(items.len());
                    for item in items.iter() {
                        let Some(document_id) = item.get("document_id").and_then(serde_json::Value::as_str) else { stale.push(item.clone()); continue; };
                        let Some(link_id) = item.get("link_id").and_then(serde_json::Value::as_str) else { stale.push(item.clone()); continue; };
                        let Some(generation) = item.get("credential_generation").and_then(serde_json::Value::as_i64) else { stale.push(item.clone()); continue; };
                        let live = connection
                            .query_row(
                                "SELECT d.slug,d.id,d.title,d.created_at,d.published_at,d.updated_at,
                                        d.ownership_mode,d.owner_id,d.status,d.stored_bytes,d.reserved_bytes,
                                        d.source_format,d.main_path
                                 FROM links l JOIN documents d ON d.id=l.document_id
                                   JOIN accounts a ON a.id=d.owner_id AND a.status='active'
                                 WHERE d.id=?1 AND l.id=?2 AND l.credential_generation=?3
                                   AND d.status='active'
                                   AND (l.expires_at IS NULL OR l.expires_at>?4)",
                                params![document_id, link_id, generation, unix_millis()],
                                visibility_document,
                            )
                            .optional()
                            .map_err(CatalogError::from)?;
                        if let Some(document) = live {
                            if cursor_time.is_none_or(|at| {
                                let updated = crate::util::parse_timestamp_millis(&document.updated_at)
                                    .unwrap_or_default();
                                updated < at || (updated == at && cursor_slug.is_some_and(|slug| document.slug.as_str() < slug))
                            }) {
                                documents.push(document);
                            }
                            retained.push(item.clone());
                        } else {
                            stale.push(item.clone());
                        }
                    }
                    *items = retained;
                }
            }
            Ok((documents, stale))
        })?;

        if !stale_bookmarks.is_empty() {
            let account = account.to_string();
            self.immediate(|tx| {
                let payload: String = tx
                    .query_row(
                        "SELECT bookmarks_json FROM accounts WHERE id=?1",
                        [&account],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                let mut json: serde_json::Value = serde_json::from_str(&payload)
                    .map_err(|e| CatalogError::Invalid(format!("invalid bookmarks: {e}")))?;
                let items = json
                    .get_mut("items")
                    .and_then(serde_json::Value::as_array_mut)
                    .ok_or_else(|| {
                        CatalogError::Invalid("bookmarks_json has invalid shape".into())
                    })?;
                items.retain(|item| !stale_bookmarks.iter().any(|stale| stale == item));
                tx.execute(
                    "UPDATE accounts SET bookmarks_json=?1 WHERE id=?2",
                    params![
                        serde_json::to_string(&json)
                            .map_err(|e| CatalogError::Invalid(e.to_string()))?,
                        account
                    ],
                )
                .map_err(CatalogError::from)?;
                Ok(())
            })?;
        }

        let mut by_slug = std::collections::HashMap::new();
        for document in documents {
            by_slug.entry(document.slug.clone()).or_insert(document);
        }
        let mut documents: Vec<_> = by_slug.into_values().collect();
        documents.sort_by(|left, right| {
            crate::util::parse_timestamp_millis(&right.updated_at)
                .unwrap_or_default()
                .cmp(&crate::util::parse_timestamp_millis(&left.updated_at).unwrap_or_default())
                .then_with(|| right.slug.cmp(&left.slug))
        });
        documents.truncate(limit as usize);
        Ok(documents)
    }
}

#[cfg(test)]
mod envelope_tests {
    use super::*;

    #[test]
    fn sealed_links_require_a_key_identified_envelope() {
        let key = [7; 32];
        let token = "opaque-read-token";
        let digest = hex::encode(sha2::Sha256::digest(token.as_bytes()));
        let current =
            seal_link_envelope(&key, &link_key_id(&key), "doc", "reader", &digest, token).unwrap();
        assert_eq!(
            open_link_envelope(&key, "doc", "reader", &digest, &current).unwrap(),
            token
        );
        // The nonce and ciphertext are still authentic; only the obsolete
        // header omitting the sealing-key identity differs.
        let mut obsolete = b"KLINK1".to_vec();
        obsolete.extend_from_slice(&current[22..]);
        assert!(open_link_envelope(&key, "doc", "reader", &digest, &obsolete).is_err());
        assert!(envelope_key_id(&obsolete).is_err());
        let catalog = Catalog::open_in_memory().unwrap();
        catalog.set_link_sealing_key(&key).unwrap();
        assert_eq!(
            catalog
                .open_link_key("doc", "reader", &digest, &current)
                .unwrap(),
            token
        );
        assert!(catalog
            .open_link_key("doc", "reader", &digest, &obsolete)
            .is_err());
    }
}
