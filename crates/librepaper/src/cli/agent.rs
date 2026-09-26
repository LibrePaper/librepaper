//! `librepaper mcp` and `librepaper run-agent`: the two entry points the
//! sidebar assistant spawns on this computer. Both take a document link and
//! speak nothing but MCP or ACP frames, and neither is listed, because only
//! the local app runs them.

use clap::Args;

use crate::automation::peer::{AutomationPeer, DocumentLink};
use crate::cli::Deployment;

/// Serve the document MCP tools over stdio for the sidebar runner's agent.
///
/// `--connection NAME` names a runner record this computer holds, so the
/// agent's command line carries a name rather than a document key. A literal
/// link is still accepted, and `-` reads it from LIBREPAPER_DOCUMENT.
#[derive(Args, Clone, Debug)]
pub(crate) struct McpArgs {
    #[arg(
        required_unless_present = "connection",
        conflicts_with = "connection",
        default_value = ""
    )]
    link: String,
    /// A runner record on this computer.
    #[arg(long, value_name = "NAME")]
    connection: Option<String>,
    #[command(flatten)]
    deployment: Deployment,
}

/// Drive a local ACP agent against a private sidebar conversation.
#[derive(Args, Clone, Debug)]
pub(crate) struct RunAgentArgs {
    link: String,
    /// Private conversation identifier from the LibrePaper sidebar.
    conversation: String,
    /// Conversation credential.
    #[arg(
        long = "chat-token",
        env = "LIBREPAPER_CHAT_TOKEN",
        hide_env_values = true
    )]
    chat_token: Option<String>,
    /// Command line for an Agent Client Protocol implementation.
    /// `allow_hyphen_values` because an adapter's arguments are flags of
    /// its own: `gemini --experimental-acp`, `npx -y pi-acp`. Without it
    /// clap claims the adapter's flag as one of ours and exits 2.
    #[arg(
        long = "agent",
        env = "LIBREPAPER_AGENT",
        value_delimiter = ' ',
        allow_hyphen_values = true,
        required = true,
        value_name = "COMMAND"
    )]
    agent: Vec<String>,
    #[command(flatten)]
    deployment: Deployment,
}

pub(crate) async fn run_mcp(args: McpArgs) -> Result<(), String> {
    let Deployment { server, token } = args.deployment;
    if let Some(name) = args.connection {
        let resolved =
            crate::local::connections::ConnectionStore::new(&crate::local::paths::state_home()?)
                .resolve(&name)?;
        if let (Some(conversation), Some(chat_token)) =
            (&resolved.conversation, &resolved.chat_token)
        {
            std::env::set_var("LIBREPAPER_CONVERSATION", conversation);
            std::env::set_var("LIBREPAPER_CHAT_TOKEN", chat_token);
        }
        let link = DocumentLink::parse(&resolved.link, server.as_deref().unwrap_or(""))?;
        let peer = AutomationPeer::open(link, server.as_deref(), token.as_deref()).await?;
        return crate::automation::mcp::stdio(&peer).await;
    }
    let peer = open_peer(args.link, server, token).await?;
    crate::automation::mcp::stdio(&peer).await
}

pub(crate) async fn run_agent(args: RunAgentArgs) -> Result<(), String> {
    let Deployment { server, token } = args.deployment;
    let peer = open_peer(args.link, server, token).await?;
    let config = crate::assistant::runtime::config(args.conversation, args.chat_token, args.agent)?;
    std::env::remove_var("LIBREPAPER_DOCUMENT");
    std::env::remove_var("LIBREPAPER_CHAT_TOKEN");
    crate::assistant::runtime::run(&peer, config).await
}

/// `-` reads the link from LIBREPAPER_DOCUMENT, which keeps the document key
/// off the child's command line.
async fn open_peer(
    link: String,
    server: Option<String>,
    token: Option<String>,
) -> Result<AutomationPeer, String> {
    let link = if link == "-" {
        std::env::var("LIBREPAPER_DOCUMENT")
            .map_err(|_| "LIBREPAPER_DOCUMENT is required when the link is -".to_string())?
    } else {
        link
    };
    let link = DocumentLink::parse(&link, server.as_deref().unwrap_or(""))?;
    AutomationPeer::open(link, server.as_deref(), token.as_deref()).await
}
