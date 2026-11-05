---
title: "Agents"
---

LibrePaper works with the coding agent you already use: Claude Code, Codex, Pi or opencode. The agent reads your document, comments on it and suggests changes. You accept or reject each suggestion, as you would a coauthor's. Your model and your credentials stay with you, and no AI runs on the LibrePaper server.

You can connect an agent in two ways:

- **With an agent token.** You copy one block from the project into the agent you already use. It works from the browser or the desktop app, and the agent can run anywhere.
- **In the desktop app.** You pick an agent in the sidebar and chat with it next to your document.

> **Warning:** A shared document holds text from everyone with edit access. An agent that reads it may follow instructions hidden in that text.

## Agent tokens

A token lets an agent you start yourself work on one project. The agent can run anywhere: on your computer, on a home server, in CI or behind a chat bot. It does not need the desktop app.

### Create a token

1. Open the project, then Share, then the Agents tab.
2. Click New. Pick a permission and an expiry.
3. Under Advanced you can name the token (its comments and suggestions appear under that name) and set an hourly comment limit. The name defaults to "Agent".
4. Click Create token. Copy the block and paste it into Claude Code, Codex or another agent. LibrePaper shows it only once.

The agent fetches its instructions and asks you what to do.

The block contains the token, so it sits in your agent's chat, where your model provider and the chat history can see it. To keep it out of the chat, open **Other ways to connect** and use MCP, described below.

Codex's default sandbox blocks network access for shell commands. Allow it if Codex asks, or start Codex with `codex -c sandbox_workspace_write.network_access=true`.

| Permission | What the agent can do |
|---|---|
| Read | Read the project. |
| Comment | Read and comment. |
| Edit | Everything Comment allows, plus tracked suggestions, direct edits, and creating or deleting text files. Tracked suggestions need Edit; Comment cannot change text. Edit tokens expire after one day by default. |

### Keep the token out of the chat: Claude Code

Install the [librepaper command](cli.html#install), then register it with Claude Code once. You give the token each time you start Claude, so it is never written to a file.

```sh
# Once: register LibrePaper for every folder
claude mcp add librepaper --scope user -- librepaper mcp
# Each session: paste the token when asked; nothing is echoed or saved
read -rs LIBREPAPER_AGENT_TOKEN && export LIBREPAPER_AGENT_TOKEN
claude
# Without a person to approve tools, allow them up front
claude -p "Read the document and comment on the introduction" --allowedTools "mcp__librepaper"
```

To work on another project, export a different token before you start Claude.

### Keep the token out of the chat: other agents

Codex keeps the token in its own config, so it stays out of the chat but not out of that file:

```sh
codex mcp add librepaper --env LIBREPAPER_AGENT_TOKEN="$LIBREPAPER_AGENT_TOKEN" -- librepaper mcp
```

Any other MCP client runs `librepaper mcp` with the token in its environment. In a config file, that looks like this:

```toml
[mcp_servers.librepaper]
command = "librepaper"
args = ["mcp"]
env = { LIBREPAPER_AGENT_TOKEN = "lpt_..." }
```

### Without MCP

A program without MCP can call the project's tools over HTTPS. The host and the slug are the ones in the project's address bar.

```sh
# List the tools this token may use, with their full schemas
curl -H "Authorization: Bearer lpt_..." "https://<host>/api/documents/<slug>/tools?schema=full"
# Call one tool; the body is its arguments as JSON
curl -X POST -H "Authorization: Bearer lpt_..." -H "Content-Type: application/json" \
  -d '<arguments>' https://<host>/api/documents/<slug>/tools/<name>
```

Every write needs an `operation` object, which `librepaper mcp` otherwise adds for you. Its `epoch` is the `operation_epoch` from your latest `document_read`. Its `id` is new for each write and looks like `v2.<unix milliseconds>.<32 hex characters>`. When a write times out, ask `document_result` with `{"kind": "operation", "target_operation": <the same object>}` whether it landed before you send it again.

### What a token cannot do

A token reaches only its own project. It cannot upload images, publish, share or run code. Each token has an hourly comment limit, which you set under Advanced when you create it. A single replacement is at most 60,000 characters and a single new file at most 262,144 characters, so agents split larger changes.

A token stops working when it expires, when you revoke it with the trash button in the Agents tab, when the project changes owner, or when your account is blocked or erased. Signing out does not stop it, so the agent keeps working while you are signed out or LibrePaper is closed.

## Desktop app

The [desktop app](desktop-app.html) finds the agents installed on your computer. Install one of the agents below, restart LibrePaper, and pick it in the agent sidebar.

Claude Code and Codex connect through small adapters from npm, which need Node.js 22 or newer. On Linux, install the current LTS from [nodejs.org](https://nodejs.org), because distribution packages are often too old.

```sh
# Install Node.js on macOS
brew install node
# Install Node.js on Windows
winget install OpenJS.NodeJS.LTS
```

### Claude Code

```sh
# Install Claude Code on macOS or Linux
curl -fsSL https://claude.ai/install.sh | bash
# Install Claude Code on Windows, in PowerShell
irm https://claude.ai/install.ps1 | iex
# Log in once
claude
# Install the adapter
npm install -g @agentclientprotocol/claude-agent-acp
```

If you skip the adapter, LibrePaper offers to fetch it the first time you start Claude Code. Installing it yourself keeps a version that does not change under you.

### Codex

```sh
# Install Codex and log in once
npm install -g @openai/codex
codex login
# Install the adapter
npm install -g @agentclientprotocol/codex-acp
```

- Your Access setting in LibrePaper decides which document tools Codex may run without asking.
- Do not add a `librepaper` server to your own Codex config. Codex then ignores the one LibrePaper gives it.
- Codex on Windows is experimental. LibrePaper uses a Codex installed in Windows, not one inside WSL.

### Pi

Pi needs an extension to reach the document tools.

```sh
# Install the extension
pi install npm:pi-mcp-adapter
# Log in to a model provider: start pi, then type /login
pi
```

Then add this entry to `~/.config/mcp/mcp.json`:

```json
{ "mcpServers": { "librepaper": { "command": "librepaper", "args": ["mcp"], "lifecycle": "keep-alive", "directTools": true } } }
```

- Use this global file. Pi ignores project files when LibrePaper starts it.
- With the macOS `.dmg`, set `command` to `/Applications/LibrePaper.app/Contents/MacOS/librepaper`, because the `.dmg` does not put `librepaper` on your PATH.

### opencode

```sh
# Log in to a model provider once
opencode auth login
```

### Another agent

Any agent that speaks the [Agent Client Protocol](https://agentclientprotocol.com) can work in the app. Open Settings, go to Agents and click Add agent. Give it a name and the command that starts it, such as `myagent --acp`. To remove it later, click Remove next to it.

## Agent skill

The LibrePaper skill teaches your agent how to use LibrePaper. With it, the agent knows how to work through a token, sync a Git repository, run a server and work on the LibrePaper code. It works with Claude Code, Codex, Pi, opencode and any agent that reads `SKILL.md` folders.

```sh
# Install the skill for every agent on this computer
npx skills add LibrePaper/librepaper --skill librepaper --global
# Update it later
npx skills update librepaper
```

You can also ask your agent to install it. Paste this into the chat:

```text
Install the LibrePaper agent skill from https://github.com/LibrePaper/librepaper/tree/main/skills/librepaper into your skills folder.
```

Restart the agent afterwards so it loads the skill. Inside a clone of the LibrePaper repository, Claude Code and Codex find the skill without installing it.
