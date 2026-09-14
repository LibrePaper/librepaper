//! Account storage status and fixed retention policy reporting.

use super::*;
use crate::document::quota::RetentionPolicy;

#[derive(Clone)]
struct AccountStorageUsage {
    charged_bytes: i64,
    document_count: i64,
    checkpoint_count: i64,
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
                &json!({"error": "storage policy is not available for automation mode"}),
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
                    &json!({"error": "sign in to view storage"}),
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
    async fn quota_usage(&self, account_id: &str) -> Result<AccountStorageUsage, Reply> {
        let catalog = &self.store.catalog;
        let id = uuid::Uuid::parse_str(account_id)
            .map_err(|_| write_json(401, &json!({"error":"invalid account"})))?;
        let _account = catalog
            .account(id)
            .await
            .map_err(|e| write_json(503, &json!({"error":e.to_string()})))?
            .ok_or_else(|| write_json(401, &json!({"error":"account not found"})))?;
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
        let usage = match self.quota_usage(&account_id).await {
            Ok(usage) => usage,
            Err(reply) => return reply,
        };
        let policy = RetentionPolicy::fixed();
        let mut response = write_json(
            200,
            &json!({
                "usage": {
                    "chargedBytes": usage.charged_bytes,
                    "documentCount": usage.document_count,
                    "checkpointCount": usage.checkpoint_count,
                    "hardQuotaBytes": self.config.storage.per_owner,
                },
                "constraints": {
                    "hardQuotaBytes": self.config.storage.per_owner,
                    "maxCheckpointCount": policy.max_checkpoint_count,
                },
                "policy": policy,
            }),
        );
        set(&mut response, "cache-control", "private, no-store");
        response
    }
}
