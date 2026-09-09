//! Conversations: the ephemeral channels and the bounded message digests kept
//! for them.

use super::*;

impl Catalog {
    pub fn create_conversation(&self, conversation: &Conversation) -> CatalogResult<Conversation> {
        if conversation.id.is_empty()
            || conversation.token_hash.is_empty()
            || conversation.expires_at < 0
        {
            return Err(CatalogError::Invalid("invalid conversation".into()));
        }
        self.immediate(|tx| {
            let active: Option<String> = tx
                .query_row(
                    "SELECT status FROM documents WHERE slug=?1",
                    [&conversation.slug],
                    |r| r.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?;
            if active.as_deref() != Some("active") {
                return Err(CatalogError::NotFound);
            }
            tx.execute(
                "INSERT INTO conversations(slug,id,token_hash,expires_at) VALUES(?1,?2,?3,?4)",
                params![
                    conversation.slug,
                    conversation.id,
                    conversation.token_hash,
                    conversation.expires_at
                ],
            )
            .map_err(CatalogError::from)?;
            Ok(conversation.clone())
        })
    }

    pub fn conversation(
        &self,
        slug: &str,
        id: &str,
        now: i64,
    ) -> CatalogResult<Option<Conversation>> {
        self.with_connection(|c| c.query_row("SELECT slug,id,token_hash,expires_at FROM conversations WHERE slug=?1 AND id=?2 AND expires_at>?3", params![slug,id,now], |r| Ok(Conversation{slug:r.get(0)?,id:r.get(1)?,token_hash:r.get(2)?,expires_at:r.get(3)?})).optional().map_err(CatalogError::from))
    }

    pub fn delete_expired_conversations(&self, now: i64, limit: u32) -> CatalogResult<u32> {
        let limit = i64::from(limit.clamp(1, 1000));
        self.immediate(|tx| {
            let ids: Vec<(String, String)> = {
                let mut s = tx.prepare("SELECT slug,id FROM conversations WHERE expires_at<=?1 ORDER BY expires_at,slug,id LIMIT ?2").map_err(CatalogError::from)?;
                let rows = s.query_map(params![now, limit], |r| Ok((r.get(0)?, r.get(1)?))).map_err(CatalogError::from)?;
                rows.collect::<Result<_, _>>().map_err(CatalogError::from)?
            };
            let mut removed = 0;
            for (slug, id) in ids { removed += tx.execute("DELETE FROM conversations WHERE slug=?1 AND id=?2", params![slug, id]).map_err(CatalogError::from)? as u32; }
            Ok(removed)
        })
    }

    pub fn append_message(&self, message: &Message) -> CatalogResult<Message> {
        if message.id.is_empty() || message.text.len() > 65_536 {
            return Err(CatalogError::Invalid("invalid message".into()));
        }
        self.immediate(|tx| { let cursor:i64=tx.query_row("SELECT COALESCE(MAX(cursor)+1,0) FROM messages WHERE slug=?1 AND conversation_id=?2",params![message.slug,message.conversation_id],|r|r.get(0)).map_err(CatalogError::from)?; tx.execute("INSERT INTO messages(slug,conversation_id,cursor,id,role,text,context) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![message.slug,message.conversation_id,cursor,message.id,message.role,message.text,message.context]).map_err(CatalogError::from)?; Ok(Message{cursor,..message.clone()}) })
    }

    pub fn messages(
        &self,
        slug: &str,
        conversation_id: &str,
        after_cursor: Option<i64>,
        limit: u32,
    ) -> CatalogResult<Vec<Message>> {
        let limit = i64::from(limit.clamp(1, 200));
        self.with_connection(|c| { let mut s=c.prepare("SELECT slug,conversation_id,cursor,id,role,text,context FROM messages WHERE slug=?1 AND conversation_id=?2 AND (?3 IS NULL OR cursor>?3) ORDER BY cursor LIMIT ?4").map_err(CatalogError::from)?; let mut rows=s.query(params![slug,conversation_id,after_cursor,limit]).map_err(CatalogError::from)?; let mut out=Vec::new(); while let Some(r)=rows.next().map_err(CatalogError::from)? { out.push(Message{slug:r.get(0).map_err(CatalogError::from)?,conversation_id:r.get(1).map_err(CatalogError::from)?,cursor:r.get(2).map_err(CatalogError::from)?,id:r.get(3).map_err(CatalogError::from)?,role:r.get(4).map_err(CatalogError::from)?,text:r.get(5).map_err(CatalogError::from)?,context:r.get(6).map_err(CatalogError::from)?}); } Ok(out) })
    }
}
