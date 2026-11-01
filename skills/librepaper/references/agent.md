# Acting on a project with an agent token

An **agent token** gives one agent access to one project on one server, at the role the owner picked: Read, Comment or Edit.

## The token

- Format: `lpt_<base64url origin>.<slug>.<key>`. Split at the first and last dot. The first part decodes to the server origin (for example `https://app.librepaper.org`). The slug is the project in that server's address bar.
- The owner makes one in the project: Sharing, Agent tokens tab, New. It is shown once.
- It works only on two routes for its own project: `GET` and `POST /api/documents/<slug>/tools[/<name>]`. Nothing else accepts it.
- It cannot upload images, publish, share or run code. Comments carry the token's name. Each token has an hourly comment budget (`budget_exceeded` when spent). Edit tokens expire after one day by default.
- It is a credential. Never print it whole, write it into a repo, a config file or a commit, or send it anywhere but its own origin. When echoing it, show `lpt_…` plus the last four characters.

## Step 1: Pick the connection

Check whether `mcp__librepaper__document_read` (or other `mcp__librepaper__*` tools) is available.

- **Tools present.** Use them (Step 2). `librepaper mcp` adds the `operation` key to every write for you.
- **Tools absent or the server failed to connect, and the user pasted a token.** Use the HTTP helper (Step 1b). It works at once with no restart.
- **Neither.** Tell the user how to set up MCP ([agent-setup.md](agent-setup.md)) and ask for the token.

The MCP server reads `LIBREPAPER_AGENT_TOKEN` from its environment when the client starts it. A token pasted into a running session cannot reach an MCP server that is already running, so do not tell the user to "just restart the tool". Either use the HTTP helper now or have them relaunch the client with the variable set.

## Step 1b: HTTP helper

The helper is `scripts/lp` in this skill's directory. It needs only Python 3.

When the user pastes the token in chat, write it once to a private file and from then on refer only to the file, never to the literal token:

```sh
# Store the token readable by you only
mkdir -p ~/.cache && (umask 077 && printf %s 'lpt_...' > ~/.cache/librepaper-agent-token)
```

```sh
H=<this skill's directory>/scripts/lp
$H whoami      # origin and slug, token redacted
$H tools       # tools this token may call
$H call document_read '{"queries":[{"kind":"context","path":"main.tex"},{"kind":"manifest"}]}'
$H write document_comment '{"action":"create","view_id":"...","range_id":"...","body":"..."}'
```

- `write` adds the `operation` object: a fresh id, and the epoch from the last `document_read` this helper saw.
- The token file defaults to `~/.cache/librepaper-agent-token`. `LIBREPAPER_AGENT_TOKEN` in the environment wins over it.
- When the work is done, offer to delete the file.

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

## Source of truth

The live tool list (`$H tools` or MCP `tools/list`) wins over this file. In the repository, the schemas are `crates/librepaper-server/src/server/mcp/tools.json`, the rules the desktop app hands its agents are `crates/librepaper-companion/src/assistant/writing.md`, and the user documentation is `docs/agents.md`.
