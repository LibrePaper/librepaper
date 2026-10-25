//! The job surface: admitting a render, running it, and answering for it.
//!
//! `POST jobs` is the only door a render comes through, and everything from
//! the multipart upload to the workspace it is staged into to the status a
//! caller polls is here. Admission in particular: what a request has to
//! satisfy before anything is copied onto this computer, which is the whole
//! of what stands between a page in a browser and the author's files.
//!
//! Split out of `service` for size rather than for structure -- these
//! handlers reach `Inner` through the same `&Inner`/`pub(super)` pattern the
//! rest of the module uses, and nothing about the routing changed.

use super::*;

use crate::local::render::workspace::{self, MirrorError};

pub(super) async fn handle_jobs_post(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let origin = origin.unwrap_or_default().to_string();

    // The description carries the manifest of the whole project, and a folder
    // project may list as many files as its save does, so it takes the bound
    // of a folder save's manifest and not the fixed one for other JSON.
    let upload = match read_multipart_upload(
        request,
        "job",
        "bad job part",
        "job description is too large",
        "missing the job part",
        projects::MAX_PROJECT_FILES,
        projects::MAX_FOLDER_MANIFEST_BYTES,
    )
    .await
    {
        Ok(upload) => upload,
        Err(response) => return response,
    };
    // Never hold the admission gate across an upload.
    let uploads = upload.files;
    let raw_job: Value = match serde_json::from_str(&upload.metadata) {
        Ok(job) => job,
        Err(_) => return write_json(400, &json!({"error": "bad job description"})),
    };
    // The version is refused before the rest of the body is decoded, and a
    // protocol 2 page is told to update.
    let job = match JobRequest::decode(raw_job) {
        Ok(job) => job,
        Err(error) => return write_json(400, &json!({"error": error})),
    };
    let Some(project) = store::valid_project(&job.project) else {
        return write_json(400, &json!({"error": "job needs a valid project"}));
    };
    if store::normalize_origin(&job.origin) != store::normalize_origin(&origin) {
        return write_json(
            403,
            &json!({"error": "job scope does not match the request origin"}),
        );
    }
    if let Err(response) = validate_manifest_shape(&job.manifest, projects::MAX_PROJECT_FILES) {
        return response;
    }
    // Whether *this* caller holds the binding the job names. The workspace
    // it points at is written to before the command runs. The entry file may
    // be one the folder save has not written yet, so only its path is checked
    // here; the render checks the file itself once the uploads are mirrored.
    if inner
        .bindings
        .validate_scoped_entrypoint(&job.binding_id, &origin, &project, &job.entrypoint)
        .is_err()
    {
        return write_json(
            403,
            &json!({"error": "workspace binding is not granted for this origin and project"}),
        );
    }
    if let Err(response) = validate_manifest(&job.manifest, &uploads, true) {
        return response;
    }
    if !job
        .manifest
        .iter()
        .any(|entry| entry.path == job.entrypoint)
    {
        return write_json(
            400,
            &json!({"error": "the input inventory must include the entry file"}),
        );
    }

    let id = librepaper_base::util::new_id();
    let root = inner.jobs_root.join(&id);
    let project_dir = root.join("project");
    if std::fs::create_dir_all(&project_dir).is_err() {
        return write_json(500, &json!({"error": "could not create a workspace"}));
    }
    if let Err(error) = stage_uploads(&project_dir, &uploads) {
        let _ = std::fs::remove_dir_all(&root);
        return write_json(400, &json!({"error": error}));
    }

    let (cancel_tx, cancel_rx) = watch::channel(false);
    let status = JobStatus {
        id: id.clone(),
        status: "queued".to_string(),
        stage: "staging".to_string(),
        snapshot: job.snapshot.clone(),
        generation: job.generation,
        provenance: protocol::Provenance {
            backend: "local".into(),
            format: job.format.clone(),
            output: job.output.clone(),
        },
        ..Default::default()
    };
    let generation = job.generation;
    let binding_id = job.binding_id.clone();

    let _admission = inner.admission.lock().await;
    let mut jobs = inner.jobs.lock().await;
    supersede_older_generations(&mut jobs, &origin, &project, generation);
    // A job already running in this workspace is stopped, so the new one
    // starts as soon as the queue reaches it and two commands never share a
    // folder.
    cancel_running_in_workspace(&jobs, &origin, &project, &binding_id);
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
            root,
            cancel_tx,
            cancel_rx,
            queued: true,
            superseded: false,
            finished_at: None,
        },
    );
    queue.push_back(id.clone());
    drop(queue);
    drop(jobs);
    inner.work.notify_one();

    write_json(202, &json!({"id": id, "status": "queued"}))
}

/// `PUT folder`: mirror the whole project into the project folder the binding
/// names. The manifest lists every file; the uploads carry the bytes of some.
pub(super) async fn handle_folder_put(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let origin = origin.unwrap_or_default().to_string();
    let Some(project) = project_query(request.uri()) else {
        return write_json(400, &json!({"error": "folder needs a valid project"}));
    };
    let Some(binding_id) = query_value(request.uri(), "binding") else {
        return write_json(400, &json!({"error": "folder needs a binding"}));
    };
    if binding_id == crate::local::bindings::HOSTED_BINDING {
        return write_json(400, &json!({"error": "only a project folder is mirrored"}));
    }
    // Only a project binding is stored, so any other id does not resolve.
    let granted = inner
        .bindings
        .validate_scoped_root(&binding_id, &origin, &project);
    let Ok(binding) = granted else {
        return write_json(
            403,
            &json!({"error": "workspace binding is not granted for this origin and project"}),
        );
    };

    // A folder project may hold more files than a render job carries, and the
    // manifest of a save names every one of them.
    let max_files = projects::MAX_PROJECT_FILES;
    let upload = match read_multipart_upload(
        request,
        "manifest",
        "bad manifest part",
        "the manifest is too large",
        "missing the manifest part",
        max_files,
        projects::MAX_FOLDER_MANIFEST_BYTES,
    )
    .await
    {
        Ok(upload) => upload,
        Err(response) => return response,
    };
    #[derive(serde::Deserialize)]
    struct FolderManifest {
        manifest: Vec<ManifestEntry>,
    }
    let Ok(FolderManifest { manifest }) = serde_json::from_str(&upload.metadata) else {
        return write_json(400, &json!({"error": "bad manifest"}));
    };
    if let Err(response) = validate_manifest_shape(&manifest, max_files) {
        return response;
    }
    if let Err(response) = validate_manifest(&manifest, &upload.files, false) {
        return response;
    }

    let staging = match std::fs::create_dir_all(&inner.jobs_root)
        .and_then(|()| tempfile::tempdir_in(&inner.jobs_root))
    {
        Ok(staging) => staging,
        Err(_) => return write_json(500, &json!({"error": "could not create a workspace"})),
    };
    if let Err(error) = stage_uploads(staging.path(), &upload.files) {
        return write_json(400, &json!({"error": error}));
    }
    let record = inner.bindings.written_record(&binding.id);
    let root = binding.root.clone();
    let staged = staging.path().to_path_buf();
    let outcome = tokio::task::spawn_blocking(move || {
        workspace::mirror_granted_folder(&staged, &root, &manifest, &record)
    })
    .await;
    drop(staging);
    match outcome {
        Ok(Ok(report)) => write_json(
            200,
            &json!({
                "kept": report.kept.iter()
                    .map(|kept| json!({"path": kept.path, "copy": kept.copy}))
                    .collect::<Vec<_>>(),
                "trashed": report.trashed,
                "renamed": report.renamed.iter()
                    .map(|renamed| json!({"from": renamed.from, "to": renamed.to}))
                    .collect::<Vec<_>>(),
                "kept_unreadable": report.kept_unreadable,
            }),
        ),
        Ok(Err(MirrorError::Missing(missing))) => write_json(409, &json!({"missing": missing})),
        Ok(Err(MirrorError::Failed(error))) => write_json(500, &json!({"error": error})),
        Err(_) => write_json(500, &json!({"error": "the folder mirror stopped"})),
    }
}

/// Every manifest entry has a bounded count, safe path, and total size.
#[allow(clippy::result_large_err)]
fn validate_manifest_shape(manifest: &[ManifestEntry], max_files: usize) -> Result<(), Reply> {
    if manifest.len() > max_files {
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
    require_all: bool,
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
    if require_all && provided.len() != expected.len() {
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
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let jobs = inner.jobs.lock().await;
    let Some(entry) = jobs.get(id) else {
        return write_json(404, &json!({"error": "not found"}));
    };
    if !owned_by(entry, origin) {
        // Answered identically to "not found": a token for another origin
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
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let jobs = inner.jobs.lock().await;
    let Some(entry) = jobs.get(id) else {
        return plain(404, "not found");
    };
    if !owned_by(entry, origin) {
        return plain(404, "not found");
    }
    if entry.superseded {
        return write_json(409, &json!({"error": "superseded by a newer job"}));
    }
    let Some(bytes) = entry.files.get(name) else {
        return plain(404, "not found");
    };
    let mut response = Response::new(Body::from(bytes.clone()));
    let mime = match (name, entry.request.output.as_str()) {
        ("artifact", "pdf") => "application/pdf",
        ("artifact", "html") => "text/html; charset=utf-8",
        ("artifact", "docx") => {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        }
        ("log", _) => "text/plain; charset=utf-8",
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
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let mut jobs = inner.jobs.lock().await;
    let Some(entry) = jobs.get_mut(id) else {
        return write_json(404, &json!({"error": "not found"}));
    };
    if !owned_by(entry, origin) {
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
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let mut jobs = inner.jobs.lock().await;
    let Some(entry) = jobs.get(id) else {
        return write_json(404, &json!({"error": "not found"}));
    };
    if !owned_by(entry, origin) {
        return write_json(404, &json!({"error": "not found"}));
    }
    let _ = entry.cancel_tx.send(true);
    let root = entry.root.clone();
    jobs.remove(id);
    drop(jobs);
    inner.queue.lock().await.retain(|qid| qid != id);
    let _ = std::fs::remove_dir_all(&root);
    write_json(200, &json!({"ok": true}))
}
