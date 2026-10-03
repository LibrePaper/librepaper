---
title: "Agents"
---

Use the coding agent you already have: Claude Code, Codex, Pi, or opencode. LibrePaper never sees your model or credentials, and no inference runs on the LibrePaper server.

The [Agent Client Protocol](https://agentclientprotocol.com) (ACP) starts your agent and relays requests and permission requests. The [Model Context Protocol](https://modelcontextprotocol.io) (MCP) gives the agent tools to read, comment, suggest changes and, with edit access, apply them.

## Local app

The LibrePaper local app finds agents installed on your computer, starts the one you pick, and hands it the document tools. See the [install page](install.html).

```sh
librepaper                           # start in the background
librepaper --at-login                # also start every time you log in
librepaper status                    # address, pairings, tools and agents found
librepaper stop                      # stop it

# Any other ACP agent: everything after -- is its command
librepaper agent add myagent --label "My Agent" -- myagent --acp
```

> **Warning:** An agent reading a shared document reads text anyone with edit access wrote, so treat it as acting on their instructions too.
