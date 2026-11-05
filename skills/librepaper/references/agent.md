# Acting on a project with an agent token

An **agent token** gives one agent access to one project on one server, at the role the owner picked: Read, Comment or Edit.

## The token

- Format: `lpt_<base64url origin>.<slug>.<key>`. Split at the first and last dot. The first part decodes to the server origin (for example `https://app.librepaper.org`). The slug is the project in that server's address bar.
- The owner makes one in the project: Share, Agents tab, New. It is shown once.
- It works only on two routes for its own project: `GET` and `POST /api/documents/<slug>/tools[/<name>]`. Nothing else accepts it.
- It cannot upload images, publish, share or run code. Comments carry the token's name. Each token has an hourly comment budget (`budget_exceeded` when spent). Edit tokens expire after one day by default.
- It is a credential. Never print it whole, write it into a repo, a config file or a commit, or send it anywhere but its own origin. When echoing it, show `lpt_…` plus the last four characters. A user who pastes it into chat has made their own decision about that; it does not relax this rule for you.

## Step 1: Pick the connection

A user may paste a block with a token into the chat. That is normal. It means use the HTTP helper now, with no restart.

Check whether `mcp__librepaper__document_read` (or other `mcp__librepaper__*` tools) is available.

- **Tools present.** Use them (Step 2). `librepaper mcp` adds the `operation` key to every write for you.
- **Tools absent or the server failed to connect, and the user pasted a token.** Use the HTTP helper (Step 1b). It works at once with no restart.
- **Neither.** Tell the user how to set up MCP ([agent-setup.md](agent-setup.md)) and ask for the token.

The MCP server reads `LIBREPAPER_AGENT_TOKEN` from its environment when the client starts it. A token pasted into a running session cannot reach an MCP server that is already running, so do not tell the user to "just restart the tool". Either use the HTTP helper now or have them relaunch the client with the variable set.

## Step 1b: HTTP helper

The helper is `scripts/lp` in this skill's directory. It needs only Python 3, and you run it with `python3`, not by file mode.

If you are reading this file from its raw URL and have no skill directory, download the helper into a private directory outside the project, then use that copy:

```sh
mkdir -p ~/.cache/librepaper && curl -fsSL -o ~/.cache/librepaper/lp https://raw.githubusercontent.com/LibrePaper/librepaper/main/skills/librepaper/scripts/lp
H=~/.cache/librepaper/lp
```

The other references are in the same place: `https://raw.githubusercontent.com/LibrePaper/librepaper/main/skills/librepaper/references/`. Fetch them as needed, for example [agent-tools.md](agent-tools.md).

When the user pastes the token in chat, write it once to a private file and from then on refer only to the file, never to the literal token:

```sh
# Store the token readable by you only
mkdir -p ~/.cache && (umask 077 && printf %s 'lpt_...' > ~/.cache/librepaper-agent-token)
```

```sh
H=<this skill's directory>/scripts/lp    # or the downloaded copy above
python3 "$H" whoami      # origin and slug, token redacted
python3 "$H" tools       # tools this token may call, with full schemas including operation
python3 "$H" call document_read '{"queries":[{"kind":"context","path":"main.tex"},{"kind":"manifest"}]}'
python3 "$H" write document_comment '{"action":"create","view_id":"...","range_id":"...","body":"..."}'
```

- `write` adds the `operation` object: a fresh id, and the epoch from the last `document_read` this helper saw.
- The token file defaults to `~/.cache/librepaper-agent-token`. `LIBREPAPER_AGENT_TOKEN` in the environment wins over it.

### If a sandbox blocks you

- The token file defaults to `~/.cache/librepaper-agent-token`, which some sandboxes make read-only (Codex's default workspace-write mode does). If the write fails, use a private file under the temp directory and point the helper at it: `export LIBREPAPER_AGENT_TOKEN_FILE="${TMPDIR:-/tmp}/librepaper-agent-token"`. Shell state may not persist between commands, so set it on each command. Never store the token inside the project folder.
- A request that fails with a network error before it reaches the server is most likely the sandbox. A read can be retried, and so can a write that never left the machine. A write with an unknown outcome is still not resent; follow Step 4.
- Tell the user exactly what to allow. For Codex: approve running the command outside the sandbox when asked, or start Codex with `codex -c sandbox_workspace_write.network_access=true`. The alternative is the MCP route in [agent-setup.md](agent-setup.md), which needs a new session.
- For Claude Code: approval prompts are normal for shell calls. The user can allow the helper command for the session.

## Step 2: Read once, broadly

Start with one `document_read` that combines queries: `{"kind":"manifest"}` to list files, then `context`, `source` or `outline` for the file in scope. Every read returns a `view_id`, `range_id` handles, effective permissions, limits and `operation_epoch`.

- `context`, `source` and `section` need `path`. There is no main file, so read the manifest when unsure.
- Follow `next_cursor` against the same `view_id` until done. Never call a partial read a review of the whole document.
- Reuse handles. Never rebuild source text or compute hashes yourself.
- Query kinds and fields are in [agent-tools.md](agent-tools.md).

## Step 3: Act within the role and the request

| Request | What to do | Role needed |
|---|---|---|
| Explain, summarize, outline | Answer in chat; no document effect | Read |
| Review, respond to a thread | `document_comment` create or reply on a `range_id` | Comment |
| Proofread, tighten, rewrite | `document_propose` with `publish: "suggestions"`: tracked changes an editor accepts or rejects | Edit |
| Direct edit, no tracking | `document_propose` with `publish: "private"`, then `document_apply` on the candidate | Edit, and the user said "edit directly" |
| New or deleted text file | `document_files` (untracked) | Edit, and the user asked for it |

- A request to proofread or rewrite is not permission for untracked edits. Default to tracked suggestions.
- With Comment access you cannot change text. Say so and suggest the owner issue an Edit token.
- You cannot accept or reject tracked changes. That is the editor's act.
- Keep replacements minimal and inside the asked scope. Preserve math, references, code and markup.
- Use `validation: "compile"` when the change must still build, and only claim it compiles from a matching render result.

## Step 4: Writes and recovery

- Every write carries `operation: {epoch, id}`. `epoch` is `operation_epoch` from the latest read, copied verbatim. `id` is `v2.<unix ms>.<32 lowercase hex>`, new per write, reused only for that same write.
- On an expired epoch, read again and use the new one.
- **Never resend a write whose outcome is unknown** (timeout, dropped connection). Ask instead: `document_result` with `{"kind":"operation","target_operation":<the same operation object>}`. Report it as unknown if that does not settle it.
- On a conflict, read fresh source and re-target the intended passage. Do not weaken `consistency` to force it through.
- For existing comments, pass the returned `comment_version` as `expected_version`.

## Step 5: Report

End with a short plain-prose answer: what was read, what changed (only effects the server confirmed), and anything incomplete, refused, conflicting or unknown.

Then offer, when they apply:

- Delete the token file (the path from Step 1b, or the one set in `LIBREPAPER_AGENT_TOKEN_FILE`).
- If the LibrePaper skill is not installed, install it from https://github.com/LibrePaper/librepaper/tree/main/skills/librepaper. An installed skill takes effect in the next session.

## Source of truth

The live tool list (`python3 "$H" tools` or MCP `tools/list`) wins over this file. In the repository, the schemas are `crates/librepaper-server/src/server/mcp/tools.json`, the rules the desktop app hands its agents are `crates/librepaper-companion/src/assistant/writing.md`, and the user documentation is `docs/agents.md`.
