//! Account-wide history preferences, quota status, and safe preview/apply.
//!
//! The routes intentionally expose the catalogue's physical-accounting
//! capability.  During the legacy-ledger migration a preview can select age
//! candidates, but it never claims that logical tree bytes are reclaimable.

use super::*;
use crate::document::quota::{
    effective_retention, select_retained, QuotaPreferences, RetentionBounds,
};
use crate::storage::catalog::{AccountStorageUsage, QuotaPreferencesRecord};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreviewRequest {
    #[serde(default)]
    revision: i64,
    preferences: QuotaPreferences,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApplyRequest {
    revision: i64,
    generation: String,
    preferences: QuotaPreferences,
    confirmed: bool,
}

fn bounds(server: &Server) -> RetentionBounds {
    RetentionBounds {
        hard_quota: server.config.storage.per_owner,
        max_checkpoint_count: if server.config.session.history_max == 0 {
            None
        } else {
            Some(server.config.session.history_max as u32)
        },
        ..RetentionBounds::default()
    }
}

fn policy_points(
    points: Vec<crate::storage::catalog::Checkpoint>,
) -> Vec<crate::document::history::Checkpoint> {
    crate::document::history::Manifest::from_catalog_rows(points)
        .map(|manifest| manifest.checkpoints)
        .unwrap_or_default()
}

fn quota_event_time(value: &str) -> i64 {
    value
        .parse::<i64>()
        .ok()
        .or_else(|| {
            time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
                .ok()
                .map(|time| time.unix_timestamp())
        })
        .unwrap_or(0)
}

fn default_record(account_id: &str) -> QuotaPreferencesRecord {
    QuotaPreferencesRecord {
        account_id: account_id.to_string(),
        revision: 0,
        payload: serde_json::to_string(&QuotaPreferences::default())
            .unwrap_or_else(|_| "{}".into()),
        policy_generation: String::new(),
        updated_at: 0,
    }
}

fn decode_preferences(record: &QuotaPreferencesRecord) -> (QuotaPreferences, bool) {
    match serde_json::from_str::<QuotaPreferences>(&record.payload) {
        Ok(preferences) if preferences.validate().is_ok() => (preferences, false),
        Ok(preferences) => (preferences, true),
        Err(_) => (QuotaPreferences::default(), true),
    }
}

fn generation(
    preferences: &QuotaPreferences,
    revision: i64,
    usage: &AccountStorageUsage,
    candidates: &str,
    evaluated_at: i64,
) -> String {
    let payload = serde_json::to_vec(preferences).unwrap_or_default();
    let mut digest = sha2::Sha256::new();
    digest.update(revision.to_be_bytes());
    digest.update(usage.charged_bytes.to_be_bytes());
    // Retention selection changes only at UTC bucket boundaries. Binding the
    // five-minute evaluation bucket makes a preview replayable across normal
    // request latency while still rejecting a preview that crossed a policy
    // boundary.
    for width in [300_i64, 3_600, 86_400, 604_800] {
        digest.update(evaluated_at.div_euclid(width).to_be_bytes());
    }
    digest.update(candidates.as_bytes());
    digest.update(payload);
    hex::encode(digest.finalize())
}

fn candidate_fingerprint(
    points: &[(String, Vec<crate::storage::catalog::Checkpoint>)],
    references: &BTreeMap<String, BTreeSet<String>>,
    selected: &[(String, String)],
) -> String {
    let mut digest = sha2::Sha256::new();
    for (slug, checkpoints) in points {
        digest.update(slug.as_bytes());
        for point in checkpoints {
            digest.update(point.sha.as_bytes());
            digest.update(point.content_sha().as_bytes());
            digest.update(point.at.as_bytes());
            digest.update(point.seq.to_be_bytes());
            digest.update(point.parent.as_bytes());
            digest.update(point.label.as_bytes());
            digest.update(point.why.as_bytes());
            digest.update(point.changed.as_deref().unwrap_or_default().as_bytes());
        }
    }
    for (slug, values) in references {
        digest.update(slug.as_bytes());
        for value in values {
            digest.update(value.as_bytes());
        }
    }
    // Age-tier boundaries can be crossed between requests even within the
    // same five-minute wall-clock bucket. Bind the exact approved set too.
    digest.update(serde_json::to_vec(selected).expect("checkpoint keys serialize"));
    hex::encode(digest.finalize())
}

fn usage_json(usage: &AccountStorageUsage) -> Value {
    json!({
        "chargedBytes": usage.charged_bytes,
        "liveBytes": usage.live_bytes,
        "sourceHistoryBytes": usage.history_bytes,
        "assetBytes": usage.asset_bytes,
        "metadataBytes": usage.metadata_bytes,
        "documentCount": usage.document_count,
        "checkpointCount": usage.checkpoint_count,
        "physicalAccounting": usage.physical_accounting,
        "historySizeKind": if usage.physical_accounting { "charged-physical" } else { "logical-estimate" },
    })
}

fn profile_density(profile: &str) -> i8 {
    match profile {
        "moreRecoveryPoints" | "more-recovery-points" => 2,
        "balanced" => 1,
        "useLessStorage" | "use-less-storage" => 0,
        _ => 1,
    }
}

fn is_destructive_change(old: &QuotaPreferences, new: &QuotaPreferences, hard_quota: i64) -> bool {
    new.history_budget.target_bytes(hard_quota) < old.history_budget.target_bytes(hard_quota)
        || profile_density(&new.retention_profile) < profile_density(&old.retention_profile)
        || old.milestone_preferences.named && !new.milestone_preferences.named
        || old.milestone_preferences.cli && !new.milestone_preferences.cli
        || old.milestone_preferences.publish && !new.milestone_preferences.publish
        || old.milestone_preferences.restore && !new.milestone_preferences.restore
        || old.milestone_preferences.accept && !new.milestone_preferences.accept
        || old.milestone_preferences.comment && !new.milestone_preferences.comment
}

impl Server {
    #[allow(clippy::result_large_err)] // Route errors are ready-to-return HTTP responses.
    async fn quota_account(&self, headers: &HeaderMap, arrival: &Arrival) -> Result<String, Reply> {
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
        Ok(identity.id)
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

    #[allow(clippy::result_large_err)]
    async fn quota_job(
        &self,
        account_id: &str,
    ) -> Result<Option<crate::storage::catalog::RetentionJob>, Reply> {
        let Some(catalog) = &self.store.catalog else {
            return Err(write_json(
                503,
                &json!({"error": "local catalogue unavailable"}),
            ));
        };
        let id = account_id.to_string();
        catalog
            .execute_catalog(SERVER_JOB_BYTES + id.len(), move |catalog| {
                catalog.latest_retention_job(&id)
            })
            .await
            .map_err(|error| write_json(503, &json!({"error": error.to_string()})))
    }

    pub(super) async fn handle_quota_storage(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
    ) -> Reply {
        let account_id = match self.quota_account(request.headers(), arrival).await {
            Ok(account_id) => account_id,
            Err(response) => return response,
        };
        let record = match self.quota_record(&account_id).await {
            Ok(record) => record,
            Err(response) => return response,
        };
        let usage = match self.quota_usage(&account_id).await {
            Ok(usage) => usage,
            Err(response) => return response,
        };
        let (preferences, incompatible) = decode_preferences(&record);
        let job = match self.quota_job(&account_id).await {
            Ok(job) => job,
            Err(response) => return response,
        };
        let last_completed = if let Some(catalog) = &self.store.catalog {
            let id = account_id.clone();
            match catalog
                .execute_catalog(SERVER_JOB_BYTES + id.len(), move |catalog| {
                    catalog.last_completed_retention_generation(&id)
                })
                .await
            {
                Ok(generation) => generation,
                Err(error) => return write_json(503, &json!({"error": error.to_string()})),
            }
        } else {
            None
        };
        let retention = effective_retention(&preferences, &bounds(self));
        let hard_quota = self.config.storage.per_owner;
        let soft_target = preferences.history_budget.target_bytes(hard_quota);
        let mut response = write_json(
            200,
            &json!({
                "canManage": !incompatible,
                "revision": record.revision,
                "preferences": preferences.clone(),
                "effective": {
                    "historyBudgetBytes": soft_target,
                    "retention": retention.clone(),
                    "profile": preferences.retention_profile.clone(),
                    "tiers": retention.tiers.clone(),
                    "incompatible": incompatible,
                    "displayTimezone": if incompatible { "UTC" } else { preferences.display_timezone.as_str() },
                },
                "constraints": {
                    "hardQuotaBytes": hard_quota,
                    "softHistoryTargetBytes": soft_target,
                    "minimumBucketSeconds": 300,
                    "maximumRetentionSeconds": Value::Null,
                    "maxCheckpointCount": bounds(self).max_checkpoint_count,
                    "documentOverrides": false,
                    "graceSeconds": 86_400,
                    "physicalAccounting": usage.physical_accounting,
                },
                "usage": usage_json(&usage),
                "thinning": {
                    "status": job.as_ref().map(|job| job.status.as_str()).unwrap_or("idle"),
                    "generation": job.as_ref().map(|job| job.generation.as_str()),
                    "graceUntil": job.as_ref().map(|job| job.grace_until),
                    "lastCompletedPolicyGeneration": last_completed
                },
                "profiles": ["moreRecoveryPoints", "balanced", "useLessStorage", "custom"],
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
        let account_id = match self.quota_account(request.headers(), arrival).await {
            Ok(account_id) => account_id,
            Err(response) => return response,
        };
        let body = match to_bytes(request.into_body(), 256 * 1024).await {
            Ok(body) => body,
            Err(_) => return write_json(413, &json!({"error": "preference request too large"})),
        };
        let asked: PreviewRequest = match serde_json::from_slice(&body) {
            Ok(asked) => asked,
            Err(error) => return write_json(400, &json!({"error": error.to_string()})),
        };
        if let Err(error) = asked.preferences.validate() {
            return write_json(400, &json!({"error": error}));
        }
        let record = match self.quota_record(&account_id).await {
            Ok(record) => record,
            Err(response) => return response,
        };
        if asked.revision != record.revision {
            return write_json(
                409,
                &json!({"error": "quota preference revision is stale", "revision": record.revision}),
            );
        }
        let usage = match self.quota_usage(&account_id).await {
            Ok(usage) => usage,
            Err(response) => return response,
        };
        let effective = effective_retention(&asked.preferences, &bounds(self));
        let soft_target = asked
            .preferences
            .history_budget
            .target_bytes(bounds(self).hard_quota);
        let required_history_reclaim = usage.history_bytes.saturating_sub(soft_target);
        let (affected_count, protected_count, oldest, fingerprint, reclaimable_bytes) =
            if let Some(catalog) = &self.store.catalog {
                let id = account_id.clone();
                match catalog
                    .execute_catalog(SERVER_JOB_BYTES + id.len(), move |catalog| {
                        Ok((
                            catalog.account_checkpoints(&id)?,
                            catalog.account_open_annotation_references(&id)?,
                        ))
                    })
                    .await
                {
                    Ok((points, references)) => {
                        let mut by_document = BTreeMap::<String, Vec<_>>::new();
                        for point in points {
                            by_document
                                .entry(point.slug.clone())
                                .or_default()
                                .push(point);
                        }
                        let grouped: Vec<_> = by_document.into_iter().collect();
                        let mut candidate_keys = Vec::new();
                        let mut emergency_keys = Vec::new();
                        let mut candidate_order =
                            BTreeMap::<(String, String), (i64, String, i64)>::new();
                        let mut new_protected = BTreeSet::new();
                        let mut old_protected = BTreeSet::new();
                        let (old_preferences, _) = decode_preferences(&record);
                        let mut oldest: Option<String> = None;
                        for (slug, points) in &grouped {
                            let refs = references.get(slug).cloned().unwrap_or_default();
                            let policy = policy_points(points.clone());
                            let decision = select_retained(
                                &policy,
                                crate::util::now_unix(),
                                &asked.preferences,
                                &bounds(self),
                                &refs,
                            );
                            let newest = policy
                                .iter()
                                .enumerate()
                                .max_by_key(|(index, point)| {
                                    (
                                        quota_event_time(&point.at),
                                        point.seq.max(*index as i64),
                                        point.sha.clone(),
                                    )
                                })
                                .map(|(_, point)| point.sha.as_str());
                            for sha in &decision.removed {
                                candidate_keys.push((slug.clone(), sha.clone()));
                                if let Some(point) = points.iter().find(|point| point.sha == *sha) {
                                    candidate_order.insert(
                                        (slug.clone(), sha.clone()),
                                        (quota_event_time(&point.at), point.at.clone(), point.seq),
                                    );
                                }
                            }
                            for point in &policy {
                                if newest != Some(point.sha.as_str())
                                    && !decision.protected.contains(&point.sha)
                                {
                                    emergency_keys.push((slug.clone(), point.sha.clone()));
                                    candidate_order
                                        .entry((slug.clone(), point.sha.clone()))
                                        .or_insert_with(|| {
                                            (
                                                quota_event_time(&point.at),
                                                point.at.clone(),
                                                point.seq,
                                            )
                                        });
                                }
                            }
                            new_protected.extend(
                                decision
                                    .protected
                                    .into_iter()
                                    .map(|sha| format!("{slug}:{sha}")),
                            );
                            if let Some(time) = points
                                .iter()
                                .filter(|point| decision.removed.contains(&point.sha))
                                .map(|point| point.at.clone())
                                .min()
                            {
                                oldest = Some(match oldest {
                                    Some(previous) => previous.min(time),
                                    None => time,
                                });
                            }
                            let old_decision = select_retained(
                                &policy,
                                crate::util::now_unix(),
                                &old_preferences,
                                &bounds(self),
                                &refs,
                            );
                            old_protected.extend(
                                old_decision
                                    .protected
                                    .into_iter()
                                    .map(|sha| format!("{slug}:{sha}")),
                            );
                        }
                        candidate_keys.sort_by(|left, right| {
                            candidate_order
                                .get(left)
                                .cmp(&candidate_order.get(right))
                                .then_with(|| left.cmp(right))
                        });
                        emergency_keys.sort_by(|left, right| {
                            candidate_order
                                .get(left)
                                .cmp(&candidate_order.get(right))
                                .then_with(|| left.cmp(right))
                        });
                        let (candidate_keys, reclaimable) = if usage.physical_accounting {
                            let age_keys = candidate_keys;
                            let mut routine_keys = emergency_keys;
                            routine_keys.retain(|key| !age_keys.contains(key));
                            let required = required_history_reclaim;
                            let selected = catalog
                                .execute_catalog(
                                    SERVER_JOB_BYTES + (age_keys.len() + routine_keys.len()) * 80,
                                    move |catalog| {
                                        let mut selected = age_keys;
                                        let mut reclaimable = 0;
                                        if !selected.is_empty() {
                                            reclaimable =
                                                catalog.reclaimable_checkpoint_bytes(&selected)?;
                                        }
                                        if required > reclaimable {
                                            for candidate in routine_keys {
                                                selected.push(candidate);
                                                reclaimable = catalog
                                                    .reclaimable_checkpoint_bytes(&selected)?;
                                                if reclaimable >= required {
                                                    break;
                                                }
                                            }
                                        }
                                        Ok((selected, reclaimable))
                                    },
                                )
                                .await;
                            match selected {
                                Ok(value) => value,
                                Err(error) => {
                                    return write_json(503, &json!({"error": error.to_string()}))
                                }
                            }
                        } else {
                            (candidate_keys, 0)
                        };
                        let affected = candidate_keys.len();
                        oldest = candidate_keys
                            .iter()
                            .filter_map(|candidate| {
                                candidate_order.get(candidate).map(|(_, at, _)| at.clone())
                            })
                            .min();
                        let fingerprint =
                            candidate_fingerprint(&grouped, &references, &candidate_keys);
                        let newly_unprotected = old_protected.difference(&new_protected).count();
                        (
                            affected,
                            newly_unprotected,
                            oldest,
                            fingerprint,
                            reclaimable,
                        )
                    }
                    Err(error) => return write_json(503, &json!({"error": error.to_string()})),
                }
            } else {
                (0, 0, None, String::new(), 0)
            };
        let policy_bounds = bounds(self);
        let fingerprint = format!(
            "hard-quota={};min-bucket={};max-count={:?};max-age={:?};{}",
            self.config.storage.per_owner,
            policy_bounds.min_bucket_seconds,
            policy_bounds.max_checkpoint_count,
            policy_bounds.max_retention_seconds,
            fingerprint
        );
        let evaluated_at = crate::util::now_unix();
        let generation = generation(
            &asked.preferences,
            record.revision,
            &usage,
            &fingerprint,
            evaluated_at,
        );
        write_json(
            200,
            &json!({
                "generation": generation,
                "revision": record.revision,
                "evaluatedAt": evaluated_at,
                "requiresConfirmation": affected_count > 0,
                "affectedCount": affected_count,
                "graceSeconds": 86_400,
                "protectedCount": protected_count,
                "oldestAffected": oldest,
                "reclaimableBytes": if usage.physical_accounting { Some(reclaimable_bytes) } else { None },
                "reclaimEstimateAvailable": usage.physical_accounting,
                "estimate": true,
                "effective": effective,
                "explanation": if usage.physical_accounting { Vec::<String>::new() } else { vec![String::from("Physical retained-object accounting is not authoritative; no reclaimable-byte claim is made.")]},
            }),
        )
    }

    pub(super) async fn handle_quota_apply(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
    ) -> Reply {
        let account_id = match self.quota_account(request.headers(), arrival).await {
            Ok(account_id) => account_id,
            Err(response) => return response,
        };
        let body = match to_bytes(request.into_body(), 256 * 1024).await {
            Ok(body) => body,
            Err(_) => return write_json(413, &json!({"error": "preference request too large"})),
        };
        let asked: ApplyRequest = match serde_json::from_slice(&body) {
            Ok(asked) => asked,
            Err(error) => return write_json(400, &json!({"error": error.to_string()})),
        };
        if let Err(error) = asked.preferences.validate() {
            return write_json(400, &json!({"error": error}));
        }
        let record = match self.quota_record(&account_id).await {
            Ok(record) => record,
            Err(response) => return response,
        };
        let (old_preferences, incompatible) = decode_preferences(&record);
        if incompatible {
            return write_json(
                409,
                &json!({
                    "error": "stored quota preferences are newer or invalid; update the server before applying",
                    "readOnly": true,
                    "revision": record.revision,
                }),
            );
        }
        if is_destructive_change(
            &old_preferences,
            &asked.preferences,
            self.config.storage.per_owner,
        ) && !asked.confirmed
        {
            return write_json(400, &json!({"error": "explicit confirmation is required"}));
        }
        let usage = match self.quota_usage(&account_id).await {
            Ok(usage) => usage,
            Err(response) => return response,
        };
        let soft_target = asked
            .preferences
            .history_budget
            .target_bytes(bounds(self).hard_quota);
        let required_history_reclaim = usage.history_bytes.saturating_sub(soft_target);
        if record.policy_generation == asked.generation && !record.policy_generation.is_empty() {
            if serde_json::from_str::<Value>(&record.payload).ok()
                != serde_json::to_value(&asked.preferences).ok()
            {
                return write_json(
                    409,
                    &json!({"error": "policy generation belongs to different preferences"}),
                );
            }
            return write_json(
                202,
                &json!({
                    "status": "accepted",
                    "revision": record.revision,
                    "generation": record.policy_generation,
                    "thinning": "pending",
                    "graceSeconds": 86_400,
                }),
            );
        }
        let id_for_candidates = account_id.clone();
        let (points, references) = if let Some(catalog) = &self.store.catalog {
            match catalog
                .execute_catalog(SERVER_JOB_BYTES + id_for_candidates.len(), move |catalog| {
                    Ok((
                        catalog.account_checkpoints(&id_for_candidates)?,
                        catalog.account_open_annotation_references(&id_for_candidates)?,
                    ))
                })
                .await
            {
                Ok(value) => value,
                Err(error) => return write_json(503, &json!({"error": error.to_string()})),
            }
        } else {
            (Vec::new(), BTreeMap::new())
        };
        let mut by_document = BTreeMap::<String, Vec<_>>::new();
        for point in points {
            by_document
                .entry(point.slug.clone())
                .or_default()
                .push(point);
        }
        let grouped: Vec<_> = by_document.into_iter().collect();
        let mut candidate_keys = Vec::new();
        let mut emergency_keys = Vec::new();
        let mut candidate_order = BTreeMap::<(String, String), (i64, String, i64)>::new();
        for (slug, points) in &grouped {
            let refs = references.get(slug).cloned().unwrap_or_default();
            let policy = policy_points(points.clone());
            let decision = select_retained(
                &policy,
                crate::util::now_unix(),
                &asked.preferences,
                &bounds(self),
                &refs,
            );
            let newest = policy
                .iter()
                .enumerate()
                .max_by_key(|(index, point)| {
                    (
                        quota_event_time(&point.at),
                        point.seq.max(*index as i64),
                        point.sha.clone(),
                    )
                })
                .map(|(_, point)| point.sha.as_str());
            for sha in decision.removed {
                if let Some(point) = points.iter().find(|point| point.sha == sha) {
                    candidate_order.insert(
                        (slug.clone(), sha.clone()),
                        (quota_event_time(&point.at), point.at.clone(), point.seq),
                    );
                }
                candidate_keys.push((slug.clone(), sha));
            }
            for point in &policy {
                if newest != Some(point.sha.as_str()) && !decision.protected.contains(&point.sha) {
                    emergency_keys.push((slug.clone(), point.sha.clone()));
                    candidate_order
                        .entry((slug.clone(), point.sha.clone()))
                        .or_insert_with(|| {
                            (quota_event_time(&point.at), point.at.clone(), point.seq)
                        });
                }
            }
        }
        candidate_keys.sort_by(|left, right| {
            candidate_order
                .get(left)
                .cmp(&candidate_order.get(right))
                .then_with(|| left.cmp(right))
        });
        emergency_keys.sort_by(|left, right| {
            candidate_order
                .get(left)
                .cmp(&candidate_order.get(right))
                .then_with(|| left.cmp(right))
        });
        if usage.physical_accounting && required_history_reclaim > 0 {
            let Some(catalog) = &self.store.catalog else {
                return write_json(503, &json!({"error": "local catalogue unavailable"}));
            };
            let age_keys = candidate_keys.clone();
            let mut routine_keys = emergency_keys;
            routine_keys.retain(|key| !age_keys.contains(key));
            let selected = catalog
                .execute_catalog(
                    SERVER_JOB_BYTES + (age_keys.len() + routine_keys.len()) * 80,
                    move |catalog| {
                        let mut selected = age_keys;
                        let mut reclaimable = if selected.is_empty() {
                            0
                        } else {
                            catalog.reclaimable_checkpoint_bytes(&selected)?
                        };
                        if reclaimable < required_history_reclaim {
                            for candidate in routine_keys {
                                selected.push(candidate);
                                reclaimable = catalog.reclaimable_checkpoint_bytes(&selected)?;
                                if reclaimable >= required_history_reclaim {
                                    break;
                                }
                            }
                        }
                        Ok(selected)
                    },
                )
                .await;
            candidate_keys = match selected {
                Ok(selected) => selected,
                Err(error) => return write_json(503, &json!({"error": error.to_string()})),
            };
        }
        let policy_bounds = bounds(self);
        let fingerprint = format!(
            "hard-quota={};min-bucket={};max-count={:?};max-age={:?};{}",
            self.config.storage.per_owner,
            policy_bounds.min_bucket_seconds,
            policy_bounds.max_checkpoint_count,
            policy_bounds.max_retention_seconds,
            candidate_fingerprint(&grouped, &references, &candidate_keys)
        );
        let now = crate::util::now_unix();
        let expected_generation = generation(
            &asked.preferences,
            record.revision,
            &usage,
            &fingerprint,
            now,
        );
        if record.revision != asked.revision || expected_generation != asked.generation {
            return write_json(
                409,
                &json!({"error": "preview is stale; request a new preview", "revision": record.revision}),
            );
        }
        let Some(catalog) = &self.store.catalog else {
            return write_json(503, &json!({"error": "local catalogue unavailable"}));
        };
        let payload = match serde_json::to_string(&asked.preferences) {
            Ok(payload) => payload,
            Err(error) => return write_json(400, &json!({"error": error.to_string()})),
        };
        let id = account_id.clone();
        let expected_revision = asked.revision;
        let generation_value = asked.generation.clone();
        let fingerprint_value = fingerprint.clone();
        let grace_until = now.saturating_add(86_400);
        let hard_count_limit = bounds(self).max_checkpoint_count;
        let save = catalog
            .execute_catalog(
                SERVER_JOB_BYTES + id.len() + payload.len() + candidate_keys.len() * 80,
                move |catalog| {
                    catalog.save_quota_preferences_and_schedule_with_hard_count_limit(
                        &id,
                        expected_revision,
                        &payload,
                        &generation_value,
                        &fingerprint_value,
                        &candidate_keys,
                        grace_until,
                        now,
                        hard_count_limit,
                    )
                },
            )
            .await;
        match save {
            Ok(saved) => {
                eprintln!(
                    "{}",
                    json!({
                        "event": "quota_preferences_applied",
                        "generation": saved.generation,
                        "revision": saved.revision,
                        "thinning": saved.status,
                    })
                );
                write_json(
                    202,
                    &json!({
                        "status": "accepted",
                        "revision": saved.revision,
                        "generation": saved.generation,
                        "thinning": saved.status,
                        "graceSeconds": saved.grace_until.saturating_sub(now),
                    }),
                )
            }
            Err(crate::storage::catalog::CatalogExecError::Catalog(
                crate::storage::catalog::CatalogError::Conflict(error),
            )) => write_json(409, &json!({"error": error})),
            Err(error) => write_json(503, &json!({"error": error.to_string()})),
        }
    }
}
