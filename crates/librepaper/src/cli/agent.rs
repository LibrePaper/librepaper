//! `librepaper mcp`: the one entry point the sidebar assistant's own chosen
//! ACP agent spawns on this computer, as its own child process, following the
//! command the companion's session handed it at `session/new`. It speaks
//! nothing but MCP frames over stdio, and it is not listed, because only that
//! agent runs it -- never a human, and never LibrePaper's own companion,
//! which drives ACP directly in-process instead of spawning a runner (see
//! `crate::assistant::registry`).

use clap::Args;

use crate::automation::peer::AutomationPeer;
use crate::cli::Deployment;

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
        crate::local::connections::ConnectionStore::new(&crate::local::paths::state_home()?)
            .resolve(&args.connection)?;
    if let (Some(conversation), Some(chat_token)) = (&resolved.conversation, &resolved.chat_token) {
        std::env::set_var("LIBREPAPER_CONVERSATION", conversation);
        std::env::set_var("LIBREPAPER_CHAT_TOKEN", chat_token);
    }
    let link = crate::automation::peer::DocumentLink::parse(
        &resolved.link,
        server.as_deref().unwrap_or(""),
    )?;
    let peer = AutomationPeer::open(link, server.as_deref(), token.as_deref()).await?;
    crate::automation::mcp::stdio(&peer).await
}
