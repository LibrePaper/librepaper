---
title: "Agents"
---

LibrePaper works with the coding agent you already use: Claude Code, Codex, Pi or opencode. The agent reads your document, comments on it and suggests changes. You accept or reject each suggestion, as you would a coauthor's. Your model and your credentials stay with you, and no AI runs on the LibrePaper server.

> **Warning:** A shared document holds text from everyone with edit access. An agent that reads it may follow instructions hidden in that text.

## Connect your agent

**Connecting takes two copies and two pastes, all from the Agent panel.** You configure nothing by hand, and you need no local copy of the project: the token tells the agent which server and project to reach.

Open the project, then the Agent panel (the robot icon in the sidebar), then the Tokens tab.

1. **Install the skill, once per computer.** Click **Copy install instructions** and paste them into your agent. The skill is a folder of instructions and a small script. When the agent says it is installed, start a new session, because agents load skills when a session starts.
2. **Connect to the project, once per project.** Click **New token**, choose a permission and an expiry, and click **Create token**. Then click **Copy connection instructions** and paste them into the new session.

The agent connects, tells you what it reached and asks what you want done.

To connect again later, use the copy icon on the token's row, which copies the same instructions. Tokens created before this release have no copy icon; create a new token and revoke the old one to get one.

The connection instructions contain the token, so it sits in your agent's chat, where your model provider and the chat history can see it. To keep it out of the chat, see [Other connections](#other-connections).

### If the agent cannot install the skill

You can still work right away. Many agents run in a sandbox that blocks their skills folder, or the install command needs your input. The agent then copies the skill to a temporary folder and uses it for that session. It tells you that the skill is not installed, what blocked it, and the exact command to run in your terminal so it is installed next time.

The same happens if you paste connection instructions into an agent that has no skill: it makes the temporary copy, connects, and gives you the command to install the skill.

The agent still needs network access. Codex's default sandbox blocks it for shell commands, so allow it if Codex asks, or start Codex with `codex -c sandbox_workspace_write.network_access=true`.

### If the skill is out of date

When a server needs a newer skill than the one installed, the skill's script says so, and your agent passes on the command to update it:

```sh
npx skills update librepaper
```

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

A token stops working when it expires, when you revoke it with the trash button in the Tokens tab of the Agent panel, when the project changes owner, or when your account is blocked or erased. Signing out does not stop it, so the agent keeps working while you are signed out or LibrePaper is closed.

## Tasks

The Tasks tab of the Agent panel lists tasks. Each task shows its instruction text with a copy icon. Click the icon, paste the text into your agent, and the agent works on it in the project.

The tab has two ready-made tasks, **Address comments** and **Fix diagnostics**. They replace the robot buttons that used to sit on comments and diagnostics.

## Other connections

Most people do not need this section. Use it to keep the token out of your agent's chat, or to connect a program that cannot use the skill.

**Other connection methods**, at the bottom of the Tokens tab, has copy icons for the Claude Code commands, an MCP configuration and the token itself. If you have several tokens, choose which one to use there.

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

**Copy install instructions**, in [Connect your agent](#connect-your-agent), is the easiest way to install it. To install it yourself:

```sh
# Install the skill for every agent on this computer
npx skills add LibrePaper/librepaper --skill librepaper --global
# Update it later
npx skills update librepaper
```

The `npx` command needs Node.js. On Linux, install the current LTS from [nodejs.org](https://nodejs.org), because distribution packages are often too old.

Start a new agent session afterwards so it loads the skill. Inside a clone of the LibrePaper repository, Claude Code and Codex find the skill without installing it.
