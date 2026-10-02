---
title: "Agents"
---

No additional subscription is needed. Use the coding agent you already have and are already signed in to: Claude Code, Codex, Pi or opencode. LibrePaper never sees your model or your credentials, and no inference runs on the LibrePaper server.

The [Agent Client Protocol](https://agentclientprotocol.com) (ACP) is how the local app starts your agent and talks to it: it sends your requests and relays the agent's permission requests to the sidebar. The [Model Context Protocol](https://modelcontextprotocol.io) (MCP) is how the agent reaches the document: it gets tools to read, comment, suggest changes and, with edit access, apply them.

## The local app

A browser cannot start programs on your computer, so the LibrePaper local app does it. It finds the coding agents installed on your computer, starts the one you pick, and hands it the document tools. The same app renders Quarto and Typst with your native tools. Pair it once per browser: click Connect in the Agent sidebar and allow it in the dialog the app shows.

## Install

Follow the [install page](install.html).

## Run

```sh
librepaper                           # start in the background
librepaper --at-login                # also start every time you log in
librepaper status                    # address, pairings, tools and agents found
librepaper stop                      # stop it

# Any other ACP agent: everything after -- is its command
librepaper agent add myagent --label "My Agent" -- myagent --acp
```

## In the document

The Local button in the document's top bar shows a green dot when the app is connected. Click it for the local app settings.

![The Local button in the document's top bar.](../images/agent-local-button.png)

In the Agent sidebar's Chat tab, pick the agent and its access (Comment or Edit), then type a request. Sending starts the agent.

![The Chat tab of the Agent sidebar.](../images/agent-chat.png)

The Tasks tab holds ready-made requests: proofread, tighten or rewrite a selected passage, check citations, summarize the document.

![The Tasks tab of the Agent sidebar.](../images/agent-tasks.png)
