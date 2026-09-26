//! Account storage status, including each document's editing log.

use super::*;

use crate::storage::postgres::DocumentStorage;

#[derive(Clone)]
struct AccountStorageUsage {
    charged_bytes: i64,
    document_count: i64,
    label_count: i64,
    documents: Vec<DocumentStorage>,
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
        let (document_count, labels) = catalog
            .document_counts_by_owner(id)
            .await
            .map_err(|e| write_json(503, &json!({"error":e.to_string()})))?;
        let documents = catalog
            .document_storage_by_owner(id)
            .await
            .map_err(|e| write_json(503, &json!({"error":e.to_string()})))?;
        Ok(AccountStorageUsage {
            charged_bytes,
            document_count,
            label_count: labels,
            documents,
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
        let documents = usage
            .documents
            .iter()
            .map(|d| {
                json!({
                    "id": d.id,
                    "slug": d.slug,
                    "title": d.title,
                    "figureBytes": d.figure_bytes,
                    "archiveBytes": d.archive_bytes,
                    "historyBytes": d.history_bytes,
                })
            })
            .collect::<Vec<_>>();
        let mut response = write_json(
            200,
            &json!({
                "usage": {
                    "chargedBytes": usage.charged_bytes,
                    "documentCount": usage.document_count,
                    "labelCount": usage.label_count,
                    "hardQuotaBytes": self.config.storage.per_owner,
                    "documents": documents,
                },
                "constraints": {
                    "hardQuotaBytes": self.config.storage.per_owner,
                },
            }),
        );
        set(&mut response, "cache-control", "private, no-store");
        response
    }
}
