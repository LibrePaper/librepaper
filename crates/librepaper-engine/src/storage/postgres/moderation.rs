//! Reversible operator moderation and its durable audit trail.

use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{new_id, Error, PostgresCatalog, Result};

/// One account as the operator's accounts list shows it.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct AccountUsage {
    pub id: Uuid,
    pub handle: String,
    pub provider: Option<String>,
    pub kind: String,
    pub status: String,
    pub created_at: OffsetDateTime,
    pub last_seen_at: OffsetDateTime,
    pub documents: i64,
    pub bytes: i64,
}

/// What blocking a name did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockOutcome {
    /// An account had this name or ID and is now blocked.
    Blocked { id: Uuid, label: String },
    /// No account has this name yet. Its first sign-in creates the account blocked.
    Banned { id: Uuid, label: String },
}

/// A blocked account, with the operator action that blocked it.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct BlockedAccount {
    pub id: Uuid,
    pub handle: String,
    pub provider: Option<String>,
    pub kind: String,
    pub blocked_at: Option<OffsetDateTime>,
    pub actor: Option<String>,
    pub reason: Option<String>,
}

/// A name refused before its first sign-in.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct BannedHandle {
    pub id: Uuid,
    pub provider: String,
    pub handle: String,
    pub actor: String,
    pub reason: String,
    pub created_at: OffsetDateTime,
}

/// A hidden project, with the operator action that hid it.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct HiddenProject {
    pub id: Uuid,
    pub slug: String,
    pub title: String,
    pub owner: String,
    pub hidden_at: OffsetDateTime,
    pub actor: Option<String>,
    pub reason: Option<String>,
}

/// One entry of the moderation audit trail.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct AuditEntry {
    pub occurred_at: OffsetDateTime,
    pub actor: String,
    pub action: String,
    pub target_kind: String,
    pub target_label: String,
    pub reason: String,
}

/// Everything the operator's moderation page lists.
#[derive(Clone, Debug)]
pub struct ModerationOverview {
    pub blocked_accounts: Vec<BlockedAccount>,
    pub banned_handles: Vec<BannedHandle>,
    pub hidden_projects: Vec<HiddenProject>,
    pub recent: Vec<AuditEntry>,
}

/// Per-owner bytes: the same five sources `owner_usage_bytes` charges each owner.
const OWNER_USAGE: &str = "SELECT owner_id, COALESCE(sum(bytes),0)::bigint AS bytes FROM (
                 SELECT d.owner_id, x.byte_length AS bytes FROM document_archives x
                   JOIN documents d ON d.id=x.document_id
                 UNION ALL SELECT d.owner_id, x.byte_length FROM document_assets x
                   JOIN documents d ON d.id=x.document_id
                 UNION ALL SELECT d.owner_id, s.snapshot_bytes FROM document_snapshots s
                   JOIN documents d ON d.id=s.document_id WHERE s.delete_after IS NULL
                 UNION ALL SELECT d.owner_id, d.uncompacted_update_bytes + d.chat_bytes FROM documents d
                 UNION ALL SELECT d.owner_id, r.bytes FROM pending_log_reservations r
                   JOIN documents d ON d.id=r.document_id
               ) usage GROUP BY owner_id";

#[derive(Clone, Copy, Debug)]
pub enum ModerationAction {
    BlockAccount,
    UnblockAccount,
    HideProject,
    UnhideProject,
    BanHandle,
    UnbanHandle,
    EraseAccount,
}

impl ModerationAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::BlockAccount => "block_account",
            Self::UnblockAccount => "unblock_account",
            Self::HideProject => "hide_project",
            Self::UnhideProject => "unhide_project",
            Self::BanHandle => "ban_handle",
            Self::UnbanHandle => "unban_handle",
            Self::EraseAccount => "erase_account",
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
        let account = lock_account(&mut tx, selector).await?;
        let next = if blocked { "blocked" } else { "active" };
        let expected = if blocked { "active" } else { "blocked" };
        if account.status != expected && account.status != next {
            return Err(Error::Conflict(format!(
                "account is {}; moderation cannot change that lifecycle state",
                account.status
            )));
        }
        if account.status == expected {
            let query = if blocked {
                "UPDATE accounts SET status='blocked',session_generation=session_generation+1 WHERE id=$1"
            } else {
                "UPDATE accounts SET status='active' WHERE id=$1"
            };
            sqlx::query(query)
                .bind(account.id)
                .execute(&mut *tx)
                .await?;
        }
        let label = account.label();
        insert_audit(
            &mut tx,
            actor,
            if blocked {
                ModerationAction::BlockAccount
            } else {
                ModerationAction::UnblockAccount
            },
            "account",
            account.id,
            &label,
            reason,
        )
        .await?;
        tx.commit().await?;
        Ok((account.id, label))
    }

    /// Starts the same erasure the owner's Settings button starts: the account
    /// becomes `erasing`, its sessions end, and its active documents are marked
    /// `deleting`. The background worker then purges those documents after the
    /// deletion grace and deletes the account row. The running service picks
    /// this durable state up on its hourly rescan or at startup.
    pub async fn erase_account_by_operator(
        &self,
        selector: &str,
        actor: &str,
        reason: &str,
    ) -> Result<(Uuid, String)> {
        validate_operator_fields(actor, reason)?;
        let mut tx = self.begin_metered().await?;
        let account = lock_account(&mut tx, selector).await?;
        if account.kind != "registered" {
            return Err(Error::Conflict(
                "only registered accounts can be erased".into(),
            ));
        }
        if account.status == "erasing" {
            return Err(Error::Conflict("account is already being erased".into()));
        }
        // A blocked account is accepted on purpose. The owner path refuses
        // blocked accounts so a blocked person cannot erase the record
        // themselves, but the operator may.
        sqlx::query("UPDATE accounts SET status='erasing',session_generation=session_generation+1 WHERE id=$1")
            .bind(account.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE documents SET status='deleting',deleted_at=COALESCE(deleted_at,now()),updated_at=now()
             WHERE owner_id=$1 AND status='active'",
        )
        .bind(account.id)
        .execute(&mut *tx)
        .await?;
        let label = account.label();
        insert_audit(
            &mut tx,
            actor,
            ModerationAction::EraseAccount,
            "account",
            account.id,
            &label,
            reason,
        )
        .await?;
        tx.commit().await?;
        Ok((account.id, label))
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

    /// Blocks the account with this name or ID, or, when no account has the
    /// name yet, bans the name so its first sign-in creates the account blocked.
    pub async fn block_or_ban(
        &self,
        provider: &str,
        name: &str,
        actor: &str,
        reason: &str,
    ) -> Result<BlockOutcome> {
        validate_operator_fields(actor, reason)?;
        if Uuid::parse_str(name.trim()).is_ok() {
            let (id, label) = self
                .moderate_account(name.trim(), true, actor, reason)
                .await?;
            return Ok(BlockOutcome::Blocked { id, label });
        }
        if !librepaper_base::auth::valid_provider_id(provider) {
            return Err(Error::Invalid(
                "provider must be github, google, entra or oidc-<slug>".into(),
            ));
        }
        let handle = name.trim().trim_start_matches('@').trim();
        if handle.is_empty() {
            return Err(Error::Invalid("name must not be empty".into()));
        }
        if handle.len() > 256 {
            return Err(Error::Invalid("name must contain at most 256 bytes".into()));
        }
        let matches = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM accounts WHERE provider=$1 AND lower(handle)=lower($2)
             ORDER BY id LIMIT 2",
        )
        .bind(provider)
        .bind(handle)
        .fetch_all(&self.pool)
        .await?;
        match matches.as_slice() {
            [id] => {
                let (id, label) = self
                    .moderate_account(&id.to_string(), true, actor, reason)
                    .await?;
                Ok(BlockOutcome::Blocked { id, label })
            }
            [] => {
                let mut tx = self.begin_metered().await?;
                let id = new_id();
                let inserted = sqlx::query_scalar::<_, Uuid>(
                    "INSERT INTO banned_handles(id,provider,handle,actor,reason)
                     VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING RETURNING id",
                )
                .bind(id)
                .bind(provider)
                .bind(handle)
                .bind(actor.trim())
                .bind(reason.trim())
                .fetch_optional(&mut *tx)
                .await?;
                let id = match inserted {
                    Some(id) => id,
                    None => {
                        return Err(Error::Conflict("this name is already banned".into()));
                    }
                };
                let label = format!("{provider}:{handle}");
                insert_audit(
                    &mut tx,
                    actor,
                    ModerationAction::BanHandle,
                    "handle",
                    id,
                    &label,
                    reason,
                )
                .await?;
                tx.commit().await?;
                Ok(BlockOutcome::Banned { id, label })
            }
            _ => Err(Error::Invalid(
                "more than one account has this name; use its account ID".into(),
            )),
        }
    }

    /// Removes a name ban and returns its `provider:handle` label.
    pub async fn unban_handle(&self, id: Uuid, actor: &str, reason: &str) -> Result<String> {
        validate_operator_fields(actor, reason)?;
        let mut tx = self.begin_metered().await?;
        let row = sqlx::query("DELETE FROM banned_handles WHERE id=$1 RETURNING provider, handle")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(Error::NotFound)?;
        let provider: String = row.try_get("provider")?;
        let handle: String = row.try_get("handle")?;
        let label = format!("{provider}:{handle}");
        insert_audit(
            &mut tx,
            actor,
            ModerationAction::UnbanHandle,
            "handle",
            id,
            &label,
            reason,
        )
        .await?;
        tx.commit().await?;
        Ok(label)
    }

    /// The blocked accounts, name bans, hidden projects and latest audit
    /// entries that the operator's moderation page lists.
    pub async fn moderation_overview(&self) -> Result<ModerationOverview> {
        let blocked_accounts = sqlx::query_as::<_, BlockedAccount>(
            "SELECT a.id, a.handle, a.provider, a.kind, m.occurred_at AS blocked_at, m.actor, m.reason
               FROM accounts a
               LEFT JOIN LATERAL (
                 SELECT occurred_at, actor, reason FROM moderation_audit
                  WHERE target_kind='account' AND target_id=a.id AND action='block_account'
                  ORDER BY occurred_at DESC, id DESC LIMIT 1
               ) m ON true
              WHERE a.status='blocked'
              ORDER BY m.occurred_at DESC NULLS LAST, a.id
              LIMIT 500",
        )
        .fetch_all(&self.pool)
        .await?;
        let banned_handles = sqlx::query_as::<_, BannedHandle>(
            "SELECT id, provider, handle, actor, reason, created_at FROM banned_handles
              ORDER BY created_at DESC, id DESC LIMIT 500",
        )
        .fetch_all(&self.pool)
        .await?;
        let hidden_projects = sqlx::query_as::<_, HiddenProject>(
            "SELECT d.id, d.slug, d.title, a.handle AS owner, mp.hidden_at, m.actor, m.reason
               FROM moderated_projects mp
               JOIN documents d ON d.id=mp.document_id
               JOIN accounts a ON a.id=d.owner_id
               LEFT JOIN LATERAL (
                 SELECT actor, reason FROM moderation_audit
                  WHERE target_kind='project' AND target_id=d.id AND action='hide_project'
                  ORDER BY occurred_at DESC, id DESC LIMIT 1
               ) m ON true
              ORDER BY mp.hidden_at DESC, d.id
              LIMIT 500",
        )
        .fetch_all(&self.pool)
        .await?;
        let recent = sqlx::query_as::<_, AuditEntry>(
            "SELECT occurred_at, actor, action, target_kind, target_label, reason
               FROM moderation_audit ORDER BY occurred_at DESC, id DESC LIMIT 50",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(ModerationOverview {
            blocked_accounts,
            banned_handles,
            hidden_projects,
            recent,
        })
    }

    pub async fn project_is_hidden(&self, document_id: Uuid) -> Result<bool> {
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM moderated_projects WHERE document_id=$1)",
        )
        .bind(document_id)
        .fetch_one(&self.pool)
        .await?)
    }

    /// Every account but the system ones, largest stored bytes first. Each byte
    /// count is what `owner_usage_bytes` charges that owner. An account with no
    /// documents shows 0.
    pub async fn all_accounts(&self) -> Result<Vec<AccountUsage>> {
        let sql = format!(
            "SELECT a.id, a.handle, a.provider, a.kind, a.status, a.created_at, a.last_seen_at,
                    (SELECT count(*) FROM documents d WHERE d.owner_id=a.id)::bigint AS documents,
                    COALESCE(u.bytes,0)::bigint AS bytes
             FROM accounts a LEFT JOIN ({OWNER_USAGE}) u ON u.owner_id=a.id
             WHERE a.kind <> 'system'
             ORDER BY bytes DESC, a.id"
        );
        Ok(sqlx::query_as::<_, AccountUsage>(&sql)
            .fetch_all(&self.pool)
            .await?)
    }

    /// How many accounts the inactivity sweep would erase for this cutoff, and the bytes they hold.
    pub async fn inactive_summary(&self, seen_before: OffsetDateTime) -> Result<(i64, i64)> {
        let sql = format!(
            "SELECT count(*)::bigint, COALESCE(sum(u.bytes),0)::bigint
               FROM accounts a LEFT JOIN ({OWNER_USAGE}) u ON u.owner_id=a.id
              WHERE a.kind='registered' AND a.status='active' AND a.last_seen_at < $1"
        );
        Ok(sqlx::query_as::<_, (i64, i64)>(&sql)
            .bind(seen_before)
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

/// One account row, locked for the rest of the transaction.
struct LockedAccount {
    id: Uuid,
    kind: String,
    provider: Option<String>,
    handle: String,
    status: String,
}

impl LockedAccount {
    /// The name the audit trail records: `provider:handle`, or the handle alone.
    fn label(&self) -> String {
        match &self.provider {
            Some(provider) => format!("{provider}:{}", self.handle),
            None => self.handle.clone(),
        }
    }
}

/// Resolve an operator's selector, a UUID or a unique handle, to one account
/// row and lock it. Two accounts with the same handle are refused rather than
/// guessed between.
async fn lock_account(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    selector: &str,
) -> Result<LockedAccount> {
    let matches = if let Ok(id) = Uuid::parse_str(selector) {
        sqlx::query("SELECT id,kind,provider,handle,status FROM accounts WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_all(&mut **tx)
            .await?
    } else {
        sqlx::query(
            "SELECT id,kind,provider,handle,status FROM accounts
             WHERE lower(handle)=lower($1) ORDER BY id LIMIT 2 FOR UPDATE",
        )
        .bind(selector.trim())
        .fetch_all(&mut **tx)
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
    Ok(LockedAccount {
        id: row.try_get("id")?,
        kind: row.try_get("kind")?,
        provider: row.try_get("provider")?,
        handle: row.try_get("handle")?,
        status: row.try_get("status")?,
    })
}

pub(super) async fn insert_audit(
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
    use crate::storage::postgres::{NewAccount, PostgresCatalog, PostgresOptions};

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
    async fn operator_erasure_marks_the_account_and_its_documents_and_audits_it() {
        let catalog = test_catalog().await;
        crate::testing::reset(&catalog).await;
        let id = insert_account(&catalog, "registered", "active", 0).await;
        insert_document_with_bytes(&catalog, id, 100).await;
        let before: i64 = sqlx::query_scalar("SELECT session_generation FROM accounts WHERE id=$1")
            .bind(id)
            .fetch_one(catalog.pool())
            .await
            .unwrap();
        let handle = format!("inactive-{id}");

        let (erased, label) = catalog
            .erase_account_by_operator(&handle, "operator", "erase request")
            .await
            .unwrap();
        assert_eq!(erased, id);
        assert_eq!(label, format!("github:{handle}"));
        let status: String = sqlx::query_scalar("SELECT status FROM accounts WHERE id=$1")
            .bind(id)
            .fetch_one(catalog.pool())
            .await
            .unwrap();
        let generation: i64 =
            sqlx::query_scalar("SELECT session_generation FROM accounts WHERE id=$1")
                .bind(id)
                .fetch_one(catalog.pool())
                .await
                .unwrap();
        assert_eq!(status, "erasing");
        assert_eq!(generation, before + 1);
        let documents: Vec<(String, bool)> = sqlx::query_as(
            "SELECT status, deleted_at IS NOT NULL FROM documents WHERE owner_id=$1",
        )
        .bind(id)
        .fetch_all(catalog.pool())
        .await
        .unwrap();
        assert_eq!(documents.len(), 1);
        assert!(documents
            .iter()
            .all(|(status, deleted)| status == "deleting" && *deleted));
        let audits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM moderation_audit WHERE target_id=$1 AND action='erase_account'",
        )
        .bind(id)
        .fetch_one(catalog.pool())
        .await
        .unwrap();
        assert_eq!(audits, 1);
        assert!(matches!(
            catalog
                .erase_account_by_operator(&handle, "operator", "erase request")
                .await,
            Err(Error::Conflict(_))
        ));

        let blocked = insert_account(&catalog, "registered", "blocked", 0).await;
        catalog
            .erase_account_by_operator(&blocked.to_string(), "operator", "erase request")
            .await
            .unwrap();
        let blocked_status: String = sqlx::query_scalar("SELECT status FROM accounts WHERE id=$1")
            .bind(blocked)
            .fetch_one(catalog.pool())
            .await
            .unwrap();
        assert_eq!(blocked_status, "erasing");

        let anonymous = insert_account(&catalog, "anonymous", "active", 0).await;
        assert!(matches!(
            catalog
                .erase_account_by_operator(&anonymous.to_string(), "operator", "erase request")
                .await,
            Err(Error::Conflict(_))
        ));
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
        crate::testing::reset(&catalog).await;
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
        crate::testing::reset(&catalog).await;
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

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn all_accounts_orders_by_owner_usage_and_keeps_accounts_without_documents() {
        let catalog = test_catalog().await;
        crate::testing::reset(&catalog).await;
        let big = Uuid::now_v7();
        let medium = Uuid::now_v7();
        let small = Uuid::now_v7();
        let empty = Uuid::now_v7();
        for owner in [big, medium, small, empty] {
            sqlx::query(
                "INSERT INTO accounts(id,kind,handle,display_name,status)
                 VALUES($1,'anonymous',$2,'Largest accounts test','active')",
            )
            .bind(owner)
            .bind(format!("largest-{owner}"))
            .execute(catalog.pool())
            .await
            .unwrap();
        }
        let mut documents = Vec::new();
        for (owner, count) in [(big, 3), (medium, 1), (small, 2)] {
            for _ in 0..count {
                let document = Uuid::now_v7();
                sqlx::query(
                    "INSERT INTO documents(id,slug,owner_id,ownership_mode,title,status,source_format,main_path)
                     VALUES($1,$2,$3,'owned','Largest accounts test','active','markdown','main.md')",
                )
                .bind(document)
                .bind(format!("largest-document-{document}"))
                .bind(owner)
                .execute(catalog.pool())
                .await
                .unwrap();
                documents.push((owner, document));
            }
        }
        let bytes_by_document = [
            (documents[0].1, 1000_i64),
            (documents[3].1, 400),
            (documents[4].1, 100),
        ];
        for (document, bytes) in bytes_by_document {
            sqlx::query("UPDATE documents SET uncompacted_update_bytes=$2 WHERE id=$1")
                .bind(document)
                .bind(bytes)
                .execute(catalog.pool())
                .await
                .unwrap();
        }
        sqlx::query(
            "INSERT INTO document_assets(id,document_id,storage_key,digest,byte_length,media_type)
             VALUES($1,$2,$3,$4,500,'image/png')",
        )
        .bind(Uuid::now_v7())
        .bind(documents[0].1)
        .bind(format!("largest-asset-{}", documents[0].1))
        .bind(vec![7_u8; 32])
        .execute(catalog.pool())
        .await
        .unwrap();

        let test_ids = [big, medium, small, empty];
        let all: Vec<AccountUsage> = catalog
            .all_accounts()
            .await
            .unwrap()
            .into_iter()
            .filter(|row| test_ids.contains(&row.id))
            .collect();
        let order: Vec<Uuid> = all.iter().map(|row| row.id).collect();
        assert_eq!(order, vec![big, medium, small, empty]);
        let totals: Vec<(i64, i64)> = all.iter().map(|row| (row.documents, row.bytes)).collect();
        assert_eq!(totals, vec![(3, 1500), (1, 400), (2, 100), (0, 0)]);
        for row in &all {
            let charged =
                crate::storage::postgres::repository::owner_usage_bytes(catalog.pool(), row.id)
                    .await
                    .unwrap();
            assert_eq!(row.bytes, charged);
        }
        catalog.close().await;
    }

    /// Inserts an account last seen `days_ago` days back. Registered accounts
    /// get a provider identity; every other kind must have none.
    async fn insert_account(
        catalog: &PostgresCatalog,
        kind: &str,
        status: &str,
        days_ago: i32,
    ) -> Uuid {
        let id = Uuid::now_v7();
        let (provider, subject) = if kind == "registered" {
            (Some("github".to_owned()), Some(format!("inactive-{id}")))
        } else {
            (None, None)
        };
        sqlx::query(
            "INSERT INTO accounts(id,kind,handle,provider,provider_subject,display_name,status,last_seen_at)
             VALUES($1,$2,$3,$4,$5,'Inactive accounts test',$6,now() - make_interval(days => $7))",
        )
        .bind(id)
        .bind(kind)
        .bind(format!("inactive-{id}"))
        .bind(provider)
        .bind(subject)
        .bind(status)
        .bind(days_ago)
        .execute(catalog.pool())
        .await
        .unwrap();
        id
    }

    async fn insert_document_with_bytes(catalog: &PostgresCatalog, owner: Uuid, bytes: i64) {
        let document = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO documents(id,slug,owner_id,ownership_mode,title,status,source_format,main_path)
             VALUES($1,$2,$3,'owned','Inactive accounts test','active','markdown','main.md')",
        )
        .bind(document)
        .bind(format!("inactive-document-{document}"))
        .bind(owner)
        .execute(catalog.pool())
        .await
        .unwrap();
        sqlx::query("UPDATE documents SET uncompacted_update_bytes=$2 WHERE id=$1")
            .bind(document)
            .bind(bytes)
            .execute(catalog.pool())
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn inactive_accounts_are_registered_active_accounts_not_seen_since_the_cutoff() {
        let catalog = test_catalog().await;
        crate::testing::reset(&catalog).await;
        let a = insert_account(&catalog, "registered", "active", 400).await;
        let b = insert_account(&catalog, "registered", "active", 500).await;
        let _c = insert_account(&catalog, "registered", "active", 10).await;
        let _d = insert_account(&catalog, "registered", "blocked", 400).await;
        let _e = insert_account(&catalog, "anonymous", "active", 400).await;
        let cutoff = time::OffsetDateTime::now_utc() - time::Duration::days(365);

        assert_eq!(
            catalog.inactive_accounts(cutoff, 10).await.unwrap(),
            vec![b, a]
        );
        assert_eq!(catalog.inactive_accounts(cutoff, 1).await.unwrap(), vec![b]);
        assert!(catalog.inactive_accounts(cutoff, 0).await.is_err());
        catalog.close().await;
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn inactive_summary_counts_inactive_accounts_and_their_bytes() {
        let catalog = test_catalog().await;
        crate::testing::reset(&catalog).await;
        let a = insert_account(&catalog, "registered", "active", 400).await;
        let _b = insert_account(&catalog, "registered", "active", 400).await;
        let c = insert_account(&catalog, "registered", "active", 10).await;
        insert_document_with_bytes(&catalog, a, 700).await;
        insert_document_with_bytes(&catalog, c, 300).await;
        let cutoff = time::OffsetDateTime::now_utc() - time::Duration::days(365);

        assert_eq!(catalog.inactive_summary(cutoff).await.unwrap(), (2, 700));
        catalog.close().await;
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn a_banned_name_creates_its_account_blocked_at_first_sign_in() {
        let catalog = test_catalog().await;
        crate::testing::reset(&catalog).await;
        let suffix = Uuid::now_v7().simple().to_string();
        let handle = format!("Spammer-{suffix}");
        let outcome = catalog
            .block_or_ban("github", &format!("@{handle}"), "operator", "spam")
            .await
            .unwrap();
        assert!(matches!(
            &outcome,
            BlockOutcome::Banned { label, .. } if *label == format!("github:{handle}")
        ));
        assert!(matches!(
            catalog
                .block_or_ban("github", &format!("@{handle}"), "operator", "spam")
                .await,
            Err(Error::Conflict(_))
        ));
        let overview = catalog.moderation_overview().await.unwrap();
        assert_eq!(overview.banned_handles.len(), 1);
        assert_eq!(overview.banned_handles[0].handle, handle);

        let account = NewAccount {
            kind: "registered".into(),
            provider: Some("github".into()),
            provider_subject: Some(format!("gh-{suffix}")),
            handle: handle.to_lowercase(),
            display_name: "Spammer".into(),
            email: None,
        };
        assert!(matches!(
            catalog.upsert_registered_account(account.clone()).await,
            Err(Error::Conflict(_))
        ));
        let status: String = sqlx::query_scalar(
            "SELECT status FROM accounts WHERE provider='github' AND provider_subject=$1",
        )
        .bind(format!("gh-{suffix}"))
        .fetch_one(catalog.pool())
        .await
        .unwrap();
        assert_eq!(status, "blocked");

        let overview = catalog.moderation_overview().await.unwrap();
        assert!(overview.banned_handles.is_empty());
        assert_eq!(overview.blocked_accounts.len(), 1);
        assert_eq!(
            overview.blocked_accounts[0].actor.as_deref(),
            Some("operator")
        );
        assert_eq!(overview.blocked_accounts[0].reason.as_deref(), Some("spam"));

        assert!(matches!(
            catalog.upsert_registered_account(account).await,
            Err(Error::Conflict(_))
        ));
        let audits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM moderation_audit WHERE action IN ('ban_handle','block_account')",
        )
        .fetch_one(catalog.pool())
        .await
        .unwrap();
        assert_eq!(audits, 2);
        catalog.close().await;
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn blocking_a_name_with_an_account_blocks_it_and_unban_removes_a_ban() {
        let catalog = test_catalog().await;
        crate::testing::reset(&catalog).await;
        let id = insert_account(&catalog, "registered", "active", 0).await;
        let outcome = catalog
            .block_or_ban("github", &format!("inactive-{id}"), "operator", "abuse")
            .await
            .unwrap();
        assert!(matches!(
            &outcome,
            BlockOutcome::Blocked { id: blocked, .. } if *blocked == id
        ));
        let status: String = sqlx::query_scalar("SELECT status FROM accounts WHERE id=$1")
            .bind(id)
            .fetch_one(catalog.pool())
            .await
            .unwrap();
        assert_eq!(status, "blocked");

        let ban = catalog
            .block_or_ban("github", "nobody-yet", "operator", "abuse")
            .await
            .unwrap();
        let BlockOutcome::Banned { id: ban_id, .. } = ban else {
            panic!("a name with no account is banned");
        };
        assert_eq!(
            catalog
                .unban_handle(ban_id, "operator", "mistake")
                .await
                .unwrap(),
            "github:nobody-yet"
        );
        assert!(matches!(
            catalog.unban_handle(ban_id, "operator", "mistake").await,
            Err(Error::NotFound)
        ));
        assert!(matches!(
            catalog
                .block_or_ban("gitlab", "someone", "operator", "abuse")
                .await,
            Err(Error::Invalid(_))
        ));

        let recent = catalog.moderation_overview().await.unwrap().recent;
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].action, "unban_handle");
        catalog.close().await;
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn account_examples_are_pending_until_marked() {
        let catalog = test_catalog().await;
        crate::testing::reset(&catalog).await;
        let suffix = Uuid::now_v7().simple().to_string();
        let account = catalog
            .upsert_registered_account(NewAccount {
                kind: "registered".into(),
                provider: Some("github".into()),
                provider_subject: Some(format!("gh-examples-{suffix}")),
                handle: format!("examples-{suffix}"),
                display_name: "Examples".into(),
                email: None,
            })
            .await
            .unwrap();
        assert!(catalog.account_examples_pending(account.id).await.unwrap());
        catalog.mark_account_examples(account.id).await.unwrap();
        assert!(!catalog.account_examples_pending(account.id).await.unwrap());
        catalog.close().await;
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn a_sign_in_issuer_is_bound_on_first_boot_and_never_moves() {
        let catalog = test_catalog().await;
        // The reset list does not name this table, so the test removes its own
        // row. A fresh provider id keeps it independent of earlier runs.
        let suffix = Uuid::now_v7().simple().to_string();
        let provider = format!("oidc-test-{}", &suffix[..12]);
        let first = "https://idp.example.test/realms/one";
        let second = "https://idp.example.test/realms/two";
        assert_eq!(
            catalog.bind_sign_in_issuer(&provider, first).await.unwrap(),
            first
        );
        assert_eq!(
            catalog.bind_sign_in_issuer(&provider, first).await.unwrap(),
            first
        );
        assert_eq!(
            catalog
                .bind_sign_in_issuer(&provider, second)
                .await
                .unwrap(),
            first
        );
        let stored: String =
            sqlx::query_scalar("SELECT issuer FROM sign_in_issuers WHERE provider=$1")
                .bind(&provider)
                .fetch_one(catalog.pool())
                .await
                .unwrap();
        assert_eq!(stored, first);
        sqlx::query("DELETE FROM sign_in_issuers WHERE provider=$1")
            .bind(&provider)
            .execute(catalog.pool())
            .await
            .unwrap();
        catalog.close().await;
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn a_name_can_be_banned_under_any_provider_id_the_server_accepts() {
        let catalog = test_catalog().await;
        let suffix = Uuid::now_v7().simple().to_string();
        let handle = format!("Banned-{suffix}");
        for provider in ["entra", "oidc-x"] {
            let outcome = catalog
                .block_or_ban(provider, &handle, "operator", "spam")
                .await
                .unwrap();
            assert!(matches!(
                &outcome,
                BlockOutcome::Banned { label, .. } if *label == format!("{provider}:{handle}")
            ));
            let rows: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM banned_handles WHERE provider=$1 AND handle=$2",
            )
            .bind(provider)
            .bind(&handle)
            .fetch_one(catalog.pool())
            .await
            .unwrap();
            assert_eq!(rows, 1);
        }
        for provider in ["oidc-", "x|y"] {
            assert!(matches!(
                catalog
                    .block_or_ban(provider, &format!("Refused-{suffix}"), "operator", "spam")
                    .await,
                Err(Error::Invalid(_))
            ));
        }
        // The reset list does not name banned_handles. Remove these rows so the
        // first sign-in test still sees only its own ban.
        sqlx::query("DELETE FROM banned_handles WHERE handle=$1")
            .bind(&handle)
            .execute(catalog.pool())
            .await
            .unwrap();
        catalog.close().await;
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn banned_handles_check_accepts_known_provider_ids_and_refuses_malformed_ones() {
        let catalog = test_catalog().await;
        let suffix = Uuid::now_v7().simple().to_string();
        let handle = format!("Constraint-{suffix}");
        sqlx::query(
            "INSERT INTO banned_handles(id,provider,handle,actor,reason)
             VALUES($1,'github',$2,'operator','spam')",
        )
        .bind(Uuid::now_v7())
        .bind(&handle)
        .execute(catalog.pool())
        .await
        .unwrap();
        let refused = sqlx::query(
            "INSERT INTO banned_handles(id,provider,handle,actor,reason)
             VALUES($1,'oidc-',$2,'operator','spam')",
        )
        .bind(Uuid::now_v7())
        .bind(&handle)
        .execute(catalog.pool())
        .await
        .expect_err("oidc- has no slug");
        assert_eq!(
            refused
                .as_database_error()
                .and_then(|error| error.code())
                .as_deref(),
            Some("23514")
        );
        sqlx::query(
            "INSERT INTO banned_handles(id,provider,handle,actor,reason)
             VALUES($1,'oidc-x',$2,'operator','spam')",
        )
        .bind(Uuid::now_v7())
        .bind(&handle)
        .execute(catalog.pool())
        .await
        .unwrap();
        // The reset list does not name banned_handles, so remove these rows.
        sqlx::query("DELETE FROM banned_handles WHERE handle=$1")
            .bind(&handle)
            .execute(catalog.pool())
            .await
            .unwrap();
        catalog.close().await;
    }
}
