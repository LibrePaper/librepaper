//! Document and agent connection: resolving documents from connections and
//! spawning the MCP server that agents use.

use crate::automation::peer::{AutomationPeer, DocumentLink};

pub(crate) async fn run_mcp(
    link: String,
    connection: Option<String>,
    server: Option<String>,
    token: Option<String>,
) -> Result<(), String> {
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

pub(crate) async fn run_connect(
    link: String,
    conversation: String,
    chat_token: Option<String>,
    state_dir: Option<std::path::PathBuf>,
    agent: Vec<String>,
    background: bool,
    server: Option<String>,
    token: Option<String>,
) -> Result<(), String> {
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
        println!("{}", serde_json::json!({"started":true,"conversation":conversation}));
    } else {
        crate::assistant::runtime::run(&peer, config).await?;
    }
    Ok(())
}
