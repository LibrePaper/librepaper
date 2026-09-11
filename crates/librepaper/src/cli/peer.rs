//! Reusable, headless automation peer for LibrePaper documents.
//!
//! The peer deliberately keeps the share key in memory and out of all
//! serialised output.  A link is a complete credential: a cached bearer may
//! identify the caller for attribution, but never widens the role carried by
//! the link.

use std::time::Duration;

use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use crate::cli::{link_key, stored_agent_token_for};
use crate::http::{detail_of, KEY_HEADER};

use super::runner;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The authority conveyed by a document link.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Capabilities {
    pub role: String,
    pub can_read: bool,
    pub can_comment: bool,
    pub can_edit: bool,
    pub can_resolve: bool,
    pub can_delete: bool,
    pub can_checkpoint: bool,
}

impl Capabilities {
    fn from_document(document: &Value) -> Self {
        let role = document
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let can_edit = document
            .get("can_edit")
            .and_then(Value::as_bool)
            .unwrap_or(matches!(role.as_str(), "owner" | "editor"));
        let can_comment = document
            .get("can_comment")
            .and_then(Value::as_bool)
            .unwrap_or(matches!(role.as_str(), "owner" | "editor" | "commenter"));
        Self {
            can_read: document
                .get("can_read")
                .and_then(Value::as_bool)
                .unwrap_or(!role.is_empty()),
            can_comment,
            can_edit,
            can_resolve: document
                .get("can_resolve")
                .and_then(Value::as_bool)
                .unwrap_or(can_comment),
            can_delete: document
                .get("can_delete")
                .and_then(Value::as_bool)
                .unwrap_or(can_comment),
            can_checkpoint: document
                .get("can_checkpoint")
                .and_then(Value::as_bool)
                .unwrap_or(can_edit),
            role,
        }
    }
}

/// A parsed document link. `key` is intentionally private and never appears
/// in Debug, JSON, or error text.
#[derive(Clone, PartialEq, Eq)]
pub struct DocumentLink {
    server: String,
    slug: String,
    path: String,
    key: String,
}

impl std::fmt::Debug for DocumentLink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DocumentLink")
            .field("server", &self.server)
            .field("slug", &self.slug)
            .field("path", &self.path)
            .field("key", &"[redacted]")
            .finish()
    }
}

impl DocumentLink {
    /// Parse a pasted `/docs/<slug>#k=...` or `/raw/<slug>#k=...` link.
    /// Bare slugs are accepted when `default_server` is supplied.
    pub fn parse(input: &str, default_server: &str) -> Result<Self, String> {
        let input = input.trim();
        if input.is_empty() {
            return Err("a document link is required".into());
        }
        let (server, slug, path, key) = if let Ok(url) = url::Url::parse(input) {
            if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
                return Err("document links must use http or https".into());
            }
            if !url.username().is_empty() || url.password().is_some() {
                return Err("document links may not contain userinfo".into());
            }
            let server = url.origin().ascii_serialization();
            let segments: Vec<_> = url
                .path_segments()
                .map(|parts| parts.collect())
                .unwrap_or_default();
            let slug = match segments.as_slice() {
                [prefix, slug] if matches!(*prefix, "docs" | "raw" | "document") => *slug,
                [slug] => *slug,
                _ => return Err("the document link must identify one document".into()),
            };
            if slug.is_empty() {
                return Err("the document link has no document id".into());
            }
            let path = url
                .query_pairs()
                .find_map(|(name, value)| (name == "file").then_some(value.into_owned()))
                .unwrap_or_default();
            validate_file_path(&path)?;
            let key = key_from_url(&url);
            (server, slug.to_string(), path, key)
        } else {
            let server = default_server.trim().trim_end_matches('/');
            if server.is_empty() {
                return Err("a full document link or --server is required".into());
            }
            (
                server.to_string(),
                input.trim_matches('/').to_string(),
                String::new(),
                String::new(),
            )
        };
        if slug.is_empty() || slug.contains('/') || slug == "." || slug == ".." {
            return Err("the document link has an invalid document id".into());
        }
        Ok(Self {
            server,
            slug,
            path,
            key,
        })
    }

    pub fn server(&self) -> &str {
        &self.server
    }
    pub fn slug(&self) -> &str {
        &self.slug
    }
    pub fn has_key(&self) -> bool {
        !self.key.is_empty()
    }
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Return the credential-bearing document URL for the local assistant
    /// process. This value is passed only through its environment, never
    /// included in chat output or runner state.
    pub(super) fn credential_url(&self) -> String {
        let Ok(mut url) = url::Url::parse(&self.server) else {
            return self.server.clone();
        };
        {
            let mut segments = match url.path_segments_mut() {
                Ok(segments) => segments,
                Err(_) => return self.server.clone(),
            };
            segments.clear().push("docs").push(&self.slug);
        }
        url.set_query(None);
        if !self.path.is_empty() {
            url.query_pairs_mut().append_pair("file", &self.path);
        }
        if !self.key.is_empty() {
            url.set_fragment(Some(&format!("k={}", self.key)));
        }
        url.to_string()
    }
}

fn key_from_url(url: &url::Url) -> String {
    let fragment = url.fragment().unwrap_or_default();
    let from_fragment = link_key(&format!("#{}", fragment));
    if !from_fragment.is_empty() {
        return from_fragment;
    }
    url.query_pairs()
        .find(|(name, _)| name == "k" || name == "key")
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default()
}

fn validate_file_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.starts_with('/')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        if path.is_empty() {
            return Ok(());
        }
        return Err("the selected file path is invalid".into());
    }
    Ok(())
}

/// One source-and-annotation read from the authenticated v1 snapshot route.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub title: String,
    /// Canonical live source-tree revision used by anchored suggestions.
    #[serde(default)]
    pub sha: String,
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub main: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub source_sha: String,
    /// Canonical source tree metadata used to derive isolated preview revisions.
    #[serde(default)]
    pub tree: Value,
    #[serde(default)]
    pub comments: Vec<Value>,
    #[serde(default)]
    pub files: Value,
    #[serde(default)]
    pub texts: Value,
    #[serde(default)]
    pub capabilities: Capabilities,
}

#[derive(Clone)]
pub struct AutomationPeer {
    link: DocumentLink,
    token: String,
    capabilities: Capabilities,
    client: reqwest::Client,
}

impl std::fmt::Debug for AutomationPeer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AutomationPeer")
            .field("link", &self.link)
            .field("token", &"[redacted]")
            .field("capabilities", &self.capabilities)
            .field("client", &self.client)
            .finish()
    }
}

impl AutomationPeer {
    /// Open a link and report the effective role before any write is
    /// attempted. `configured_server` is the global `--server` this process
    /// was given, if any: an explicit `token` is sent only when the link's
    /// own server matches it, since a pasted link cannot select the
    /// destination of an ambient bearer.
    pub async fn open(
        link: DocumentLink,
        configured_server: Option<&str>,
        token: Option<&str>,
    ) -> Result<Self, String> {
        let token = stored_agent_token_for(link.server(), configured_server, token);
        let client = new_client()?;
        let mut request = client.get(format!("{}/api/documents/{}", link.server(), link.slug()));
        if !token.is_empty() {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        if !link.key.is_empty() {
            request = request.header(KEY_HEADER, &link.key);
        }
        let response = request
            .header("x-librepaper-automation", "1")
            .header("x-librepaper-client", "1")
            .send()
            .await
            .map_err(|err| format!("request failed: {err}"))?;
        let status = response.status().as_u16();
        let raw = response
            .bytes()
            .await
            .map_err(|err| format!("could not read document response: {err}"))?;
        let document: Value = serde_json::from_slice(&raw)
            .unwrap_or_else(|_| json!({"error": String::from_utf8_lossy(&raw)}));
        if status != 200 {
            return Err(format!(
                "could not open document ({}): {}",
                status,
                detail_of(&document)
            ));
        }
        Ok(Self {
            link,
            token,
            capabilities: Capabilities::from_document(&document),
            client,
        })
    }

    pub fn link(&self) -> &DocumentLink {
        &self.link
    }

    pub async fn snapshot(&self) -> Result<Snapshot, String> {
        let endpoint = format!(
            "{}/api/documents/{}/snapshot",
            self.link.server(),
            self.link.slug()
        );
        let response = self.request(reqwest::Method::GET, &endpoint, None).await?;
        if !response.status().is_success() {
            return Err(http_error(response).await);
        }
        let raw = response
            .bytes()
            .await
            .map_err(|err| format!("could not read snapshot response: {err}"))?;
        let value: Value = serde_json::from_slice(&raw)
            .map_err(|err| format!("invalid snapshot response: {err}"))?;
        if value["version"] != 1 || value["protocol"] != "librepaper.snapshot.v1" {
            return Err("server does not support the document snapshot format".into());
        }
        let mut snapshot: Snapshot = serde_json::from_value(value)
            .map_err(|err| format!("invalid snapshot response: {err}"))?;
        snapshot.capabilities = self.capabilities.clone();
        if snapshot.source_sha.is_empty() {
            snapshot.source_sha = source_sha(&snapshot.source);
        }
        Ok(snapshot)
    }
    pub(crate) fn chat_socket_request(
        &self,
        conversation: &str,
    ) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, String> {
        let base = self.link.server().trim_end_matches('/');
        let base = base
            .strip_prefix("https://")
            .map(|rest| format!("wss://{rest}"))
            .or_else(|| {
                base.strip_prefix("http://")
                    .map(|rest| format!("ws://{rest}"))
            })
            .unwrap_or_else(|| format!("ws://{base}"));
        let url = format!(
            "{base}/api/documents/{}/chat/{conversation}/socket",
            self.link.slug()
        );
        let mut request = url.into_client_request().map_err(|err| err.to_string())?;
        if !self.token.is_empty() {
            request.headers_mut().insert(
                "authorization",
                format!("Bearer {}", self.token)
                    .parse()
                    .map_err(|_| "invalid credential header")?,
            );
        }
        if !self.link.key.is_empty() {
            request.headers_mut().insert(
                KEY_HEADER,
                self.link
                    .key
                    .parse()
                    .map_err(|_| "invalid credential header")?,
            );
        }
        request
            .headers_mut()
            .insert("x-librepaper-automation", "1".parse().unwrap());
        request
            .headers_mut()
            .insert("x-librepaper-client", "1".parse().unwrap());
        Ok(request)
    }

    pub(super) async fn request(
        &self,
        method: reqwest::Method,
        endpoint: &str,
        body: Option<Value>,
    ) -> Result<reqwest::Response, String> {
        let mut request = self.client.request(method, endpoint);
        if !self.token.is_empty() {
            request = request.header("authorization", format!("Bearer {}", self.token));
        }
        if !self.link.key.is_empty() {
            request = request.header(KEY_HEADER, &self.link.key);
        }
        request = request.header("x-librepaper-automation", "1");
        request = request.header("x-librepaper-client", "1");
        if let Some(body) = body {
            request = request.json(&body);
        }
        request
            .send()
            .await
            .map_err(|err| format!("request failed: {err}"))
    }

    /// Forward one MCP JSON-RPC message to the document service.
    ///
    /// Authentication is deliberately taken from this already-open peer. MCP
    /// tool arguments therefore contain only document/view identifiers; the
    /// share key and any ambient agent token stay in HTTP headers. The adapter
    /// uses a separate method because MCP's protocol metadata is carried in
    /// headers by the document endpoint rather than in model-visible input.
    pub(super) async fn mcp_request(
        &self,
        method_name: &str,
        tool_name: Option<&str>,
        body: &Value,
    ) -> Result<(u16, String, String), String> {
        self.mcp_request_with_epoch(method_name, tool_name, body, None)
            .await
    }

    pub(super) async fn mcp_request_with_epoch(
        &self,
        method_name: &str,
        tool_name: Option<&str>,
        body: &Value,
        captured_epoch: Option<&str>,
    ) -> Result<(u16, String, String), String> {
        if method_name.is_empty()
            || method_name.len() > 128
            || method_name
                .bytes()
                .any(|byte| byte == b'\r' || byte == b'\n' || byte == 0)
        {
            return Err("invalid MCP method name".into());
        }
        let endpoint = format!(
            "{}/api/documents/{}/mcp",
            self.link.server(),
            self.link.slug()
        );
        let mut request = self.client.post(endpoint);
        if !self.token.is_empty() {
            request = request.header("authorization", format!("Bearer {}", self.token));
        }
        if !self.link.key.is_empty() {
            request = request.header(KEY_HEADER, &self.link.key);
        }
        request = request
            .header("x-librepaper-automation", "1")
            .header("x-librepaper-client", "1")
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", method_name)
            .header("Mcp-Name", tool_name.unwrap_or(method_name))
            .header("Accept", "application/json, text/event-stream")
            .json(body);
        // The background runner gives the app-server a protected copy of the
        // sidebar channel credentials. They are transport headers only: the
        // MCP tool arguments and model context never contain either secret.
        if let Ok(conversation) = std::env::var("LIBREPAPER_CONVERSATION") {
            if !conversation.is_empty() {
                request = request.header("x-librepaper-conversation", conversation.clone());
                // This separate marker distinguishes a sidebar runner's
                // protected provenance from an external browser render
                // request that merely routes through the same conversation.
                if std::env::var_os("LIBREPAPER_RUNNER_EPOCH_FILE").is_some() {
                    request = request.header("x-librepaper-runner-conversation", conversation);
                }
            }
        }
        if let Ok(chat_token) = std::env::var("LIBREPAPER_CHAT_TOKEN") {
            if !chat_token.is_empty() {
                request = request.header("x-librepaper-chat-token", chat_token);
            }
        }
        let epoch = captured_epoch.map(str::to_owned).or_else(|| {
            std::env::var_os("LIBREPAPER_RUNNER_EPOCH_FILE").and_then(|path| {
                super::runner_journal::execution_epoch(std::path::Path::new(&path))
            })
        });
        if let Some(epoch) = epoch {
            request = request.header("x-librepaper-execution-epoch", epoch);
        }
        let response = request
            .send()
            .await
            .map_err(|err| format!("MCP request failed: {err}"))?;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/json")
            .to_string();
        if response
            .content_length()
            .is_some_and(|length| length > 64 * 1024)
        {
            return Err("MCP response exceeds the 64 KiB control-message limit".into());
        }
        let mut bytes = Vec::new();
        let mut response = response;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|err| format!("could not read MCP response: {err}"))?
        {
            if bytes.len().saturating_add(chunk.len()) > 64 * 1024 {
                return Err("MCP response exceeds the 64 KiB control-message limit".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        let body = String::from_utf8(bytes)
            .map_err(|_| "MCP response was not valid UTF-8 JSON".to_string())?;
        Ok((status, content_type, body))
    }
}

fn new_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|err| format!("could not create HTTP client: {err}"))
}

async fn http_error(response: reqwest::Response) -> String {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let value = serde_json::from_str::<Value>(&body).unwrap_or_else(|_| json!({"error": body}));
    format!(
        "document request failed ({}): {}",
        status.as_u16(),
        detail_of(&value)
    )
}

pub fn source_sha(source: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(source.as_bytes());
    hex::encode(digest.finalize())
}

/// Standard Yjs awareness update containing a single provider-neutral user
/// state. The browser consumes this using y-protocols' awareness decoder.
pub fn awareness_update(client_id: u32, clock: u32, name: &str, color: &str) -> Vec<u8> {
    let state =
        serde_json::to_vec(&json!({"user":{"name":name,"color":color}})).unwrap_or_default();
    let mut out = Vec::new();
    put_varuint(&mut out, 1);
    put_varuint(&mut out, client_id);
    put_varuint(&mut out, clock);
    put_varuint(&mut out, state.len() as u32);
    out.extend_from_slice(&state);
    out
}

/// Derive the awareness client id from a Yrs replica id. It stays stable over
/// reconnects while the peer owns its document, so reconnects update one
/// presence entry instead of leaving ghosts behind.
pub fn awareness_client_id(doc: &yrs::Doc) -> u32 {
    let digest = Sha256::digest(doc.client_id().to_string().as_bytes());
    u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]])
}

/// Provider-neutral automation commands. Every command accepts the pasted
/// document link as its first argument and prints one JSON object, making the
/// CLI suitable for an agent process without a shell parser.
#[derive(Subcommand, Clone, Debug)]
pub enum AgentCommand {
    /// Keep a local Codex app-server session connected to a private sidebar conversation.
    Connect {
        link: String,
        /// Private conversation identifier from the LibrePaper sidebar.
        conversation: String,
        /// Conversation credential
        #[arg(
            long = "chat-token",
            env = "LIBREPAPER_CHAT_TOKEN",
            hide_env_values = true
        )]
        chat_token: Option<String>,
        /// Directory for the local thread id and completed task ids.
        #[arg(
            long = "state-directory",
            env = "LIBREPAPER_ASSISTANT_STATE_DIR",
            value_name = "DIRECTORY"
        )]
        state_dir: Option<std::path::PathBuf>,
        /// The Codex executable the runner drives.
        #[arg(
            long,
            env = "LIBREPAPER_CODEX",
            default_value = "codex",
            value_name = "EXECUTABLE"
        )]
        codex: String,
        /// Start a detached runner and wait until it is ready.
        #[arg(long)]
        background: bool,
    },
    /// Serve the document MCP tools over stdio for an MCP host.
    ///
    /// The document link is read from LIBREPAPER_DOCUMENT when it is `-`,
    /// which lets a host keep the credential in its protected environment.
    Mcp { link: String },
    /// Show the local runner state for a conversation.
    Status {
        link: String,
        conversation: String,
        #[arg(
            long = "state-directory",
            env = "LIBREPAPER_ASSISTANT_STATE_DIR",
            value_name = "DIRECTORY"
        )]
        state_dir: Option<std::path::PathBuf>,
    },
    /// Ask the local runner to stop through its nonce-bound control file.
    Stop {
        link: String,
        conversation: String,
        #[arg(
            long = "state-directory",
            env = "LIBREPAPER_ASSISTANT_STATE_DIR",
            value_name = "DIRECTORY"
        )]
        state_dir: Option<std::path::PathBuf>,
    },
    /// Ask the connected runner to have the browser render a candidate tree.
    Preview {
        link: String,
        conversation: String,
        #[arg(long)]
        revision: String,
        #[arg(long, value_name = "FILE")]
        files: std::path::PathBuf,
        #[arg(long, value_name = "ID")]
        task_id: String,
        /// The runner's state directory; the runner hands it to the tools it
        /// spawns through the environment.
        #[arg(
            long = "state-directory",
            env = "LIBREPAPER_ASSISTANT_STATE_DIR",
            value_name = "DIRECTORY"
        )]
        state_dir: Option<std::path::PathBuf>,
    },
}

pub(crate) fn validate_conversation(conversation: &str, token: &str) -> Result<(), String> {
    if conversation.is_empty()
        || conversation.len() > 128
        || !conversation
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err("invalid conversation identifier".into());
    }
    if token.is_empty() {
        return Err("provide --chat-token or LIBREPAPER_CHAT_TOKEN for this conversation".into());
    }
    Ok(())
}

/// clap has already merged `--chat-token` and `$LIBREPAPER_CHAT_TOKEN`; this
/// only rejects the case neither supplied one.
pub(crate) fn chat_token(token: Option<String>) -> Result<String, String> {
    token
        .filter(|token| !token.is_empty())
        .ok_or_else(|| "provide --chat-token or LIBREPAPER_CHAT_TOKEN for this conversation".into())
}

pub async fn run_cli(
    command: AgentCommand,
    server: Option<String>,
    token: Option<String>,
) -> Result<(), String> {
    let link_text = match &command {
        AgentCommand::Connect { link, .. }
        | AgentCommand::Mcp { link }
        | AgentCommand::Status { link, .. }
        | AgentCommand::Stop { link, .. }
        | AgentCommand::Preview { link, .. } => link,
    };
    let link_text = if link_text == "-" {
        std::env::var("LIBREPAPER_DOCUMENT")
            .map_err(|_| "LIBREPAPER_DOCUMENT is required for the background runner".to_string())?
    } else {
        link_text.to_owned()
    };
    let link = DocumentLink::parse(&link_text, server.as_deref().unwrap_or(""))?;
    match &command {
        AgentCommand::Status {
            conversation,
            state_dir,
            ..
        } => {
            let status =
                super::runner_lifecycle::status(&link, conversation, state_dir.as_deref())?;
            println!(
                "{}",
                serde_json::to_string(&status).map_err(|err| err.to_string())?
            );
            return Ok(());
        }
        AgentCommand::Stop {
            conversation,
            state_dir,
            ..
        } => {
            super::runner_lifecycle::stop(&link, conversation, state_dir.as_deref())?;
            println!("{}", json!({"stopped":true,"conversation":conversation}));
            return Ok(());
        }
        _ => {}
    }
    let peer = AutomationPeer::open(link, server.as_deref(), token.as_deref()).await?;
    match command {
        AgentCommand::Connect {
            conversation,
            chat_token,
            state_dir,
            codex,
            background,
            ..
        } => {
            let config =
                runner::config(conversation.clone(), chat_token, state_dir.clone(), codex)?;
            if background {
                super::runner_lifecycle::start_background(
                    peer.link(),
                    &conversation,
                    &config.token,
                    config.state_dir.as_deref(),
                    &config.executable,
                )?;
                println!("{}", json!({"started":true,"conversation":conversation}));
            } else {
                runner::run(&peer, config).await?;
            }
        }
        AgentCommand::Mcp { .. } => {
            super::mcp::stdio(&peer).await?;
        }
        AgentCommand::Status { .. } | AgentCommand::Stop { .. } => {
            unreachable!("local lifecycle handled before opening document")
        }
        AgentCommand::Preview {
            conversation,
            revision,
            files,
            task_id,
            state_dir,
            ..
        } => {
            let raw = std::fs::read_to_string(&files)
                .map_err(|err| format!("could not read {}: {err}", files.display()))?;
            let files: Value = serde_json::from_str(&raw)
                .map_err(|err| format!("invalid preview files JSON: {err}"))?;
            let result = super::runner_preview::preview(
                &peer,
                &conversation,
                state_dir.as_deref(),
                &revision,
                &task_id,
                files,
            )
            .await?;
            println!(
                "{}",
                serde_json::to_string(&result).map_err(|err| err.to_string())?
            );
        }
    }
    Ok(())
}

fn put_varuint(out: &mut Vec<u8>, mut value: u32) {
    while value > 0x7f {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pasted_link_without_exposing_key() {
        let link = DocumentLink::parse("https://docs.example/docs/abc#k=secret", "").unwrap();
        assert_eq!(link.server(), "https://docs.example");
        assert_eq!(link.slug(), "abc");
        assert!(link.has_key());
        assert!(!format!("{link:?}").contains("secret"));
    }

    #[test]
    fn credential_url_reconstructs_document_scope() {
        let link = DocumentLink::parse(
            "https://docs.example/raw/paper?file=chapters%2Fintro.md#k=secret",
            "",
        )
        .expect("link");
        let credential = link.credential_url();
        assert!(credential
            .starts_with("https://docs.example/docs/paper?file=chapters%2Fintro.md#k=secret"));
    }

    #[test]
    fn awareness_contains_sync_identity() {
        let bytes = awareness_update(4, 1, "agent (sync)", "#3366cc");
        assert!(bytes.len() > 10);
        assert!(String::from_utf8_lossy(&bytes).contains("agent (sync)"));
    }

    #[test]
    fn automation_peer_debug_redacts_bearer() {
        let peer = AutomationPeer {
            link: DocumentLink::parse("https://docs.example/docs/paper#k=share-key", "")
                .expect("link"),
            token: "bearer-that-must-not-leak".into(),
            capabilities: Capabilities::default(),
            client: new_client().expect("client"),
        };
        let debug = format!("{peer:?}");
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("bearer-that-must-not-leak"));
    }
}
