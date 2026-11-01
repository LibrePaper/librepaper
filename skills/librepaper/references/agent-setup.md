# Connecting an agent to a LibrePaper project

## 1. Get a token

In LibrePaper, open the project, then Sharing, the Agent tokens tab, New. Name it (comments appear under that name), pick Read, Comment or Edit, and copy the `lpt_...` string. It is shown once.

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
- No MCP at all: `GET` or `POST https://<origin>/api/documents/<slug>/tools[/<name>]` with `Authorization: Bearer lpt_...`. The POST body is the tool's arguments.

## Desktop app instead of a token

The LibrePaper desktop app can start Claude Code itself (through the `@agentclientprotocol/claude-agent-acp` adapter, Node 22 or later) from the editor's agent sidebar and hand it the tools. No token is involved. That path is for chatting inside the editor. A token is for an agent you start yourself.

## When it stops working

A token stops at its expiry, when revoked (trash button in Sharing), when the project changes owner, or when the owner's account is blocked or erased. Signing out does not stop it. A 401 means one of these, so ask the owner for a new token.
