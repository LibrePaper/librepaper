//! The execution-engine boundary for the local service.
//!
//! The service owns admission, authentication, queueing, cancellation and
//! persistence.  This module owns the small amount of engine selection needed
//! to hand a request to the implementation that knows its source inventory,
//! invocation policy, output capture and bundle exclusions. Quarto jobs go to
//! the Quarto module; Typst and Calepin jobs go to the native builder runner.
//! LaTeX is built in the browser, so no TeX or Biber job reaches here.

use tokio::sync::{mpsc, watch};

use super::protocol::{Capabilities, JobOutcome, JobRequest, JobStatus, Workspace};
use super::quarto;

/// Dispatches an admitted job while preserving the shared runner contract.
/// The caller still supplies the same workspace, cancellation receiver and
/// progress channel used by the service's queue worker.
pub async fn run(
    request: JobRequest,
    workspace: Workspace,
    cancel: watch::Receiver<bool>,
    progress: mpsc::UnboundedSender<JobStatus>,
    bindings: &super::quarto::BindingStore,
) -> JobOutcome {
    let job_id = workspace
        .root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    // The binding a bound snapshot named is checked again here rather than
    // only at admission: the queue delay between the two is long enough for
    // the folder to be moved, replaced or revoked, and what runs is what is
    // on disk now.
    if let super::protocol::WorkspaceRequest::Snapshot {
        binding_id: Some(binding_id),
    } = &request.workspace
    {
        // The hosted workspace takes the entrypoint the job names, as a live preview does.
        if let Err(error) = bindings.validate_scoped_entrypoint(
            binding_id,
            &request.origin,
            &request.project,
            &request.entrypoint,
            true,
        ) {
            return unsupported(&request, &job_id, &error);
        }
    }
    if request.builder == "quarto" {
        return quarto::run_job_with_bindings(request, workspace, cancel, progress, bindings).await;
    }
    let tools = crate::local::discovery::tool_paths().await;
    crate::local::builders::runner::run(request, workspace, tools, cancel, progress).await
}

/// Capabilities remain the existing browser-facing shape.  Keeping discovery
/// behind this function means the service has no engine-specific capability
/// branch and cannot accidentally advertise a future engine.
pub async fn capabilities(refresh: bool) -> Capabilities {
    let zotero_client = super::zotero::Client::local();
    let (mut capabilities, zotero) =
        tokio::join!(super::discovery::discover(refresh), zotero_client.probe());
    capabilities.zotero = match zotero {
        Ok(()) => super::protocol::Tool {
            available: true,
            version: Some("api-v3".into()),
            note: "Zotero desktop local API is available".into(),
        },
        Err(error) => super::protocol::Tool {
            available: false,
            version: None,
            note: error.to_string(),
        },
    };
    capabilities
}

fn unsupported(request: &JobRequest, id: &str, error: &str) -> JobOutcome {
    JobOutcome {
        status: JobStatus {
            id: id.to_string(),
            kind: request.kind.clone(),
            status: "failed".into(),
            stage: "finished".into(),
            error: Some(error.into()),
            snapshot: request.snapshot.clone(),
            generation: request.generation,
            ..Default::default()
        },
        files: Default::default(),
    }
}
