---
title: "Agents"
---

LibrePaper works with the coding agent you already use: Claude Code, Codex, Pi or opencode. The agent reads your document, comments on it and suggests changes. You accept or reject each suggestion, as you would a coauthor's. Your model and your credentials stay with you, and no AI runs on the LibrePaper server.

There are two ways to connect an agent:

- **The built-in agent, in the desktop app.** It is an agent installed on your computer, and it chats with you next to the document. See [Built-in agent](#built-in-agent).
- **An external agent, with a token.** You paste one block of instructions into the agent you already use, and it connects to one project. This works from the browser or the desktop app, and the agent can run anywhere. See [Connect your agent](#connect-your-agent).

> **Warning:** A shared document holds text from everyone with edit access. An agent that reads it may follow instructions hidden in that text.

## Connect your agent

**Connecting takes one copy and one paste, all from the Agent panel.** You configure nothing by hand, and you need no local copy of the project: the token tells the agent which server and project to reach. Each project has its own token, so you repeat this for each project.

Open the project, then the Agent panel (the robot icon in the sidebar), then the Agent tab.

1. Click **New token**.
2. Choose a permission and an expiry.
3. Click **Create token**.
4. Click **Copy connection instructions**.
5. Paste the text into your agent.

The first paste installs the LibrePaper skill if it is missing, which happens once per computer. The skill is a folder of instructions and a small script. The agent reads it directly and connects to the project in the same session, so you do not need to start a new session.

The agent connects, tells you what it reached and asks what you want done.

To connect again later, use the copy icon on the token's row, which copies the same instructions. Tokens created before this release have no copy icon; create a new token and revoke the old one to get one.

The connection instructions contain the token, so it sits in your agent's chat, where your model provider and the chat history can see it. To keep it out of the chat, see [Other connections](#other-connections).

### If the agent cannot install the skill

You can still work right away. Many agents run in a sandbox that blocks their skills folder, or the install command needs your input. The agent then copies the skill to a temporary folder and uses it for that session. It tells you that the skill is not installed, what blocked it, and the exact command to run in your terminal so it is installed next time.

The agent still needs network access. Codex's default sandbox blocks it for shell commands, so allow it if Codex asks, or start Codex with `codex -c sandbox_workspace_write.network_access=true`.

### If the skill is out of date

When a server needs a newer skill than the one installed, the skill's script says so, and your agent passes on the command to update it:

```sh
npx skills update librepaper
```

## Built-in agent

The [desktop app](desktop-app.html) finds the agents installed on your computer. Install one of the agents below, restart LibrePaper, and pick it in the Chat tab of the Agent panel. In a browser, the Chat tab points you to the desktop app.

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

Any agent that speaks the [Agent Client Protocol](https://agentclientprotocol.com) can work in the app. Open Settings, go to AI agents and click Add agent. Give it a name and the command that starts it, such as `myagent --acp`. To remove it later, click Remove next to it.

## Agent tokens

A token gives one agent access to one project, and you can revoke it at any time. The agent can run anywhere: on your computer, on a home server, in CI or behind a chat bot.

When you click **New token**, you pick a permission and an expiry. You can also name the token (its comments and suggestions appear under that name), and set an hourly comment limit. The name defaults to "Agent".

| Permission | What the agent can do |
|---|---|
| Read | Read the project. |
| Comment | Read and comment. |
| Edit | Everything Comment allows, plus tracked suggestions, direct edits, and creating or deleting text files. Tracked suggestions need Edit; Comment cannot change text. Edit tokens expire after one day by default. |

### What a token cannot do

A token reaches only its own project. It cannot upload images, publish, share or run code. Each token has an hourly comment limit, which you set when you create it. A single replacement is at most 60,000 characters and a single new file at most 262,144 characters, so agents split larger changes.

A token stops working when it expires, when you revoke it with the trash button in the Agent tab of the Agent panel, when the project changes owner, or when your account is blocked or erased. Signing out does not stop it, so the agent keeps working while you are signed out or LibrePaper is closed.

## Tasks

The Tasks tab of the Agent panel lists tasks. Each task shows its instruction text with a copy icon. Click the icon, paste the text into any agent, and the agent works on it in the project.

In the desktop app, most tasks also have a **Run** button. It opens the task in the built-in agent. There you choose the scope (the selection, one file or the whole document), and then send it.

The tab has two ready-made tasks, **Address comments** and **Fix diagnostics**. Comments and diagnostics have a robot button too, which sends that item to the built-in agent.

## Other connections

Most people do not need this section. Use it to keep the token out of your agent's chat, or to connect a program that cannot use the skill.

**Other connection methods**, at the bottom of the Agent tab, has copy icons for the Claude Code commands, an MCP configuration and the token itself. If you have several tokens, choose which one to use there.

### Claude Code

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

### Codex

Codex keeps the token in its own config, so it stays out of the chat but not out of that file:

```sh
codex mcp add librepaper --env LIBREPAPER_AGENT_TOKEN="$LIBREPAPER_AGENT_TOKEN" -- librepaper mcp
```

### Other MCP clients

Any other MCP client runs `librepaper mcp` with the token in its environment. In a config file, that looks like this:

```toml
[mcp_servers.librepaper]
command = "librepaper"
args = ["mcp"]
env = { LIBREPAPER_AGENT_TOKEN = "lpt_..." }
```

### HTTPS, without MCP

A program without MCP can call the project's tools over HTTPS. The host and the slug are the ones in the project's address bar.

```sh
# List the tools this token may use, with their full schemas
curl -H "Authorization: Bearer lpt_..." "https://<host>/api/documents/<slug>/tools?schema=full"
# Call one tool; the body is its arguments as JSON
curl -X POST -H "Authorization: Bearer lpt_..." -H "Content-Type: application/json" \
  -d '<arguments>' https://<host>/api/documents/<slug>/tools/<name>
```

Every write needs an `operation` object, which `librepaper mcp` otherwise adds for you. Its `epoch` is the `operation_epoch` from your latest `document_read`. Its `id` is new for each write and looks like `v2.<unix milliseconds>.<32 hex characters>`. When a write times out, ask `document_result` with `{"kind": "operation", "target_operation": <the same object>}` whether it landed before you send it again.

## Agent skill

The LibrePaper skill teaches your agent how to use LibrePaper. With it, the agent knows how to work through a token, sync a Git repository, run a server and work on the LibrePaper code. It works with Claude Code, Codex, Pi, opencode and any agent that reads `SKILL.md` folders.

The paste in [Connect your agent](#connect-your-agent) installs it the first time. To install it yourself:

```sh
# Install the skill for every agent on this computer
npx skills add LibrePaper/librepaper --skill librepaper --global
# Update it later
npx skills update librepaper
```

The `npx` command needs Node.js. On Linux, install the current LTS from [nodejs.org](https://nodejs.org), because distribution packages are often too old.

Start a new agent session afterwards so it loads the skill. Inside a clone of the LibrePaper repository, Claude Code and Codex find the skill without installing it.
