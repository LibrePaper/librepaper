//! Reusable, headless automation peer for LibrePaper documents.
//!
//! The peer deliberately keeps the share key in memory and out of all
//! serialised output.  A link is a complete credential: a cached bearer may
//! identify the caller for attribution, but never widens the role carried by
//! the link.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use crate::http::{detail_of, KEY_HEADER};

/// Extract a share key from either a literal key or a pasted document URL.
pub fn link_key(flag: &str) -> String {
    let flag = flag.trim();
    let Some((_, fragment)) = flag.split_once('#') else {
        return if flag.contains("://") {
            String::new()
        } else {
            flag.to_string()
        };
    };
    fragment
        .split('&')
        .find_map(|part| part.strip_prefix("k="))
        .and_then(|key| {
            url::form_urlencoded::parse(format!("k={key}").as_bytes())
                .next()
                .map(|(_, value)| value.into_owned())
        })
        .unwrap_or_default()
}

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

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
    pub(crate) fn credential_url(&self) -> String {
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

/// The headers every authenticated document request carries: the bearer or
/// share key (whichever this peer holds) plus the two markers that identify
/// an automation caller. Used by every request-building site -- `open`,
/// `chat_socket_request`, and the document tools endpoint -- so the set of
/// headers a credential turns into is written once.
fn auth_headers(token: &str, key: &str) -> Vec<(&'static str, String)> {
    let mut headers = Vec::new();
    if !token.is_empty() {
        headers.push(("authorization", format!("Bearer {token}")));
    }
    if !key.is_empty() {
        headers.push((KEY_HEADER, key.to_string()));
    }
    headers.push(("x-librepaper-automation", "1".to_string()));
    headers.push(("x-librepaper-client", "1".to_string()));
    headers
}

#[derive(Clone)]
pub struct AutomationPeer {
    link: DocumentLink,
    token: String,
    client: reqwest::Client,
}

impl std::fmt::Debug for AutomationPeer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AutomationPeer")
            .field("link", &self.link)
            .field("token", &"[redacted]")
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
        let token = crate::local::credentials::agent_token(
            &crate::local::paths::state_home()?,
            link.server(),
            configured_server,
            token,
        );
        let client = new_client()?;
        let mut request = client.get(format!("{}/api/documents/{}", link.server(), link.slug()));
        for (name, value) in auth_headers(&token, &link.key) {
            request = request.header(name, value);
        }
        let response = request
            .send()
            .await
            // An agent that cannot reach the document must say so and stop.
            // The bare transport error reads like a retryable hiccup, and the
            // observed failure is an agent quietly answering from a local file
            // instead, which is a false report even when the text matches.
            .map_err(|err| {
                format!(
                    "the LibrePaper document at {} could not be reached ({err}). \
                     Report this and stop: no file on disk is this document.",
                    link.server()
                )
            })?;
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
            client,
        })
    }

    pub fn link(&self) -> &DocumentLink {
        &self.link
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
        for (name, value) in auth_headers(&self.token, &self.link.key) {
            request.headers_mut().insert(
                name,
                value.parse().map_err(|_| "invalid credential header")?,
            );
        }
        Ok(request)
    }

    /// One document tools request, with authentication, provenance, and the
    /// runner's execution epoch applied. Every call to the document tools
    /// endpoint goes through this, so the header set is written once. The
    /// epoch is read from the runner's session file synchronously, before
    /// the request is sent, so a replacement runner's later write cannot be
    /// observed mid-request.
    async fn document_request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(u16, Value), String> {
        let endpoint = format!(
            "{}/api/documents/{}{path}",
            self.link.server(),
            self.link.slug()
        );
        let mut request = self.client.request(method, endpoint);
        for (name, value) in auth_headers(&self.token, &self.link.key) {
            request = request.header(name, value);
        }
        let session = std::env::var_os("LIBREPAPER_RUNNER_SESSION")
            .map(|path| crate::assistant::journal::read_session(Path::new(&path)));
        let epoch = session.as_ref().and_then(|session| session.epoch.clone());
        let conversation = std::env::var("LIBREPAPER_CONVERSATION").unwrap_or_default();
        let (conversation, runner, _) =
            provenance_headers(Some(conversation.as_str()), epoch.as_deref());
        if let Some(conversation) = conversation {
            request = request.header("x-librepaper-conversation", conversation);
        }
        if let Some(runner) = runner {
            request = request.header("x-librepaper-runner-conversation", runner);
        }
        if let Ok(chat_token) = std::env::var("LIBREPAPER_CHAT_TOKEN") {
            if !chat_token.is_empty() {
                request = request.header("x-librepaper-chat-token", chat_token);
            }
        }
        if let Some(epoch) = &epoch {
            request = request.header("x-librepaper-execution-epoch", epoch);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .map_err(|err| format!("document request failed: {err}"))?;
        let status = response.status().as_u16();
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err("document response exceeds the 64 KiB control-message limit".into());
        }
        let mut bytes = Vec::new();
        let mut response = response;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|err| format!("could not read document response: {err}"))?
        {
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err("document response exceeds the 64 KiB control-message limit".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        Ok((status, value))
    }

    /// `GET .../tools`: the served tool schemas and the operating
    /// instructions, fetched once at bridge startup.
    pub(crate) async fn tools(&self) -> Result<(u16, Value), String> {
        self.document_request(reqwest::Method::GET, "/tools", None)
            .await
    }

    /// `POST .../tools/{name}` with `args` as the JSON body.
    pub(crate) async fn call_tool(&self, name: &str, args: &Value) -> Result<(u16, Value), String> {
        if name.is_empty()
            || name.len() > 128
            || name
                .bytes()
                .any(|byte| byte == b'\r' || byte == b'\n' || byte == 0)
        {
            return Err("invalid tool name".into());
        }
        self.document_request(reqwest::Method::POST, &format!("/tools/{name}"), Some(args))
            .await
    }
}

fn new_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|err| format!("could not create HTTP client: {err}"))
}

/// Which provenance headers a document request carries.
///
/// The rule this exists to hold: the marker claiming a runner's protected
/// provenance may only be sent together with the lease that proves it. The
/// server refuses "runner conversation, no lease", and the adapter used to
/// send exactly that on its first request, because that request runs while
/// the ACP session is still being created and the runner's transport has not
/// yet minted an epoch. Every request was refused and the agent ended up
/// with a document server that had no tools.
pub(crate) fn provenance_headers<'a>(
    conversation: Option<&'a str>,
    epoch: Option<&'a str>,
) -> (Option<&'a str>, Option<&'a str>, Option<&'a str>) {
    let conversation = conversation.filter(|value| !value.is_empty());
    let epoch = epoch.filter(|value| !value.is_empty());
    let runner = conversation.filter(|_| epoch.is_some());
    (conversation, runner, epoch)
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
    fn automation_peer_debug_redacts_bearer() {
        let peer = AutomationPeer {
            link: DocumentLink::parse("https://docs.example/docs/paper#k=share-key", "")
                .expect("link"),
            token: "bearer-that-must-not-leak".into(),
            client: new_client().expect("client"),
        };
        let debug = format!("{peer:?}");
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("bearer-that-must-not-leak"));
    }
    #[test]
    fn runner_provenance_is_only_claimed_with_a_lease_to_prove_it() {
        // The whole session runs before the transport mints an epoch, so this
        // is the ordinary case at startup, not an edge one. Claiming the
        // marker here is what the document refuses.
        let (conversation, runner, epoch) = provenance_headers(Some("conv"), None);
        assert_eq!(conversation, Some("conv"));
        assert_eq!(runner, None, "no lease, so no claim of runner provenance");
        assert_eq!(epoch, None);

        let (conversation, runner, epoch) = provenance_headers(Some("conv"), Some("lease"));
        assert_eq!(conversation, Some("conv"));
        assert_eq!(runner, Some("conv"));
        assert_eq!(epoch, Some("lease"));

        // An empty value is no value, in either position.
        assert_eq!(provenance_headers(Some(""), Some("lease")).1, None);
        assert_eq!(provenance_headers(Some("conv"), Some("")).1, None);
        assert_eq!(provenance_headers(None, None), (None, None, None));
    }
}
