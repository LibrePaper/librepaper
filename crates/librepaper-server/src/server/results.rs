//! Shared Quarto results: `/api/documents/{slug}/results`.
//!
//! One record per project, uploaded by an editor after a local render and
//! shown by every collaborator's draft. It is not a project file: it is never
//! listed, synced or versioned. Its bytes count toward the owner's storage.

use serde::Serialize;

use super::*;

const MAX_BODY_BYTES: usize = 20 * 1024 * 1024;
const MAX_CELLS: usize = 2000;
const MAX_PATH_BYTES: usize = 1024;
const MAX_LABEL_BYTES: usize = 256;
const QUOTA_MESSAGE: &str = "Shared results would exceed your storage quota.";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ResultsRecord {
    version: u32,
    path: String,
    cells: Vec<ResultsCell>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ResultsCell {
    fingerprint: String,
    label: Option<String>,
    html: String,
}

/// Checks the limits serde cannot express. The message is the 400 body.
fn validate(record: &ResultsRecord) -> Result<(), &'static str> {
    if record.version != 1 {
        return Err("unsupported results version");
    }
    if record.path.is_empty() || record.path.len() > MAX_PATH_BYTES {
        return Err("results path must be 1 to 1024 bytes");
    }
    if record.cells.len() > MAX_CELLS {
        return Err("too many result cells");
    }
    for cell in &record.cells {
        let hex = cell.fingerprint.len() == 64
            && cell
                .fingerprint
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !hex {
            return Err("a cell fingerprint must be 64 lowercase hex characters");
        }
        if cell
            .label
            .as_ref()
            .is_some_and(|l| l.len() > MAX_LABEL_BYTES)
        {
            return Err("a cell label is too long");
        }
    }
    Ok(())
}

fn rfc3339(at: time::OffsetDateTime) -> String {
    at.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

impl Server {
    pub(super) async fn handle_results(
        &self,
        request: Request<Body>,
        context: &RequestContext,
        slug: &str,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error": "bad slug"}));
        }
        let headers = request.headers().clone();
        if cross_site_refused(&headers, &context.arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let query = request.uri().query().map(str::to_string);
        let mut response = match request.method().as_str() {
            "GET" => {
                let auth = match self
                    .authorize::<access::rung::Read>(context, &headers, slug, query.as_deref())
                    .await
                {
                    Ok(auth) => auth,
                    Err(refusal) => return access::deny(&refusal),
                };
                self.read_results(&auth.entry.storage_id).await
            }
            "PUT" => {
                let auth = match self
                    .authorize::<access::rung::Edit>(context, &headers, slug, query.as_deref())
                    .await
                {
                    Ok(auth) => auth,
                    Err(refusal) => return access::deny(&refusal),
                };
                let bytes = match to_bytes(request.into_body(), MAX_BODY_BYTES).await {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        return write_json(413, &json!({"error": "shared results are too large"}))
                    }
                };
                let record: ResultsRecord = match serde_json::from_slice(&bytes) {
                    Ok(record) => record,
                    Err(_) => return write_json(400, &json!({"error": "invalid results record"})),
                };
                if let Err(message) = validate(&record) {
                    return write_json(400, &json!({"error": message}));
                }
                let Ok(body) = serde_json::to_string(&record) else {
                    return write_json(400, &json!({"error": "invalid results record"}));
                };
                let written = self
                    .mutate(&auth, |viewer, entry| async move {
                        let Ok(document_id) = uuid::Uuid::parse_str(&entry.storage_id) else {
                            return write_json(404, &json!({"error": "not found"}));
                        };
                        let name = viewer.authorship("A collaborator");
                        match self
                            .store
                            .catalog
                            .put_results(document_id, &body, viewer.account_id(), name.display())
                            .await
                        {
                            Ok(done) => write_json(
                                200,
                                &json!({
                                    "bytes": done.byte_length,
                                    "updatedAt": rfc3339(done.updated_at),
                                }),
                            ),
                            Err(error) => results_error(error),
                        }
                    })
                    .await;
                match written {
                    Ok(reply) => {
                        if reply.status() == 200 {
                            self.announce_results(slug).await;
                        }
                        reply
                    }
                    Err(refusal) => return access::deny(&refusal),
                }
            }
            "DELETE" => {
                let auth = match self
                    .authorize::<access::rung::Edit>(context, &headers, slug, query.as_deref())
                    .await
                {
                    Ok(auth) => auth,
                    Err(refusal) => return access::deny(&refusal),
                };
                let deleted = self
                    .mutate(&auth, |_, entry| async move {
                        let Ok(document_id) = uuid::Uuid::parse_str(&entry.storage_id) else {
                            return write_json(404, &json!({"error": "not found"}));
                        };
                        match self.store.catalog.delete_results(document_id).await {
                            Ok(freed) => write_json(200, &json!({"bytes": freed})),
                            Err(error) => results_error(error),
                        }
                    })
                    .await;
                match deleted {
                    Ok(reply) => {
                        if reply.status() == 200 {
                            self.announce_results(slug).await;
                        }
                        reply
                    }
                    Err(refusal) => return access::deny(&refusal),
                }
            }
            _ => return write_json(405, &json!({"error": "unsupported results operation"})),
        };
        set(&mut response, "cache-control", "private, no-store");
        response
    }

    async fn read_results(&self, storage_id: &str) -> Reply {
        let Ok(document_id) = uuid::Uuid::parse_str(storage_id) else {
            return write_json(404, &json!({"error": "no shared results"}));
        };
        let stored = match self.store.catalog.get_results(document_id).await {
            Ok(Some(stored)) => stored,
            Ok(None) => return write_json(404, &json!({"error": "no shared results"})),
            Err(error) => return results_error(error),
        };
        let Ok(Value::Object(mut record)) = serde_json::from_str::<Value>(&stored.body) else {
            return write_json(500, &json!({"error": "stored results are unreadable"}));
        };
        record.insert("uploaderName".into(), json!(stored.uploader_name));
        record.insert("updatedAt".into(), json!(rfc3339(stored.updated_at)));
        record.insert("bytes".into(), json!(stored.byte_length));
        write_json(200, &Value::Object(record))
    }

    /// Tells everyone in the room that the record changed. A room that cannot
    /// open is not an error: nobody is there to tell.
    async fn announce_results(&self, slug: &str) {
        if let Ok(room) = self.rooms.get(slug).await {
            room.broadcast(&json!({"type": "results-changed"})).await;
        }
    }
}

fn results_error(error: librepaper_engine::storage::postgres::Error) -> Reply {
    use librepaper_engine::storage::postgres::{Error, QuotaScope};
    match error {
        Error::Quota(QuotaScope::Owner) => write_json(413, &json!({"error": QUOTA_MESSAGE})),
        Error::Quota(QuotaScope::Deployment) => write_json(
            413,
            &json!({"error": "This server has no room left for shared results."}),
        ),
        Error::NotFound => write_json(404, &json!({"error": "not found"})),
        Error::Busy => write_json(503, &json!({"error": "storage is busy; try again shortly"})),
        _ => write_json(503, &json!({"error": "could not store shared results"})),
    }
}
