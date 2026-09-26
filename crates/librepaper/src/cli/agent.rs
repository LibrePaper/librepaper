//! The mcp and run-agent commands: spawning MCP servers and agents that talk
//! to the browser through agent connections.

use clap::Args;

use crate::automation::peer::{AutomationPeer, DocumentLink};
use crate::cli::Deployment;

/// MCP server for an agent to call, with document and credential access
#[derive(Args, Clone, Debug)]
pub(crate) struct McpArgs {
    #[arg(
        required_unless_present = "connection",
        conflicts_with = "connection",
        default_value = ""
    )]
    pub(crate) link: String,
    /// A connection registered by the browser on this computer.
    #[arg(long, value_name = "NAME")]
    pub(crate) connection: Option<String>,
    #[command(flatten)]
    pub(crate) deployment: Deployment,
}

/// Drive a local ACP agent against a private sidebar conversation.
#[derive(Args, Clone, Debug)]
pub(crate) struct RunAgentArgs {
    pub(crate) link: String,
    /// Private conversation identifier from the LibrePaper sidebar.
    pub(crate) conversation: String,
    /// Conversation credential.
    #[arg(
        long = "chat-token",
        env = "LIBREPAPER_CHAT_TOKEN",
        hide_env_values = true
    )]
    pub(crate) chat_token: Option<String>,
    /// Directory for the local thread id and completed task ids.
    #[arg(
        long = "state-directory",
        env = "LIBREPAPER_ASSISTANT_STATE_DIR",
        value_name = "DIRECTORY"
    )]
    pub(crate) state_dir: Option<std::path::PathBuf>,
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
    pub(crate) agent: Vec<String>,
    /// Start a detached runner and wait until it is ready.
    #[arg(long)]
    pub(crate) background: bool,
    #[command(flatten)]
    pub(crate) deployment: Deployment,
}

pub(crate) async fn run_mcp(args: McpArgs) -> Result<(), String> {
    let link = args.link;
    let connection = args.connection;
    let server = args.deployment.server;
    let token = args.deployment.token;

    if let Some(name) = connection {
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

    let link_text = if link == "-" {
        std::env::var("LIBREPAPER_DOCUMENT")
            .map_err(|_| "LIBREPAPER_DOCUMENT is required when link is -".to_string())?
    } else {
        link
    };
    let link = DocumentLink::parse(&link_text, server.as_deref().unwrap_or(""))?;
    let peer = AutomationPeer::open(link, server.as_deref(), token.as_deref()).await?;
    crate::automation::mcp::stdio(&peer).await
}

pub(crate) async fn run_connect(args: RunAgentArgs) -> Result<(), String> {
    let link = args.link;
    let conversation = args.conversation;
    let chat_token = args.chat_token;
    let state_dir = args.state_dir;
    let agent = args.agent;
    let background = args.background;
    let server = args.deployment.server;
    let token = args.deployment.token;

    let link_text = if link == "-" {
        std::env::var("LIBREPAPER_DOCUMENT")
            .map_err(|_| "LIBREPAPER_DOCUMENT is required for the background runner".to_string())?
    } else {
        link
    };
    let link = DocumentLink::parse(&link_text, server.as_deref().unwrap_or(""))?;
    let peer = AutomationPeer::open(link, server.as_deref(), token.as_deref()).await?;

    let config = crate::assistant::runtime::config(
        conversation.clone(),
        chat_token,
        state_dir.clone(),
        agent,
    )?;
    std::env::remove_var("LIBREPAPER_DOCUMENT");
    std::env::remove_var("LIBREPAPER_CHAT_TOKEN");
    if background {
        crate::assistant::lifecycle::start_background(
            peer.link(),
            &conversation,
            &config.token,
            config.state_dir.as_deref(),
            &config.agent,
        )?;
        println!(
            "{}",
            serde_json::json!({"started":true,"conversation":conversation})
        );
    } else {
        crate::assistant::runtime::run(&peer, config).await?;
    }
    Ok(())
}
