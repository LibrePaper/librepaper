---
title: "Agents"
---

Use the coding agent you already have: Claude Code, Codex, Pi, or opencode. LibrePaper never sees your model or credentials, and no inference runs on the LibrePaper server.

The [Agent Client Protocol](https://agentclientprotocol.com) (ACP) starts your agent and relays requests and permission requests. The [Model Context Protocol](https://modelcontextprotocol.io) (MCP) gives the agent tools to read, comment, suggest changes and, with edit access, apply them.

## Desktop app

Agents run in the [desktop app](desktop-app.html). The app finds agents installed on your computer, starts the one you pick, and hands it the document tools.

To offer another ACP agent in the app, add its command with the CLI. Everything after `--` is the command.

```sh
# Any other ACP agent
librepaper agent add myagent --label "My Agent" -- myagent --acp
```

> **Warning:** An agent reading a shared document reads text anyone with edit access wrote, so treat it as acting on their instructions too.
