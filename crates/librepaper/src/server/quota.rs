//! Account storage counters and simple, advisory retention previews.
use super::*;
use crate::document::quota::{
    effective_retention, select_retained, QuotaPreferences, RetentionBounds,
};
use crate::storage::catalog::{AccountStorageUsage, QuotaPreferencesRecord};
use std::collections::BTreeMap;

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
fn default_record(account_id: &str) -> QuotaPreferencesRecord {
    QuotaPreferencesRecord {
        account_id: account_id.into(),
        revision: 0,
        payload: serde_json::to_string(&QuotaPreferences::default())
            .expect("preferences serialize"),
        updated_at: 0,
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
        let account_id = account_id.to_string();
        catalog
            .execute_catalog(SERVER_JOB_BYTES + account_id.len(), move |catalog| {
                Ok(catalog
                    .quota_preferences(&account_id)?
                    .unwrap_or_else(|| default_record(&account_id)))
            })
            .await
            .map_err(|error| write_json(503, &json!({"error": error.to_string()})))
    }

    #[allow(clippy::result_large_err)]
    async fn quota_usage(&self, account_id: &str) -> Result<AccountStorageUsage, Reply> {
        let Some(catalog) = &self.store.catalog else {
            return Err(write_json(
                503,
                &json!({"error": "local catalogue unavailable"}),
            ));
        };
        let account_id = account_id.to_string();
        catalog
            .execute_catalog(SERVER_JOB_BYTES + account_id.len(), move |catalog| {
                catalog.account_storage_usage(&account_id)
            })
            .await
            .map_err(|error| write_json(503, &json!({"error": error.to_string()})))
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
        let preview = catalog
            .execute_catalog(SERVER_JOB_BYTES + account_id.len(), move |catalog| {
                let points = catalog.account_checkpoints(&account_id)?;
                let references = catalog.account_open_annotation_references(&account_id)?;
                let mut grouped = BTreeMap::<String, Vec<_>>::new();
                for point in points {
                    grouped.entry(point.slug.clone()).or_default().push(point);
                }
                let mut removed = 0usize;
                let mut protected = 0usize;
                for (slug, points) in grouped {
                    let manifest = crate::document::history::Manifest::from_catalog_rows(points)
                        .map_err(crate::storage::catalog::CatalogError::Invalid)?;
                    let refs = references.get(&slug).cloned().unwrap_or_default();
                    let selected = select_retained(
                        &manifest.checkpoints,
                        now,
                        &asked.preferences,
                        &policy_bounds,
                        &refs,
                    );
                    removed += selected.removed.len();
                    protected += selected.protected.len();
                }
                Ok((removed, protected))
            })
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
        let payload = match serde_json::to_string(&asked.preferences) {
            Ok(payload) => payload,
            Err(error) => return write_json(400, &json!({"error":error.to_string()})),
        };
        let Some(catalog) = &self.store.catalog else {
            return write_json(503, &json!({"error":"catalog unavailable"}));
        };
        let saved = catalog
            .execute_catalog(
                SERVER_JOB_BYTES + account_id.len() + generation.len() + payload.len(),
                move |catalog| {
                    catalog.save_quota_preferences_authorized(
                        &account_id,
                        &generation,
                        asked.revision,
                        &payload,
                        crate::util::now_millis(),
                    )
                },
            )
            .await;
        match saved {
            Ok(record) => write_json(
                200,
                &json!({"status":"saved", "revision":record.revision, "graceMs":86_400_000}),
            ),
            Err(crate::storage::catalog::CatalogExecError::Catalog(
                crate::storage::catalog::CatalogError::Refused(_, error),
            )) => write_json(403, &json!({"error":error})),
            Err(crate::storage::catalog::CatalogExecError::Catalog(
                crate::storage::catalog::CatalogError::Conflict(error),
            )) => write_json(409, &json!({"error":error})),
            Err(error) => write_json(503, &json!({"error":error.to_string()})),
        }
    }
}
