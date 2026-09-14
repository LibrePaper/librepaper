//! Account storage counters and simple, advisory retention previews.
use super::*;
use crate::document::quota::{
    effective_retention, select_retained, QuotaPreferences, RetentionBounds,
};
#[derive(Clone)]
struct QuotaPreferencesRecord {
    revision: i64,
    payload: String,
}
struct AccountStorageUsage {
    charged_bytes: i64,
    document_count: i64,
    checkpoint_count: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreferenceRequest {
    revision: i64,
    preferences: QuotaPreferences,
}

fn bounds(server: &Server) -> RetentionBounds {
    RetentionBounds {
        hard_quota: server.config.storage.per_owner,
        ..RetentionBounds::default()
    }
}
fn default_record(_account_id: &str) -> QuotaPreferencesRecord {
    QuotaPreferencesRecord {
        revision: 0,
        payload: serde_json::to_string(&QuotaPreferences::default())
            .expect("preferences serialize"),
    }
}
fn decode_preferences(record: &QuotaPreferencesRecord) -> (QuotaPreferences, bool) {
    match serde_json::from_str::<QuotaPreferences>(&record.payload) {
        Ok(preferences) if preferences.validate().is_ok() => (preferences, false),
        _ => (QuotaPreferences::default(), true),
    }
}
fn usage_json(usage: &AccountStorageUsage) -> Value {
    json!({"chargedBytes": usage.charged_bytes, "documentCount": usage.document_count,
        "checkpointCount": usage.checkpoint_count, "physicalAccounting": true})
}
impl Server {
    #[allow(clippy::result_large_err)] // Route errors are ready-to-return HTTP responses.
    async fn quota_account(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
    ) -> Result<(String, String), Reply> {
        if Self::is_automation(headers) {
            return Err(write_json(
                403,
                &json!({"error": "quota preferences are unavailable in automation mode"}),
            ));
        }
        if cross_site_refused(headers, arrival) {
            return Err(write_json(403, &cross_site_refusal()));
        }
        let identity = match self.authenticated_identity(headers, arrival).await {
            Ok(identity) if identity.is_signed_in() && self.provider_configured(&identity) => {
                identity
            }
            Ok(_) | Err(AuthenticationFailure::Invalid) => {
                return Err(write_json(
                    401,
                    &json!({"error": "sign in to manage quota preferences"}),
                ))
            }
            Err(AuthenticationFailure::Unavailable) => {
                return Err(write_json(
                    503,
                    &json!({"error": "authentication service temporarily unavailable"}),
                ))
            }
        };
        Ok((identity.id, identity.session_generation))
    }

    #[allow(clippy::result_large_err)] // As quota_account: avoid boxing the HTTP response.
    async fn quota_record(&self, account_id: &str) -> Result<QuotaPreferencesRecord, Reply> {
        let Some(catalog) = &self.store.catalog else {
            return Err(write_json(
                503,
                &json!({"error": "local catalogue unavailable"}),
            ));
        };
        let id = uuid::Uuid::parse_str(account_id)
            .map_err(|_| write_json(401, &json!({"error":"invalid account"})))?;
        let account = catalog
            .account(id)
            .await
            .map_err(|e| write_json(503, &json!({"error":e.to_string()})))?
            .ok_or_else(|| write_json(401, &json!({"error":"account not found"})))?;
        if account.preferences.is_null() || account.preferences == json!({}) {
            return Ok(default_record(account_id));
        }
        Ok(QuotaPreferencesRecord {
            revision: account
                .preferences
                .get("revision")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            payload: account
                .preferences
                .get("value")
                .map(Value::to_string)
                .unwrap_or_else(|| serde_json::to_string(&QuotaPreferences::default()).unwrap()),
        })
    }

    #[allow(clippy::result_large_err)]
    async fn quota_usage(&self, account_id: &str) -> Result<AccountStorageUsage, Reply> {
        let Some(catalog) = &self.store.catalog else {
            return Err(write_json(
                503,
                &json!({"error": "local catalogue unavailable"}),
            ));
        };
        let id = uuid::Uuid::parse_str(account_id)
            .map_err(|_| write_json(401, &json!({"error":"invalid account"})))?;
        let charged_bytes = catalog
            .usage_bytes(Some(id))
            .await
            .map_err(|e| write_json(503, &json!({"error":e.to_string()})))?;
        let (document_count, checkpoints) = catalog
            .document_counts_by_owner(id)
            .await
            .map_err(|e| write_json(503, &json!({"error":e.to_string()})))?;
        Ok(AccountStorageUsage {
            charged_bytes,
            document_count,
            checkpoint_count: checkpoints,
        })
    }

    pub(super) async fn handle_quota_storage(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
    ) -> Reply {
        let (account_id, _) = match self.quota_account(request.headers(), arrival).await {
            Ok(id) => id,
            Err(reply) => return reply,
        };
        let record = match self.quota_record(&account_id).await {
            Ok(record) => record,
            Err(reply) => return reply,
        };
        let usage = match self.quota_usage(&account_id).await {
            Ok(usage) => usage,
            Err(reply) => return reply,
        };
        let (preferences, incompatible) = decode_preferences(&record);
        let mut response = write_json(
            200,
            &json!({
                "canManage": !incompatible, "revision": record.revision, "preferences": preferences,
                "effective": { "retention": effective_retention(&preferences, &bounds(self)), "incompatible": incompatible },
                "constraints": { "hardQuotaBytes": self.config.storage.per_owner, "maxCheckpointCount": 4096,
                    "graceMs": 86_400_000, "physicalAccounting": true },
                "usage": usage_json(&usage), "profiles": ["default", "manual", "custom"]
            }),
        );
        set(&mut response, "cache-control", "private, no-store");
        response
    }
    pub(super) async fn handle_quota_preview(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
    ) -> Reply {
        let (account_id, _) = match self.quota_account(request.headers(), arrival).await {
            Ok(id) => id,
            Err(reply) => return reply,
        };
        let body = match to_bytes(request.into_body(), 65_536).await {
            Ok(body) => body,
            Err(_) => return write_json(413, &json!({"error":"preference request too large"})),
        };
        let asked: PreferenceRequest = match serde_json::from_slice(&body) {
            Ok(value) => value,
            Err(error) => return write_json(400, &json!({"error":error.to_string()})),
        };
        if let Err(error) = asked.preferences.validate() {
            return write_json(400, &json!({"error":error}));
        }
        let record = match self.quota_record(&account_id).await {
            Ok(record) => record,
            Err(reply) => return reply,
        };
        if record.revision != asked.revision {
            return write_json(
                409,
                &json!({"error":"preference revision is stale","revision":record.revision}),
            );
        }
        let Some(catalog) = &self.store.catalog else {
            return write_json(503, &json!({"error":"catalog unavailable"}));
        };
        let policy_bounds = bounds(self);
        let effective = effective_retention(&asked.preferences, &policy_bounds);
        let now = crate::util::now_millis();
        let preview: Result<(usize, usize), crate::storage::postgres::Error> = async {
            let id = uuid::Uuid::parse_str(&account_id)
                .map_err(|_| crate::storage::postgres::Error::Invalid("invalid account".into()))?;
            let mut removed = 0;
            let mut protected = 0;
            let mut document_cursor = None;
            loop {
                let docs = catalog
                    .documents_by_owner_page(id, document_cursor, 200)
                    .await?;
                if docs.is_empty() {
                    break;
                }
                for doc in &docs {
                    let mut rows = Vec::new();
                    let mut version_cursor = None;
                    loop {
                        let page = catalog.version_page(doc.id, version_cursor, 200).await?;
                        if page.is_empty() {
                            break;
                        }
                        version_cursor = page.last().map(|version| version.sequence);
                        rows.extend(page);
                    }
                    rows.reverse();
                    let points = rows
                        .iter()
                        .map(crate::room::checkpoint::checkpoint_from_version)
                        .collect::<Vec<_>>();
                    let selected = select_retained(
                        &points,
                        now,
                        &asked.preferences,
                        &policy_bounds,
                        &std::collections::BTreeSet::new(),
                    );
                    removed += selected.removed.len();
                    protected += selected.protected.len();
                }
                document_cursor = docs.last().map(|doc| (doc.updated_at, doc.id));
            }
            Ok((removed, protected))
        }
        .await;
        match preview {
            Ok((removed, protected)) => write_json(
                200,
                &json!({"revision": record.revision, "evaluatedAt": now,
                "affectedCount": removed, "protectedCount": protected, "estimate": true,
                "graceMs": 86_400_000, "effective": effective}),
            ),
            Err(error) => write_json(503, &json!({"error":error.to_string()})),
        }
    }
    pub(super) async fn handle_quota_apply(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
    ) -> Reply {
        let (account_id, generation) = match self.quota_account(request.headers(), arrival).await {
            Ok(id) => id,
            Err(reply) => return reply,
        };
        let body = match to_bytes(request.into_body(), 65_536).await {
            Ok(body) => body,
            Err(_) => return write_json(413, &json!({"error":"preference request too large"})),
        };
        let asked: PreferenceRequest = match serde_json::from_slice(&body) {
            Ok(value) => value,
            Err(error) => return write_json(400, &json!({"error":error.to_string()})),
        };
        if let Err(error) = asked.preferences.validate() {
            return write_json(400, &json!({"error":error}));
        }
        let payload = match serde_json::to_value(&asked.preferences) {
            Ok(payload) => payload,
            Err(error) => return write_json(400, &json!({"error":error.to_string()})),
        };
        let Some(catalog) = &self.store.catalog else {
            return write_json(503, &json!({"error":"catalog unavailable"}));
        };
        let saved = async {
            let id = uuid::Uuid::parse_str(&account_id)
                .map_err(|_| crate::storage::postgres::Error::Invalid("invalid account".into()))?;
            let account = catalog
                .account(id)
                .await?
                .ok_or(crate::storage::postgres::Error::NotFound)?;
            if account.session_generation.to_string() != generation {
                return Err(crate::storage::postgres::Error::Conflict(
                    "account session changed".into(),
                ));
            }
            let revision = account
                .preferences
                .get("revision")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            if revision != asked.revision {
                return Err(crate::storage::postgres::Error::Conflict(
                    "preference revision is stale".into(),
                ));
            }
            let next = revision + 1;
            let changed = sqlx::query(
                "UPDATE accounts SET preferences=$2
                 WHERE id=$1 AND COALESCE((preferences->>'revision')::bigint,0)=$3",
            )
            .bind(id)
            .bind(json!({"revision":next,"value":payload}))
            .bind(revision)
            .execute(catalog.pool())
            .await?;
            if changed.rows_affected() != 1 {
                return Err(crate::storage::postgres::Error::Conflict(
                    "preference revision is stale".into(),
                ));
            }
            Ok(next)
        }
        .await;
        match saved {
            Ok(revision) => write_json(
                200,
                &json!({"status":"saved", "revision":revision, "graceMs":86_400_000}),
            ),
            Err(crate::storage::postgres::Error::Conflict(error)) => {
                write_json(409, &json!({"error":error}))
            }
            Err(error) => write_json(503, &json!({"error":error.to_string()})),
        }
    }
}
