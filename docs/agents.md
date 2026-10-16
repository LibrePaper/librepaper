---
title: "Agents"
---

Use the coding agent you already have: Claude Code, Codex, Pi, or opencode. LibrePaper never sees your model or credentials, and no inference runs on the LibrePaper server.

The [Agent Client Protocol](https://agentclientprotocol.com) (ACP) starts your agent and relays requests and permission requests. The [Model Context Protocol](https://modelcontextprotocol.io) (MCP) gives the agent tools to read, comment and make tracked changes. Editors accept or reject those changes.

## Desktop app

Agents run in the [desktop app](desktop-app.html). The app finds agents installed on your computer, starts the one you pick, and hands it the document tools.

To offer another ACP agent in the app, add it in Settings.

1. Open Settings and go to Agents.
2. Click Add agent.
3. Enter a Name and a Command. The command is the program and its arguments, as you would type them in a terminal, for example `myagent --acp`.
4. Click Add agent.

To remove an added agent, click Remove next to it.

> **Warning:** An agent reading a shared document reads text anyone with edit access wrote, so treat it as acting on their instructions too.
