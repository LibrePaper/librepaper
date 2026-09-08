//! Accounts: the row an identity becomes, its sessions, and erasing one.

use super::*;

impl Catalog {
    /// Insert or refresh a profile.  Lifecycle state and session generation
    /// are never overwritten by a profile refresh.
    pub fn upsert_account(&self, profile: &Account) -> CatalogResult<Account> {
        if profile.id.is_empty() || profile.session_generation.is_empty() {
            return Err(CatalogError::Invalid(
                "account id and generation are required".into(),
            ));
        }
        self.immediate(|tx| {
            let existing: Option<(String, String)> = tx
                .query_row(
                    "SELECT status, session_generation FROM accounts WHERE id = ?1",
                    [&profile.id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?;
            if let Some((status, generation)) = existing {
                if status != "active" {
                    return Err(CatalogError::Conflict(format!(
                        "account is {status} and cannot sign in"
                    )));
                }
                tx.execute(
                    "UPDATE accounts SET provider = ?2, handle = ?3, name = ?4,
                     email = ?5, last_seen = CASE
                       WHEN substr(last_seen, 1, 10) < substr(?6, 1, 10) THEN ?6
                       ELSE last_seen END
                     WHERE id = ?1",
                    params![
                        profile.id,
                        profile.provider,
                        profile.handle,
                        profile.name,
                        profile.email,
                        profile.last_seen
                    ],
                )
                .map_err(CatalogError::from)?;
                return self.account_in_tx(tx, &profile.id, Some(generation));
            }
            tx.execute(
                "INSERT INTO accounts
                 (id, provider, handle, name, email, first_seen, last_seen, plan,
                  status, session_generation, erasure_cursor)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'active', ?9, NULL)",
                params![
                    profile.id,
                    profile.provider,
                    profile.handle,
                    profile.name,
                    profile.email,
                    profile.first_seen,
                    profile.last_seen,
                    profile.plan,
                    profile.session_generation
                ],
            )
            .map_err(CatalogError::from)?;
            for position in 0..4 {
                tx.execute(
                    "INSERT INTO account_examples (account_id, position, slug) VALUES (?1, ?2, ?3)",
                    params![
                        profile.id,
                        position,
                        format!("starter-{}", crate::util::new_id())
                    ],
                )
                .map_err(CatalogError::from)?;
            }
            self.account_in_tx(tx, &profile.id, None)
        })
    }

    pub fn account(&self, id: &str) -> CatalogResult<Option<Account>> {
        self.with_connection(|connection| {
            Self::account_on(connection, id).map_err(CatalogError::from)
        })
    }

    /// The durable remaining work for this account's first sign-in.
    pub fn pending_account_examples(&self, id: &str) -> CatalogResult<Vec<(usize, String)>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT position, slug FROM account_examples WHERE account_id = ?1 AND completed = 0 ORDER BY position",
            )?;
            let rows = statement.query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect::<Result<Vec<_>, _>>().map_err(CatalogError::from)
        })
    }

    pub fn complete_account_example(&self, id: &str, position: usize) -> CatalogResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE account_examples SET completed = 1 WHERE account_id = ?1 AND position = ?2",
                params![id, position],
            )?;
            Ok(())
        })
    }

    pub(super) fn account_in_tx(
        &self,
        tx: &Transaction<'_>,
        id: &str,
        _existing_generation: Option<String>,
    ) -> CatalogResult<Account> {
        tx.query_row(
            "SELECT id, provider, handle, name, email, first_seen, last_seen, plan,
                    status, session_generation, erasure_cursor
             FROM accounts WHERE id = ?1",
            [id],
            Self::read_account,
        )
        .map_err(CatalogError::from)
    }

    pub(super) fn account_on(
        connection: &Connection,
        id: &str,
    ) -> rusqlite::Result<Option<Account>> {
        connection
            .query_row(
                "SELECT id, provider, handle, name, email, first_seen, last_seen, plan,
                        status, session_generation, erasure_cursor
                 FROM accounts WHERE id = ?1",
                [id],
                Self::read_account,
            )
            .optional()
    }

    pub(super) fn read_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<Account> {
        Ok(Account {
            id: row.get(0)?,
            provider: row.get(1)?,
            handle: row.get(2)?,
            name: row.get(3)?,
            email: row.get(4)?,
            first_seen: row.get(5)?,
            last_seen: row.get(6)?,
            plan: row.get(7)?,
            status: row.get(8)?,
            session_generation: row.get(9)?,
            erasure_cursor: row.get(10)?,
        })
    }

    /// Change a session generation in the same authoritative transaction that
    /// marks revocation.  Returns the new generation for cookie invalidation.
    pub fn revoke_sessions(&self, id: &str, new_generation: &str) -> CatalogResult<String> {
        if new_generation.is_empty() {
            return Err(CatalogError::Invalid("session generation is empty".into()));
        }
        self.immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE accounts SET session_generation = ?2 WHERE id = ?1 AND status = 'active'",
                    params![id, new_generation],
                )
                .map_err(CatalogError::from)?;
            if changed == 0 {
                return Err(CatalogError::NotFound);
            }
            Ok(new_generation.to_owned())
        })
    }

    /// Mark an account erasing and revoke all sessions atomically.
    pub fn begin_erasure(&self, id: &str, new_generation: &str) -> CatalogResult<()> {
        if new_generation.is_empty() {
            return Err(CatalogError::Invalid("session generation is empty".into()));
        }
        self.immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE accounts SET status = 'erasing', session_generation = ?2,
                     erasure_cursor = NULL WHERE id = ?1 AND status = 'active'",
                    params![id, new_generation],
                )
                .map_err(CatalogError::from)?;
            if changed == 0 {
                return Err(CatalogError::NotFound);
            }
            // Withdrawal is part of the lifecycle transition.  The erasure
            // worker still owns physical reclamation and must retain these
            // rows (and their reservations) until object cleanup succeeds.
            // Withdraw every owned lifecycle row, including a creation whose
            // publication receipt is still prepared.  A prepared receipt is
            // an externally visible reservation even though its document is
            // not listed; leaving it behind would let a restart resurrect
            // content for an account that has already been erased.
            tx.execute(
                "UPDATE documents SET status='deleting', pending_publication=NULL
                 WHERE owner_id=?1 AND status IN ('active','creating')",
                [id],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE catalog_operations SET status='aborted',
                 result='account erasure withdrew the publication'
                 WHERE status='prepared' AND storage_id IN
                   (SELECT storage_id FROM documents WHERE owner_id=?1 AND status='deleting')",
                [id],
            )
            .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    /// Record qualifying authenticated activity.  Activity is intentionally
    /// monotonic: an imported older timestamp cannot make an account look
    /// recently active, and repeated requests on one UTC day are a no-op.
    pub fn record_activity(&self, id: &str, at: &str) -> CatalogResult<()> {
        if id.is_empty() || at.len() < 10 {
            return Err(CatalogError::Invalid("invalid account activity".into()));
        }
        self.immediate(|tx| {
            let active: Option<String> = tx
                .query_row("SELECT status FROM accounts WHERE id = ?1", [id], |r| r.get(0))
                .optional()
                .map_err(CatalogError::from)?;
            if active.as_deref() != Some("active") {
                return Err(CatalogError::Conflict("account is not active".into()));
            }
            tx.execute(
                "INSERT INTO account_activity(account_id, last_qualified_at)
                 VALUES (?1, ?2)
                 ON CONFLICT(account_id) DO UPDATE SET last_qualified_at =
                   CASE WHEN account_activity.last_qualified_at < excluded.last_qualified_at
                        THEN excluded.last_qualified_at ELSE account_activity.last_qualified_at END",
                params![id, at],
            )
            .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    /// Persist one bounded erasure cursor.  A worker may safely repeat a
    /// batch after a crash because the cursor update is in the same tx as the
    /// deletions performed by the caller through `with_erasure_batch`.
    pub fn erasure_batch(
        &self,
        id: &str,
        stage: &str,
        cursor: Option<&str>,
        updated_at: i64,
        limit: u32,
    ) -> CatalogResult<u32> {
        if stage.is_empty() || stage.len() > 64 || limit == 0 || limit > 1000 {
            return Err(CatalogError::Invalid("invalid erasure batch".into()));
        }
        self.immediate(|tx| {
            let status: String = tx
                .query_row("SELECT status FROM accounts WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            if status != "erasing" {
                return Err(CatalogError::Conflict("account is not erasing".into()));
            }
            tx.execute(
                "INSERT INTO erasure_batches(account_id, stage, cursor, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(account_id) DO UPDATE SET stage=excluded.stage,
                   cursor=excluded.cursor, updated_at=excluded.updated_at",
                params![id, stage, cursor, updated_at],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE accounts SET erasure_cursor = ?2 WHERE id = ?1",
                params![id, cursor],
            )
            .map_err(CatalogError::from)?;
            Ok(limit)
        })
    }

    /// Apply one resumable logical-erasure batch.  Physical document cleanup
    /// remains owned by `begin_delete`/`finish_delete`; this method handles
    /// account references in documents that belong to other users.
    pub fn erase_account_batch(
        &self,
        id: &str,
        stage: &str,
        cursor: Option<&str>,
        updated_at: i64,
        limit: u32,
    ) -> CatalogResult<u32> {
        if limit == 0 || limit > 1000 || stage.is_empty() {
            return Err(CatalogError::Invalid("invalid erasure batch".into()));
        }
        self.immediate(|tx| {
            let status: Option<String> = tx
                .query_row("SELECT status FROM accounts WHERE id=?1", [id], |r| r.get(0))
                .optional()
                .map_err(CatalogError::from)?;
            if status.as_deref() != Some("erasing") {
                return Err(CatalogError::Conflict("account is not erasing".into()));
            }
            // These tables deliberately use WITHOUT ROWID primary keys in
            // the catalogue contract.  Advance by immutable primary-key
            // tuples rather than SQLite's hidden rowid: the cursor remains
            // valid across vacuum/backup/restore and a crash can repeat only
            // an already committed keyset batch.
            let cursor_parts = cursor
                .map(|value| {
                    serde_json::from_str::<Vec<String>>(value).map_err(|_| {
                        CatalogError::Invalid("invalid erasure cursor".into())
                    })
                })
                .transpose()?;
            let n_and_cursor = match stage {
                "grants" => {
                    let (after_slug, after_role) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 2 {
                                return Err(CatalogError::Invalid("invalid grants cursor".into()));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", ""));
                    let mut rows = tx
                        .prepare(
                            "SELECT slug, role FROM grants
                             WHERE account_id=?1
                               AND (slug>?2 OR (slug=?2 AND role>?3))
                             ORDER BY slug, role LIMIT ?4",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(params![id, after_slug, after_role, i64::from(limit)], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let last = rows.last().cloned();
                    let has_rows = last.is_some();
                    for (slug, role) in rows.drain(..) {
                        tx.execute(
                            "DELETE FROM grants WHERE slug=?1 AND role=?2 AND account_id=?3",
                            params![slug, role, id],
                        )
                        .map_err(CatalogError::from)?;
                    }
                    (last.map(|(slug, role)| serde_json::json!([slug, role]).to_string()), has_rows)
                }
                "guests" => {
                    let (after_slug, after_link) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 2 {
                                return Err(CatalogError::Invalid("invalid guests cursor".into()));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", ""));
                    let mut rows = tx
                        .prepare(
                            "SELECT slug, link_hash FROM guests
                             WHERE account_id=?1
                               AND (slug>?2 OR (slug=?2 AND link_hash>?3))
                             ORDER BY slug, link_hash LIMIT ?4",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(params![id, after_slug, after_link, i64::from(limit)], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let last = rows.last().cloned();
                    let has_rows = last.is_some();
                    for (slug, link_hash) in rows.drain(..) {
                        tx.execute(
                            "DELETE FROM guests WHERE slug=?1 AND account_id=?2 AND link_hash=?3",
                            params![slug, id, link_hash],
                        )
                        .map_err(CatalogError::from)?;
                    }
                    (last.map(|(slug, link)| serde_json::json!([slug, link]).to_string()), has_rows)
                }
                "comments" => {
                    let (after_slug, after_id) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 2 {
                                return Err(CatalogError::Invalid("invalid comments cursor".into()));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", ""));
                    let mut rows = tx
                        .prepare(
                            "SELECT slug, id FROM comments
                             WHERE author=?1
                               AND (slug>?2 OR (slug=?2 AND id>?3))
                             ORDER BY slug, id LIMIT ?4",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(params![id, after_slug, after_id, i64::from(limit)], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let last = rows.last().cloned();
                    let has_rows = last.is_some();
                    for (slug, comment_id) in rows.drain(..) {
                        tx.execute(
                            "DELETE FROM comments WHERE slug=?1 AND id=?2 AND author=?3",
                            params![slug, comment_id, id],
                        )
                        .map_err(CatalogError::from)?;
                    }
                    (last.map(|(slug, comment_id)| serde_json::json!([slug, comment_id]).to_string()), has_rows)
                }
                "replies" => {
                    let (after_slug, after_comment, after_id) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 3 {
                                return Err(CatalogError::Invalid("invalid replies cursor".into()));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str(), parts[2].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", "", ""));
                    let mut rows = tx
                        .prepare(
                            "SELECT slug, comment_id, id FROM replies
                             WHERE author=?1
                               AND (slug>?2 OR (slug=?2 AND comment_id>?3)
                                    OR (slug=?2 AND comment_id=?3 AND id>?4))
                             ORDER BY slug, comment_id, id LIMIT ?5",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(
                            params![id, after_slug, after_comment, after_id, i64::from(limit)],
                            |row| {
                                Ok((
                                    row.get::<_, String>(0)?,
                                    row.get::<_, String>(1)?,
                                    row.get::<_, String>(2)?,
                                ))
                            },
                        )
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let last = rows.last().cloned();
                    let has_rows = last.is_some();
                    for (slug, comment_id, reply_id) in rows.drain(..) {
                        tx.execute(
                            "DELETE FROM replies
                             WHERE slug=?1 AND comment_id=?2 AND id=?3 AND author=?4",
                            params![slug, comment_id, reply_id, id],
                        )
                        .map_err(CatalogError::from)?;
                    }
                    (last.map(|(slug, comment, reply)| serde_json::json!([slug, comment, reply]).to_string()), has_rows)
                }
                "checkpoints" => {
                    let (after_slug, after_sha) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 2 {
                                return Err(CatalogError::Invalid("invalid checkpoints cursor".into()));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", ""));
                    let mut rows = tx
                        .prepare(
                            "SELECT slug, sha FROM checkpoints
                             WHERE by=?1
                               AND (slug>?2 OR (slug=?2 AND sha>?3))
                             ORDER BY slug, sha LIMIT ?4",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(params![id, after_slug, after_sha, i64::from(limit)], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let last = rows.last().cloned();
                    let has_rows = last.is_some();
                    for (slug, sha) in rows.drain(..) {
                        tx.execute(
                            "UPDATE checkpoints SET by='Deleted user'
                             WHERE slug=?1 AND sha=?2 AND by=?3",
                            params![slug, sha, id],
                        )
                        .map_err(CatalogError::from)?;
                    }
                    (last.map(|(slug, sha)| serde_json::json!([slug, sha]).to_string()), has_rows)
                }
                _ => return Err(CatalogError::Invalid("unknown erasure stage".into())),
            };
            let n = u32::from(n_and_cursor.1);
            let next_cursor = n_and_cursor.0;
            tx.execute(
                "INSERT INTO erasure_batches(account_id,stage,cursor,updated_at) VALUES(?1,?2,?3,?4)
                 ON CONFLICT(account_id) DO UPDATE SET stage=excluded.stage,cursor=excluded.cursor,updated_at=excluded.updated_at",
                params![id, stage, next_cursor, updated_at],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE accounts SET erasure_cursor=?2 WHERE id=?1",
                params![id, next_cursor],
            )
            .map_err(CatalogError::from)?;
            Ok(n)
        })
    }

    pub fn finish_erasure(&self, id: &str) -> CatalogResult<()> {
        self.immediate(|tx| {
            let status: Option<String> = tx
                .query_row("SELECT status FROM accounts WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .optional()
                .map_err(CatalogError::from)?;
            if status.as_deref() != Some("erasing") {
                return Err(CatalogError::NotFound);
            }
            let owned: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM documents WHERE owner_id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .map_err(CatalogError::from)?;
            if owned != 0 {
                return Err(CatalogError::Conflict("owned documents remain".into()));
            }
            let references: i64 = tx
                .query_row(
                    "SELECT
                       (SELECT COUNT(*) FROM grants WHERE account_id=?1) +
                       (SELECT COUNT(*) FROM guests WHERE account_id=?1) +
                       (SELECT COUNT(*) FROM comments WHERE author=?1) +
                       (SELECT COUNT(*) FROM replies WHERE author=?1) +
                       (SELECT COUNT(*) FROM checkpoints WHERE by=?1)",
                    [id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if references != 0 {
                return Err(CatalogError::Conflict("account attribution remains".into()));
            }
            tx.execute("DELETE FROM accounts WHERE id = ?1", [id])
                .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    /// Accounts awaiting the bounded erasure worker, ordered by immutable id.
    pub fn erasing_accounts(
        &self,
        after_id: Option<&str>,
        limit: u32,
    ) -> CatalogResult<Vec<String>> {
        let limit = i64::from(limit.clamp(1, 100));
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id FROM accounts WHERE status='erasing' AND (?1 IS NULL OR id>?1) ORDER BY id LIMIT ?2"
            ).map_err(CatalogError::from)?;
            let rows = statement.query_map(params![after_id,limit], |row| row.get(0)).map_err(CatalogError::from)?;
            rows.collect::<Result<_,_>>().map_err(CatalogError::from)
        })
    }

    pub fn erasure_stage(&self, id: &str) -> CatalogResult<Option<String>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT stage FROM erasure_batches WHERE account_id=?1",
                    [id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)
        })
    }

    pub fn erasure_progress(&self, id: &str) -> CatalogResult<Option<(String, Option<String>)>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT stage, cursor FROM erasure_batches WHERE account_id=?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)
        })
    }
}
