//! `librepaper mcp`: the one entry point the sidebar assistant's own chosen
//! ACP agent spawns on this computer, as its own child process, following the
//! command the companion's session handed it at `session/new`. It speaks
//! nothing but MCP frames over stdio, and it is not listed, because only that
//! agent runs it -- never a human, and never LibrePaper's own companion,
//! which drives ACP directly in-process instead of spawning a runner (see
//! `librepaper_companion::assistant::registry`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use clap::Args;

use crate::cli::Deployment;
use librepaper_companion::automation::peer::AutomationPeer;
use librepaper_companion::local::connections::GRANT_FILE_VARIABLE;

/// Serve the document MCP tools over stdio for the sidebar assistant's own
/// agent.
///
/// `--connection NAME` names a runner record this computer holds, so the
/// agent's command line carries a name rather than a document key. The
/// companion's session (`assistant::acp::Agent::start`) is the only thing
/// that ever constructs this command; there is no other accepted form.
#[derive(Args, Clone, Debug)]
pub(crate) struct McpArgs {
    /// A runner record on this computer.
    #[arg(long, value_name = "NAME")]
    connection: String,
    #[command(flatten)]
    deployment: Deployment,
}

pub(crate) async fn run_mcp(args: McpArgs) -> Result<(), String> {
    let Deployment { server, token } = args.deployment;
    let resolved =
        librepaper_companion::local::connections::ConnectionStore::new(
            &librepaper_companion::local::paths::state_home()?,
        )
        .resolve(&args.connection)?;
    if let (Some(conversation), Some(chat_token)) = (&resolved.conversation, &resolved.chat_token) {
        std::env::set_var("LIBREPAPER_CONVERSATION", conversation);
        std::env::set_var("LIBREPAPER_CHAT_TOKEN", chat_token);
    }
    let link = librepaper_companion::automation::peer::DocumentLink::parse(
        &resolved.link,
        server.as_deref().unwrap_or(""),
    )?;
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
