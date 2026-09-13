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
    crate::util::parse_timestamp(value)
        .map(|value| value.saturating_mul(1_000))
        .ok_or_else(|| {
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
    pub(super) fn mutation_authorized_in_tx(
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

    pub fn update_document_access(
        &self,
        _document: &Document,
        _grants: &[Grant],
        _links: &[Link],
        _guests: &[Guest],
        _actor: Option<(&str, &str, &str)>,
    ) -> CatalogResult<Document> {
        Err(CatalogError::Invalid(
            "bulk access replacement was removed; mutate v2 grants and links individually".into(),
        ))
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
                    "SELECT d.slug,g.role,g.account_id,CAST(g.created_at AS TEXT) FROM grants g JOIN documents d ON d.id=g.document_id
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
        if link.budget.is_some_and(|n| n < 0) {
            return Err(CatalogError::Invalid(
                "invalid link expiry or budget".into(),
            ));
        }
        self.immediate(|tx| {
            let key_id = envelope_key_id(&link.sealed);
            let document_id: String = tx.query_row("SELECT d.id FROM documents d JOIN accounts a ON a.id=d.owner_id
                WHERE d.slug=?1 AND d.status='active' AND a.status='active'", [&link.slug], |row| row.get(0)).map_err(CatalogError::from)?;
            let created_at = link_time(&link.since)?;
            let expires_at = if link.until.is_empty() { None } else { Some(link_time(&link.until)?) };
            if expires_at.is_some_and(|expires_at| expires_at < created_at) { return Err(CatalogError::Invalid("link expiry precedes creation".into())); }
            tx.execute("INSERT INTO links(document_id,id,role,token_hash,sealed_token,sealing_key_id,label,budget,created_at,expires_at)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
                ON CONFLICT(document_id,role) DO UPDATE SET
                credential_generation=links.credential_generation+CASE WHEN links.token_hash<>excluded.token_hash THEN 1 ELSE 0 END,
                token_hash=excluded.token_hash,sealed_token=excluded.sealed_token,
                label=excluded.label,budget=excluded.budget,expires_at=excluded.expires_at,sealing_key_id=excluded.sealing_key_id",
                params![document_id, format!("{}-{}", link.role, hex::encode(crate::auth::random_bytes(8))), link.role, link.hash, link.sealed, key_id, link.label, link.budget, created_at, expires_at]).map_err(CatalogError::from)?;
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
            Ok(n == 1)
        })
    }

    pub fn links(&self, slug: &str) -> CatalogResult<Vec<Link>> {
        self.with_connection(|c| {
            let mut s = c.prepare("SELECT d.slug,l.role,l.token_hash,l.sealed_token,l.label,l.budget,CAST(l.created_at AS TEXT),COALESCE(CAST(l.expires_at AS TEXT),'') FROM links l JOIN documents d ON d.id=l.document_id JOIN accounts a ON a.id=d.owner_id WHERE d.slug=?1 AND d.status='active' AND a.status='active' ORDER BY l.role").map_err(CatalogError::from)?;
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
            let (document_id,link_id,generation): (String,String,i64) = tx.query_row("SELECT d.id,l.id,l.credential_generation FROM documents d JOIN accounts a ON a.id=d.owner_id JOIN links l ON l.document_id=d.id WHERE d.slug=?1 AND d.status='active' AND a.status='active' AND l.token_hash=?2 AND (l.expires_at IS NULL OR l.expires_at>?3)", params![guest.slug,guest.link_hash,unix_millis()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(CatalogError::from)?.ok_or(CatalogError::NotFound)?;
            let payload: String = tx.query_row("SELECT bookmarks_json FROM accounts WHERE id=?1", [&guest.account_id], |r| r.get(0)).map_err(CatalogError::from)?;
            let mut json: serde_json::Value = serde_json::from_str(&payload).map_err(|e| CatalogError::Invalid(format!("invalid bookmarks: {e}")))?;
            let items = json.get_mut("items").and_then(serde_json::Value::as_array_mut).ok_or_else(|| CatalogError::Invalid("bookmarks_json has invalid shape".into()))?;
            items.retain(|item| item.get("document_id").and_then(serde_json::Value::as_str) != Some(document_id.as_str()));
            if items.len() >= 1_000 { return Err(CatalogError::refused(CatalogRefusal::Other,"bookmark limit exceeded")); }
            items.push(serde_json::json!({"document_id":document_id,"link_id":link_id,"credential_generation":generation,"pinned_at":guest.since}));
            tx.execute("UPDATE accounts SET bookmarks_json=?1 WHERE id=?2", params![serde_json::to_string(&json).map_err(|e| CatalogError::Invalid(e.to_string()))?,guest.account_id]).map_err(CatalogError::from)?;
            Ok(guest.clone())
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
            let items = json
                .get_mut("items")
                .and_then(serde_json::Value::as_array_mut)
                .ok_or_else(|| CatalogError::Invalid("bookmarks_json has invalid shape".into()))?;
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
        return self.with_connection(|connection| {
            let cursor_time = cursor.and_then(|value| value.0.parse::<i64>().ok());
            let cursor_slug = cursor.map(|value| value.1);
            let account = account_id.unwrap_or("");
            let mut statement = connection
                .prepare(
                    "SELECT d.slug,d.id,d.title,d.created_at,d.published_at,d.updated_at,
                        d.ownership_mode,d.owner_id,d.status,d.stored_bytes,d.reserved_bytes,
                        d.source_format,d.main_path
                 FROM documents d JOIN accounts owner ON owner.id=d.owner_id AND owner.status='active'
                 WHERE d.status='active'
                   AND (?2 IS NULL OR d.updated_at<?2 OR (d.updated_at=?2 AND d.slug<?3))
                   AND (d.ownership_mode='open'
                        OR (?4=1 AND d.ownership_mode='example')
                        OR (d.owner_id=?1)
                        OR EXISTS(SELECT 1 FROM grants g WHERE g.document_id=d.id
                                  AND g.account_id=?1))
                 ORDER BY d.updated_at DESC,d.slug DESC LIMIT ?5",
                )
                .map_err(CatalogError::from)?;
            let mut rows = statement
                .query(params![
                    account,
                    cursor_time,
                    cursor_slug,
                    include_examples,
                    limit as i64
                ])
                .map_err(CatalogError::from)?;
            let mut documents = Vec::new();
            while let Some(row) = rows.next().map_err(CatalogError::from)? {
                documents.push(Document {
                    slug: row.get(0)?,
                    storage_id: row.get(1)?,
                    title: row.get(2)?,
                    sha: String::new(),
                    created_at: row.get::<_, i64>(3)?.to_string(),
                    published_at: row
                        .get::<_, Option<i64>>(4)?
                        .map_or_else(String::new, |value| value.to_string()),
                    updated_at: row.get::<_, i64>(5)?.to_string(),
                    example: matches!(row.get::<_, String>(6)?.as_str(), "example"),
                    owner_key: String::new(),
                    owner_id: row.get(7)?,
                    status: row.get(8)?,
                    size: row.get(9)?,
                    counted_size: row
                        .get::<_, i64>(9)?
                        .checked_add(row.get(10)?)
                        .ok_or_else(|| rusqlite::Error::InvalidQuery)?,
                    maintenance_reserved: 0,
                    comment_seq: 0,
                    last_auto_checkpoint_at: 0,
                    pending_publication: None,
                    last_publication_id: String::new(),
                    source_format: row.get(11)?,
                    main: row.get(12)?,
                });
            }
            Ok(documents)
        });
    }
}
