//! `librepaper mcp`: the stdio entry point that serves the document MCP tools.
//! Two kinds of caller run it, never a person at a prompt. The sidebar
//! assistant's own chosen ACP agent spawns it on this computer, as its own
//! child process, following the command the local service's session handed it
//! at `session/new`. An agent on another machine runs it with `--document` and
//! an agent token from the environment. It speaks nothing but MCP frames over
//! stdio, and it is not listed in help. LibrePaper's own local service is
//! neither caller: it drives ACP directly in-process instead of spawning a
//! runner (see `librepaper_companion::assistant::registry`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use clap::Args;

use crate::cli::Deployment;
use librepaper_companion::automation::peer::{AutomationPeer, DocumentLink};
use librepaper_companion::local::connections::{AGENT_TOKEN_VARIABLE, GRANT_FILE_VARIABLE};

/// The prefix every agent token starts with. A token that lacks it is refused
/// before any request is sent.
const AGENT_TOKEN_PREFIX: &str = "lpt_";

/// Serve the document MCP tools over stdio.
///
/// The sidebar assistant's agent names a runner record this computer holds, so
/// its command carries a name rather than a document key. It comes from
/// `--connection NAME` or, when that is absent, from
/// `LIBREPAPER_MCP_CONNECTION`, which the local service's session
/// (`assistant::acp::Agent::start`) sets for the agent and its bridge.
///
/// A remote agent passes `--document URL` instead. The URL carries no key:
/// the agent token comes from `LIBREPAPER_AGENT_TOKEN`, never from a flag.
#[derive(Args, Clone, Debug)]
pub(crate) struct McpArgs {
    /// A runner record on this computer.
    #[arg(long, value_name = "NAME", conflicts_with = "document")]
    connection: Option<String>,
    /// A document URL, such as `https://host/docs/<slug>`, served with the
    /// agent token in `LIBREPAPER_AGENT_TOKEN`.
    #[arg(long, value_name = "URL")]
    document: Option<String>,
    #[command(flatten)]
    deployment: Deployment,
}

pub(crate) async fn run_mcp(args: McpArgs) -> Result<(), String> {
    let Deployment { server, token } = args.deployment;
    if let Some(document) = args.document {
        if token.is_some() {
            return Err(
                "--token is not used with --document; the agent token comes from LIBREPAPER_AGENT_TOKEN"
                    .to_string(),
            );
        }
        let agent_token = agent_token()?;
        let link = DocumentLink::parse(&document, "")?;
        let peer = AutomationPeer::open_agent_token(link, agent_token).await?;
        return librepaper_companion::automation::mcp::stdio(&peer).await;
    }
    use librepaper_companion::local::connections::{
        ConnectionStore, CONNECTION_VARIABLE, STATE_HOME_VARIABLE,
    };
    let connection = args
        .connection
        .or_else(|| std::env::var(CONNECTION_VARIABLE).ok())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            format!(
                "librepaper mcp needs a connection or a document: pass --connection NAME, \
                 or --document URL with {AGENT_TOKEN_VARIABLE} set, or set {CONNECTION_VARIABLE}"
            )
        })?;
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

/// The agent token from `LIBREPAPER_AGENT_TOKEN`. An unset or empty variable
/// and a value without the `lpt_` prefix are both refused.
fn agent_token() -> Result<String, String> {
    let token = std::env::var(AGENT_TOKEN_VARIABLE).unwrap_or_default();
    if token.is_empty() {
        return Err(format!(
            "librepaper mcp --document needs an agent token in {AGENT_TOKEN_VARIABLE}; \
             create one in the document's Sharing panel"
        ));
    }
    if !token.starts_with(AGENT_TOKEN_PREFIX) {
        return Err(format!(
            "{AGENT_TOKEN_VARIABLE} does not hold an agent token; agent tokens start with {AGENT_TOKEN_PREFIX}"
        ));
    }
    Ok(token)
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
