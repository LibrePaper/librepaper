---
title: "Agents"
---

LibrePaper works with the coding agent you already use: Claude Code, Codex, Pi or opencode. The agent reads your document, comments on it and suggests changes. You accept or reject each suggestion, as you would a coauthor's. Your model and your credentials stay with you, and no AI runs on the LibrePaper server.

You connect your agent with an agent token. A token gives one agent access to one project, and you can revoke it at any time.

> **Warning:** A shared document holds text from everyone with edit access. An agent that reads it may follow instructions hidden in that text.

## Agent tokens

A token lets an agent you start yourself work on one project. The agent can run anywhere: on your computer, on a home server, in CI or behind a chat bot.

### Create a token

1. Open the project, then the Agent panel (the robot icon in the sidebar), then the Tokens tab.
2. Click New. Pick a permission and an expiry.
3. You can also name the token (its comments and suggestions appear under that name), and set an hourly comment limit. The name defaults to "Agent".
4. Click Create token.

| Permission | What the agent can do |
|---|---|
| Read | Read the project. |
| Comment | Read and comment. |
| Edit | Everything Comment allows, plus tracked suggestions, direct edits, and creating or deleting text files. Tracked suggestions need Edit; Comment cannot change text. Edit tokens expire after one day by default. |

### Connect your agent

1. The Tokens tab of the Agent panel starts with the install command. Click its copy button and paste the command into a terminal, or into Claude Code, Codex or another agent, and run it once on each computer. It installs the skill: the agent copies a folder of instructions and a small script into its skills folder. Then start a new agent session, because agents load skills when a session starts.
2. After you create the token, click **Copy connection instructions** and paste the text into the new session. It holds the token and asks the agent to connect. You do this for each project. You can copy it again later with the copy icon on the token's row. Tokens created before this release have no copy icon; create a new token and revoke the old one to get one.

The agent connects, tells you what it reached and asks what to do.

The connection instructions contain the token, so they sit in your agent's chat, where your model provider and the chat history can see it. To keep it out of the chat, use **Copy token** under **Other connection methods** in the panel, and the MCP setup below.

Codex's default sandbox blocks network access for shell commands. Allow it if Codex asks, or start Codex with `codex -c sandbox_workspace_write.network_access=true`. If the install is blocked, the agent gives you a command to run in your terminal.

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

A token reaches only its own project. It cannot upload images, publish, share or run code. Each token has an hourly comment limit, which you set when you create it. A single replacement is at most 60,000 characters and a single new file at most 262,144 characters, so agents split larger changes.

A token stops working when it expires, when you revoke it with the trash button in the Tokens tab of the Agent panel, when the project changes owner, or when your account is blocked or erased. Signing out does not stop it, so the agent keeps working while you are signed out or LibrePaper is closed.

### Tasks

The Tasks tab of the Agent panel lists tasks. Each task shows its instruction text with a copy icon. Click the icon, paste the text into your agent, and the agent works on it in the project.

The tab has two ready-made tasks, **Address comments** and **Fix diagnostics**. They replace the robot buttons that used to sit on comments and diagnostics.

## Agent skill

The LibrePaper skill teaches your agent how to use LibrePaper. With it, the agent knows how to work through a token, sync a Git repository, run a server and work on the LibrePaper code. It works with Claude Code, Codex, Pi, opencode and any agent that reads `SKILL.md` folders.

```sh
# Install the skill for every agent on this computer
npx skills add LibrePaper/librepaper --skill librepaper --global
# Update it later
npx skills update librepaper
```

The `npx` command needs Node.js. On Linux, install the current LTS from [nodejs.org](https://nodejs.org), because distribution packages are often too old.

You can also ask your agent to install it. Paste this into the chat:

```text
Install the LibrePaper agent skill from https://github.com/LibrePaper/librepaper/tree/main/skills/librepaper into your skills folder.
```

Start a new agent session afterwards so it loads the skill. Inside a clone of the LibrePaper repository, Claude Code and Codex find the skill without installing it.
