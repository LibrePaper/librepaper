---
title: "Agents"
---

LibrePaper works with the coding agent you already use: Claude Code, Codex, Pi or opencode. The agent reads your document, comments on it and suggests changes. You accept or reject each suggestion, as you would a coauthor's. Your model and your credentials stay with you. The LibrePaper server runs no AI model of its own.

There are three ways to connect an agent:

- **An app or coding agent, with a connector.** This is the main way. You add the project's URL to the app as a remote MCP server or custom connector, and approve the connection in LibrePaper. See [Connect an app or agent](#connect-an-app-or-agent).
- **The built-in agent, in the desktop app.** It is an agent installed on your computer, and it chats with you next to the document. See [Built-in agent](#built-in-agent).
- **An external agent, with a token.** The fallback, for scripts, sync, and agents that cannot add a connector. You paste one block of instructions into the agent you already use, and it connects to one project. This works from the browser or the desktop app, and the agent can run anywhere. See [Use a token](#use-a-token).

> **Warning:** A shared document holds text from everyone with suggest or edit access. An agent that reads it may follow instructions hidden in that text.

## Two ways an agent reaches a project

An agent reaches a project in one of two ways. They differ in whose authority the agent uses.

1. **An agent token from the owner.** The owner makes a token in the project's Agent panel, or approves an app through the connector. Both give an `lpt_` token. The token is the authority: it has its own role (Read, Comment, Suggest or Edit), expiry and name. The owner sees it in the Agent panel and revokes it there. Its comments and suggestions carry the token's name. Only the owner can create tokens or approve the connector.
2. **Your own share link, with your sign-in.** Anyone with a share link to the project who is not its owner can let an agent work as them, at that link's role. Two clients do this: the desktop app's built-in agent, and the `librepaper` command's `mcp --link` option, which uses the sign-in from `librepaper login`. The work appears under your own name. There is no separate agent entry in the Agent panel. It ends when the owner revokes the link or when your sign-in expires, after 90 days.

Owners use a token or the connector. The owner has no share link of their own. Collaborators use their share link, or ask the owner for a token if they want a separately named agent that the owner can revoke on its own. To use a share link from the command line, sign in once and register the link with Claude Code:

```sh
librepaper login
claude mcp add librepaper --scope user -- librepaper mcp --link 'https://app.librepaper.org/docs/<slug>#k=...'
```

The link must include its key after the `#`, exactly as the Share panel gives it. Start a new Claude Code session afterwards, because an MCP server added during a session does not appear until then.

## Connect an app or agent

## Connect an app or agent

Use the connector to let a chat app or a coding agent work on one project. It is the main way to connect. You add the project's URL to the app, and the app opens a LibrePaper page where you approve the connection. No token goes into the chat, and you configure nothing by hand.

Only the project owner sees the URL and can approve connections. Each project has its own URL, so you connect each project once.

1. Open the project, then the Agent panel (the robot icon in the sidebar). In the Connect tab, under **Connect an app**, click the copy icon next to the URL. It has the form `https://<server>/mcp/<slug>`, where `<slug>` is the project's slug in its address bar.
2. Add the URL to your app as a remote MCP server or custom connector:
   - **claude.ai and Claude Desktop:** go to Settings → Connectors → Add custom connector, and paste the URL.
   - **Claude Code:** run `claude mcp add --transport http librepaper https://<server>/mcp/<slug>`. Then run `/mcp` in a session, and choose to authenticate. The approval page opens in your browser.
   - **ChatGPT:** open Plugins at chatgpt.com/plugins, select the plus button, then **Add custom MCP server**. Enter a name, enter the URL as the **Server URL** under **Connection**, and choose **OAuth**. Select **I understand and want to continue**, then **Create as a plugin**, and install it. In a chat, type `@` and choose it. The approval page opens when ChatGPT connects. OpenAI documents this for ChatGPT on the web. A workspace admin may need to allow custom MCP servers.
   - **Any other app:** add the URL as a remote MCP server. The app opens the approval page.
3. A LibrePaper page opens. Sign in if asked.
4. Choose a role for the app. Suggest is the default.
5. Click **Approve**.

The app now has the LibrePaper tools for that project. The connection is an agent token named after the app, so its comments and suggestions appear under that name. The roles are the same for every app.

| Role | What the app can do |
|---|---|
| Read | Read the project. |
| Comment | Read and comment. |
| Suggest | Everything Comment allows, plus tracked suggestions. |
| Edit | Everything Suggest allows, plus direct edits, and creating, renaming or deleting text files. It can also read the project's history and sync its files. |

The connection lasts 30 days and renews while the app keeps using it. It is listed under **Connected** in the Connect tab. To end it at any time, click the revoke button on its row. Revoking it stops the app at once.

Chat apps such as claude.ai and ChatGPT call the server from the app maker's servers. For them, the LibrePaper server must be reachable from the internet. A server that is only on a local network or a Tailscale network cannot be used by a chat app. Coding agents connect from your computer, so this does not apply to them.

A host can turn connectors off with `agent_connectors = false` in `librepaper.toml`. The Agent panel then hides this section. A host can also turn every agent off with `agents = false`: the Agent panel is then hidden and existing tokens and connections are refused. See [Agents and local builds](host/advanced.html#agents-and-local-builds).

## Use a token

Use a token for scripts, sync, or an agent that cannot add a connector. The one-paste flow below works in any session, with no setup, so it is the fallback when a connector is not possible. Make tokens in the **Tokens** section of the Connect tab; they are listed under **Connected**.

**Connecting takes one copy and one paste, all from the Agent panel.** You configure nothing by hand, and you need no local copy of the project: the token tells the agent which server and project to reach. Each project has its own token, so you repeat this for each project.

Open the project, then the Agent panel (the robot icon in the sidebar), then the Connect tab.

1. Click **New token**.
2. Choose a permission and an expiry.
3. Click **Create token**.
4. Click **Copy connection instructions**.
5. Paste the text into your agent.

The first paste installs the LibrePaper skill if it is missing, which happens once per computer. The skill is a folder of instructions and a small script. The agent reads it directly and connects to the project in the same session, so you do not need to start a new session.

The agent connects, tells you what it reached and asks what you want done.

To connect again later, use the copy icon on the token's row, which copies the same instructions. A row has a copy icon only if LibrePaper holds the token. Tokens made by earlier versions have none, so create a new token and revoke the old one.

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

Claude Code, Codex and Pi connect through small adapters from npm, which need Node.js 22 or newer. On Linux, install the current LTS from [nodejs.org](https://nodejs.org), because distribution packages are often too old.

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

If you skip the adapter, LibrePaper offers to fetch it the first time you use the agent. A fetched adapter downloads again each time you use it, so install it yourself to keep a version that does not change under you.

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

```sh
# Log in to a model provider: start pi, then type /login
pi
```

- LibrePaper hands Pi the document tools itself. This needs a Pi with built-in MCP support; run `pi update` if the agent reports that it found no LibrePaper tools.
- If you added a `librepaper` entry to `~/.config/mcp/mcp.json` for an earlier version, remove it.

### opencode

```sh
# Log in to a model provider once
opencode auth login
```

### Another agent

Any agent that speaks the [Agent Client Protocol](https://agentclientprotocol.com) can work in the app. Open Settings, go to AI agents and click Add agent. Give it a name and the command that starts it, such as `myagent --acp`. To remove it later, click Remove next to it.

## Agent tokens

A token gives one agent access to one project, and you can revoke it at any time. The agent can run anywhere: on your computer, on a home server, in CI or behind a chat bot.

When you click **New token**, you pick a permission and an expiry. The defaults are Comment and 7 days. You can also name the token. Its comments and suggestions appear under that name, which defaults to "Agent". A token that can comment also takes an hourly comment limit. Leave it empty for no limit.

| Permission | What the agent can do |
|---|---|
| Read | Read the project. |
| Comment | Read and comment. |
| Suggest | Everything Comment allows, plus tracked suggestions. A Suggest token cannot apply edits directly; the server enforces this. |
| Edit | Everything Suggest allows, plus direct edits, and creating, renaming or deleting text files. Comment cannot change text, and Suggest only as tracked suggestions. Choosing Edit turns a 7-day expiry into one day. |

### What a token cannot do

A token reaches only its own project, through the agent tools, the connector endpoint and, for an Edit token, the sync that `librepaper push` uses. An Edit token can therefore upload files, images included, with `librepaper push`. The agent tools themselves cannot upload images, publish, share or run code. A single replacement is at most 60,000 characters and a single new file at most 262,144 characters, so agents split larger changes.

A token stops working when it expires, when you revoke it with the revoke button on its row in the Connect tab, when the project changes owner, or when your account is blocked or erased. Signing out does not stop it, so the agent keeps working while you are signed out or LibrePaper is closed.

## Tasks

The Tasks tab of the Agent panel lists ready-made tasks in groups: Writing, Review and Document. Search filters them. Each task shows its instruction text with a copy icon. Click the icon, paste the text into any agent, and the agent works on it in the project.

In the desktop app, most tasks also have a **Run** button. It opens the task in the built-in agent. There you choose the scope (the selection, one file or the whole document), and then send it. **Address comments** and **Fix diagnostics** have no Run button; copy them instead.

Each unresolved comment and each diagnostic has a robot button, which sends that item to the built-in agent. The built-in agent runs in the desktop app.

## Other connections

Most people do not need this section. Use it to keep the token out of your agent's chat, or to connect a program that cannot use the skill.

**Command line and MCP configuration**, in the Tokens section of the Connect tab, has copy icons for the Claude Code commands, an MCP configuration and the token itself. If you have several tokens, choose which one to use there.

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

The paste in [Use a token](#use-a-token) installs it the first time. To install it yourself:

```sh
# Install the skill for every agent on this computer
npx skills add LibrePaper/librepaper --skill librepaper --global
# Update it later
npx skills update librepaper
```

The `npx` command needs Node.js. On Linux, install the current LTS from [nodejs.org](https://nodejs.org), because distribution packages are often too old.

Start a new agent session afterwards so it loads the skill. Inside a clone of the LibrePaper repository, Claude Code and Codex find the skill without installing it.
