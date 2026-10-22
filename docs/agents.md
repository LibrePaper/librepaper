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

## Before you start

Claude Code and Codex reach LibrePaper through ACP adapters published on npm. The adapters need Node.js 22 or newer.

- macOS: `brew install node`
- Windows: `winget install OpenJS.NodeJS.LTS`
- Linux: install the current LTS from https://nodejs.org. Distribution packages are often older than 22.

After you install anything for an agent, restart LibrePaper so the app finds it.

## Claude Code

Claude Code runs through the `@agentclientprotocol/claude-agent-acp` adapter.

```sh
# Install Claude Code on macOS or Linux
curl -fsSL https://claude.ai/install.sh | bash
# Install Claude Code on Windows, in PowerShell
irm https://claude.ai/install.ps1 | iex
# Log in to Claude Code before first use
claude
# Install the adapter
npm install -g @agentclientprotocol/claude-agent-acp
```

Restart LibrePaper.

- Without the adapter, the app can fetch it with `npx` on first use. It asks first. Installing it keeps a version that does not change under you.

## Codex

Codex runs through the `@agentclientprotocol/codex-acp` adapter. The app fetches it with `npx` when it is not installed.

```sh
# Install Codex
npm install -g @openai/codex
# Log in to Codex before first use
codex login
# Install the adapter
npm install -g @agentclientprotocol/codex-acp
```

Restart LibrePaper.

- LibrePaper starts the adapter in workspace-write mode, so your Access choice decides which document tools run without asking.
- Do not add a `librepaper` server to your own Codex config. The adapter then skips the one LibrePaper hands it.
- On Windows, Codex support is experimental. LibrePaper uses a Codex installed in Windows, not one inside WSL.

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

## opencode

```sh
# Log in to a model provider before first use
opencode auth login
```

> **Warning:** An agent reading a shared document reads text anyone with edit access wrote, so treat it as acting on their instructions too.

## Remote agents

An agent on another machine, such as a home server or a chat bot, works on a document with an agent token. The token reaches that one document and nothing else.

- The owner creates the token in the Sharing panel, on the Agent tokens tab. It takes a name, Comment or Suggest permission, and an expiry.
- LibrePaper shows the token once. Copy it then.

Install the [librepaper CLI](cli.html) on the agent's machine. Then add the document to the agent's MCP config. This Codex example sets the token in the environment:

```toml
[mcp_servers.librepaper]
command = "librepaper"
args = ["mcp", "--document", "https://<host>/docs/<slug>"]
env = { LIBREPAPER_AGENT_TOKEN = "lpt_..." }
```

Agents without MCP call the document's tool routes over HTTP:

```sh
# List the tools this token may use
curl -H "Authorization: Bearer lpt_..." https://<host>/api/documents/<slug>/tools
# Call one tool; the body is its arguments as JSON
curl -X POST -H "Authorization: Bearer lpt_..." -H "Content-Type: application/json" \
  -d '<arguments>' https://<host>/api/documents/<slug>/tools/<name>
```

- Every write needs an `operation` object, which `librepaper mcp` adds for you. The writing tools are `document_comment`, `document_propose` and `document_apply`.
  - `epoch` is the `operation_epoch` from your latest `document_read` result.
  - `id` is a new key for each write, in the form `v2.<unix milliseconds>.<32 hex characters>`.
  - When a write times out, call `document_result` with `{"kind": "operation", "target_operation": <the same operation object>}` to learn whether it landed. Do not send the write again first.
- The token works only on these two routes, and only for its own document.
- Comments and suggestions appear under the token's name. A person accepts or rejects each suggestion.
- A Comment token gives comment access only. A Suggest token never applies changes, writes files, runs code or publishes.
- A token stops working at its expiry, when you revoke it, when you Replace it, when you sign out everywhere, and when the document changes owner. Replace issues a new token, and the old one stops at once.
