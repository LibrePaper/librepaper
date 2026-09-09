//! The local bridge protocol, version 1: the shapes that cross between the
//! browser and the local LibrePaper service, and between the service and the
//! native runner. See `docs/specs/latex-interfaces.md`, section 5.
//!
//! Shared by `service.rs` (the loopback HTTP surface), `native.rs` (the
//! runner) and `discovery.rs` (the tools). Field names are the wire names.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The protocol versions this binary speaks.
pub const PROTOCOL_VERSIONS: &[u32] = &[1];

/// The default loopback port. Configurable with `librepaper local start
/// --port`.
pub const DEFAULT_PORT: u16 = 8763;

/// The base path every route hangs under.
pub const BASE_PATH: &str = "/librepaper/local/v1";

/// Largest JSON body accepted.
pub const MAX_JSON_BYTES: usize = 64 * 1024;
/// Largest multipart upload accepted, all parts together.
pub const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;
/// Most files one job may carry.
pub const MAX_FILES: usize = 2000;
/// Largest PDF the runner returns.
pub const MAX_PDF_BYTES: usize = 64 * 1024 * 1024;
/// Largest log kept per stage.
pub const MAX_LOG_BYTES: usize = 4 * 1024 * 1024;
/// Default whole-job deadline.
pub const DEFAULT_DEADLINE_SECONDS: u64 = 300;
/// Default bounded pass count.
pub const DEFAULT_MAX_PASSES: u32 = 8;

/// `GET health`: enough to identify the service and negotiate, and nothing
/// else. No tool paths, no projects, no jobs.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Health {
    pub service: String,
    pub protocol: Vec<u32>,
    pub version: String,
    pub instance: String,
}

/// `POST connect` request: a deliberate pairing, code in hand.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ConnectRequest {
    pub origin: String,
    pub project: String,
    pub code: String,
}

/// `POST connect` answer: a bearer token scoped to (origin, project).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ConnectResponse {
    pub token: String,
    pub expires: i64,
}

/// One native tool, as the browser is allowed to know it: whether, which
/// version, and a note. Never a path.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Tool {
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default)]
    pub note: String,
}

/// Which engines and helpers the app found.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Tools {
    pub pdflatex: Tool,
    pub xelatex: Tool,
    pub lualatex: Tool,
    pub bibtex: Tool,
    pub bibtex8: Tool,
    pub biber: Tool,
    pub makeindex: Tool,
}

/// Whether native execution can be confined on this machine, and how.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Confinement {
    pub available: bool,
    /// `bwrap`, `sandbox-exec`, or `none`.
    pub kind: String,
    #[serde(default)]
    pub reason: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Distribution {
    pub name: String,
    pub year: String,
}

/// `GET capabilities`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Capabilities {
    pub tools: Tools,
    pub confinement: Confinement,
    pub platform: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distribution: Option<Distribution>,
}

/// One input file the browser says it is sending.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ManifestEntry {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct JobOptions {
    #[serde(default = "default_deadline")]
    pub deadline_seconds: u64,
    #[serde(default = "default_passes")]
    pub max_passes: u32,
}

fn default_deadline() -> u64 {
    DEFAULT_DEADLINE_SECONDS
}
fn default_passes() -> u32 {
    DEFAULT_MAX_PASSES
}

impl Default for JobOptions {
    fn default() -> Self {
        JobOptions {
            deadline_seconds: DEFAULT_DEADLINE_SECONDS,
            max_passes: DEFAULT_MAX_PASSES,
        }
    }
}

/// The `job` part of `POST jobs`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct JobRequest {
    pub protocol: u32,
    /// `biber` or `tex`.
    pub kind: String,
    pub project: String,
    pub origin: String,
    pub snapshot: String,
    pub generation: u64,
    /// `pdflatex`, `xelatex` or `lualatex`; tex jobs only. Respected, never
    /// substituted.
    #[serde(default)]
    pub engine: String,
    /// Project-relative main file; tex jobs only.
    #[serde(default)]
    pub main: String,
    /// The job name whose `.bcf` Biber reads; biber jobs only.
    #[serde(default)]
    pub stem: String,
    pub manifest: Vec<ManifestEntry>,
    #[serde(default)]
    pub options: JobOptions,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct OutputEntry {
    pub size: u64,
    pub sha256: String,
}

/// One diagnostic, in the shape `web/src/lib/latex/log.js` emits, with the
/// workspace path already normalised back to the project-relative one. The
/// wire table in section 5 shows only `severity`/`message`/`file`/`line`;
/// `hints`, `column`, `end_line` and `end_column` are carried too, additively,
/// so a native diagnostic is the same shape `engine/src/diagnostic.rs` and
/// `log.js` already agree on, and a consumer that only reads the four wire
/// fields is unaffected.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Diagnostic {
    pub severity: String,
    pub message: String,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub line: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hints: Vec<String>,
    #[serde(default)]
    pub column: u32,
    #[serde(default)]
    pub end_line: u32,
    #[serde(default)]
    pub end_column: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ToolVersions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bibtex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biber: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub makeindex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distribution: Option<String>,
}

/// Who and what produced a result. `backend` is always `local` here.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Provenance {
    pub backend: String,
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub tools: ToolVersions,
    /// `bwrap`, `sandbox-exec` or `none`.
    #[serde(default)]
    pub confinement: String,
}

/// `GET jobs/<id>`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct JobStatus {
    pub id: String,
    pub kind: String,
    /// `queued`, `running`, `done`, `failed`, `canceled`.
    pub status: String,
    /// `staging`, `tex`, `bibtex`, `biber`, `makeindex`, `finished`.
    #[serde(default)]
    pub stage: String,
    #[serde(default)]
    pub passes: u32,
    #[serde(default)]
    pub exit: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub snapshot: String,
    pub generation: u64,
    #[serde(default)]
    pub log_tail: String,
    #[serde(default)]
    pub outputs: BTreeMap<String, OutputEntry>,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
    #[serde(default)]
    pub provenance: Provenance,
    /// Biber found a control-file version it does not speak.
    #[serde(default)]
    pub incompatible: bool,
}

/// What the runner (`native.rs`) hands back to the service for one job: the
/// status object and the output bytes, kept apart so the bytes are served
/// raw and never base64.
#[derive(Clone, Debug, Default)]
pub struct JobOutcome {
    pub status: JobStatus,
    /// `pdf`, `synctex`, `bbl`, `blg`, `log` as raw bytes.
    pub files: BTreeMap<String, Vec<u8>>,
}

/// Where a staged job lives on disk: the private workspace `native.rs`
/// compiles in. Created by the service from verified uploads, removed by it
/// when the job is deleted or expires.
#[derive(Clone, Debug)]
pub struct Workspace {
    /// The workspace root; inputs are under `project/`, outputs under `out/`.
    pub root: std::path::PathBuf,
}

impl Workspace {
    pub fn project(&self) -> std::path::PathBuf {
        self.root.join("project")
    }
    pub fn out(&self) -> std::path::PathBuf {
        self.root.join("out")
    }
}

/// Whether a project-relative path may be staged at all: relative, no `..`,
/// no `.`, no backslash, no control characters, no empty component, and no
/// leading slash. Shared by the service (upload) and the runner (outputs).
pub fn safe_relative_path(path: &str) -> bool {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') || path.contains('\0') {
        return false;
    }
    if path.chars().any(|c| c.is_control()) {
        return false;
    }
    path.split('/')
        .all(|part| !part.is_empty() && part != "." && part != "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_path_is_safe_and_an_escaping_one_is_not() {
        assert!(safe_relative_path("main.tex"));
        assert!(safe_relative_path("chapters/01.tex"));
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../b",
            "a/./b",
            "a\\b",
            "a\u{0}b",
            "a//b",
            "x\n",
        ] {
            assert!(!safe_relative_path(bad), "{bad:?} was allowed");
        }
    }

    #[test]
    fn job_options_default_when_absent() {
        let request: JobRequest = serde_json::from_str(
            r#"{"protocol":1,"kind":"biber","project":"p","origin":"https://x","snapshot":"s","generation":1,"stem":"main","manifest":[]}"#,
        )
        .expect("parses");
        assert_eq!(request.options.deadline_seconds, DEFAULT_DEADLINE_SECONDS);
        assert_eq!(request.options.max_passes, DEFAULT_MAX_PASSES);
    }
}
