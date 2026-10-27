//! `librepaper mcp`: the stdio entry point that serves the document MCP tools.
//! Two kinds of caller run it, never a person at a prompt. The sidebar
//! assistant's own chosen ACP agent spawns it on this computer, as its own
//! child process, following the command the local service's session handed it
//! at `session/new`. An agent on another machine runs it with an agent token
//! in `LIBREPAPER_AGENT_TOKEN`, which names both the server and the project.
//! It speaks nothing but MCP frames over stdio, and it is not listed in help.
//! LibrePaper's own local service is neither caller: it drives ACP directly
//! in-process instead of spawning a runner (see
//! `librepaper_companion::assistant::registry`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use clap::Args;

use crate::cli::Deployment;
use librepaper_base::auth::AGENT_TOKEN_PREFIX;
use librepaper_base::ids::AgentToken;
use librepaper_companion::automation::peer::{AutomationPeer, DocumentLink};
use librepaper_companion::local::connections::{AGENT_TOKEN_VARIABLE, GRANT_FILE_VARIABLE};

/// Serve the document MCP tools over stdio.
///
/// The sidebar assistant's agent names a runner record this computer holds, so
/// its command carries a name rather than a document key. It comes from
/// `--connection NAME` or, when that is absent, from
/// `LIBREPAPER_MCP_CONNECTION`, which the local service's session
/// (`assistant::acp::Agent::start`) sets for the agent and its bridge.
///
/// A remote agent sets neither and passes an agent token in
/// `LIBREPAPER_AGENT_TOKEN`. The token carries its server and its project, so
/// no flag is needed.
#[derive(Args, Clone, Debug)]
pub(crate) struct McpArgs {
    /// A runner record on this computer.
    #[arg(long, value_name = "NAME")]
    connection: Option<String>,
    #[command(flatten)]
    deployment: Deployment,
}

pub(crate) async fn run_mcp(args: McpArgs) -> Result<(), String> {
    use librepaper_companion::local::connections::{
        ConnectionStore, CONNECTION_VARIABLE, STATE_HOME_VARIABLE,
    };
    let Deployment { server, token } = args.deployment;
    let connection = args
        .connection
        .or_else(|| std::env::var(CONNECTION_VARIABLE).ok())
        .filter(|name| !name.is_empty());
    let Some(connection) = connection else {
        let agent_token = std::env::var(AGENT_TOKEN_VARIABLE).unwrap_or_default();
        if agent_token.is_empty() {
            return Err(format!(
                "librepaper mcp needs a connection or an agent token: pass --connection NAME, \
                 or set {CONNECTION_VARIABLE}, or set {AGENT_TOKEN_VARIABLE} to a token from the project's Sharing panel"
            ));
        }
        // The account token may come from LIBREPAPER_TOKEN exported for other
        // commands, so it is not refused, only named as unused. Stdout is the
        // MCP channel; the note goes to stderr.
        if token.is_some() {
            eprintln!(
                "librepaper mcp: the account token (--token or LIBREPAPER_TOKEN) is not used with an agent token; the agent token comes from {AGENT_TOKEN_VARIABLE}"
            );
        }
        let (origin, slug) = parse_agent_token(&agent_token)?;
        let link = DocumentLink::parse(&format!("{origin}/docs/{slug}"), "")?;
        let peer = AutomationPeer::open_agent_token(link, agent_token).await?;
        return librepaper_companion::automation::mcp::stdio(&peer).await;
    };
    let state_home = match std::env::var_os(STATE_HOME_VARIABLE).filter(|path| !path.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => librepaper_companion::local::paths::state_home()?,
    };
    let resolved = ConnectionStore::new(&state_home).resolve(&connection)?;
    if let (Some(conversation), Some(chat_token)) = (&resolved.conversation, &resolved.chat_token) {
        std::env::set_var("LIBREPAPER_CONVERSATION", conversation);
        std::env::set_var("LIBREPAPER_CHAT_TOKEN", chat_token);
    }
    let link = DocumentLink::parse(&resolved.link, server.as_deref().unwrap_or(""))?;
    let peer = match std::env::var_os(GRANT_FILE_VARIABLE) {
        // Started by a sidebar runner: its scoped grant is the only
        // credential this bridge may use. Falling back to this computer's
        // login would either fail with a sign-in error or, worse, quietly
        // give the agent more authority than the browser delegated.
        Some(path) => {
            let path = PathBuf::from(path);
            let peer = AutomationPeer::open_scoped(link, read_grant(&path)?).await?;
            follow_grant(path, peer.token_source());
            peer
        }
        None => AutomationPeer::open(link, server.as_deref(), token.as_deref()).await?,
    };
    librepaper_companion::automation::mcp::stdio(&peer).await
}

/// Split an agent token into the app origin and the project slug. The key is
/// not returned.
fn parse_agent_token(token: &str) -> Result<(String, String), String> {
    let parsed = token.parse::<AgentToken>().map_err(|_| {
        format!(
            "{AGENT_TOKEN_VARIABLE} does not hold an agent token; agent tokens look like {AGENT_TOKEN_PREFIX}<server>.<project>.<key>, so create a new one in the project's Sharing panel"
        )
    })?;
    Ok((parsed.origin, parsed.slug.to_string()))
}

/// Read the runner's scoped grant. The runner publishes the file by rename,
/// so a read sees one whole grant or another, never part of one; anything
/// that is not a scoped grant is refused rather than sent.
fn read_grant(path: &Path) -> Result<String, String> {
    let token = std::fs::read_to_string(path).map_err(|error| {
        format!("could not read the assistant's document authorization: {error}")
    })?;
    if !token.starts_with(librepaper_base::auth::AGENT_GRANT_PREFIX) {
        return Err("the assistant's document authorization is not a scoped grant; start the assistant again from the browser".into());
    }
    Ok(token)
}

/// Keep this bridge on the runner's current grant. A renewal reaches the
/// runner, which republishes the file; this picks the new value up within a
/// second. A file that is briefly missing or invalid keeps the last good
/// grant: the runner removes it only when it stops, and its agent, and so
/// this process, go with it.
fn follow_grant(path: PathBuf, source: Arc<RwLock<String>>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tick.tick().await;
            let Ok(token) = read_grant(&path) else {
                continue;
            };
            let mut current = source.write().unwrap_or_else(|error| error.into_inner());
            if *current != token {
                *current = token;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn token(origin: &str, slug: &str, key: &str) -> String {
        let origin = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(origin);
        format!("lpt_{origin}.{slug}.{key}")
    }

    #[test]
    fn an_agent_token_names_its_server_and_project() {
        let (origin, slug) =
            parse_agent_token(&token("http://localhost:8081", "paper", "abcd")).unwrap();
        assert_eq!(origin, "http://localhost:8081");
        assert_eq!(slug, "paper");
    }

    #[test]
    fn a_bad_agent_token_is_refused() {
        // Not base64url.
        assert!(parse_agent_token("lpt_!!!.paper.abcd").is_err());
        // Decodes to a non-http origin.
        assert!(parse_agent_token(&token("ftp://example.test", "paper", "abcd")).is_err());
        // Decodes to something with a path.
        assert!(parse_agent_token(&token("https://example.test/x", "paper", "abcd")).is_err());
        // The old two-part shape.
        assert!(parse_agent_token("lpt_paper.abcd").is_err());
        // No prefix, and empty parts.
        assert!(parse_agent_token(&token("http://a.test", "paper", "abcd")[4..]).is_err());
        assert!(parse_agent_token(&token("http://a.test", "", "abcd")).is_err());
        assert!(parse_agent_token(&token("http://a.test", "paper", "")).is_err());
    }

    #[test]
    fn only_a_scoped_grant_file_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runner-x.grant");
        assert!(read_grant(&path).is_err());

        std::fs::write(&path, "").unwrap();
        assert!(read_grant(&path).is_err());

        // A device login token is exactly what this path must never send.
        std::fs::write(&path, "lp_device-login-token").unwrap();
        assert!(read_grant(&path).is_err());

        let scoped = format!(
            "{}v1.payload.signature",
            librepaper_base::auth::AGENT_GRANT_PREFIX
        );
        std::fs::write(&path, &scoped).unwrap();
        assert_eq!(read_grant(&path).unwrap(), scoped);
    }
}
