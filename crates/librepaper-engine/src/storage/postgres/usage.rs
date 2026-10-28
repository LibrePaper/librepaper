//! The one definition of what counts toward a storage quota.
//!
//! Every quota check names the sources it charges, so a deliberate omission
//! (the sequencer's soft ledger leaves out pending reservations) shows at
//! the call site instead of hiding in a hand-written sum.

use sqlx::PgConnection;
use uuid::Uuid;

use super::{Error, Result};

/// Whose bytes are summed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UsageScope {
    /// Every byte the deployment stores.
    Deployment,
    /// Every byte the documents of one account store.
    Owner(Uuid),
}

/// The kinds of billable bytes a sum includes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Sources(u8);

impl Sources {
    pub(super) const ARCHIVES: Self = Self(1);
    /// In the deployment scope, assets and archives share one retained-bytes
    /// counter, so selecting either one includes both.
    pub(super) const ASSETS: Self = Self(1 << 1);
    pub(super) const SNAPSHOTS: Self = Self(1 << 2);
    pub(super) const UNCOMPACTED_AND_CHAT: Self = Self(1 << 3);
    pub(super) const RESERVATIONS: Self = Self(1 << 4);
    pub(super) const ALL: Self = Self(0b1_1111);
    /// Everything but pending source reservations.
    pub(super) const DURABLE: Self = Self(0b0_1111);

    fn contains(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

impl std::ops::BitOr for Sources {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Sums the selected sources for one scope in a single statement.
pub(super) async fn usage_bytes(
    tx: &mut PgConnection,
    scope: UsageScope,
    sources: Sources,
) -> Result<i64> {
    let (sql, owner) = match scope {
        UsageScope::Deployment => (deployment_sql(sources), None),
        UsageScope::Owner(owner) => (owner_sql(sources), Some(owner)),
    };
    let Some(sql) = sql else {
        return Ok(0);
    };
    let query = sqlx::query_scalar::<_, i64>(&sql);
    let query = match owner {
        Some(owner) => query.bind(owner),
        None => query,
    };
    query.fetch_one(tx).await.map_err(Error::from)
}

/// Takes the deployment-wide accounting mutex. Every quota decision that
/// reads a total holds it until its transaction ends.
pub(super) async fn lock_storage_usage(tx: &mut PgConnection) -> Result<()> {
    sqlx::query("SELECT bytes FROM storage_usage WHERE singleton FOR UPDATE")
        .fetch_one(tx)
        .await?;
    Ok(())
}

fn deployment_sql(sources: Sources) -> Option<String> {
    let mut terms = Vec::new();
    if sources.contains(Sources::ARCHIVES | Sources::ASSETS) {
        terms.push("(SELECT bytes FROM storage_usage WHERE singleton)");
    }
    if sources.contains(Sources::SNAPSHOTS) {
        terms.push(
            "COALESCE((SELECT sum(snapshot_bytes)::bigint FROM document_snapshots WHERE delete_after IS NULL),0)",
        );
    }
    if sources.contains(Sources::UNCOMPACTED_AND_CHAT) {
        terms.push(
            "COALESCE((SELECT sum(uncompacted_update_bytes + chat_bytes)::bigint FROM documents),0)",
        );
    }
    if sources.contains(Sources::RESERVATIONS) {
        terms.push("COALESCE((SELECT sum(bytes)::bigint FROM pending_log_reservations),0)");
    }
    if terms.is_empty() {
        return None;
    }
    Some(format!("SELECT ({})::bigint", terms.join(" + ")))
}

fn owner_sql(sources: Sources) -> Option<String> {
    let mut parts = Vec::new();
    if sources.contains(Sources::ARCHIVES) {
        parts.push(
            "SELECT a.byte_length AS bytes FROM document_archives a \
             JOIN documents d ON d.id=a.document_id WHERE d.owner_id=$1",
        );
    }
    if sources.contains(Sources::ASSETS) {
        parts.push(
            "SELECT a.byte_length AS bytes FROM document_assets a \
             JOIN documents d ON d.id=a.document_id WHERE d.owner_id=$1",
        );
    }
    if sources.contains(Sources::SNAPSHOTS) {
        parts.push(
            "SELECT s.snapshot_bytes AS bytes FROM document_snapshots s \
             JOIN documents d ON d.id=s.document_id \
             WHERE d.owner_id=$1 AND s.delete_after IS NULL",
        );
    }
    if sources.contains(Sources::UNCOMPACTED_AND_CHAT) {
        parts.push(
            "SELECT d.uncompacted_update_bytes + d.chat_bytes AS bytes FROM documents d \
             WHERE d.owner_id=$1",
        );
    }
    if sources.contains(Sources::RESERVATIONS) {
        parts.push(
            "SELECT r.bytes AS bytes FROM pending_log_reservations r \
             JOIN documents d ON d.id=r.document_id WHERE d.owner_id=$1",
        );
    }
    if parts.is_empty() {
        return None;
    }
    Some(format!(
        "SELECT COALESCE(sum(bytes),0)::bigint FROM ({}) usage",
        parts.join(" UNION ALL ")
    ))
}
