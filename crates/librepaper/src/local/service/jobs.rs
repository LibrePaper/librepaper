//! The job surface: admitting a build, running it, and answering for it.
//!
//! `POST jobs` is the only door a build comes through, and everything from
//! the multipart upload to the workspace it is staged into to the status a
//! caller polls is here. Admission in particular: what a request has to
//! satisfy before anything is copied onto this computer, which is the whole
//! of what stands between a page in a browser and the author's files.
//!
//! Split out of `service` for size rather than for structure -- these
//! handlers reach `Inner` through the same `&Inner`/`pub(super)` pattern the
//! rest of the module uses, and nothing about the routing changed.

use super::*;

/* ------------------------------------------------------------ workspace */

/// `PUT workspace`: sync the hosted workspace for this (origin, project)
/// scope from a fresh multipart upload, the same shape as `jobs`'s own
/// `manifest`/`file` parts but named `manifest` here since there is no job
/// to describe. Only ever touches the hosted workspace `get_scoped` returns
/// for `HOSTED_BINDING`; a granted (non-hosted) root is never written here.
pub(super) async fn handle_workspace_put(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    let project = match authenticate(inner, headers, origin) {
        Ok(project) => project,
        Err(response) => return response,
    };
    let origin = origin.unwrap_or_default().to_string();

    let upload = match read_multipart_upload(
        request,
        "manifest",
        "bad manifest part",
        "manifest is too large",
        "missing the manifest part",
    )
    .await
    {
        Ok(upload) => upload,
        Err(response) => return response,
    };
    let uploads = upload.files;
    let manifest_text = upload.metadata;
    let manifest: Vec<ManifestEntry> = match serde_json::from_str(&manifest_text) {
        Ok(manifest) => manifest,
        Err(_) => return write_json(400, &json!({"error": "bad manifest"})),
    };
    if let Err(response) = validate_manifest_shape(&manifest) {
        return response;
    }
    if let Err(response) = validate_manifest(&manifest, &uploads) {
        return response;
    }

    let Some(binding) = inner
        .quarto_bindings
        .get_scoped(HOSTED_BINDING, &origin, &project)
    else {
        return write_json(
            404,
            &json!({"error": "this local app keeps no hosted workspace"}),
        );
    };

    let staged = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(_) => return write_json(500, &json!({"error": "could not create a workspace"})),
    };
    if let Err(error) = stage_uploads(staged.path(), &uploads) {
        return write_json(400, &json!({"error": error}));
    }

    match sync_hosted_workspace(staged.path(), &binding.root, &manifest) {
        Ok(()) => write_json(200, &json!({"synced": manifest.len()})),
        Err(error) => write_json(400, &json!({"error": error})),
    }
}

pub(super) async fn handle_jobs_post(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    let project = match authenticate(inner, headers, origin) {
        Ok(project) => project,
        Err(response) => return response,
    };
    let origin = origin.unwrap_or_default().to_string();

    let upload = match read_multipart_upload(
        request,
        "job",
        "bad job part",
        "job description is too large",
        "missing the job part",
    )
    .await
    {
        Ok(upload) => upload,
        Err(response) => return response,
    };
    let mut uploads = upload.files;
    let job_text = upload.metadata;
    let raw_job: Value = match serde_json::from_str(&job_text) {
        Ok(job) => job,
        Err(_) => return write_json(400, &json!({"error": "bad job description"})),
    };
    // The version is refused here, at the wire, and nowhere else. There used
    // to be a second decode below for anything that was not protocol 2,
    // producing a `JobRequest` whose required fields were absent and which
    // every consumer then had to treat as optional -- for a version
    // `PROTOCOL_VERSIONS` has not advertised since v2 landed.
    if raw_job.get("protocol").and_then(Value::as_u64) != Some(2) {
        return write_json(400, &json!({"error": "unsupported protocol version"}));
    }
    let mut job: JobRequest = {
        let request: protocol::BuildRequestV2 = match serde_json::from_value(raw_job) {
            Ok(request) => request,
            Err(error) => {
                return write_json(
                    400,
                    &json!({"error": format!("bad protocol 2 job description: {error}")}),
                )
            }
        };
        if let Err(error) = request.validate_shape() {
            return write_json(400, &json!({"error": error}));
        }
        let is_quarto = request.builder == "quarto";
        if request.builder == "calepin"
            && !matches!(
                request.workspace,
                protocol::WorkspaceRequest::Snapshot {
                    binding_id: Some(_)
                }
            )
        {
            return write_json(
                400,
                &json!({"error": "Calepin builds require a scoped snapshot binding"}),
            );
        }
        let inputs = if is_quarto {
            let binding_id = match &request.workspace {
                protocol::WorkspaceRequest::Snapshot {
                    binding_id: Some(id),
                } => id.clone(),
                _ => {
                    return write_json(
                        400,
                        &json!({"error": "Quarto builds require a scoped snapshot binding"}),
                    )
                }
            };
            protocol::BuildInputs::Quarto {
                overrides: request.options.clone(),
                options: protocol::QuartoJobOptions {
                    binding_id,
                    main: request.entrypoint.clone(),
                    format: request.output.clone(),
                    execution_mode: protocol::QuartoExecutionMode::IsolatedSnapshot,
                    profile: request
                        .options
                        .get("profile")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned),
                    parameters: request
                        .options
                        .get("parameters")
                        .and_then(Value::as_object)
                        .map(|values| {
                            values
                                .iter()
                                .map(|(key, value)| (key.clone(), value.clone()))
                                .collect()
                        })
                        .unwrap_or_default(),
                    policy: request
                        .options
                        .get("policy")
                        .map(|value| serde_json::from_value(value.clone()))
                        .transpose()
                        .unwrap_or_default()
                        .unwrap_or_default(),
                    data_inputs: request
                        .options
                        .get("data_inputs")
                        .and_then(Value::as_array)
                        .map(|values| {
                            values
                                .iter()
                                .filter_map(Value::as_str)
                                .map(str::to_owned)
                                .collect()
                        })
                        .unwrap_or_default(),
                    shared_inventory_complete: true,
                    shared_tree_sha256: Some(manifest_tree_digest(&request.manifest, &uploads)),
                    ..Default::default()
                },
            }
        } else {
            protocol::BuildInputs::Native {
                options: request.options,
            }
        };
        JobRequest {
            protocol: 2,
            kind: if is_quarto {
                "quarto".into()
            } else {
                "build".into()
            },
            project: request.project,
            origin: request.origin,
            snapshot: request.snapshot,
            generation: request.generation,
            inputs,
            manifest: request.manifest,
            source: request.source,
            options: JobOptions {
                deadline_seconds: request
                    .deadline_seconds
                    .unwrap_or(protocol::DEFAULT_DEADLINE_SECONDS),
            },
            builder: request.builder,
            workspace: request.workspace,
            entrypoint: request.entrypoint,
            output: request.output,
            preset: request.preset,
        }
    };
    // Bound local reads must follow authentication and inventory limits.
    if job.project != project
        || pairing::normalize_origin(&job.origin) != pairing::normalize_origin(&origin)
    {
        return write_json(
            403,
            &json!({"error": "job scope does not match the connected token"}),
        );
    }
    if let Err(response) = validate_manifest_shape(&job.manifest) {
        return response;
    }
    if let Some(source) = &job.source {
        if let Err(error) = source.validate() {
            return write_json(400, &json!({"error": error}));
        }
        if source.tree_sha256 != job.snapshot || source.main_path != job.entrypoint {
            return write_json(
                400,
                &json!({"error": "source snapshot identity does not match the build request"}),
            );
        }
        if source.manifest_sha256 != input_manifest_digest(&job.manifest) {
            return write_json(
                400,
                &json!({"error": "source manifest digest does not match the materialized inputs"}),
            );
        }
    }
    {
        // Shape, size and safety of `builder`, `entrypoint`, `output` and
        // `workspace` were settled by `BuildRequestV2::validate_shape` above,
        // which is the one validation this protocol has. What is left here is
        // what that check cannot reach: whether *this* caller holds the
        // binding a bound snapshot names, and what its files actually are.
        let builder = job.builder.as_str();
        let workspace = &job.workspace;
        let entrypoint = job.entrypoint.as_str();
        // Builds always run in service-owned temporary workspaces. Bound
        // directories are a preview surface; a snapshot may name a binding
        // solely to authorize copying selected inputs into its temp workspace.
        if matches!(workspace, protocol::WorkspaceRequest::Bound { .. }) {
            return write_json(
                400,
                &json!({"error": "bound workspace is only available for managed previews"}),
            );
        }
        if let protocol::WorkspaceRequest::Snapshot {
            binding_id: Some(binding_id),
        } = workspace
        {
            let Some(binding) = inner
                .quarto_bindings
                .get_scoped(binding_id, &origin, &project)
            else {
                return write_json(
                    403,
                    &json!({"error": "workspace binding is not granted for this origin and project"}),
                );
            };
            if binding.entrypoint != entrypoint {
                return write_json(
                    403,
                    &json!({"error": "entrypoint does not match the scoped binding"}),
                );
            }
            let authorized_root = match std::fs::canonicalize(&binding.root) {
                Ok(root) if root.is_dir() && root == binding.root => root,
                _ => {
                    return write_json(403, &json!({"error": "scoped binding root is unavailable"}))
                }
            };
            // A bound snapshot contributes only files explicitly named by
            // the manifest. Copying happens into this request's fresh job
            // workspace below; the binding root itself is never executed.
            for entry in &job.manifest {
                if uploads.iter().any(|(name, _)| name == &entry.path) {
                    continue;
                }
                let source = binding.root.join(&entry.path);
                let source = match std::fs::canonicalize(&source) {
                    Ok(source) if source.starts_with(&authorized_root) && source.is_file() => {
                        source
                    }
                    _ => {
                        return write_json(
                            400,
                            &json!({"error": format!("bound input is outside the authorized folder: {}", entry.path)}),
                        )
                    }
                };
                let bytes = match read_bounded_public(&source, MAX_UPLOAD_BYTES) {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        return write_json(
                            400,
                            &json!({"error": format!("bound input is unavailable: {}", entry.path)}),
                        )
                    }
                };
                if bytes.len() as u64 != entry.size
                    || crate::results::sha256(&bytes) != entry.sha256
                {
                    return write_json(
                        400,
                        &json!({"error": format!("bound input digest mismatch: {}", entry.path)}),
                    );
                }
                uploads.push((entry.path.clone(), bytes));
            }
        }
        debug_assert_eq!(job.kind == "quarto", builder == "quarto");
    }
    if job.project != project {
        return write_json(
            403,
            &json!({"error": "project does not match the connected token"}),
        );
    }
    if pairing::normalize_origin(&job.origin) != pairing::normalize_origin(&origin) {
        return write_json(
            403,
            &json!({"error": "origin does not match the connected token"}),
        );
    }
    if let Err(response) = validate_manifest_shape(&job.manifest) {
        return response;
    }
    let previews = inner.previews.lock().await;
    if job.kind == "quarto" {
        if previews.0.values().any(|p| {
            job.inputs.quarto().is_some_and(|q| {
                p.binding == q.binding_id
                    || inner
                        .quarto_bindings
                        .get_scoped(&q.binding_id, &origin, &project)
                        .is_some_and(|binding| binding.root == p.root)
            })
        }) {
            return write_json(
                409,
                &json!({"error":"Stop managed preview before rendering or publishing."}),
            );
        }
        let Some(options) = job.inputs.quarto() else {
            return write_json(
                400,
                &json!({"error": "quarto job is missing typed options"}),
            );
        };
        if let Err(error) = options.validate() {
            return write_json(400, &json!({"error": error}));
        }
        if inner
            .quarto_bindings
            .get_scoped(&options.binding_id, &origin, &project)
            .is_none()
        {
            return write_json(
                403,
                &json!({"error": "quarto binding is not granted for this origin and project"}),
            );
        }
    }
    if job.manifest.len() > MAX_FILES {
        return write_json(413, &json!({"error": "too many files"}));
    }

    if let Err(response) = validate_manifest(&job.manifest, &uploads) {
        return response;
    }
    if let Some(options) = job.inputs.quarto_mut() {
        options.shared_tree_sha256 = Some(manifest_tree_digest(&job.manifest, &uploads));
    }

    // Answer a repeat before staging anything, so a retry does not copy the
    // uploads again only to be refused below.
    {
        let jobs = inner.jobs.lock().await;
        if let Some(response) = admitted_idempotent_response(&jobs, &origin, &project, &job) {
            return response;
        }
    }

    let id = crate::util::new_id();
    let root = inner.jobs_root.join(&id);
    let project_dir = root.join("project");
    if std::fs::create_dir_all(&project_dir).is_err() {
        return write_json(500, &json!({"error": "could not create a workspace"}));
    }
    if let Err(error) = stage_uploads(&project_dir, &uploads) {
        let _ = std::fs::remove_dir_all(&root);
        return write_json(400, &json!({"error": error}));
    }

    let workspace = Workspace { root: root.clone() };
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let status = JobStatus {
        id: id.clone(),
        kind: job.kind.clone(),
        status: "queued".to_string(),
        stage: "staging".to_string(),
        snapshot: job.snapshot.clone(),
        generation: job.generation,
        ..Default::default()
    };
    let generation = job.generation;

    let mut jobs = inner.jobs.lock().await;
    // The early lookup above avoids most duplicate staging, but it cannot
    // reserve a key across concurrent requests. Recheck while holding the
    // admission lock immediately before insertion so only one request can
    // become executable for a given scoped idempotency key.
    if let Some(response) = admitted_idempotent_response(&jobs, &origin, &project, &job) {
        drop(jobs);
        let _ = std::fs::remove_dir_all(&root);
        return response;
    }
    supersede_older_generations(&mut jobs, &origin, &project, generation);
    let mut queue = inner.queue.lock().await;
    queue.retain(|qid| jobs.get(qid).is_some_and(|entry| !entry.superseded));
    if queue.len() >= MAX_QUEUE {
        drop(queue);
        drop(jobs);
        let _ = std::fs::remove_dir_all(&root);
        return write_json(
            429,
            &json!({"error": "the local queue is full; try again shortly"}),
        );
    }
    jobs.insert(
        id.clone(),
        JobEntry {
            status,
            request: job,
            origin,
            project,
            files: BTreeMap::new(),
            workspace,
            cancel_tx,
            cancel_rx,
            queued: true,
            superseded: false,
            finished_at: None,
        },
    );
    if jobs
        .get(&id)
        .is_some_and(|entry| entry.request.kind == "quarto")
    {
        let persisted = jobs
            .get(&id)
            .ok_or_else(|| "admitted Quarto job disappeared".to_string())
            .and_then(|entry| persist_quarto_job(&id, entry));
        if persisted.is_err() {
            jobs.remove(&id);
            drop(queue);
            drop(jobs);
            let _ = std::fs::remove_dir_all(&root);
            return write_json(
                500,
                &json!({"error": "could not persist the local Quarto job admission"}),
            );
        }
    }
    queue.push_back(id.clone());
    drop(queue);
    drop(jobs);
    inner.work.notify_one();

    write_json(202, &json!({"id": id, "status": "queued"}))
}

/// Every manifest entry has a bounded count, safe path, and total size.
#[allow(clippy::result_large_err)]
fn validate_manifest_shape(manifest: &[ManifestEntry]) -> Result<(), Reply> {
    if manifest.len() > MAX_FILES {
        return Err(write_json(
            413,
            &json!({"error": "too many manifest entries"}),
        ));
    }
    if manifest
        .iter()
        .any(|entry| !protocol::safe_relative_path(&entry.path))
    {
        return Err(write_json(400, &json!({"error": "unsafe manifest path"})));
    }
    let total = manifest
        .iter()
        .try_fold(0u64, |total, entry| total.checked_add(entry.size));
    if total.is_none_or(|total| total > MAX_UPLOAD_BYTES as u64) {
        return Err(write_json(
            413,
            &json!({"error": "manifest exceeds upload limits"}),
        ));
    }
    Ok(())
}

#[allow(clippy::result_large_err)] // as `server.rs`'s `read_upload`: the error is a response
fn validate_manifest(
    manifest: &[ManifestEntry],
    uploads: &[(String, Vec<u8>)],
) -> Result<(), Reply> {
    let mut expected: HashMap<&str, &ManifestEntry> = HashMap::new();
    for entry in manifest {
        if !protocol::safe_relative_path(&entry.path) {
            return Err(write_json(
                400,
                &json!({"error": format!("unsafe path: {}", entry.path)}),
            ));
        }
        if expected.insert(entry.path.as_str(), entry).is_some() {
            return Err(write_json(
                400,
                &json!({"error": format!("duplicate path in manifest: {}", entry.path)}),
            ));
        }
    }
    let mut provided: HashSet<&str> = HashSet::new();
    for (path, bytes) in uploads {
        let Some(entry) = expected.get(path.as_str()) else {
            return Err(write_json(
                400,
                &json!({"error": format!("file not listed in the manifest: {path}")}),
            ));
        };
        if !provided.insert(path.as_str()) {
            return Err(write_json(
                400,
                &json!({"error": format!("file uploaded more than once: {path}")}),
            ));
        }
        if bytes.len() as u64 != entry.size {
            return Err(write_json(
                400,
                &json!({"error": format!("size does not match the manifest: {path}")}),
            ));
        }
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        if hex::encode(hasher.finalize()) != entry.sha256 {
            return Err(write_json(
                400,
                &json!({"error": format!("digest does not match the manifest: {path}")}),
            ));
        }
    }
    if provided.len() != expected.len() {
        return Err(write_json(
            400,
            &json!({"error": "the manifest lists a file that was never uploaded"}),
        ));
    }
    Ok(())
}

pub(super) async fn handle_job_status(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
) -> Reply {
    let project = match authenticate(inner, headers, origin) {
        Ok(project) => project,
        Err(response) => return response,
    };
    let jobs = inner.jobs.lock().await;
    let Some(entry) = jobs.get(id) else {
        return write_json(404, &json!({"error": "not found"}));
    };
    if entry.project != project {
        // Answered identically to "not found": a token for another project
        // learns nothing about whether this id even exists.
        return write_json(404, &json!({"error": "not found"}));
    }
    if entry.superseded {
        return write_json(409, &json!({"error": "superseded by a newer job"}));
    }
    write_json(
        200,
        &serde_json::to_value(&entry.status).unwrap_or(Value::Null),
    )
}

pub(super) async fn handle_job_file(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
    name: &str,
) -> Reply {
    let project = match authenticate(inner, headers, origin) {
        Ok(project) => project,
        Err(response) => return response,
    };
    let jobs = inner.jobs.lock().await;
    let Some(entry) = jobs.get(id) else {
        return plain(404, "not found");
    };
    if entry.project != project {
        return plain(404, "not found");
    }
    if entry.superseded {
        return write_json(409, &json!({"error": "superseded by a newer job"}));
    }
    let Some(bytes) = entry.files.get(name) else {
        return plain(404, "not found");
    };
    let mut response = Response::new(Body::from(bytes.clone()));
    let mime = match name {
        "pdf" => "application/pdf",
        "html" => "text/html; charset=utf-8",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "log" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    };
    set(&mut response, "content-type", mime);
    set(&mut response, "x-content-type-options", "nosniff");
    set(&mut response, "content-security-policy", "sandbox");
    response
}

pub(super) async fn handle_cancel(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
) -> Reply {
    let project = match authenticate(inner, headers, origin) {
        Ok(project) => project,
        Err(response) => return response,
    };
    let mut jobs = inner.jobs.lock().await;
    let Some(entry) = jobs.get_mut(id) else {
        return write_json(404, &json!({"error": "not found"}));
    };
    if entry.project != project {
        return write_json(404, &json!({"error": "not found"}));
    }
    if entry.superseded {
        return write_json(409, &json!({"error": "superseded by a newer job"}));
    }
    let _ = entry.cancel_tx.send(true);
    if entry.queued {
        // Never handed to the runner, so there is nothing for it to observe
        // cancelling: settle it here and drop it from the queue directly.
        entry.queued = false;
        entry.status.status = "canceled".to_string();
        entry.status.stage = "finished".to_string();
        entry.finished_at = Some(Instant::now());
        drop(jobs);
        let mut queue = inner.queue.lock().await;
        queue.retain(|qid| qid != id);
    }
    write_json(200, &json!({"ok": true}))
}

pub(super) async fn handle_job_delete(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
) -> Reply {
    let project = match authenticate(inner, headers, origin) {
        Ok(project) => project,
        Err(response) => return response,
    };
    let mut jobs = inner.jobs.lock().await;
    let Some(entry) = jobs.get(id) else {
        return write_json(404, &json!({"error": "not found"}));
    };
    if entry.project != project {
        return write_json(404, &json!({"error": "not found"}));
    }
    let _ = entry.cancel_tx.send(true);
    let root = entry.workspace.root.clone();
    jobs.remove(id);
    drop(jobs);
    inner.queue.lock().await.retain(|qid| qid != id);
    let _ = std::fs::remove_dir_all(&root);
    write_json(200, &json!({"ok": true}))
}
