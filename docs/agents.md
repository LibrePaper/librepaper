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

## Pi

Pi needs the `pi-mcp-adapter` extension to reach the document tools.

```sh
# Install the MCP extension for Pi
pi install npm:pi-mcp-adapter
# Log in to a model provider
pi    # then type /login
```

Add this entry to `~/.config/mcp/mcp.json`:

```json
{ "mcpServers": { "librepaper": { "command": "librepaper", "args": ["mcp"], "lifecycle": "keep-alive", "directTools": true } } }
```

- `keep-alive` makes Pi connect at start, so LibrePaper sees the tools.
- `directTools` puts the document tools in Pi's own tool list.
- Use the global file. Pi skips unapproved project servers in the headless mode LibrePaper uses.
- The macOS `.dmg` does not put `librepaper` on your PATH. Set `command` to `/Applications/LibrePaper.app/Contents/MacOS/librepaper` instead.

## Codex

Codex runs through the `@agentclientprotocol/codex-acp` adapter. The app fetches it with `npx` when it is not installed.

```sh
# Log in to Codex before first use
codex login
```

- LibrePaper starts the adapter in workspace-write mode, so your Access choice decides which document tools run without asking.
- Do not add a `librepaper` server to your own Codex config. The adapter then skips the one LibrePaper hands it.

## opencode

```sh
# Log in to a model provider before first use
opencode auth login
```

> **Warning:** An agent reading a shared document reads text anyone with edit access wrote, so treat it as acting on their instructions too.
