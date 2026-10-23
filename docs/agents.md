---
title: "Agents"
---

Use the coding agent you already have: Claude Code, Codex, Pi, or opencode. LibrePaper never sees your model or credentials, and no inference runs on the LibrePaper server.

The [Agent Client Protocol](https://agentclientprotocol.com) (ACP) starts your agent and relays requests and permission requests. The [Model Context Protocol](https://modelcontextprotocol.io) (MCP) gives the agent tools to read, comment and make tracked changes. Editors accept or reject those changes.

- An agent with Edit access can also create and delete text files with `document_files`. These changes are not tracked.

There are two ways to connect an agent. The [desktop app](#desktop-app) starts an agent installed on your computer and hands it the document tools. An [agent token](#agent-tokens) lets an agent you start yourself, on any machine, work on one project.

> **Warning:** An agent reading a shared document reads text anyone with edit access wrote, so treat it as acting on their instructions too.

## Desktop app

The [desktop app](desktop-app.html) finds agents installed on your computer, starts the one you pick, and hands it the document tools.

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

## Agent tokens

An agent you start yourself works on a project with an agent token. It can run anywhere: in a terminal on your own computer, on a home server, in CI, or behind a chat bot. It does not need the desktop app. The token names its server and its project, and it reaches that one project and nothing else.

Create a token:

- Open the project, then Sharing, then the Agent tokens tab, then New.
- Give it a name. The agent's comments and suggestions appear under that name.
- Pick a permission. These are the same three as share links.
  - Read: the agent reads the project.
  - Comment: the agent reads and comments.
  - Edit: the agent also proposes tracked changes, edits the project directly, and creates and deletes text files. Edit tokens expire after 1 day by default.
- LibrePaper shows the token once. Copy it then.

Install the [librepaper CLI](cli.html#install) on the agent's machine, then register LibrePaper with Claude Code once, without a token. Claude Code passes its environment to the servers it starts, so the token is given when you launch Claude and is never written to a config file:

```sh
# Once: register LibrePaper for every folder
claude mcp add librepaper --scope user -- librepaper mcp

# Each session: paste the token when asked; nothing is echoed or kept in shell history
read -rs LIBREPAPER_AGENT_TOKEN && export LIBREPAPER_AGENT_TOKEN
claude

# Headless: allow the LibrePaper tools up front, since nobody is there to approve them
claude -p "Read the document and comment on the introduction" --allowedTools "mcp__librepaper"
```

- A different token in the same shell points the same registration at another project.
- Codex keeps the token in its config file. Register it with `codex mcp add librepaper --env LIBREPAPER_AGENT_TOKEN="$LIBREPAPER_AGENT_TOKEN" -- librepaper mcp`, then run `codex exec "..."`.
- Any other MCP client runs `librepaper mcp` with `LIBREPAPER_AGENT_TOKEN` in its environment. In a config file that looks like this:

```toml
[mcp_servers.librepaper]
command = "librepaper"
args = ["mcp"]
env = { LIBREPAPER_AGENT_TOKEN = "lpt_..." }
```

Agents without MCP call the project's tool routes over HTTP. The host and the slug are the ones in the project's address bar:

```sh
# List the tools this token may use
curl -H "Authorization: Bearer lpt_..." https://<host>/api/documents/<slug>/tools
# Call one tool; the body is its arguments as JSON
curl -X POST -H "Authorization: Bearer lpt_..." -H "Content-Type: application/json" \
  -d '<arguments>' https://<host>/api/documents/<slug>/tools/<name>
```

- Every write needs an `operation` object, which `librepaper mcp` adds for you. The writing tools are `document_comment`, `document_propose`, `document_apply` and `document_files`.
  - `epoch` is the `operation_epoch` from your latest `document_read` result.
  - `id` is a new key for each write, in the form `v2.<unix milliseconds>.<32 hex characters>`.
  - When a write times out, call `document_result` with `{"kind": "operation", "target_operation": <the same operation object>}` to learn whether it landed. Do not send the write again first.
- The token works only on these two routes, and only for its own project.

What a token can and cannot do:

- Comments and suggestions appear under the token's name. A person accepts or rejects each suggestion.
- No token uploads images, publishes, shares or runs code.
- Each token has an hourly comment limit, set under Advanced when you create it.
- A token stops working at its expiry, when you revoke it with the trash button in the Sharing panel, when you sign out everywhere, and when the project changes owner.
