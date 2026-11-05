# Connecting an agent to a LibrePaper project

## The usual way

1. Install the skill. The Agent panel's Tokens tab starts with the install command. Click its copy button and run the command once per computer, in a terminal or through Claude Code, Codex or another agent. It runs `npx skills add LibrePaper/librepaper --skill librepaper --global`, or copies the skill folder into the agent's skills folder. Then start a new agent session, because agents load skills when a session starts.
2. Connect to this project. After you create a token (section 1 below), click Copy connection instructions and paste the text into the new session. It contains the token and tells the agent to use the skill. If the agent has no skill, it asks you to install it. You can copy it again later with the copy icon on the token's row. Tokens created before this release have no copy icon; create a new token and revoke the old one to get one.

The token sits in the chat, where the model provider and the chat history can see it.

The steps below (1 to 4) are the MCP route that keeps the token out of the chat, with `read -rs`. They need the `librepaper` command and a new session.

## 1. Get a token

In LibrePaper, open the project, then the Agent panel (the robot icon in the sidebar), then the Tokens tab, and click New. Pick a permission (Read, Comment or Edit) and an expiry. You can also name the token (it defaults to "Agent"; comments appear under it), and set an hourly comment limit. Click Create token. Open "Other connection methods" and click Copy token for the bare `lpt_...` string. The owner can copy the token again from its row: the copy icon there copies the connection instructions, which include the token.

## 2. Install the `librepaper` command

The desktop packages (Homebrew, Scoop, Windows installer, .deb, .rpm, Nix) already include it. On a machine without the desktop app:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-installer.sh | sh
```

On macOS with the .dmg, the binary is `/Applications/LibrePaper.app/Contents/MacOS/librepaper` and is not on PATH.

## 3. Register the MCP server once, without a token

```sh
claude mcp add librepaper --scope user -- librepaper mcp
```

Claude Code passes its environment to the stdio servers it starts, so the token never needs to be in a config file.

## 4. Each session: give the token at launch

```sh
# Paste the token; nothing is echoed or kept in shell history
read -rs LIBREPAPER_AGENT_TOKEN && export LIBREPAPER_AGENT_TOKEN
claude
```

- Another token in another shell points the same registration at another project.
- Headless: `claude -p "Comment on the introduction" --allowedTools "mcp__librepaper"`.
- `librepaper mcp` needs only the token, no `--document` flag. The token carries server and project.

## Already in a session without MCP

Use the HTTP helper ([agent.md](agent.md), Step 1b). It reads the token from a 0600 file and works without restarting the client.

## Other clients

- Codex keeps the token in its config: `codex mcp add librepaper --env LIBREPAPER_AGENT_TOKEN="$LIBREPAPER_AGENT_TOKEN" -- librepaper mcp`.
- Any MCP client: run `librepaper mcp` with `LIBREPAPER_AGENT_TOKEN` in its environment.
- No MCP at all: `GET https://<origin>/api/documents/<slug>/tools?schema=full` lists the tools with their full schemas, and `POST https://<origin>/api/documents/<slug>/tools/<name>` calls one. Both take `Authorization: Bearer lpt_...`. The POST body is the tool's arguments.

## When it stops working

A token stops at its expiry, when revoked (trash button in the Agent panel's Tokens tab), when the project changes owner, or when the owner's account is blocked or erased. Signing out does not stop it. A 401 means one of these, so ask the owner for a new token.
