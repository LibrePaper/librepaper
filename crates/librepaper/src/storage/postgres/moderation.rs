//! Reversible operator moderation and its durable audit trail.

use sqlx::Row;
use uuid::Uuid;

use super::{new_id, Error, PostgresCatalog, Result};

#[derive(Clone, Copy, Debug)]
pub enum ModerationAction {
    BlockAccount,
    UnblockAccount,
    HideProject,
    UnhideProject,
}

impl ModerationAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::BlockAccount => "block_account",
            Self::UnblockAccount => "unblock_account",
            Self::HideProject => "hide_project",
            Self::UnhideProject => "unhide_project",
        }
    }
}

impl PostgresCatalog {
    /// Block or unblock a registered account selected by UUID or provider
    /// handle. Blocking increments the session generation, invalidating
    /// existing cookies and device credentials, and the account status is
    /// checked by every subsequent authentication and write authorization.
    pub async fn moderate_account(
        &self,
        selector: &str,
        blocked: bool,
        actor: &str,
        reason: &str,
    ) -> Result<(Uuid, String)> {
        validate_operator_fields(actor, reason)?;
        let mut tx = self.begin_metered().await?;
        let matches = if let Ok(id) = Uuid::parse_str(selector) {
            sqlx::query("SELECT id,provider,handle,status FROM accounts WHERE id=$1 FOR UPDATE")
                .bind(id)
                .fetch_all(&mut *tx)
                .await?
        } else {
            sqlx::query(
                "SELECT id,provider,handle,status FROM accounts
                 WHERE lower(handle)=lower($1) ORDER BY id LIMIT 2 FOR UPDATE",
            )
            .bind(selector.trim())
            .fetch_all(&mut *tx)
            .await?
        };
        let row = match matches.as_slice() {
            [] => return Err(Error::NotFound),
            [row] => row,
            _ => {
                return Err(Error::Invalid(
                    "account handle is ambiguous; use its UUID".into(),
                ))
            }
        };
        let id: Uuid = row.try_get("id")?;
        let provider: Option<String> = row.try_get("provider")?;
        let handle: String = row.try_get("handle")?;
        let status: String = row.try_get("status")?;
        let next = if blocked { "blocked" } else { "active" };
        let expected = if blocked { "active" } else { "blocked" };
        if status != expected && status != next {
            return Err(Error::Conflict(format!(
                "account is {status}; moderation cannot change that lifecycle state"
            )));
        }
        if status == expected {
            let query = if blocked {
                "UPDATE accounts SET status='blocked',session_generation=session_generation+1 WHERE id=$1"
            } else {
                "UPDATE accounts SET status='active' WHERE id=$1"
            };
            sqlx::query(query).bind(id).execute(&mut *tx).await?;
        }
        let label = provider
            .map(|provider| format!("{provider}:{handle}"))
            .unwrap_or(handle);
        insert_audit(
            &mut tx,
            actor,
            if blocked {
                ModerationAction::BlockAccount
            } else {
                ModerationAction::UnblockAccount
            },
            "account",
            id,
            &label,
            reason,
        )
        .await?;
        tx.commit().await?;
        Ok((id, label))
    }

    /// Hide or restore an active project by slug. Hidden rows are separate
    /// from the document lifecycle, so the expiry and erasure workers retain
    /// their ordinary active/deleting state transitions.
    pub async fn moderate_project(
        &self,
        slug: &str,
        hidden: bool,
        actor: &str,
        reason: &str,
    ) -> Result<Uuid> {
        validate_operator_fields(actor, reason)?;
        let slug = slug.trim();
        if slug.is_empty() {
            return Err(Error::Invalid("project slug must not be empty".into()));
        }
        let mut tx = self.begin_metered().await?;
        let row =
            sqlx::query("SELECT id FROM documents WHERE slug=$1 AND status='active' FOR UPDATE")
                .bind(slug)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(Error::NotFound)?;
        let id: Uuid = row.try_get("id")?;
        let changed = if hidden {
            sqlx::query(
                "INSERT INTO moderated_projects(document_id) VALUES($1)
                 ON CONFLICT(document_id) DO NOTHING",
            )
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
        } else {
            sqlx::query("DELETE FROM moderated_projects WHERE document_id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await?
                .rows_affected()
        };
        if !hidden && changed == 0 {
            return Err(Error::Conflict("project is not hidden".into()));
        }
        insert_audit(
            &mut tx,
            actor,
            if hidden {
                ModerationAction::HideProject
            } else {
                ModerationAction::UnhideProject
            },
            "project",
            id,
            slug,
            reason,
        )
        .await?;
        tx.commit().await?;
        Ok(id)
    }

    pub async fn project_is_hidden(&self, document_id: Uuid) -> Result<bool> {
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM moderated_projects WHERE document_id=$1)",
        )
        .bind(document_id)
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn project_is_hidden_by_slug(&self, slug: &str) -> Result<bool> {
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(
                 SELECT 1 FROM documents d
                 JOIN moderated_projects m ON m.document_id=d.id
                 WHERE d.slug=$1
             )",
        )
        .bind(slug)
        .fetch_one(&self.pool)
        .await?)
    }
}

fn validate_operator_fields(actor: &str, reason: &str) -> Result<()> {
    if actor.trim().is_empty() || actor.len() > 256 {
        return Err(Error::Invalid("actor must contain 1 to 256 bytes".into()));
    }
    if reason.trim().is_empty() || reason.len() > 2000 {
        return Err(Error::Invalid("reason must contain 1 to 2000 bytes".into()));
    }
    Ok(())
}

async fn insert_audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor: &str,
    action: ModerationAction,
    target_kind: &str,
    target_id: Uuid,
    target_label: &str,
    reason: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO moderation_audit
         (id,actor,action,target_kind,target_id,target_label,reason)
         VALUES($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(new_id())
    .bind(actor.trim())
    .bind(action.as_str())
    .bind(target_kind)
    .bind(target_id)
    .bind(target_label)
    .bind(reason.trim())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::postgres::{PostgresCatalog, PostgresOptions};

    async fn test_catalog() -> PostgresCatalog {
        let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
            .expect("set LIBREPAPER_TEST_POSTGRES_URL to a throwaway PostgreSQL database");
        let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .expect("connect to PostgreSQL");
        catalog
            .migrate()
            .await
            .expect("apply PostgreSQL migrations");
        catalog
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn account_moderation_revokes_generation_and_audits_both_transitions() {
        let catalog = test_catalog().await;
        let id = Uuid::now_v7();
        let handle = format!("moderation-{id}");
        sqlx::query(
            "INSERT INTO accounts(id,kind,handle,display_name,status)
             VALUES($1,'anonymous',$2,'Moderation test','active')",
        )
        .bind(id)
        .bind(&handle)
        .execute(catalog.pool())
        .await
        .unwrap();
        let before: i64 = sqlx::query_scalar("SELECT session_generation FROM accounts WHERE id=$1")
            .bind(id)
            .fetch_one(catalog.pool())
            .await
            .unwrap();

        catalog
            .moderate_account(&handle, true, "operator", "abuse report")
            .await
            .unwrap();
        let status: String = sqlx::query_scalar("SELECT status FROM accounts WHERE id=$1")
            .bind(id)
            .fetch_one(catalog.pool())
            .await
            .unwrap();
        let blocked_generation: i64 =
            sqlx::query_scalar("SELECT session_generation FROM accounts WHERE id=$1")
                .bind(id)
                .fetch_one(catalog.pool())
                .await
                .unwrap();
        assert_eq!(status, "blocked");
        assert_eq!(blocked_generation, before + 1);

        catalog
            .moderate_account(&id.to_string(), false, "operator", "appeal upheld")
            .await
            .unwrap();
        let audits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM moderation_audit WHERE target_id=$1 AND target_kind='account'",
        )
        .bind(id)
        .fetch_one(catalog.pool())
        .await
        .unwrap();
        assert_eq!(audits, 2);
        catalog.close().await;
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn hidden_project_keeps_its_document_row_and_can_be_restored() {
        let catalog = test_catalog().await;
        let owner = Uuid::now_v7();
        let document = Uuid::now_v7();
        let slug = format!("moderation-project-{document}");
        sqlx::query(
            "INSERT INTO accounts(id,kind,handle,display_name,status)
             VALUES($1,'anonymous',$2,'Moderation test','active')",
        )
        .bind(owner)
        .bind(format!("moderation-owner-{owner}"))
        .execute(catalog.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO documents(id,slug,owner_id,ownership_mode,title,status,source_format,main_path)
             VALUES($1,$2,$3,'owned','Moderation project','active','markdown','main.md')",
        )
        .bind(document)
        .bind(&slug)
        .bind(owner)
        .execute(catalog.pool())
        .await
        .unwrap();

        catalog
            .moderate_project(&slug, true, "operator", "abuse report")
            .await
            .unwrap();
        assert!(catalog.project_is_hidden(document).await.unwrap());
        let lifecycle: String = sqlx::query_scalar("SELECT status FROM documents WHERE id=$1")
            .bind(document)
            .fetch_one(catalog.pool())
            .await
            .unwrap();
        assert_eq!(lifecycle, "active");
        assert_eq!(
            catalog
                .access_role(document, Some(owner), None, time::OffsetDateTime::now_utc(),)
                .await
                .unwrap(),
            None
        );
        let actor = crate::storage::postgres::annotations::MutationAuthorization {
            principal_key: format!("moderation-owner-{owner}"),
            account_id: Some(owner),
            session_generation: Some(1),
            token_hash: None,
            policy_edit: true,
            policy_comment: true,
            automation: false,
        };
        assert!(matches!(
            catalog
                .authorize_document_mutation(document, &actor, true)
                .await,
            Err(Error::NotFound)
        ));

        catalog
            .moderate_project(&slug, false, "operator", "appeal upheld")
            .await
            .unwrap();
        assert!(!catalog.project_is_hidden(document).await.unwrap());
        assert_eq!(
            catalog
                .access_role(document, Some(owner), None, time::OffsetDateTime::now_utc(),)
                .await
                .unwrap(),
            Some(crate::storage::postgres::access::AccessRole::Owner)
        );
        let audits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM moderation_audit WHERE target_id=$1 AND target_kind='project'",
        )
        .bind(document)
        .fetch_one(catalog.pool())
        .await
        .unwrap();
        assert_eq!(audits, 2);
        catalog.close().await;
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn hidden_projects_do_not_consume_public_pages_and_still_expire() {
        let catalog = test_catalog().await;
        crate::tests::reset(&catalog).await;
        let owner = Uuid::now_v7();
        let hidden_id = Uuid::now_v7();
        let visible_id = Uuid::now_v7();
        let hidden_slug = format!("moderation-expired-{hidden_id}");
        let visible_slug = format!("moderation-visible-{visible_id}");
        sqlx::query(
            "INSERT INTO accounts(id,kind,handle,display_name,status)
             VALUES($1,'anonymous',$2,'Moderation pagination test','active')",
        )
        .bind(owner)
        .bind(format!("moderation-pagination-owner-{owner}"))
        .execute(catalog.pool())
        .await
        .unwrap();
        for (id, slug) in [(hidden_id, &hidden_slug), (visible_id, &visible_slug)] {
            sqlx::query(
                "INSERT INTO documents(id,slug,owner_id,ownership_mode,title,status,source_format,main_path)
                 VALUES($1,$2,$3,'owned','Moderation pagination test','active','markdown','main.md')",
            )
            .bind(id)
            .bind(slug)
            .bind(owner)
            .execute(catalog.pool())
            .await
            .unwrap();
        }
        sqlx::query("UPDATE documents SET updated_at=now()-interval '1 day' WHERE id=$1")
            .bind(hidden_id)
            .execute(catalog.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE documents SET updated_at=now()-interval '2 days' WHERE id=$1")
            .bind(visible_id)
            .execute(catalog.pool())
            .await
            .unwrap();
        catalog
            .moderate_project(&hidden_slug, true, "operator", "abuse report")
            .await
            .unwrap();

        let public = catalog.list_documents(None, 1).await.unwrap();
        assert_eq!(public.len(), 1);
        assert_eq!(public[0].id, visible_id);
        let owned = catalog
            .visible_documents(Some(owner), None, 1, false)
            .await
            .unwrap();
        assert_eq!(owned.len(), 1);
        assert_eq!(owned[0].id, visible_id);

        let maintenance = catalog
            .list_documents_for_maintenance(None, 200)
            .await
            .unwrap();
        let hidden = maintenance
            .iter()
            .find(|document| document.id == hidden_id)
            .expect("maintenance enumeration retains hidden documents");
        assert!(
            hidden.updated_at.unix_timestamp()
                <= time::OffsetDateTime::now_utc().unix_timestamp() - 86_400
        );
        let _writer = catalog.claim_writer().await.unwrap();
        assert!(catalog.mark_document_deleting(hidden_id).await.unwrap());
        let status: String = sqlx::query_scalar("SELECT status FROM documents WHERE id=$1")
            .bind(hidden_id)
            .fetch_one(catalog.pool())
            .await
            .unwrap();
        assert_eq!(status, "deleting");
        drop(_writer);
        catalog.close().await;
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn retention_maintenance_pages_reach_a_hidden_document_after_200_rows() {
        let catalog = test_catalog().await;
        crate::tests::reset(&catalog).await;
        let owner = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO accounts(id,kind,handle,display_name,status)
             VALUES($1,'anonymous',$2,'Retention pagination test','active')",
        )
        .bind(owner)
        .bind(format!("moderation-retention-owner-{owner}"))
        .execute(catalog.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO documents
             (id,slug,owner_id,ownership_mode,title,status,source_format,main_path,updated_at)
             SELECT md5('moderation-retention-' || n::text)::uuid,
                    'moderation-retention-' || n::text,$1,'owned',
                    'Retention pagination test','active','markdown','main.md',
                    now() - n * interval '1 day'
             FROM generate_series(0,200) AS series(n)",
        )
        .bind(owner)
        .execute(catalog.pool())
        .await
        .unwrap();
        let hidden_slug = "moderation-retention-200";
        let hidden_id: Uuid = sqlx::query_scalar("SELECT id FROM documents WHERE slug=$1")
            .bind(hidden_slug)
            .fetch_one(catalog.pool())
            .await
            .unwrap();
        catalog
            .moderate_project(hidden_slug, true, "operator", "abuse report")
            .await
            .unwrap();

        let first = catalog
            .list_documents_for_maintenance(None, 200)
            .await
            .unwrap();
        assert_eq!(first.len(), 200);
        let last = first.last().unwrap();
        let second = catalog
            .list_documents_for_maintenance(Some((last.updated_at, last.id)), 200)
            .await
            .unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].id, hidden_id);

        let _writer = catalog.claim_writer().await.unwrap();
        assert!(catalog.mark_document_deleting(hidden_id).await.unwrap());
        let status: String = sqlx::query_scalar("SELECT status FROM documents WHERE id=$1")
            .bind(hidden_id)
            .fetch_one(catalog.pool())
            .await
            .unwrap();
        assert_eq!(status, "deleting");
        drop(_writer);
        catalog.close().await;
    }
}
