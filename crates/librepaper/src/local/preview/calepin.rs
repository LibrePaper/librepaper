//! The Calepin preview adapter: command construction, option validation and
//! log-line recognition for a managed `calepin watch` watcher over a Typst
//! document.
//!
//! `calepin watch <input.typ> [OUTPUT] --format html|pdf [-- <typst watch
//! args>]` delegates to `typst watch`. For HTML it writes a self-contained
//! page (images embedded as data URIs) and, unless told `--no-serve`,
//! starts its own web server we never want; `--no-serve` is forwarded
//! through Calepin's own `--` argument passthrough. It logs `compiling ...`
//! then `compiled successfully in Nms` or `compiled with warnings in Nms`
//! (ANSI-coloured -- stripped before this module ever sees a line).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::process::Command;

use super::{ArtifactKind, Plan, Session, Watch};
use crate::local::protocol::{PreviewRequest, Tool};
use crate::local::quarto::{verify_bound_manifest, BindingStore};

pub(crate) struct CalepinWatch {
    entrypoint: String,
    kind: ArtifactKind,
}

impl Watch for CalepinWatch {
    fn rendering_started(&self, line: &str) -> bool {
        line.contains("compiling")
    }

    fn render_finished(&self, line: &str) -> bool {
        line.contains("compiled successfully in") || line.contains("compiled with warnings in")
    }

    fn output_path(&self, root: &Path) -> Option<PathBuf> {
        Some(root.join(sibling_output(&self.entrypoint, self.kind)))
    }

    fn kind(&self) -> ArtifactKind {
        self.kind
    }
}

/// Where Calepin is told to write its output: beside the entrypoint, same
/// stem, extension matching the requested format.
fn sibling_output(entrypoint: &str, kind: ArtifactKind) -> PathBuf {
    let extension = match kind {
        ArtifactKind::Html => "html",
        ArtifactKind::Pdf => "pdf",
    };
    Path::new(entrypoint).with_extension(extension)
}

/// Validates a `POST previews` request naming the Calepin engine and builds
/// the `calepin watch` command to run. `existing` is the live session map,
/// consulted for the same one-binding/one-root exclusions the Quarto
/// adapter enforces.
pub(crate) fn plan(
    existing: &HashMap<String, Session>,
    request: &PreviewRequest,
    bindings: &BindingStore,
) -> Result<Plan, String> {
    let crate::local::protocol::PreviewInputs::Calepin(options) = &request.inputs else {
        return Err("calepin preview planned for another engine".into());
    };
    options.validate()?;
    let super::BoundScope {
        binding,
        entrypoint,
    } = super::bind_scope(
        existing,
        request,
        bindings,
        &options.binding_id,
        &options.main,
    )?;
    verify_bound_manifest(&binding.root, &request.manifest)?;
    super::refuse_escaping_entrypoint(&binding.root, &entrypoint)?;
    let kind = match options.format.as_str() {
        "pdf" => ArtifactKind::Pdf,
        _ => ArtifactKind::Html,
    };
    let output = sibling_output(&entrypoint, kind);
    let mut command = Command::new(find_calepin().ok_or("Calepin is not installed")?);
    command
        .current_dir(&binding.root)
        .arg("watch")
        .arg(&entrypoint)
        .arg(&output)
        .args(["--format", &options.format]);
    if kind == ArtifactKind::Html {
        // typst's own HTML watch mode starts a web server unless told
        // otherwise; `--serve`/`--port` are website-only and never used
        // here. This app renders the file itself.
        command.arg("--").arg("--no-serve");
    }

    Ok(Plan {
        command,
        adapter: Box::new(CalepinWatch {
            entrypoint: entrypoint.clone(),
            kind,
        }),
        entrypoint,
        root: binding.root,
        binding_id: options.binding_id.clone(),
    })
}

/// Discover Calepin without exposing its path. A configured path is
/// accepted for tests and app packaging; normal operation searches PATH
/// explicitly -- the same shape as `local::quarto::find_quarto`.
pub(crate) fn find_calepin() -> Option<PathBuf> {
    crate::local::tools::find("LIBREPAPER_CALEPIN_PATH", "calepin")
}

/// `GET capabilities`'s `calepin` entry: whether the tool was found and its
/// version string, through the one bounded probe every tool lookup in this
/// service shares ([`crate::local::tools`]).
pub(crate) async fn discover() -> Tool {
    let Some(path) = find_calepin() else {
        return Tool {
            available: false,
            note: "calepin not found: install Calepin or add it to PATH".into(),
            ..Default::default()
        };
    };
    let version = version_of(&path).await;
    Tool {
        available: version.is_some(),
        version,
        note: String::new(),
    }
}

/// The preview adapter probes the tool it is about to run, in the
/// environment the watcher will use.
async fn version_of(path: &Path) -> Option<String> {
    crate::local::tools::version_line(
        path,
        crate::local::tools::Probe::inherited(Duration::from_secs(5)),
    )
    .await
}
