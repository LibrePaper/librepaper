//! Author-only managed watchers. Their URLs are never portable bundle artifacts.
use super::{
    protocol::{JobRequest, QuartoExecutionMode, QuartoRenderPolicy, QuartoRenderScope},
    quarto::{find_quarto, terminate_process_group, verify_bound_manifest, BindingStore},
};
use std::{
    collections::HashMap,
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::process::{Child, Command};

pub(crate) struct Preview {
    pub origin: String,
    pub project: String,
    pub binding: String,
    pub root: std::path::PathBuf,
    pub url: String,
    pub started: Instant,
    child: Child,
}

#[derive(Default)]
pub(crate) struct Previews(pub HashMap<String, Preview>);

impl Previews {
    pub async fn start(
        &mut self,
        request: &JobRequest,
        bindings: &BindingStore,
    ) -> Result<(String, String), String> {
        let options = request.quarto.as_ref().ok_or("missing Quarto options")?;
        options.validate()?;
        if options.execution_mode != QuartoExecutionMode::WorkingTree {
            return Err("managed preview supports linked working-tree mode only".into());
        }
        if options.render_scope != QuartoRenderScope::Document {
            return Err("managed preview supports document scope only".into());
        }
        if options.policy != QuartoRenderPolicy::ProjectDefaults {
            return Err("managed preview uses project-default render policy".into());
        }
        if self.0.values().any(|p| p.binding == options.binding_id) {
            return Err("Stop the existing preview before starting another watcher.".into());
        }
        if self.0.len() >= 8 {
            return Err("Too many active previews".into());
        }
        let binding = bindings
            .resolve_scoped(&options.binding_id, &request.origin, &request.project)
            .map_err(|_| "Preview binding is not authorized")?;
        if self.0.values().any(|p| p.root == binding.root) {
            return Err("Another managed preview already watches this project directory".into());
        }
        if options.main != binding.entrypoint
            || !request
                .manifest
                .iter()
                .any(|f| f.path == binding.entrypoint)
        {
            return Err("Preview inventory must contain the bound entrypoint".into());
        }
        if std::fs::canonicalize(&binding.root).map_err(|e| e.to_string())? != binding.root {
            return Err("Bound root changed".into());
        }
        let mut inventory = request.manifest.clone();
        super::quarto::add_declared_inputs(&binding.root, &options.data_inputs, &mut inventory)?;
        verify_bound_manifest(&binding.root, &inventory)?;
        if let Some(expected) = options.shared_tree_sha256.as_deref() {
            let actual = super::quarto::inventory_manifest_impl(&binding.root, &request.manifest)?
                .tree_sha256;
            if actual != expected {
                return Err(
                    "preview source inventory is stale; synchronize before previewing".into(),
                );
            }
        }
        let main = std::fs::canonicalize(binding.root.join(&binding.entrypoint))
            .map_err(|e| e.to_string())?;
        if !main.starts_with(&binding.root) {
            return Err("Preview entrypoint escapes bound root".into());
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        drop(listener);
        let mut command = Command::new(find_quarto().ok_or("Quarto is not installed")?);
        command
            .current_dir(&binding.root)
            .arg("preview")
            .arg(&binding.entrypoint)
            .args([
                "--no-browser",
                "--host",
                "127.0.0.1",
                "--port",
                &port.to_string(),
                "--timeout",
                "300",
                "--to",
                &options.format,
            ]);
        if let Some(profile) = &options.profile {
            command.args(["--profile", profile]);
        }
        for (key, value) in &options.parameters {
            command.arg("-P").arg(format!(
                "{key}:{}",
                serde_json::to_string(value).map_err(|e| e.to_string())?
            ));
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|e| format!("Start Quarto preview: {e}"))?;
        // Do not expose a dead preview URL when Quarto rejects its arguments
        // or exits during initial startup. Longer initial renders remain pending.
        tokio::time::sleep(Duration::from_millis(250)).await;
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            terminate_process_group(&mut child).await;
            return Err(format!("Quarto preview exited during startup ({status}); check the project with quarto preview locally"));
        }
        let id = crate::util::new_id();
        let url = format!("http://127.0.0.1:{port}/");
        self.0.insert(
            id.clone(),
            Preview {
                origin: request.origin.clone(),
                project: request.project.clone(),
                binding: options.binding_id.clone(),
                root: binding.root.clone(),
                url: url.clone(),
                started: Instant::now(),
                child,
            },
        );
        Ok((id, url))
    }

    pub async fn stop(&mut self, id: &str) {
        if let Some(mut preview) = self.0.remove(id) {
            terminate_process_group(&mut preview.child).await;
        }
    }

    pub async fn stop_scope(&mut self, origin: &str, project: &str) {
        let ids: Vec<_> = self
            .0
            .iter()
            .filter(|(_, preview)| preview.origin == origin && preview.project == project)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.stop(&id).await;
        }
    }

    pub async fn reap(&mut self, active_scopes: &[(String, String)], bindings: &BindingStore) {
        let expired: Vec<_> = self
            .0
            .iter_mut()
            .filter_map(|(id, p)| {
                let pairing_expired = !active_scopes
                    .iter()
                    .any(|(origin, project)| origin == &p.origin && project == &p.project);
                (pairing_expired
                    || bindings
                        .get_scoped(&p.binding, &p.origin, &p.project)
                        .is_none()
                    || p.started.elapsed() > Duration::from_secs(3600)
                    || !matches!(p.child.try_wait(), Ok(None)))
                .then(|| id.clone())
            })
            .collect();
        for id in expired {
            self.stop(&id).await;
        }
    }
}
