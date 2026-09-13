//! Domain operations behind the five MCP tools. Transport ids never identify effects.
use super::*;
use crate::room::agent::{
    self, Affinity, AgentAuthority, OperationKey, Patch, PatchRequest, SourceFile, SourceTree,
};
use crate::room::agent_comments::AnnotationBatch;
use crate::room::{BatchCaller, Comment, SourceAnchor};

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
    pub base_revision: String,
    pub source_revision: String,
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
        agent::AgentError::NotFound => "not_found",
        agent::AgentError::Storage(_) => "outcome_unknown",
    };
    Failure::new(code, error.to_string())
}

pub(super) fn tree_of_view(view: &View) -> Result<SourceTree, Failure> {
    let canonical: crate::document::history::Tree =
        serde_json::from_value(view.snapshot.tree.clone())
            .map_err(|e| Failure::new("internal", e.to_string()))?;
    let files = view
        .snapshot
        .texts
        .iter()
        .map(|(path, text)| {
            (
                path.clone(),
                SourceFile {
                    file_id: canonical
                        .files
                        .get(path)
                        .map(|entry| entry.id.clone())
                        .unwrap_or_default(),
                    text: text.clone(),
                },
            )
        })
        .collect();
    Ok(SourceTree::with_canonical(canonical, files))
}

pub(super) fn candidate_texts(
    view: &View,
    candidate: &Candidate,
) -> Result<std::collections::BTreeMap<String, String>, Failure> {
    let mut texts = view.snapshot.texts.clone();
    let mut patches = candidate.patches.iter().collect::<Vec<_>>();
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
    Ok(texts)
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

    pub(super) async fn mcp_cancelled_children(
        &self,
        slug: &str,
        actor: &str,
        key: &OperationKey,
        result: Value,
    ) -> Result<Value, Failure> {
        if result["status"] != "cancel_requested" {
            return Ok(result);
        }
        let _ = (slug, actor, key);
        Ok(result)
    }

    /// Private staging has no annotation rows, but its terminal receipt uses
    /// the same transactional authority/cancellation boundary. A separate
    /// object write would race cancellation between its check and commit.
    #[allow(clippy::too_many_arguments)]
    async fn mcp_private_receipt(
        &self,
        slug: &str,
        actor: &str,
        key: &OperationKey,
        digest: &str,
        result: &Value,
        who: &Viewer,
        headers: &HeaderMap,
        parent_request_id: &str,
    ) -> Result<Value, Failure> {
        let _ = (slug, actor, key, digest, who, headers, parent_request_id);
        Ok(result.clone())
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

    pub(super) async fn mcp_receipt(
        &self,
        slug: &str,
        actor: &str,
        who: &Viewer,
        key: &OperationKey,
        digest: Option<&str>,
    ) -> Result<Option<Value>, Failure> {
        self.mcp_epoch(actor, key, true)?;
        let id = key.scoped_request_id(actor);
        let admitted = match self
            .mcp_load::<Admission>(slug, actor, who, &id, "admission")
            .await
        {
            Ok(admitted) if digest.is_some_and(|digest| digest != admitted.digest) => {
                return Err(Failure::new(
                    "operation_key_reused",
                    "operation key already admitted different arguments",
                ));
            }
            Ok(_) => true,
            Err(error) if error.code == "view_expired" => false,
            Err(error) => return Err(error),
        };
        let _ = (slug, who, id);
        if admitted && digest.is_none() {
            Ok(Some(
                json!({"operation":key,"status":"admitted","reconciliation":"retry the original tool call with unchanged arguments; no committed outcome is retained yet"}),
            ))
        } else {
            Ok(None)
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
        let digest = hex::encode(Sha256::digest(
            json!({"tool":name,"arguments":args}).to_string(),
        ));
        let current = self.mcp_recheck(slug, headers, arrival, actor).await?;
        let receipt = self
            .mcp_receipt(slug, actor, who, &key, Some(&digest))
            .await?;
        if let Some(result) = receipt {
            return Ok(result);
        }
        // Cancellation of uncommitted work precedes fresh admission. A
        // retained terminal receipt above remains the authoritative outcome.
        if let Some(cancellation) = self.mcp_cancellation(slug, actor, who, &key).await? {
            return Ok(cancellation);
        }
        self.mcp_admit(slug, actor, who, &key, &digest).await?;
        match name {
            "document_propose" => {
                if args["batch"] == "independent" {
                    let mut items = Vec::new();
                    for (index, patch) in
                        args["patches"].as_array().into_iter().flatten().enumerate()
                    {
                        if let Some(cancellation) =
                            self.mcp_cancellation(slug, actor, who, &key).await?
                        {
                            return Ok(cancellation);
                        }
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
                        items.push(match outcome {Ok(result)=>json!({"index":index,"status":result["status"],"candidate_id":result["candidate_id"],"effects":result["effects"]}),Err(error)=>json!({"index":index,"error":{"code":error.code,"message":error.message}})});
                    }
                    let result = json!({"operation":key,"status":"committed","batch":"independent","items":items,"replay":false});
                    let current = self.mcp_recheck(slug, headers, arrival, actor).await?;
                    return self
                        .mcp_private_receipt(
                            slug,
                            actor,
                            &key,
                            &digest,
                            &result,
                            &current,
                            headers,
                            parent_request_id,
                        )
                        .await;
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
                if let Some(cancellation) = self
                    .mcp_cancellation(slug, actor, who, &candidate.operation)
                    .await?
                {
                    return Ok(cancellation);
                }
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
                    if receipt.status != "verified"
                        || receipt.source_revision != candidate.source_revision
                    {
                        return Err(Failure::new(
                            "validation_failed",
                            "candidate has no matching successful render receipt",
                        ));
                    }
                }
                let request = PatchRequest {
                    acceptance: None,
                    operation: key,
                    base_tree: candidate.base_revision,
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
                    .try_get(slug)
                    .await
                    .map_err(|e| Failure::new("unavailable", e.to_string()))?;
                let receipt = room
                    .apply_agent_request(request, authority)
                    .await
                    .map_err(failure)?;
                Ok(json!(receipt))
            }
            "document_comment" => {
                if args["action"] == "accept" {
                    return self
                        .mcp_accept(slug, actor, &current, headers, args, &key, &digest)
                        .await;
                }
                if args["action"] == "checkpoint" {
                    return self
                        .mcp_checkpoint(
                            slug, actor, &current, headers, arrival, peer, args, &key, &digest,
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

    #[allow(clippy::too_many_arguments)]
    async fn mcp_accept(
        &self,
        slug: &str,
        actor: &str,
        who: &Viewer,
        headers: &HeaderMap,
        args: &Value,
        key: &OperationKey,
        digest: &str,
    ) -> Result<Value, Failure> {
        if !who.at_least(Role::Editor) {
            return Err(Failure::new(
                "permission_changed",
                "editor access is required to accept a suggestion",
            ));
        }
        let view: View = self
            .mcp_load(
                slug,
                actor,
                who,
                args["view_id"]
                    .as_str()
                    .ok_or_else(|| Failure::new("invalid_params", "accept requires view_id"))?,
                "view",
            )
            .await?;
        let room = self
            .rooms
            .try_get(slug)
            .await
            .map_err(|e| Failure::new("unavailable", e.to_string()))?;
        let id = args["comment_id"]
            .as_str()
            .ok_or_else(|| Failure::new("invalid_params", "accept requires comment_id"))?;
        let expected = args["expected_version"]
            .as_str()
            .ok_or_else(|| Failure::new("invalid_params", "accept requires expected_version"))?;
        let comment = room
            .agent_comment(id)
            .await
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
        if comment.revision != view.snapshot.source_revision {
            return Err(Failure::new(
                "conflict",
                "suggestion belongs to another source tree; capture and propose against the current passage",
            ));
        }
        let anchor = comment
            .source
            .ok_or_else(|| Failure::new("invalid_range", "suggestion has no source anchor"))?;
        let text = view
            .snapshot
            .texts
            .get(&anchor.path)
            .ok_or_else(|| Failure::new("conflict", "suggestion file disappeared"))?;
        let position = anchor
            .position
            .and_then(|p| usize::try_from(p).ok())
            .ok_or_else(|| {
                Failure::new(
                    "ambiguous_range",
                    "suggestion requires an exact captured position",
                )
            })?;
        let mut units = 0;
        let mut start = None;
        for (byte, ch) in text.char_indices() {
            if units == position {
                start = Some(byte);
                break;
            }
            units += ch.len_utf16();
        }
        if start.is_none() && units == position {
            start = Some(text.len());
        }
        let start = start
            .ok_or_else(|| Failure::new("invalid_range", "position splits a Unicode character"))?;
        let end = start.saturating_add(anchor.exact.len());
        if text.get(start..end) != Some(anchor.exact.as_str()) {
            return Err(Failure::new("conflict", "suggestion passage changed"));
        }
        let tree = tree_of_view(&view)?;
        let request = PatchRequest {
            acceptance: Some(agent::AgentAcceptance {
                comment_id: id.to_string(),
                expected_version: expected.to_string(),
                expected_seq: comment.seq,
            }),
            operation: key.clone(),
            base_tree: view.snapshot.source_revision,
            consistency: agent::Consistency::ExactTree,
            dependencies: Vec::new(),
            patches: vec![Patch {
                path: anchor.path.clone(),
                file_id: tree.files[&anchor.path].file_id.clone(),
                start,
                end,
                exact: anchor.exact,
                replacement: comment.proposed.ok_or_else(|| {
                    Failure::new("invalid_params", "suggestion has no replacement")
                })?,
                affinity: Affinity::Before,
            }],
            request_digest: digest.to_string(),
        };
        let authority = AgentAuthority {
            automation: true,
            account_id: who.id.id.clone(),
            owner_key: who.key.clone(),
            generation: who.id.session_generation.clone(),
            link_hash: who.link.clone(),
            policy_editor: self.publishers.allows(&who.id.handle),
            unowned_publisher: false,
            operation_scope: actor.to_string(),
            execution_epoch: runner_execution_epoch(headers),
        };
        room.apply_agent_request(request, authority)
            .await
            .map(|receipt| json!(receipt))
            .map_err(failure)
    }

    #[allow(clippy::too_many_arguments)]
    async fn mcp_checkpoint(
        &self,
        slug: &str,
        actor: &str,
        who: &Viewer,
        headers: &HeaderMap,
        _arrival: &Arrival,
        _peer: SocketAddr,
        args: &Value,
        key: &OperationKey,
        digest: &str,
    ) -> Result<Value, Failure> {
        if !who.at_least(Role::Editor) {
            return Err(Failure::new(
                "permission_changed",
                "editor access is required to checkpoint",
            ));
        }
        if let Some(cancellation) = self.mcp_cancellation(slug, actor, who, key).await? {
            return Ok(cancellation);
        }
        let view: View = self
            .mcp_load(
                slug,
                actor,
                who,
                args["view_id"]
                    .as_str()
                    .ok_or_else(|| Failure::new("invalid_params", "checkpoint requires view_id"))?,
                "view",
            )
            .await?;
        let room = self
            .rooms
            .try_get(slug)
            .await
            .map_err(|e| Failure::new("unavailable", e.to_string()))?;
        let authority = AgentAuthority {
            automation: true,
            account_id: who.id.id.clone(),
            owner_key: who.key.clone(),
            generation: who.id.session_generation.clone(),
            link_hash: who.link.clone(),
            policy_editor: self.publishers.allows(&who.id.handle),
            unowned_publisher: false,
            operation_scope: actor.to_string(),
            execution_epoch: runner_execution_epoch(headers),
        };
        let commit = json!({
            "request_id": key.scoped_request_id(actor),
            "digest": digest,
            "operation": key,
            "source_revision": view.snapshot.source_revision,
        });
        let checkpoint = room
            .ensure_agent_checkpoint(
                &view.snapshot.source_revision,
                authority,
                &who.id.name,
                commit,
            )
            .await
            .map_err(|e| Failure::new("conflict", e))?;
        Ok(
            json!({"operation":key,"status":"committed","action":"checkpoint","checkpoint_id":checkpoint,"source_revision":view.snapshot.source_revision,"replay":false}),
        )
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
            acceptance: None,
            operation: key.clone(),
            base_tree: view.snapshot.source_revision.clone(),
            consistency: agent::Consistency::ExactTree,
            dependencies: Vec::new(),
            patches: patches.clone(),
            request_digest: digest.clone(),
        };
        let applied = agent::apply_patches(&tree, &request).map_err(failure)?;
        let candidate = Candidate {
            view_id: view_id.to_string(),
            base_revision: applied.before_tree,
            source_revision: applied.after_tree,
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
        peer: SocketAddr,
        candidate_id: &str,
        candidate: &Candidate,
        view: &View,
    ) -> Result<Value, Failure> {
        let key = &candidate.operation;
        if let Some(cancellation) = self.mcp_cancellation(slug, actor, who, key).await? {
            return Ok(cancellation);
        }
        let digest = candidate.digest.clone();
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
        let mut result = json!({"operation":key,"status":"committed","candidate_id":candidate_id,"source_revision":candidate.source_revision,"base_revision":candidate.base_revision,"validation":{"source":"passed","compile":"not_requested"},"effects":[],"replay":false});
        if let Some(render) = render {
            result["validation"]["compile"] = json!("passed");
            result["render"] = json!(render);
        }
        if candidate.publish == "suggestions" {
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
            let pass = key.scoped_request_id(actor);
            let rows = candidate
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
                    Comment {
                        id: format!("{}-{index}", &pass[..32]),
                        motivation: "editing".into(),
                        source: Some(SourceAnchor {
                            path: patch.path.clone(),
                            exact: patch.exact.clone(),
                            prefix,
                            suffix,
                            position: Some(source[..patch.start].encode_utf16().count() as i64),
                        }),
                        proposed: Some(patch.replacement.clone()),
                        body: candidate.notes.get(index).cloned().unwrap_or_default(),
                        creator: creator.clone(),
                        created: crate::util::timestamp(),
                        author: author.clone(),
                        via: who.link.clone(),
                        revision: view.snapshot.source_revision.clone(),
                        pass: pass.clone(),
                        ..Comment::default()
                    }
                })
                .collect::<Vec<_>>();
            result["effects"] = json!(rows
                .iter()
                .map(|c| json!({"kind":"suggestion","id":c.id}))
                .collect::<Vec<_>>());
            let room = self
                .rooms
                .try_get(slug)
                .await
                .map_err(|e| Failure::new("unavailable", e.to_string()))?;
            let address = client_address(peer, headers, &self.config.cost.trusted_proxies);
            return room
                .apply_agent_annotations(
                    AnnotationBatch {
                        request_id: pass,
                        digest,
                        base_revision: Some(view.snapshot.source_revision.clone()),
                        upserts: rows,
                        expected: Vec::new(),
                        deletes: Vec::new(),
                        replies: Vec::new(),
                        receipt: result,
                    },
                    BatchCaller {
                        address: &address,
                        author: &author,
                        via: &who.link,
                        budget: who.comment_budget,
                        creator: &creator,
                    },
                    crate::room::agent_comments::AgentAnnotationAuthority {
                        account_id: who.id.id.clone(),
                        generation: who.id.session_generation.clone(),
                        link_hash: who.link.clone(),
                        policy_comment: true,
                        require_editor: false,
                        parent_request_id: candidate.parent_request_id.clone(),
                        execution_epoch: runner_execution_epoch(headers),
                    },
                )
                .await
                .map_err(|e| Failure::new("conflict", e));
        }
        let current = self.mcp_recheck(slug, headers, arrival, actor).await?;
        self.mcp_private_receipt(
            slug,
            actor,
            key,
            &digest,
            &result,
            &current,
            headers,
            &candidate.parent_request_id,
        )
        .await
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
                if let Some(cancellation) = self.mcp_cancellation(slug, actor, &who, &key).await? {
                    return Ok(cancellation);
                }
                let result = self
                    .mcp_receipt(slug, actor, &who, &key, None)
                    .await?
                    .ok_or_else(|| {
                        Failure::new(
                            "outcome_unknown",
                            "no retained operation receipt; absence does not prove nonexecution",
                        )
                    })?;
                if result["status"] == "prepared" {
                    let who = self.mcp_recheck(slug, headers, arrival, actor).await?;
                    let room = self
                        .rooms
                        .try_get(slug)
                        .await
                        .map_err(|e| Failure::new("unavailable", e.to_string()))?;
                    let authority = AgentAuthority {
                        automation: true,
                        account_id: who.id.id.clone(),
                        owner_key: who.key.clone(),
                        generation: who.id.session_generation.clone(),
                        link_hash: who.link.clone(),
                        policy_editor: self.publishers.allows(&who.id.handle),
                        unowned_publisher: false,
                        operation_scope: actor.to_string(),
                        execution_epoch: runner_execution_epoch(headers),
                    };
                    return room
                        .recover_agent_operation(key, authority)
                        .await
                        .map(|receipt| json!(receipt))
                        .map_err(failure);
                }
                Ok(result)
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
                    if let Some(cancellation) = self
                        .mcp_cancellation(slug, actor, &who, &candidate.operation)
                        .await?
                    {
                        return Ok(cancellation);
                    }
                    if let Some(receipt) = self
                        .mcp_receipt(
                            slug,
                            actor,
                            &who,
                            &candidate.operation,
                            Some(&candidate.digest),
                        )
                        .await?
                    {
                        return Ok(receipt);
                    }
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
                if let Some(cancellation) = self
                    .mcp_cancellation(slug, actor, &who, &candidate.operation)
                    .await?
                {
                    return Ok(cancellation);
                }
                Ok(
                    json!({"candidate_id":args["id"],"source_revision":candidate.source_revision,"base_revision":candidate.base_revision,"validation":candidate.validation,"expires_at":candidate.expires_at,"status":if candidate.validation=="compile"{"awaiting_renderer"}else{"ready"}}),
                )
            }
            _ => Err(Failure::new("invalid_params", "unknown result kind")),
        }
    }
}
