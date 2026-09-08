//! Comments and replies, and the two-phase accept of a suggestion.

use super::*;

impl Catalog {
    pub fn insert_comment(&self, comment: &Comment) -> CatalogResult<Comment> {
        if comment.slug.is_empty() || comment.id.is_empty() || comment.body.len() > 65_536 {
            return Err(CatalogError::Invalid("invalid comment".into()));
        }
        self.immediate(|tx| {
            let count:i64=tx.query_row("SELECT COUNT(*) FROM comments WHERE slug=?1",[&comment.slug],|r|r.get(0)).map_err(CatalogError::from)?;
            if count >= 500 { return Err(CatalogError::Conflict("comment limit reached".into())); }
            let seq: i64 = tx.query_row("SELECT comment_seq FROM documents WHERE slug=?1 AND status='active'",[&comment.slug],|r|r.get::<_, i64>(0)).optional().map_err(CatalogError::from)?.ok_or(CatalogError::NotFound)? + 1;
            tx.execute("UPDATE documents SET comment_seq=?2 WHERE slug=?1",params![comment.slug,seq]).map_err(CatalogError::from)?;
            tx.execute("INSERT INTO comments(slug,id,seq,motivation,body,creator,author,via,created,exact,prefix,suffix,position,region,source_path,source_exact,source_prefix,source_suffix,source_position,proposed,outcome,accept_request,revision,resolved,resolved_at,resolved_in)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26)",params![comment.slug,comment.id,seq,comment.motivation,comment.body,comment.creator,comment.author,comment.via,comment.created,comment.exact,comment.prefix,comment.suffix,comment.position,comment.region,comment.source_path,comment.source_exact,comment.source_prefix,comment.source_suffix,comment.source_position,comment.proposed,comment.outcome,comment.accept_request,comment.revision,comment.resolved as i64,comment.resolved_at,comment.resolved_in]).map_err(CatalogError::from)?;
            Self::comment_in_tx(tx,&comment.slug,&comment.id)
        })
    }

    /// Insert a comment with a durable request receipt.  The sequence is
    /// allocated from documents.comment_seq inside the same transaction and
    /// is never supplied by the caller, so deletion or a stale room snapshot
    /// cannot rewind it.  A retry with the same request id and digest returns
    /// the original row without allocating another sequence.
    pub fn insert_comment_request(
        &self,
        comment: &Comment,
        request_id: &str,
        request_digest: &str,
        created_at: i64,
    ) -> CatalogResult<Comment> {
        if request_id.is_empty() {
            return self.insert_comment(comment);
        }
        if request_id.len() > 128 || request_digest.is_empty() {
            return Err(CatalogError::Invalid(
                "invalid comment request receipt".into(),
            ));
        }
        self.immediate(|tx| {
            let (storage_id, current_seq): (String, i64) = tx
                .query_row(
                    "SELECT storage_id, comment_seq FROM documents
                     WHERE slug=?1 AND status='active'",
                    [&comment.slug],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            let existing: Option<Operation> = tx
                .query_row(
                    "SELECT storage_id,request_id,kind,request_digest,status,intent,result,created_at
                     FROM catalog_operations WHERE storage_id=?1 AND request_id=?2",
                    params![storage_id, request_id],
                    Self::read_operation,
                )
                .optional()
                .map_err(CatalogError::from)?;
            if let Some(operation) = existing {
                if operation.request_digest != request_digest {
                    return Err(CatalogError::Conflict(
                        "request id was reused with different content".into(),
                    ));
                }
                if operation.status != "committed" {
                    return Err(CatalogError::Conflict(
                        "comment request has an unresolved receipt".into(),
                    ));
                }
                return Self::comment_in_tx(tx, &comment.slug, &operation.result);
            }
            let seq = current_seq + 1;
            tx.execute(
                "INSERT INTO catalog_operations
                 (storage_id,request_id,kind,request_digest,status,intent,result,created_at)
                 VALUES(?1,?2,'comment',?3,'prepared',?4,'',?5)",
                params![
                    storage_id,
                    request_id,
                    request_digest,
                    format!("{{\"slug\":{:?},\"id\":{:?}}}", comment.slug, comment.id),
                    created_at
                ],
            )
            .map_err(CatalogError::from)?;
            let region = comment.region.clone();
            let changed = tx
                .execute(
                    "UPDATE documents SET comment_seq=?2 WHERE slug=?1 AND comment_seq=?3",
                    params![comment.slug, seq, current_seq],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::Conflict("comment sequence changed".into()));
            }
            tx.execute(
                "INSERT INTO comments
                 (slug,id,seq,motivation,body,creator,author,via,created,exact,prefix,suffix,
                  position,region,source_path,source_exact,source_prefix,source_suffix,
                  source_position,proposed,outcome,accept_request,revision,resolved,resolved_at,resolved_in)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,
                        ?20,?21,?22,?23,?24,?25,?26)",
                params![
                    comment.slug,
                    comment.id,
                    seq,
                    comment.motivation,
                    comment.body,
                    comment.creator,
                    comment.author,
                    comment.via,
                    comment.created,
                    comment.exact,
                    comment.prefix,
                    comment.suffix,
                    comment.position,
                    region,
                    comment.source_path,
                    comment.source_exact,
                    comment.source_prefix,
                    comment.source_suffix,
                    comment.source_position,
                    comment.proposed,
                    comment.outcome,
                    comment.accept_request,
                    comment.revision,
                    comment.resolved as i64,
                    comment.resolved_at,
                    comment.resolved_in,
                ],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE catalog_operations SET status='committed', result=?3
                 WHERE storage_id=?1 AND request_id=?2",
                params![storage_id, request_id, comment.id],
            )
            .map_err(CatalogError::from)?;
            Self::comment_in_tx(tx, &comment.slug, &comment.id)
        })
    }

    pub fn comments(
        &self,
        slug: &str,
        after_seq: Option<i64>,
        limit: u32,
    ) -> CatalogResult<Vec<Comment>> {
        let limit = i64::from(limit.clamp(1, 500));
        self.with_connection(|c| { let mut s=c.prepare("SELECT slug,id,seq,motivation,body,creator,author,via,created,exact,prefix,suffix,position,region,source_path,source_exact,source_prefix,source_suffix,source_position,proposed,outcome,accept_request,revision,resolved,resolved_at,resolved_in FROM comments WHERE slug=?1 AND (?2 IS NULL OR seq>?2) ORDER BY seq LIMIT ?3").map_err(CatalogError::from)?; let mut rows=s.query(params![slug,after_seq,limit]).map_err(CatalogError::from)?; let mut out=Vec::new(); while let Some(r)=rows.next().map_err(CatalogError::from)? { out.push(Self::read_comment(r).map_err(CatalogError::from)?); } Ok(out) })
    }

    pub fn resolve_comment(
        &self,
        slug: &str,
        id: &str,
        resolved: bool,
        at: Option<&str>,
        in_checkpoint: &str,
    ) -> CatalogResult<Comment> {
        self.immediate(|tx| { let n=tx.execute("UPDATE comments SET resolved=?3,resolved_at=?4,resolved_in=?5 WHERE slug=?1 AND id=?2",params![slug,id,resolved as i64,at,in_checkpoint]).map_err(CatalogError::from)?; if n!=1{return Err(CatalogError::NotFound)} Self::comment_in_tx(tx,slug,id) })
    }

    pub fn update_comment(&self, comment: &Comment) -> CatalogResult<Comment> {
        self.immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE comments SET motivation=?3,body=?4,creator=?5,author=?6,via=?7,
                     created=?8,exact=?9,prefix=?10,suffix=?11,position=?12,region=?13,
                     source_path=?14,source_exact=?15,source_prefix=?16,source_suffix=?17,
                     source_position=?18,proposed=?19,outcome=?20,accept_request=?21,
                     revision=?22,resolved=?23,resolved_at=?24,resolved_in=?25
                     WHERE slug=?1 AND id=?2 AND seq=?26",
                    params![
                        comment.slug,
                        comment.id,
                        comment.motivation,
                        comment.body,
                        comment.creator,
                        comment.author,
                        comment.via,
                        comment.created,
                        comment.exact,
                        comment.prefix,
                        comment.suffix,
                        comment.position,
                        comment.region,
                        comment.source_path,
                        comment.source_exact,
                        comment.source_prefix,
                        comment.source_suffix,
                        comment.source_position,
                        comment.proposed,
                        comment.outcome,
                        comment.accept_request,
                        comment.revision,
                        comment.resolved as i64,
                        comment.resolved_at,
                        comment.resolved_in,
                        comment.seq,
                    ],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::NotFound);
            }
            Self::comment_in_tx(tx, &comment.slug, &comment.id)
        })
    }

    /// Reserve a suggestion acceptance in SQLite before changing the live
    /// session.  The receipt is deliberately separate from the comment row:
    /// an interrupted accept can be resumed after a restart, while a
    /// committed receipt is an idempotent answer even when the room cache is
    /// cold.  `Some` means the request was already committed.
    pub fn begin_suggestion_accept(
        &self,
        slug: &str,
        comment_id: &str,
        request_id: &str,
        request_digest: &str,
        created_at: i64,
    ) -> CatalogResult<Option<Comment>> {
        if request_id.is_empty() || request_id.len() > 128 || request_digest.is_empty() {
            return Err(CatalogError::Invalid(
                "invalid suggestion acceptance receipt".into(),
            ));
        }
        self.immediate(|tx| {
            let storage_id: String = tx
                .query_row(
                    "SELECT storage_id FROM documents WHERE slug=?1 AND status='active'",
                    [slug],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            let existing: Option<Operation> = tx
                .query_row(
                    "SELECT storage_id,request_id,kind,request_digest,status,intent,result,created_at
                     FROM catalog_operations WHERE storage_id=?1 AND request_id=?2",
                    params![storage_id, request_id],
                    Self::read_operation,
                )
                .optional()
                .map_err(CatalogError::from)?;
            if let Some(operation) = existing {
                if operation.kind != "suggestion_accept"
                    || operation.request_digest != request_digest
                {
                    return Err(CatalogError::Conflict(
                        "request id was reused with different content".into(),
                    ));
                }
                if operation.status == "committed" {
                    return Self::comment_in_tx(tx, slug, comment_id).map(Some);
                }
                if operation.status != "prepared" {
                    return Err(CatalogError::Conflict(
                        "suggestion acceptance was aborted".into(),
                    ));
                }
                return Ok(None);
            }
            let comment = Self::comment_in_tx(tx, slug, comment_id)?;
            if comment.motivation != "editing" {
                return Err(CatalogError::Conflict(
                    "this comment is not a suggestion".into(),
                ));
            }
            if comment.outcome == "accepted" {
                return Err(CatalogError::Conflict(
                    "this suggestion is already accepted".into(),
                ));
            }
            tx.execute(
                "INSERT INTO catalog_operations
                 (storage_id,request_id,kind,request_digest,status,intent,result,created_at)
                 VALUES(?1,?2,'suggestion_accept',?3,'prepared',?4,?5,?6)",
                params![
                    storage_id,
                    request_id,
                    request_digest,
                    format!("{{\"slug\":{:?},\"comment_id\":{:?}}}", slug, comment_id),
                    comment_id,
                    created_at,
                ],
            )
            .map_err(CatalogError::from)?;
            Ok(None)
        })
    }

    /// Commit the suggestion outcome and its receipt as one SQL transaction.
    /// A missing/prepared receipt is never silently turned into a comment
    /// update: that invariant is what prevents a checkpoint from being
    /// acknowledged without a durable idempotency record.
    pub fn finish_suggestion_accept(
        &self,
        slug: &str,
        comment_id: &str,
        request_id: &str,
        request_digest: &str,
        resolved_in: &str,
        resolved_at: &str,
    ) -> CatalogResult<Comment> {
        if request_id.is_empty() || request_digest.is_empty() || resolved_in.is_empty() {
            return Err(CatalogError::Invalid(
                "invalid suggestion acceptance result".into(),
            ));
        }
        self.immediate(|tx| {
            let storage_id: String = tx
                .query_row(
                    "SELECT storage_id FROM documents WHERE slug=?1",
                    [slug],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            let operation: Option<Operation> = tx
                .query_row(
                    "SELECT storage_id,request_id,kind,request_digest,status,intent,result,created_at
                     FROM catalog_operations WHERE storage_id=?1 AND request_id=?2",
                    params![storage_id, request_id],
                    Self::read_operation,
                )
                .optional()
                .map_err(CatalogError::from)?;
            let Some(operation) = operation else {
                return Err(CatalogError::NotFound);
            };
            if operation.kind != "suggestion_accept"
                || operation.request_digest != request_digest
                || operation.result != comment_id
            {
                return Err(CatalogError::Conflict(
                    "suggestion acceptance receipt does not match".into(),
                ));
            }
            if operation.status == "committed" {
                return Self::comment_in_tx(tx, slug, comment_id);
            }
            if operation.status != "prepared" {
                return Err(CatalogError::Conflict(
                    "suggestion acceptance was aborted".into(),
                ));
            }
            let changed = tx
                .execute(
                    "UPDATE comments SET outcome='accepted',accept_request=?3,
                     resolved=1,resolved_at=?4,resolved_in=?5
                     WHERE slug=?1 AND id=?2 AND motivation='editing'",
                    params![slug, comment_id, request_id, resolved_at, resolved_in],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::NotFound);
            }
            tx.execute(
                "UPDATE catalog_operations SET status='committed',result=?3
                 WHERE storage_id=?1 AND request_id=?2 AND status='prepared'",
                params![storage_id, request_id, comment_id],
            )
            .map_err(CatalogError::from)?;
            Self::comment_in_tx(tx, slug, comment_id)
        })
    }

    pub fn delete_comment(&self, slug: &str, id: &str) -> CatalogResult<bool> {
        self.immediate(|tx| {
            Ok(tx
                .execute(
                    "DELETE FROM comments WHERE slug=?1 AND id=?2",
                    params![slug, id],
                )
                .map_err(CatalogError::from)?
                == 1)
        })
    }

    pub fn delete_reply(&self, slug: &str, comment_id: &str, id: &str) -> CatalogResult<bool> {
        self.immediate(|tx| {
            Ok(tx
                .execute(
                    "DELETE FROM replies WHERE slug=?1 AND comment_id=?2 AND id=?3",
                    params![slug, comment_id, id],
                )
                .map_err(CatalogError::from)?
                == 1)
        })
    }

    pub(super) fn comment_in_tx(
        tx: &Transaction<'_>,
        slug: &str,
        id: &str,
    ) -> CatalogResult<Comment> {
        tx.query_row("SELECT slug,id,seq,motivation,body,creator,author,via,created,exact,prefix,suffix,position,region,source_path,source_exact,source_prefix,source_suffix,source_position,proposed,outcome,accept_request,revision,resolved,resolved_at,resolved_in FROM comments WHERE slug=?1 AND id=?2",params![slug,id],Self::read_comment).map_err(CatalogError::from)
    }

    pub(super) fn read_comment(r: &rusqlite::Row<'_>) -> rusqlite::Result<Comment> {
        Ok(Comment {
            slug: r.get(0)?,
            id: r.get(1)?,
            seq: r.get(2)?,
            motivation: r.get(3)?,
            body: r.get(4)?,
            creator: r.get(5)?,
            author: r.get(6)?,
            via: r.get(7)?,
            created: r.get(8)?,
            exact: r.get(9)?,
            prefix: r.get(10)?,
            suffix: r.get(11)?,
            position: r.get(12)?,
            region: r.get(13)?,
            source_path: r.get(14)?,
            source_exact: r.get(15)?,
            source_prefix: r.get(16)?,
            source_suffix: r.get(17)?,
            source_position: r.get(18)?,
            proposed: r.get(19)?,
            outcome: r.get(20)?,
            accept_request: r.get(21)?,
            revision: r.get(22)?,
            resolved: r.get::<_, i64>(23)? != 0,
            resolved_at: r.get(24)?,
            resolved_in: r.get(25)?,
        })
    }

    pub fn insert_reply(&self, reply: &Reply) -> CatalogResult<Reply> {
        if reply.body.len() > 65_536 || reply.id.is_empty() {
            return Err(CatalogError::Invalid("invalid reply".into()));
        }
        self.immediate(|tx| { let n:i64=tx.query_row("SELECT COUNT(*) FROM replies WHERE slug=?1 AND comment_id=?2",params![reply.slug,reply.comment_id],|r|r.get(0)).map_err(CatalogError::from)?; if n>=100{return Err(CatalogError::Conflict("reply limit reached".into()));} tx.execute("INSERT INTO replies(slug,comment_id,id,body,creator,author,created) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![reply.slug,reply.comment_id,reply.id,reply.body,reply.creator,reply.author,reply.created]).map_err(CatalogError::from)?; Ok(reply.clone()) })
    }

    pub fn insert_reply_request(
        &self,
        reply: &Reply,
        request_id: &str,
        request_digest: &str,
        created_at: i64,
    ) -> CatalogResult<Reply> {
        if request_id.is_empty() {
            return self.insert_reply(reply);
        }
        if request_id.len() > 128 || request_digest.is_empty() {
            return Err(CatalogError::Invalid(
                "invalid reply request receipt".into(),
            ));
        }
        self.immediate(|tx| {
            let storage_id: String = tx
                .query_row(
                    "SELECT storage_id FROM documents WHERE slug=?1 AND status='active'",
                    [&reply.slug],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            let existing: Option<Operation> = tx
                .query_row(
                    "SELECT storage_id,request_id,kind,request_digest,status,intent,result,created_at
                     FROM catalog_operations WHERE storage_id=?1 AND request_id=?2",
                    params![storage_id, request_id],
                    Self::read_operation,
                )
                .optional()
                .map_err(CatalogError::from)?;
            if let Some(operation) = existing {
                if operation.request_digest != request_digest {
                    return Err(CatalogError::Conflict(
                        "request id was reused with different content".into(),
                    ));
                }
                if operation.status != "committed" {
                    return Err(CatalogError::Conflict(
                        "reply request has an unresolved receipt".into(),
                    ));
                }
                return tx
                    .query_row(
                        "SELECT slug,comment_id,id,body,creator,author,created FROM replies
                         WHERE slug=?1 AND comment_id=?2 AND id=?3",
                        params![reply.slug, reply.comment_id, operation.result],
                        |row| {
                            Ok(Reply {
                                slug: row.get(0)?,
                                comment_id: row.get(1)?,
                                id: row.get(2)?,
                                body: row.get(3)?,
                                creator: row.get(4)?,
                                author: row.get(5)?,
                                created: row.get(6)?,
                            })
                        },
                    )
                    .map_err(CatalogError::from);
            }
            let count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM replies WHERE slug=?1 AND comment_id=?2",
                    params![reply.slug, reply.comment_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if count >= 100 {
                return Err(CatalogError::Conflict("reply limit reached".into()));
            }
            tx.execute(
                "INSERT INTO catalog_operations
                 (storage_id,request_id,kind,request_digest,status,intent,result,created_at)
                 VALUES(?1,?2,'reply',?3,'prepared',?4,'',?5)",
                params![
                    storage_id,
                    request_id,
                    request_digest,
                    format!("{{\"comment_id\":{:?},\"id\":{:?}}}", reply.comment_id, reply.id),
                    created_at
                ],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "INSERT INTO replies(slug,comment_id,id,body,creator,author,created)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    reply.slug,
                    reply.comment_id,
                    reply.id,
                    reply.body,
                    reply.creator,
                    reply.author,
                    reply.created,
                ],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE catalog_operations SET status='committed', result=?3
                 WHERE storage_id=?1 AND request_id=?2",
                params![storage_id, request_id, reply.id],
            )
            .map_err(CatalogError::from)?;
            Ok(reply.clone())
        })
    }

    pub fn update_reply(&self, reply: &Reply) -> CatalogResult<Reply> {
        self.immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE replies SET body=?4,creator=?5,author=?6,created=?7
                     WHERE slug=?1 AND comment_id=?2 AND id=?3",
                    params![
                        reply.slug,
                        reply.comment_id,
                        reply.id,
                        reply.body,
                        reply.creator,
                        reply.author,
                        reply.created,
                    ],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::NotFound);
            }
            Ok(reply.clone())
        })
    }

    pub fn replies(&self, slug: &str, comment_id: &str, limit: u32) -> CatalogResult<Vec<Reply>> {
        let limit = i64::from(limit.clamp(1, 100));
        self.with_connection(|c|{let mut s=c.prepare("SELECT slug,comment_id,id,body,creator,author,created FROM replies WHERE slug=?1 AND comment_id=?2 ORDER BY created,id LIMIT ?3").map_err(CatalogError::from)?;let mut rows=s.query(params![slug,comment_id,limit]).map_err(CatalogError::from)?;let mut out=Vec::new();while let Some(r)=rows.next().map_err(CatalogError::from)?{out.push(Reply{slug:r.get(0).map_err(CatalogError::from)?,comment_id:r.get(1).map_err(CatalogError::from)?,id:r.get(2).map_err(CatalogError::from)?,body:r.get(3).map_err(CatalogError::from)?,creator:r.get(4).map_err(CatalogError::from)?,author:r.get(5).map_err(CatalogError::from)?,created:r.get(6).map_err(CatalogError::from)?});}Ok(out)})
    }
}
