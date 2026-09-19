//! `librepaper agent`: the two entry points an agent process needs on this
//! computer. Both take a document link and speak nothing but JSON or MCP
//! frames, so they suit a process with no shell parser of its own.

use clap::Subcommand;
use serde_json::json;

use crate::automation::peer::{AutomationPeer, DocumentLink};

#[derive(Subcommand, Clone, Debug)]
pub(crate) enum AgentCommand {
    /// Drive a local ACP agent against a private sidebar conversation. The
    /// local app starts this; it is not listed because nobody types it.
    #[command(hide = true)]
    Connect {
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
        /// Directory for the local thread id and completed task ids.
        #[arg(
            long = "state-directory",
            env = "LIBREPAPER_ASSISTANT_STATE_DIR",
            value_name = "DIRECTORY"
        )]
        state_dir: Option<std::path::PathBuf>,
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
        /// Start a detached runner and wait until it is ready.
        #[arg(long)]
        background: bool,
    },
    /// Serve the document MCP tools over stdio for an MCP host.
    ///
    /// `--connection NAME` is how an agent's configuration file names a
    /// document without holding its key: this computer resolves the name
    /// through the connections the browser registered. A literal link is
    /// still accepted for a one-off, and `-` reads it from
    /// LIBREPAPER_DOCUMENT, which lets a host keep the credential in its
    /// protected environment.
    Mcp {
        #[arg(
            required_unless_present = "connection",
            conflicts_with = "connection",
            default_value = ""
        )]
        link: String,
        /// A connection registered by the browser on this computer.
        #[arg(long, value_name = "NAME")]
        connection: Option<String>,
    },
}

pub(crate) async fn run(
    command: AgentCommand,
    server: Option<String>,
    token: Option<String>,
) -> Result<(), String> {
    if let AgentCommand::Mcp {
        connection: Some(name),
        ..
    } = &command
    {
        let resolved =
            crate::local::connections::ConnectionStore::new(&crate::local::paths::state_home()?)
                .resolve(name)?;
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

    let link_text = match &command {
        AgentCommand::Connect { link, .. } | AgentCommand::Mcp { link, .. } => link,
    };
    let link_text = if link_text == "-" {
        std::env::var("LIBREPAPER_DOCUMENT")
            .map_err(|_| "LIBREPAPER_DOCUMENT is required for the background runner".to_string())?
    } else {
        link_text.to_owned()
    };
    let link = DocumentLink::parse(&link_text, server.as_deref().unwrap_or(""))?;
    let peer = AutomationPeer::open(link, server.as_deref(), token.as_deref()).await?;
    match command {
        AgentCommand::Connect {
            conversation,
            chat_token,
            state_dir,
            agent,
            background,
            ..
        } => {
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
                println!("{}", json!({"started":true,"conversation":conversation}));
            } else {
                crate::assistant::runtime::run(&peer, config).await?;
            }
        }
        AgentCommand::Mcp { .. } => crate::automation::mcp::stdio(&peer).await?,
    }
    Ok(())
}
