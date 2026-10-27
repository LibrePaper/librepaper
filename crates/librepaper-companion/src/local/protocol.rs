//! The local bridge protocol, version 3: the shapes that cross between the
//! browser and the local LibrePaper service, and between the service and the
//! native runner.
//!
//! Shared by `service` (the loopback HTTP surface), `native.rs` (the
//! runner) and `render` (the pipeline). Field names are the wire names.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The protocol versions this binary speaks. Version 2 carried typed
/// builders and Quarto options and is no longer spoken.
pub const PROTOCOL_VERSIONS: &[u32] = &[3];

/// The base path every route hangs under.
pub const BASE_PATH: &str = "/librepaper/local";

/// Largest JSON body accepted.
pub const MAX_JSON_BYTES: usize = 64 * 1024;
/// Largest multipart upload accepted, all parts together.
pub const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;
/// Largest result file the pipeline returns.
pub const MAX_RESULT_BYTES: usize = 64 * 1024 * 1024;
/// Largest log kept per stage.
pub const MAX_LOG_BYTES: usize = 4 * 1024 * 1024;
/// Default whole-job deadline.
pub const DEFAULT_DEADLINE_SECONDS: u64 = 300;

/// `GET health`: enough to identify the service and negotiate, and nothing
/// else. No tool paths, no projects, no jobs.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Health {
    pub service: String,
    pub protocol: Vec<u32>,
    pub version: String,
    pub instance: String,
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

/// One render command as the browser is allowed to know it: the argument
/// vector and result path in use, whether they are the user's own edit, and
/// whether `argv[0]` resolves on this computer. Never a path.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct RendererReport {
    pub command: Vec<String>,
    pub result: String,
    pub edited: bool,
    pub found: bool,
    /// The first line of `argv[0] --version`, or none.
    pub version: Option<String>,
}

/// `GET capabilities`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Capabilities {
    pub platform: String,
    /// Zotero desktop's read-only local API. No Zotero account or API key is
    /// involved; `note` distinguishes disabled, absent, and incompatible.
    #[serde(default)]
    pub zotero: Tool,
    /// Every render command, keyed `<format>.<output>`.
    #[serde(default)]
    pub renderers: BTreeMap<String, RendererReport>,
}

/// One input file the browser says it is sending.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ManifestEntry {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct JobOptions {
    #[serde(default = "default_deadline")]
    pub deadline_seconds: u64,
}

fn default_deadline() -> u64 {
    DEFAULT_DEADLINE_SECONDS
}

impl Default for JobOptions {
    fn default() -> Self {
        JobOptions {
            deadline_seconds: DEFAULT_DEADLINE_SECONDS,
        }
    }
}

/// One admitted render, as the wire carries it and as the queue and the
/// pipeline consume it. The page names a format and an output, never a
/// command: the sidecar picks the tool record from that pair.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct JobRequest {
    pub protocol: u32,
    pub project: String,
    pub origin: String,
    pub snapshot: String,
    pub generation: u64,
    pub format: String,
    pub output: String,
    pub entrypoint: String,
    /// `hosted`, or the id of a granted project folder.
    pub binding_id: String,
    pub manifest: Vec<ManifestEntry>,
    #[serde(default)]
    pub options: JobOptions,
}

impl JobRequest {
    /// Decodes a `job` part. A protocol 2 request is refused with a message
    /// asking the person to update the page, and any other version is
    /// refused as unsupported, both before the rest of the body is read.
    pub fn decode(raw: serde_json::Value) -> Result<Self, String> {
        match raw.get("protocol").and_then(serde_json::Value::as_u64) {
            Some(3) => {}
            Some(2) => {
                return Err(
                    "this page is older than the LibrePaper app: reload the page to update it"
                        .into(),
                )
            }
            _ => return Err("unsupported protocol version".into()),
        }
        let request: JobRequest = serde_json::from_value(raw)
            .map_err(|error| format!("bad protocol 3 job description: {error}"))?;
        request.validate_shape()?;
        Ok(request)
    }

    /// Shape and safety of the fields that name a tool record and an input.
    /// Whether the caller holds the binding, and what the files are, are
    /// checked later by the service.
    pub fn validate_shape(&self) -> Result<(), String> {
        if self.protocol != 3 {
            return Err("unsupported protocol version".into());
        }
        crate::local::render::Key::parse_parts(&self.format, &self.output)?;
        if !safe_relative_path(&self.entrypoint) {
            return Err("entrypoint must be a safe project-relative path".into());
        }
        if self.binding_id.is_empty() || self.binding_id.len() > 256 {
            return Err("binding_id is required and must be at most 256 bytes".into());
        }
        if self.options.deadline_seconds == 0 || self.options.deadline_seconds > 3600 {
            return Err("build limits are outside the supported range".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct OutputEntry {
    pub size: u64,
    pub sha256: String,
}

impl OutputEntry {
    /// The descriptor for bytes in hand. Size and digest have to describe the
    /// same bytes, so they are paired here rather than at each producer. A
    /// descriptor copied from already-verified metadata is built directly.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        OutputEntry {
            size: bytes.len() as u64,
            sha256: librepaper_base::digest::Digest::of(bytes).to_hex(),
        }
    }
}

/// Who and what produced a result. `backend` is always `local` here.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Provenance {
    pub backend: String,
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub output: String,
}

/// `GET jobs/<id>`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct JobStatus {
    pub id: String,
    /// `queued`, `running`, `done`, `failed`, `canceled`.
    pub status: String,
    /// `staging`, `build`, `finished`.
    #[serde(default)]
    pub stage: String,
    #[serde(default)]
    pub exit: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub snapshot: String,
    pub generation: u64,
    #[serde(default)]
    pub log_tail: String,
    /// `artifact`, `synctex` when found, and `log`.
    #[serde(default)]
    pub outputs: BTreeMap<String, OutputEntry>,
    #[serde(default)]
    pub provenance: Provenance,
}

/// What the pipeline hands back to the service for one job: the status
/// object and the output bytes, kept apart so the bytes are served raw and
/// never base64.
#[derive(Clone, Debug, Default)]
pub struct JobOutcome {
    pub status: JobStatus,
    /// `artifact`, `synctex`, `log` as raw bytes.
    pub files: BTreeMap<String, Vec<u8>>,
}

pub use librepaper_document::document::paths::safe_relative_path;

/* ------------------------------------------------- Connecting an agent */

/// Name one conversation's assistant, for status and stop. The protected
/// document link is the authority here -- it carries the document key -- so
/// there is no separate connection name to resolve first.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AssistantQuery {
    pub link: String,
    pub conversation: String,
}

/// Renew the server grant for an already-running assistant. This value is
/// received only from the desktop shell and contains no browser cookie.
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct AssistantRenewal {
    pub link: String,
    pub conversation: String,
    pub agent_token: String,
}

impl std::fmt::Debug for AssistantRenewal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AssistantRenewal")
            .field("link", &"[redacted]")
            .field("conversation", &self.conversation)
            .field("agent_token", &"[redacted]")
            .finish()
    }
}

/// Start, inspect or stop the sidebar assistant for one conversation.
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct AssistantRequest {
    /// The protected document URL. The key may be in the path or the fragment.
    pub link: String,
    /// The private sidebar conversation the runner attaches to.
    pub conversation: String,
    /// The conversation credential. It never leaves loopback and is stored
    /// only in the private connection record.
    pub chat_token: String,
    /// Five-minute document-scoped bearer minted by the signed-in browser.
    /// It is never the browser's session cookie.
    #[serde(default)]
    pub agent_token: String,
    /// Which detected agent to drive. It must advertise ACP support.
    pub agent: String,
    /// What the assistant may do in the document, as the browser's Access
    /// choice: "commenter" or "editor". It decides which LibrePaper tool
    /// permission requests the runner approves without asking.
    pub access: String,
}

impl std::fmt::Debug for AssistantRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AssistantRequest")
            .field("link", &"[redacted]")
            .field("conversation", &self.conversation)
            .field("chat_token", &"[redacted]")
            .field("agent_token", &"[redacted]")
            .field("agent", &self.agent)
            .finish()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn companion_agent_credentials_are_redacted_from_debug_output() {
        let request = AssistantRequest {
            link: "https://papers.example/docs/paper#k=link-secret".into(),
            conversation: "conversation".into(),
            chat_token: "chat-secret".into(),
            agent_token: "lpa_agent-secret".into(),
            agent: "agent".into(),
            access: "commenter".into(),
        };
        let renewal = AssistantRenewal {
            link: request.link.clone(),
            conversation: request.conversation.clone(),
            agent_token: request.agent_token.clone(),
        };
        for diagnostic in [format!("{request:?}"), format!("{renewal:?}")] {
            assert!(!diagnostic.contains("link-secret"));
            assert!(!diagnostic.contains("chat-secret"));
            assert!(!diagnostic.contains("agent-secret"));
        }
    }

    fn job_json(protocol: u32) -> serde_json::Value {
        serde_json::json!({
            "protocol": protocol, "project": "p", "origin": "https://x", "snapshot": "s",
            "generation": 1, "format": "latex", "output": "pdf",
            "entrypoint": "chapters/main.tex", "binding_id": "hosted",
            "manifest": [],
        })
    }

    #[test]
    fn job_options_default_when_absent() {
        let request = JobRequest::decode(job_json(3)).expect("parses");
        assert_eq!(request.options.deadline_seconds, DEFAULT_DEADLINE_SECONDS);
    }

    #[test]
    fn protocol_two_is_refused_with_a_request_to_update_the_page() {
        let error = JobRequest::decode(job_json(2)).unwrap_err();
        assert!(error.contains("reload the page"), "{error}");
        assert!(JobRequest::decode(job_json(1)).is_err());
    }

    #[test]
    fn a_job_names_a_tool_record_that_exists() {
        for (format, output) in [("latex", "html"), ("typst", "docx"), ("pandoc", "pdf")] {
            let mut raw = job_json(3);
            raw["format"] = format.into();
            raw["output"] = output.into();
            assert!(JobRequest::decode(raw).is_err(), "{format}.{output}");
        }
        let mut raw = job_json(3);
        raw["format"] = "markdown".into();
        raw["output"] = "docx".into();
        JobRequest::decode(raw).expect("markdown builds a docx");
    }

    #[test]
    fn a_job_cannot_carry_a_command_or_an_unsafe_entrypoint() {
        let mut raw = job_json(3);
        raw["command"] = serde_json::json!(["rm", "-rf", "/"]);
        assert!(JobRequest::decode(raw).is_err());
        let mut raw = job_json(3);
        raw["options"] = serde_json::json!({"deadline_seconds": 10, "args": []});
        assert!(JobRequest::decode(raw).is_err());
        let mut raw = job_json(3);
        raw["entrypoint"] = "../main.tex".into();
        assert!(JobRequest::decode(raw).is_err());
        let mut raw = job_json(3);
        raw["binding_id"] = "".into();
        assert!(JobRequest::decode(raw).is_err());
        let mut raw = job_json(3);
        raw["options"] = serde_json::json!({"deadline_seconds": 7200});
        assert!(JobRequest::decode(raw).is_err());
    }
}
