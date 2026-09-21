//! Domain operations behind the five MCP tools. Transport ids never identify effects.
use super::*;
use crate::room;
use crate::room::agent::{
    self, Affinity, AgentAuthority, OperationKey, Patch, PatchRequest, SourceFile, SourceTree,
};

#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct CandidateGrant {
    #[serde(default)]
    pub account_id: String,
    #[serde(default)]
    pub generation: String,
    #[serde(default)]
    pub link_hash: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Candidate {
    pub view_id: String,
    pub base_tree_digest: String,
    pub tree_digest: String,
    pub patches: Vec<Patch>,
    pub dependencies: Vec<agent::Dependency>,
    pub operation: OperationKey,
    #[serde(default)]
    pub parent_request_id: String,
    pub digest: String,
    pub validation: String,
    pub publish: String,
    pub notes: Vec<String>,
    pub expires_at: i64,
    #[serde(default)]
    pub grant: CandidateGrant,
}

#[derive(Serialize, Deserialize)]
struct Admission {
    digest: String,
}

pub(super) fn failure(error: agent::AgentError) -> Failure {
    let code = match error {
        agent::AgentError::Invalid(_) => "invalid_params",
        agent::AgentError::Conflict(_) => "conflict",
        agent::AgentError::Storage(_) => "outcome_unknown",
    };
    Failure::new(code, error.to_string())
}

/// What identifies "this operation, sent again": the tool and its arguments,
/// with every object's keys sorted at every depth.
///
/// `serde_json` is built with `preserve_order` here -- an ACP dependency
/// turns the feature on for the whole graph -- so an argument object keeps
/// whatever order the client wrote it in. Hashing that order made two
/// spellings of one call two different operations, which is not what an
/// operation key means. Array order is part of the value and stays.
///
/// Deliberately not shared with `room::catalog`'s comment-version tokens or
/// the assistant's task fingerprints. Those hash bytes that clients hold and
/// that outlive a restart, so they are not free to be defined well; this one
/// is (REVIEW finding 12).
///
/// **Admissions across this change.** The digest is not an identity of its
/// own: `mcp_refuse_if_admitted` refuses every key that has already been
/// admitted, whatever digest it carries. An admission stored under the old
/// digest therefore still refuses a retry -- with `operation_key_reused`
/// rather than `outcome_unknown`, both refusals -- and no effect can run a
/// second time because of this. Admissions are stored against the operation
/// epoch and expire with it, so the mixed spelling does not outlive one
/// epoch's lifetime. Nothing is migrated.
pub(super) fn operation_digest(name: &str, args: &Value) -> String {
    fn canonical(value: &Value) -> Value {
        match value {
            Value::Object(object) => Value::Object(
                object
                    .iter()
                    .collect::<std::collections::BTreeMap<_, _>>()
                    .into_iter()
                    .map(|(key, value)| (key.clone(), canonical(value)))
                    .collect(),
            ),
            Value::Array(values) => Value::Array(values.iter().map(canonical).collect()),
            other => other.clone(),
        }
    }
    hex::encode(Sha256::digest(
        json!({"tool": name, "arguments": canonical(args)}).to_string(),
    ))
}

pub(super) fn tree_of_view(view: &View) -> Result<SourceTree, Failure> {
    // The capture holds the `Projection` the sequencer produced (§4.4). It
    // is used here only to look up each path's stable file id; there is no
    // separate canonical checkpoint to carry alongside it any more, so
    // `canonical` is left empty.
    let projection = &view.snapshot.projection;
    let files = view
        .snapshot
        .texts
        .iter()
        .map(|(path, text)| {
            (
                path.clone(),
                SourceFile {
                    file_id: projection
                        .files
                        .get(path)
                        .map(|entry| entry.id.clone())
                        .unwrap_or_default(),
                    text: text.clone(),
                },
            )
        })
        .collect();
    Ok(SourceTree {
        main: projection.main.clone(),
        files,
        revision: view.snapshot.tree_digest.clone(),
    })
}

fn apply_candidate_patches<'a>(
    texts: &mut std::collections::BTreeMap<String, String>,
    patches: impl Iterator<Item = &'a Patch>,
) -> Result<(), Failure> {
    let mut patches = patches.collect::<Vec<_>>();
    patches.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then(b.start.cmp(&a.start))
            .then(b.end.cmp(&a.end))
    });
    for patch in patches {
        let text = texts
            .get_mut(&patch.path)
            .ok_or_else(|| Failure::new("conflict", "candidate file missing"))?;
        if text.get(patch.start..patch.end) != Some(patch.exact.as_str()) {
            return Err(Failure::new("conflict", "candidate range changed"));
        }
        text.replace_range(patch.start..patch.end, &patch.replacement);
    }
    Ok(())
}

pub(super) fn candidate_texts(
    view: &View,
    candidate: &Candidate,
) -> Result<std::collections::BTreeMap<String, String>, Failure> {
    let mut texts = view.snapshot.texts.clone();
    apply_candidate_patches(&mut texts, candidate.patches.iter())?;
    Ok(texts)
}

pub(super) fn candidate_source(
    view: &View,
    candidate: &Candidate,
    path: &str,
) -> Result<String, Failure> {
    let text = view
        .snapshot
        .texts
        .get(path)
        .cloned()
        .ok_or_else(|| Failure::new("not_found", "candidate file missing"))?;
    let mut texts = std::collections::BTreeMap::from([(path.to_string(), text)]);
    apply_candidate_patches(
        &mut texts,
        candidate.patches.iter().filter(|patch| patch.path == path),
    )?;
    texts
        .remove(path)
        .ok_or_else(|| Failure::new("not_found", "candidate file missing"))
}

impl Server {
    pub(super) async fn mcp_admit(
        &self,
        slug: &str,
        actor: &str,
        who: &Viewer,
        key: &OperationKey,
        digest: &str,
    ) -> Result<(), Failure> {
        let expiry = self.mcp_epoch(actor, key, false)?;
        let id = key.scoped_request_id(actor);
        if let Err(error) = self
            .mcp_store(
                slug,
                actor,
                who,
                &id,
                "admission",
                &Admission {
                    digest: digest.to_owned(),
                },
                expiry,
                false,
            )
            .await
        {
            if self
                .mcp_load::<Admission>(slug, actor, who, &id, "admission")
                .await
                .is_ok_and(|old| old.digest != digest)
            {
                return Err(Failure::new(
                    "operation_key_reused",
                    "operation key already admitted different arguments",
                ));
            }
            return Err(error);
        }
        Ok(())
    }

    pub(super) fn mcp_epoch(
        &self,
        actor: &str,
        key: &OperationKey,
        allow_expired: bool,
    ) -> Result<i64, Failure> {
        key.validate().map_err(failure)?;
        let parts: Vec<_> = key.epoch.split('.').collect();
        if parts.len() != 3 || !verify(&self.key, actor, &parts[..2].join("."), parts[2]) {
            return Err(Failure::new(
                "expired_epoch",
                "capture a view to obtain an operation epoch",
            ));
        }
        let expiry = parts[0]
            .parse::<i64>()
            .map_err(|_| Failure::new("expired_epoch", "invalid epoch"))?;
        if !allow_expired && expiry <= now_unix() {
            return Err(Failure::new(
                "expired_epoch",
                "capture a fresh view; expired keys cannot admit new effects",
            ));
        }
        Ok(expiry)
    }

    /// The admission guard: an operation key that was already admitted is
    /// refused rather than run a second time. No committed result is
    /// retained, so a repeat can only be told that the outcome is unknown;
    /// the domain commands carry their own replay records. Passing `digest`
    /// additionally refuses a key reused for different arguments.
    pub(super) async fn mcp_refuse_if_admitted(
        &self,
        slug: &str,
        actor: &str,
        who: &Viewer,
        key: &OperationKey,
        digest: Option<&str>,
    ) -> Result<(), Failure> {
        self.mcp_epoch(actor, key, true)?;
        let id = key.scoped_request_id(actor);
        match self
            .mcp_load::<Admission>(slug, actor, who, &id, "admission")
            .await
        {
            Ok(admitted) if digest.is_some_and(|digest| digest != admitted.digest) => Err(
                Failure::new(
                    "operation_key_reused",
                    "operation key already admitted different arguments",
                ),
            ),
            Ok(_) => Err(Failure::new(
                "outcome_unknown",
                "this operation was admitted previously, but no committed result is retained; inspect the document before submitting a new operation",
            )),
            Err(error) if error.code == "view_expired" => Ok(()),
            Err(error) => Err(error),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn mcp_operation(
        &self,
        slug: &str,
        actor: &str,
        who: &Viewer,
        headers: &HeaderMap,
        arrival: &Arrival,
        peer: SocketAddr,
        name: &str,
        args: &Value,
    ) -> Result<Value, Failure> {
        self.mcp_operation_inner(slug, actor, who, headers, arrival, peer, name, args, "")
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn mcp_operation_inner(
        &self,
        slug: &str,
        actor: &str,
        who: &Viewer,
        headers: &HeaderMap,
        arrival: &Arrival,
        peer: SocketAddr,
        name: &str,
        args: &Value,
        parent_request_id: &str,
    ) -> Result<Value, Failure> {
        if name == "document_result" {
            return self
                .mcp_result(slug, actor, headers, arrival, peer, args)
                .await;
        }
        let key: OperationKey = serde_json::from_value(args["operation"].clone())
            .map_err(|e| Failure::new("invalid_params", e.to_string()))?;
        key.validate().map_err(failure)?;
        let digest = operation_digest(name, args);
        let current = self.mcp_recheck(slug, headers, arrival, actor).await?;
        self.mcp_refuse_if_admitted(slug, actor, who, &key, Some(&digest))
            .await?;
        self.mcp_admit(slug, actor, who, &key, &digest).await?;
        match name {
            "document_propose" => {
                if args["batch"] == "independent" {
                    let mut items = Vec::new();
                    for (index, patch) in
                        args["patches"].as_array().into_iter().flatten().enumerate()
                    {
                        let mut child = args.clone();
                        child["batch"] = json!("atomic");
                        child["patches"] = json!([patch]);
                        child["operation"]["id"] = json!(key.batch_child(actor, index).id);
                        let outcome = Box::pin(self.mcp_operation_inner(
                            slug,
                            actor,
                            &current,
                            headers,
                            arrival,
                            peer,
                            name,
                            &child,
                            &key.scoped_request_id(actor),
                        ))
                        .await;
                        items.push(match outcome {Ok(result)=>json!({"index":index,"status":result["status"],"candidate_id":result["candidate_id"],"effects":result["effects"],"replay":result["replay"]}),Err(error)=>json!({"index":index,"error":{"code":error.code,"message":error.message}})});
                    }
                    // `independent` means each patch is its own command with
                    // its own retry record, so "did this batch replay" has
                    // no single answer -- one item can be a fresh commit
                    // while another, retried separately, replays. `replay`
                    // is carried per item above (`null` where the item's own
                    // command has none to report, e.g. a private candidate);
                    // there is no batch-level key here.
                    let result = json!({"operation":key,"status":"committed","batch":"independent","items":items});
                    // No receipt is retained, but access is still rechecked
                    // before the batch result is handed back.
                    self.mcp_recheck(slug, headers, arrival, actor).await?;
                    return Ok(result);
                }
                self.mcp_propose(
                    slug,
                    actor,
                    &current,
                    headers,
                    arrival,
                    peer,
                    args,
                    key,
                    digest,
                    parent_request_id,
                )
                .await
            }
            "document_apply" => {
                if !current.at_least(Role::Editor) {
                    return Err(Failure::new(
                        "permission_changed",
                        "editor access is required to apply source",
                    ));
                }
                let candidate: Candidate = self
                    .mcp_load(
                        slug,
                        actor,
                        who,
                        args["candidate_id"].as_str().unwrap_or_default(),
                        "candidate",
                    )
                    .await?;
                let consistency = serde_json::from_value(
                    args.get("consistency")
                        .cloned()
                        .unwrap_or(json!("exact_tree")),
                )
                .map_err(|e| Failure::new("invalid_params", e.to_string()))?;
                if candidate.validation == "compile" && consistency != agent::Consistency::ExactTree
                {
                    return Err(Failure::new(
                        "conflict",
                        "compiled candidates require exact_tree application",
                    ));
                }
                if candidate.validation == "compile" {
                    let receipt = self
                        .load_render_receipt(
                            slug,
                            actor,
                            who,
                            args["candidate_id"].as_str().unwrap_or_default(),
                        )
                        .await?;
                    if receipt.status != "verified" || receipt.tree_digest != candidate.tree_digest
                    {
                        return Err(Failure::new(
                            "validation_failed",
                            "candidate has no matching successful render receipt",
                        ));
                    }
                }
                let request = PatchRequest {
                    operation: key,
                    base_tree: candidate.base_tree_digest,
                    consistency,
                    dependencies: candidate.dependencies,
                    patches: candidate.patches,
                    request_digest: digest,
                };
                let authority = AgentAuthority {
                    automation: true,
                    account_id: current.id.id.clone(),
                    owner_key: current.key.clone(),
                    generation: current.id.session_generation.clone(),
                    link_hash: current.link.clone(),
                    policy_editor: self.publishers.allows(&current.id.handle),
                    unowned_publisher: false,
                    operation_scope: actor.to_string(),
                    execution_epoch: runner_execution_epoch(headers),
                };
                let room = self
                    .rooms
                    .get(slug)
                    .await
                    .map_err(|e| Failure::new("unavailable", e.to_string()))?;
                let receipt = room
                    .apply_agent_request(request, authority, || async {
                        self.mcp_recheck(slug, headers, arrival, actor)
                            .await
                            .map(|_| ())
                            .map_err(|error| agent::AgentError::Conflict(error.message))
                    })
                    .await
                    .map_err(failure)?;
                Ok(json!(receipt))
            }
            "document_comment" => {
                if args["action"] == "accept" {
                    return self
                        .mcp_accept(
                            slug, actor, &current, headers, arrival, peer, args, &key, &digest,
                        )
                        .await;
                }
                if args["action"] == "label" {
                    return self
                        .mcp_label(
                            slug,
                            actor,
                            &current,
                            headers,
                            arrival,
                            (args, &key, &digest),
                        )
                        .await;
                }
                self.mcp_comment(
                    slug, actor, who, headers, arrival, peer, args, &key, &digest,
                )
                .await
            }
            _ => Err(Failure::new("invalid_params", "unknown tool")),
        }
    }

    /// Takes a suggestion: its proposal branch is merged into a fork of head
    /// (§7.3), which is the one command here that produces source. A retry
    /// with the same operation id finds the `document_labels` row this
    /// writes and returns without merging twice (§7.2), so the request id
    /// is derived from the operation key the same way a new comment's id is.
    #[allow(clippy::too_many_arguments)]
    async fn mcp_accept(
        &self,
        slug: &str,
        actor: &str,
        who: &Viewer,
        headers: &HeaderMap,
        arrival: &Arrival,
        _peer: SocketAddr,
        args: &Value,
        key: &OperationKey,
        _digest: &str,
    ) -> Result<Value, Failure> {
        if !who.at_least(Role::Editor) {
            return Err(Failure::new(
                "permission_changed",
                "editor access is required to accept a suggestion",
            ));
        }
        let room = self
            .rooms
            .get(slug)
            .await
            .map_err(|e| Failure::new("unavailable", e.to_string()))?;
        if let Some(why) = room.unreadable().await {
            return Err(Failure::new("unavailable", why));
        }
        let id = args["comment_id"]
            .as_str()
            .ok_or_else(|| Failure::new("invalid_params", "accept requires comment_id"))?;
        let expected = args["expected_version"]
            .as_str()
            .ok_or_else(|| Failure::new("invalid_params", "accept requires expected_version"))?;
        let comment = room
            .comment_by_id(id, true)
            .await
            .map_err(|error| Failure::new("unavailable", error.to_string()))?
            .ok_or_else(|| Failure::new("not_found", "suggestion unavailable"))?;
        if crate::room::agent_comments::comment_version(&comment) != expected
            || comment.motivation != "editing"
            || comment.resolved
            || !comment.outcome.is_empty()
        {
            return Err(Failure::new(
                "conflict",
                "suggestion changed or is already decided",
            ));
        }
        let comment_id = super::comments::parse_uuid(&comment.id, "comment_id")?;
        let proposal_id = super::comments::parse_uuid(&comment.proposal, "proposal")?;
        let stored = room
            .catalog()
            .proposal(proposal_id)
            .await
            .map_err(|error| Failure::new("unavailable", error.to_string()))?
            .ok_or_else(|| Failure::new("not_found", "proposal does not exist"))?;
        let decided_by = if who.id.is_signed_in() {
            who.id.name.clone()
        } else {
            pseudonym_for(&self.mcp_author(headers, arrival, who, actor), slug)
        };
        let authorization = who.mutation_authorization(self.ceiling_for(&who.id).edit);
        let request_id = super::comments::parse_uuid(
            &super::comments::comment_uuid(&key.scoped_request_id(actor)),
            "operation id",
        )?;
        let mut cmd = room::AcceptSuggestion::new(
            room.catalog().clone(),
            room.document_id,
            comment_id,
            proposal_id,
            stored.base_frontiers,
            stored.tip_frontiers,
            stored.branch_bytes,
            decided_by,
            authorization,
            request_id,
        );
        let authority = who.document_authority();
        let (accepted, replay) = room
            .command_reporting_replay(&authority, &mut cmd)
            .await
            .map_err(super::comments::command_failure)?;
        Ok(json!({
            "operation": key,
            "status": "committed",
            "action": "accept",
            "comment_id": accepted.comment.id,
            "comment": accepted.comment,
            "resolved_in": accepted.resolved_in,
            "replay": replay,
        }))
    }

    async fn mcp_label(
        &self,
        slug: &str,
        actor: &str,
        who: &Viewer,
        headers: &HeaderMap,
        arrival: &Arrival,
        (args, key, _digest): (&Value, &OperationKey, &str),
    ) -> Result<Value, Failure> {
        if !who.at_least(Role::Editor) {
            return Err(Failure::new(
                "permission_changed",
                "editor access is required to label",
            ));
        }
        let view: View = self
            .mcp_load(
                slug,
                actor,
                who,
                args["view_id"]
                    .as_str()
                    .ok_or_else(|| Failure::new("invalid_params", "label requires view_id"))?,
                "view",
            )
            .await?;
        let _ = view;
        let room = self
            .rooms
            .get(slug)
            .await
            .map_err(|e| Failure::new("unavailable", e.to_string()))?;
        if let Some(why) = room.unreadable().await {
            return Err(Failure::new("unavailable", why));
        }
        // §7.1 checks nothing for a label: it always names whatever head
        // `take_label` finds. There is no `checkpoint_id` any more (§8.2) --
        // a label is `source_sequence` plus the frontier at that row -- so
        // that is what the response carries in place of one.
        let authority = who.document_authority();
        let by = room
            .signed_by(
                &who.id.id,
                &pseudonym_for(&self.mcp_author(headers, arrival, who, actor), slug),
            )
            .await;
        let request_id = super::comments::parse_uuid(
            &super::comments::comment_uuid(&key.scoped_request_id(actor)),
            "operation id",
        )?;
        let (recorded, replay) = room
            .take_label_reporting_replay("agent", None, by, &authority, Some(request_id))
            .await
            .map_err(|error| Failure::new("conflict", error.to_string()))?;
        Ok(json!({
            "operation": key,
            "status": "committed",
            "action": "label",
            "source_sequence": recorded.source_sequence,
            "replay": replay,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    async fn mcp_propose(
        &self,
        slug: &str,
        actor: &str,
        who: &Viewer,
        headers: &HeaderMap,
        arrival: &Arrival,
        peer: SocketAddr,
        args: &Value,
        key: OperationKey,
        digest: String,
        parent_request_id: &str,
    ) -> Result<Value, Failure> {
        let publish = args["publish"].as_str().unwrap_or("suggestions");
        if publish == "suggestions" && !who.at_least(Role::Commenter) {
            return Err(Failure::new(
                "permission_changed",
                "comment access is required to publish suggestions",
            ));
        }
        let view_id = args["view_id"].as_str().unwrap_or_default();
        let view: View = self.mcp_load(slug, actor, who, view_id, "view").await?;
        let tree = tree_of_view(&view)?;
        let mut patches = Vec::new();
        let mut dependencies = Vec::new();
        let inputs = args["patches"]
            .as_array()
            .ok_or_else(|| Failure::new("invalid_params", "patches required"))?;
        for input in inputs {
            let (path, mut start, mut end) = resolve_range(
                &view,
                view_id,
                input["range_id"].as_str().unwrap_or_default(),
                &self.key,
            )?;
            dependencies.push(agent::Dependency {
                path: path.to_string(),
                file_id: tree.files[path].file_id.clone(),
                file_hash: hex::encode(Sha256::digest(view.snapshot.texts[path].as_bytes())),
                start,
                end,
                exact: view.snapshot.texts[path][start..end].to_string(),
            });
            if let Some(find) = input["find"].as_str() {
                if find.is_empty() {
                    return Err(Failure::new(
                        "ambiguous_range",
                        "find must be nonempty; use an explicit insertion range",
                    ));
                }
                let source = &view.snapshot.texts[path][start..end];
                let matches = source
                    .char_indices()
                    .filter(|(at, _)| source[*at..].starts_with(find))
                    .take(2)
                    .collect::<Vec<_>>();
                if matches.len() != 1 {
                    return Err(Failure::new(
                        "ambiguous_range",
                        "find must identify exactly one occurrence in the captured range",
                    ));
                }
                start += matches[0].0;
                end = start + find.len();
            }
            patches.push(Patch {
                path: path.to_string(),
                file_id: tree.files[path].file_id.clone(),
                start,
                end,
                exact: view.snapshot.texts[path][start..end].to_string(),
                replacement: input["replacement"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                affinity: Affinity::Before,
            });
        }
        for handle in args["dependencies"].as_array().into_iter().flatten() {
            let (path, start, end) = resolve_range(
                &view,
                view_id,
                handle.as_str().unwrap_or_default(),
                &self.key,
            )?;
            dependencies.push(agent::Dependency {
                path: path.to_string(),
                file_id: tree.files[path].file_id.clone(),
                file_hash: hex::encode(Sha256::digest(view.snapshot.texts[path].as_bytes())),
                start,
                end,
                exact: view.snapshot.texts[path][start..end].to_string(),
            });
        }
        let request = PatchRequest {
            operation: key.clone(),
            base_tree: view.snapshot.tree_digest.clone(),
            consistency: agent::Consistency::ExactTree,
            dependencies: Vec::new(),
            patches: patches.clone(),
            request_digest: digest.clone(),
        };
        let applied = agent::apply_patches(&tree, &request).map_err(failure)?;
        let candidate = Candidate {
            view_id: view_id.to_string(),
            base_tree_digest: applied.before,
            tree_digest: applied.after,
            patches,
            dependencies,
            operation: key.clone(),
            parent_request_id: parent_request_id.to_owned(),
            digest: digest.clone(),
            validation: args["validation"].as_str().unwrap_or("source").to_string(),
            expires_at: view.expires_at,
            publish: publish.to_string(),
            notes: inputs
                .iter()
                .map(|v| v["note"].as_str().unwrap_or_default().to_string())
                .collect(),
            grant: CandidateGrant {
                account_id: who.id.id.clone(),
                generation: who.id.session_generation.clone(),
                link_hash: who.link.clone(),
            },
        };
        let candidate_id = format!(
            "candidate_{}",
            hex::encode(Sha256::digest(format!(
                "{actor}\0{}\0{digest}",
                key.request_id()
            )))
        );
        self.mcp_store(
            slug,
            actor,
            who,
            &candidate_id,
            "candidate",
            &candidate,
            candidate.expires_at,
            true,
        )
        .await?;
        self.mcp_finish_proposal(
            slug,
            actor,
            who,
            headers,
            arrival,
            peer,
            &candidate_id,
            &candidate,
            &view,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn mcp_finish_proposal(
        &self,
        slug: &str,
        actor: &str,
        who: &Viewer,
        headers: &HeaderMap,
        arrival: &Arrival,
        _peer: SocketAddr,
        candidate_id: &str,
        candidate: &Candidate,
        view: &View,
    ) -> Result<Value, Failure> {
        let key = &candidate.operation;
        let render = if candidate.validation == "compile" {
            Some(match self.load_render_receipt(slug,actor,who,candidate_id).await {
                Ok(receipt)=>receipt,
                Err(error) if error.code=="view_expired"=>self.mcp_render(slug,actor,who,headers,arrival,candidate_id,candidate,&key.id).await.map_err(|error|error.with_data(json!({"candidate_id":candidate_id,"operation":key,"retry_tool":"document_result","retry_arguments":{"kind":"render","id":candidate_id}})))?,
                Err(error)=>return Err(error),
            })
        } else {
            None
        };
        if let Some(render) = &render {
            if render.status != "verified" {
                return Err(
                    Failure::new("validation_failed", "candidate compilation failed")
                        .with_data(json!({"candidate_id":candidate_id,"render":render})),
                );
            }
        }
        // `status: committed` means the candidate was stored, not that anyone
        // can see it. This branch is `publish: "private"`, where nothing is
        // visible and nothing has changed; the default, `publish:
        // "suggestions"`, overwrites both fields below. Callers read
        // "committed" as "done" and report an edit that never happened, so
        // the result says outright what has not happened yet.
        //
        // No `replay` key here: storing a private candidate runs no §7.2
        // command at all (it is candidate-store idempotency, keyed by the
        // request digest, not a `document_labels` retry record), so there is
        // nothing honest to report until the `publish: "suggestions"` branch
        // below actually runs one.
        let mut result = json!({"operation":key,"status":"committed","candidate_id":candidate_id,"tree_digest":candidate.tree_digest,"base_tree_digest":candidate.base_tree_digest,"validation":{"source":"passed","compile":"not_requested"},"effects":[],
            "published":false,
            "next":"This candidate is private: nobody can see it and the document is unchanged. Propose again with publish: \"suggestions\" to offer it for review, or, only if the user authorized direct edits, apply it with document_apply quoting this candidate_id. Report neither until one of them returns."});
        if let Some(render) = render {
            result["validation"]["compile"] = json!("passed");
            result["render"] = json!(render);
        }
        if candidate.publish == "suggestions" {
            if candidate.patches.len() != 1 {
                return Err(Failure::new(
                    "invalid_params",
                    "publishing multiple suggestions is independent, not atomic; use batch: independent",
                ));
            }
            if !who.at_least(Role::Commenter) {
                return Err(Failure::new(
                    "permission_changed",
                    "comment access required to finish this proposal",
                ));
            }
            let author = self.mcp_author(headers, arrival, who, actor);
            let creator = if who.id.is_signed_in() {
                who.id.name.clone()
            } else if author.is_empty() {
                "Anonymous".into()
            } else {
                pseudonym_for(&author, slug)
            };
            let room = self
                .rooms
                .get(slug)
                .await
                .map_err(|e| Failure::new("unavailable", e.to_string()))?;
            if let Some(why) = room.unreadable().await {
                return Err(Failure::new("unavailable", why));
            }
            // Every suggestion in the pass is anchored fresh against the one
            // head `AgentSuggestionBatch::evaluate` locks (§7 step 2); this
            // module supplies only what the candidate actually asked for.
            // Each item's id is derived from the pass's own request id and
            // its position, so a retried pass mints the same ids and lands
            // on the same annotations (§7.2) rather than duplicating them.
            let pass = key.scoped_request_id(actor);
            let items: Vec<room::BatchSuggestion> = candidate
                .patches
                .iter()
                .enumerate()
                .map(|(index, patch)| {
                    let source = &view.snapshot.texts[&patch.path];
                    let prefix = source[..patch.start]
                        .chars()
                        .rev()
                        .take(64)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect::<String>();
                    let suffix = source[patch.end..].chars().take(64).collect::<String>();
                    let id = super::comments::parse_uuid(
                        &super::comments::comment_uuid(&format!("{pass}-{index}")),
                        "suggestion id",
                    )
                    .expect("comment_uuid always returns a UUID string");
                    room::BatchSuggestion {
                        id,
                        exact: patch.exact.clone(),
                        prefix,
                        suffix,
                        proposed: patch.replacement.clone(),
                        body: candidate.notes.get(index).cloned().unwrap_or_default(),
                    }
                })
                .collect();
            let author_account_id = uuid::Uuid::parse_str(&who.id.id).ok();
            let authorization = who.mutation_authorization(self.ceiling_for(&who.id).edit);
            let mut cmd = room::AgentSuggestionBatch::new(
                room.catalog().clone(),
                room.document_id,
                items,
                &self.config,
                creator.clone(),
                author_account_id,
                author.clone(),
                authorization,
            )
            .map_err(|error| Failure::new("invalid_params", error))?;
            let authority = who.document_authority();
            // `AgentSuggestionBatch` is idempotent by each item's own
            // primary key (§7.2's create rule), not by a `document_labels`
            // retry record, so this is honestly always `false`: dedup on
            // retry happens per item at insert time, not here.
            let (outcomes, replay) = room
                .command_reporting_replay(&authority, &mut cmd)
                .await
                .map_err(super::comments::command_failure)?;
            result["replay"] = json!(replay);
            let refusal = outcomes.iter().find_map(|outcome| match outcome {
                room::BatchItemResult::Refused(reason) => Some(reason.clone()),
                room::BatchItemResult::Created(_) => None,
            });
            if let Some(reason) = refusal {
                return Err(Failure::new("conflict", reason));
            }
            // Rows are read live; existing immutable anchors stay cached
            // so their subscribers continue receiving source movements.
            room.broadcast_comments_changed().await;
            result["effects"] = json!(outcomes
                .iter()
                .filter_map(|outcome| match outcome {
                    room::BatchItemResult::Created(comment) =>
                        Some(json!({"kind":"suggestion","id":comment.id})),
                    room::BatchItemResult::Refused(_) => None,
                })
                .collect::<Vec<_>>());
            // These suggestions are real and visible, so the warning above no
            // longer applies. They are still proposals: the document's own
            // text is unchanged until a person accepts them.
            result["published"] = json!(true);
            result["next"] = json!(
                "These suggestions are visible for review. The document's text is unchanged until someone accepts them; do not report the words as corrected."
            );
            return Ok(result);
        }
        // No receipt is retained for private staging, but access is still
        // rechecked before the result is handed back.
        self.mcp_recheck(slug, headers, arrival, actor).await?;
        Ok(result)
    }

    async fn mcp_result(
        &self,
        slug: &str,
        actor: &str,
        headers: &HeaderMap,
        arrival: &Arrival,
        peer: SocketAddr,
        args: &Value,
    ) -> Result<Value, Failure> {
        let who = self.mcp_recheck(slug, headers, arrival, actor).await?;
        if args["action"] == "cancel" {
            return self.mcp_cancel(slug, actor, headers, arrival, args).await;
        }
        match args["kind"].as_str().unwrap_or("operation") {
            "operation" => {
                let key: OperationKey = serde_json::from_value(
                    args.get("target_operation")
                        .or_else(|| args.get("operation"))
                        .cloned()
                        .unwrap_or(Value::Null),
                )
                .map_err(|_| Failure::new("invalid_params", "operation identity required"))?;
                // An admitted key reports that its outcome is unknown; an
                // unknown one reports that nothing is retained. Neither can
                // return a result, because no receipt is kept.
                self.mcp_refuse_if_admitted(slug, actor, &who, &key, None)
                    .await?;
                Err(Failure::new(
                    "outcome_unknown",
                    "no retained operation receipt; absence does not prove nonexecution",
                ))
            }
            "candidate" | "render" => {
                let candidate: Candidate = self
                    .mcp_load(
                        slug,
                        actor,
                        &who,
                        args["id"].as_str().unwrap_or_default(),
                        "candidate",
                    )
                    .await?;
                if args["kind"] == "render" {
                    let who = self.mcp_recheck(slug, headers, arrival, actor).await?;
                    let view = self
                        .mcp_load::<View>(slug, actor, &who, &candidate.view_id, "view")
                        .await?;
                    return self
                        .mcp_finish_proposal(
                            slug,
                            actor,
                            &who,
                            headers,
                            arrival,
                            peer,
                            args["id"].as_str().unwrap_or_default(),
                            &candidate,
                            &view,
                        )
                        .await;
                }
                Ok(
                    json!({"candidate_id":args["id"],"tree_digest":candidate.tree_digest,"base_tree_digest":candidate.base_tree_digest,"validation":candidate.validation,"expires_at":candidate.expires_at,"status":if candidate.validation=="compile"{"awaiting_renderer"}else{"ready"}}),
                )
            }
            _ => Err(Failure::new("invalid_params", "unknown result kind")),
        }
    }
}

/// The byte offset of a UTF-16 offset, or `None` when it falls inside a
/// character. A patch is stated in bytes and an anchor counts the way a
/// browser does, so one of the two has to be converted, exactly.
pub(super) fn byte_of_utf16(text: &str, target: usize) -> Option<usize> {
    let mut units = 0;
    for (byte, character) in text.char_indices() {
        if units == target {
            return Some(byte);
        }
        units += character.len_utf16();
    }
    (units == target).then_some(text.len())
}

#[cfg(test)]
mod digest_tests {
    use super::*;

    /// An operation key names one operation. Two spellings of the same
    /// arguments are the same operation, however the client ordered the
    /// keys -- including inside a nested object.
    #[test]
    fn reordering_keys_does_not_change_the_operation() {
        let one = json!({
            "operation": {"id": "a", "epoch": "e"},
            "patches": [{"path": "main.md", "start": 0, "end": 3}],
        });
        let other = json!({
            "patches": [{"end": 3, "path": "main.md", "start": 0}],
            "operation": {"epoch": "e", "id": "a"},
        });
        assert_eq!(
            operation_digest("document_propose", &one),
            operation_digest("document_propose", &other),
        );
    }

    /// Everything that is actually a difference stays one: a changed value,
    /// a reordered array, a different tool.
    #[test]
    fn a_real_difference_is_still_a_different_operation() {
        let base = json!({"patches": [{"path": "a.md"}, {"path": "b.md"}], "batch": "atomic"});
        let changed_value =
            json!({"patches": [{"path": "a.md"}, {"path": "b.md"}], "batch": "independent"});
        let reordered_array =
            json!({"patches": [{"path": "b.md"}, {"path": "a.md"}], "batch": "atomic"});
        let digest = operation_digest("document_propose", &base);
        assert_ne!(digest, operation_digest("document_propose", &changed_value));
        assert_ne!(
            digest,
            operation_digest("document_propose", &reordered_array),
            "an array's order is part of its value",
        );
        assert_ne!(digest, operation_digest("document_apply", &base));
    }
}
