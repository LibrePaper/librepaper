# Acting on a project with an agent token

An **agent token** gives one agent access to one project on one server, at the role the owner picked: Read, Comment, Suggest or Edit.

## Start here if the user pasted a token

The project lives on the LibrePaper server, and the token names the server and the project. You do not need a local folder, a clone or a project path, so never ask for one. Do not open or control a browser unless the user explicitly asks you to. Do this now, before you reply:

1. If LibrePaper tools are available, use them directly, following [The tools directly](#the-tools-directly). They may come from `librepaper mcp` (`mcp__librepaper__*`) or from a connector, where the names are such as `document_read`, possibly with a prefix. As with `librepaper mcp`, the operation identity is added for you on writes.
2. Otherwise set `H` to the helper and run `connect`, as [Step 1b](#step-1b-http-helper) shows. Then run the commands exactly as `connect` prints them.
3. Tell the user which project you reached, and ask what they want done. If they already said, do it.

If a step is blocked, say exactly what to allow, as [If a sandbox blocks you](#if-a-sandbox-blocks-you) explains.

If the user gives a project link instead of a token, and is not the owner, the helper cannot use it: `scripts/lp` works only with an `lpt_` token. Tell them the two options. Either they run `librepaper login` and register `librepaper mcp --link '<share link>'` with their client, then start a new session, or they ask the owner for a token. [agent-setup.md](agent-setup.md) has the steps.

## Doing the task

For reading, reviewing, proofreading, adding text and new files, use the helper's task commands. Each one reads a fresh copy of the source, so you never handle `view_id`, `range_id` or `operation` with them. Do not read schemas or [agent-tools.md](agent-tools.md) for these. Go straight to the write after `read`.

In the examples below, `lp` stands for `python3 "$H"`, with `H` set as Step 1b shows. `connect` prints each command ready to run, with its `--project <slug>`; use that form.

```sh
lp read main.tex --lines 1-80
lp comment main.tex --on 'exact text' --body 'Your comment'
lp suggest main.tex --find 'exact old text' --replace 'new text'
lp insert main.tex --after 'exact anchor text' --text 'new text'
lp create notes/summary.md --text 'file text'
lp rename notes/summary.md notes/overview.md
lp withdraw 3f9c… 7a1e…
```

| Request | Command | Role needed |
|---|---|---|
| Explain, summarize, outline, answer a question | `read`, then answer in chat | Read |
| Review | `comment` | Comment |
| See the comments already there | `call document_read` with a `thread` query, shown below | Read |
| Check that a file builds, read its errors | `call document_compile`, see the next section | Read |
| Proofread, tighten, rewrite | `suggest` (tracked change) | Suggest |
| Add text | `insert` (tracked insertion) | Suggest |
| New file, when the user asks for it | `create` (untracked) | Edit |

- Use `lp read <path> [--lines A-B]` to get exact source. Then do the write.
- To see existing comments, run `lp call document_read '{"queries":[{"kind":"thread","include":"unresolved"}]}'`. Each comment comes back with its `anchor`: the file, the passage as it reads now, and a `range_id` for it. Use `"include":"all"` to see resolved comments too, and `{"kind":"thread","id":"<comment id>"}` for one thread with its replies. Answering or resolving a comment goes through [The tools directly](#the-tools-directly).
- The text given to `--on`, `--find`, `--after` or `--before` must match the source exactly and appear once. If it does not, the helper says so, lists the lines and changes nothing. Retry with a longer excerpt.
- `insert` uses the text verbatim, so include any blank lines the text needs.
- For long text, pass `--text-file -`, `--replace-file -` or `--body-file -` to read it from stdin, or give a path.
- `suggest` and `insert` are tracked changes. You cannot accept tracked changes or reject anyone else's; that is the editor's act. You can withdraw your own pending changes with `lp withdraw <change_id> ...` (or `document_withdraw`), using the ids that `suggest` or `insert` returned in `effects`. A request to proofread or rewrite is not permission for untracked edits.
- Keep each change minimal and inside what the user asked. Preserve math, references, code and markup. Do not rewrite passages nobody asked about.
- When updating an existing document, propose targeted patches over the passages that change. Never replace a whole file or section to change part of it; keep the rest of the author's text.
- Keep the change ids that each write returns. If a pass went wrong, withdraw it with those ids before proposing again. Do not try to "restore" text by proposing the old text over your own pending change.
- With Comment access you cannot change text. With Suggest access you can only propose tracked changes. Say so and suggest the owner issue a Suggest token, or an Edit token for direct edits.
- If several projects are connected, a command without `--project` is refused and the helper lists them. Pick the project the user means and add `--project <slug>`. The helper never guesses.
- A write that fails with a network error makes the helper print the `lp result` command to check it. Run that. Never resend the write.
- For replies, resolving threads, direct edits and compile validation, use [The tools directly](#the-tools-directly).

## Builds, compile errors and running code

- To see whether a file builds, or to read its errors, compile it: `lp call document_compile '{"path":"main.tex"}'`. Every role can. Nothing compiles on the server: the owner's open editor tab does the work, and the answer has a `status`, the `diagnostics` (each with a `file` and a `line`) and the end of the compiler log. The fields, the statuses and what to do with a `pending` answer are in [agent-tools.md](agent-tools.md).
- `renderer_unavailable` from `document_compile` means the owner has no tab open on the project. Ask the user to open it in a browser signed in as its owner, then compile again. If they cannot, ask them to paste the error from the editor's build log, or read the source and reason about the error. Say which of the two you did.
- `validation: "compile"` on a proposal and the render queries are a different route. They use the in-app Agent panel's own renderer, so an agent connected by token or connector gets `renderer_unavailable` for them whatever is open. Do not create no-op candidates to probe for errors. Make the change, then compile the file with `document_compile`.
- In Quarto and Calepin projects, code runs on the user's computer, in their Run directory, not on the server. The project holds the `.qmd`, `.bib`, images and `_quarto.yml`. Code cannot read project files, so data and scripts stay in the Run directory. Never upload data, scripts, prompts or other files that code reads to fix a "file not found" or "cannot open the connection" error; it will not help. Ask the user for their Run directory path.
- `document_compile` with `"run": true` runs the code, and only when all of these hold: an Edit token, the desktop app, a Run directory, and the owner's "Let agents run code" setting for the project. A token below Edit is refused with `forbidden`, and when one of the others is missing the answer is `run_unavailable`. Nothing else a token does runs code. If you have a shell, Quarto and the user's Run directory, you can also render a local copy yourself: read the files with the helper, write them to a scratch folder, and run `quarto render <file>.qmd --execute-dir <run directory>`. Report the results. Never write into the Run directory without asking.
- For the full model, see `docs/notebooks/quarto.md`.

## The token

- Format: `lpt_<base64url origin>.<slug>.<key>`. Split at the first and last dot. The first part decodes to the server origin (for example `https://app.librepaper.org`). The slug is the project in that server's address bar.
- The owner makes one in the project's Agent panel (robot icon in the sidebar), Connect tab, New token. The owner can copy it again from the token's row: the copy icon there copies connection instructions that include the token.
- For its own project, a token works on the document tools (`GET` and `POST /api/documents/<slug>/tools[/<name>]`) and on the remote MCP endpoint `/mcp/<slug>`. An Edit token also reaches the sync routes that `librepaper pull` and `push` use, under `/api/documents/<slug>`: `GET /history`, `GET /history/<id>`, `GET /project`, `PUT|POST /assets`, `GET|HEAD /assets/<name>` and `POST /source-change`. So an Edit token can upload files through `librepaper push`. No other route accepts it.
- The document tools cannot upload images, publish, share or run code. Comments carry the token's name. Each token has an hourly comment budget (`budget_exceeded` when spent). Edit tokens expire after one day by default.
- It is a credential. Never print it whole, write it into a repo, a config file or a commit, or send it anywhere but its own origin. When echoing it, show `lpt_…` plus the last four characters. A user who pastes it into chat has made their own decision about that; it does not relax this rule for you.

## Step 1: Pick the connection

Check whether `mcp__librepaper__document_read` (or other `mcp__librepaper__*` tools) is available.

- **Tools present.** Use them. `librepaper mcp` adds the `operation` key to every write for you.
- **Tools absent or the server failed to connect, and the user pasted a token.** Use the HTTP helper (Step 1b). It works at once with no restart.
- **Neither.** Tell the user how to set up MCP ([agent-setup.md](agent-setup.md)) and ask for the token.

The MCP server reads `LIBREPAPER_AGENT_TOKEN` from its environment when the client starts it. A token pasted into a running session cannot reach an MCP server that is already running, so do not tell the user to "just restart the tool". Either use the HTTP helper now or have them relaunch the client with the variable set.

## Step 1b: HTTP helper

The helper is `scripts/lp` in this skill's directory. It needs only Python 3, and you run it with `python3`, not by file mode.

When the user pastes the token in chat, connect once:

```sh
H=<this skill's directory>/scripts/lp
printf %s 'lpt_...' | python3 "$H" connect --model '<your model name>'
```

`connect` verifies the token with the server first, stores it privately in its own per-project file, and prints the project, the access level, the files and ready-to-run commands, each with its own `--project <slug>`. Afterwards refer only to the helper's commands, never to the literal token.

- Tokens live in `~/.cache/librepaper-agent/`, or in a private temp folder if that is not writable. The helper chooses this automatically.
- `LIBREPAPER_AGENT_TOKEN` (the token itself) and `LIBREPAPER_AGENT_TOKEN_FILE` (a file holding it) still override the stored token.
- `lp disconnect` deletes the stored token.
- If the helper prints that this skill is older than the server needs, pass that message and its update command on to the user. A skill running from a temporary folder is not installed, so the user needs `npx skills add LibrePaper/librepaper --skill librepaper --global` instead.
- `--model` names the model you are running as, such as `Claude Opus 5.5`, and the project's Authors list shows it for your edits. Use your real model name. Leave it out if you do not know it. Connect again with a new name if your model changes, or set `LIBREPAPER_AGENT_LABEL` to override it.

### If a sandbox blocks you

- The helper stores tokens in `~/.cache/librepaper-agent/`, which some sandboxes make read-only (Codex's default workspace-write mode does). It then falls back to a private temp folder by itself, so no manual token file is needed. Never store the token in your working directory or in a Git repository.
- A request that fails with a network error is most likely the sandbox. A read can be retried once access is allowed. Treat a failed write as having an unknown outcome, even if you suspect the sandbox: do not resend it, and ask `document_result` as in Step 4.
- Tell the user exactly what to allow. For Codex: approve running the command outside the sandbox when asked, or start Codex with `codex -c sandbox_workspace_write.network_access=true`. The alternative is the MCP route in [agent-setup.md](agent-setup.md), which needs a new session.
- For Claude Code: approval prompts are normal for shell calls. The user can allow the helper command for the session.
- In claude.ai and Claude Desktop chats, the code sandbox reaches only allowed domains. Either the user adds the server's domain under Settings → Capabilities (on Team or Enterprise, an admin does this under Organization settings; the Free plan cannot), or, better, the owner connects the project as a connector (in Claude Code, `claude mcp add --transport http librepaper <URL>`). See [agent-setup.md](agent-setup.md).

## The tools directly

You need this only for replies, resolving threads, direct edits, compile validation, or with MCP tools. The helper's task commands cover everything else. Use `python3 "$H" call <tool> '<json>'` for reads and `python3 "$H" write <tool> '<json>'` for writes; `write` adds the `operation` object. `python3 "$H" tools <name>` prints one tool's schema. With several projects connected, add `--project <slug>` here too.

### Step 2: Read once, broadly

Start with one `document_read` that combines queries: `{"kind":"manifest"}` to list files, then `context`, `source` or `outline` for the file in scope. Every read returns a `view_id`, `range_id` handles, effective permissions, limits and `operation_epoch`.

- `context`, `source` and `section` need `path`. There is no main file, so read the manifest when unsure.
- To see the comments already on the project, add `{"kind":"thread","include":"unresolved"}` to the same read. Each one carries its `anchor`, with a `range_id` you can reply or propose on.
- Follow `next_cursor` against the same `view_id` until done. Never call a partial read a review of the whole document.
- Reuse handles. Never rebuild source text or compute hashes yourself.
- Query kinds and fields are in [agent-tools.md](agent-tools.md).

### Step 3: Act within the role and the request

| Request | What to do | Role needed |
|---|---|---|
| Explain, summarize, outline | Answer in chat; no document effect | Read |
| Review, respond to a thread | `document_comment` create or reply on a `range_id` | Comment |
| Proofread, tighten, rewrite | `document_propose` with `publish: "suggestions"`: tracked changes an editor accepts or rejects | Suggest |
| Direct edit, no tracking | `document_propose` with `publish: "private"`, then `document_apply` on the candidate | Edit, and the user said "edit directly" |
| New, renamed or deleted text file | `document_files` (untracked). Rename (`lp rename`) rather than delete and create, so comments stay attached | Edit, and the user asked for it |
| Check that a file compiles, read its errors | `document_compile` on the file path; read `diagnostics` (see [agent-tools.md](agent-tools.md)) | Read |

- A request to proofread or rewrite is not permission for untracked edits. Default to tracked suggestions.
- With Comment access you cannot change text. With Suggest access you can only propose tracked changes. Say so and suggest the owner issue a Suggest token, or an Edit token for direct edits.
- You cannot accept tracked changes or reject anyone else's; that is the editor's act. You can withdraw your own pending changes with `document_withdraw` (or `lp withdraw`).
- Keep replacements minimal and inside the asked scope. Preserve math, references, code and markup.
- When updating an existing document, propose targeted patches over the passages that change. Never replace a whole file or section to change part of it; keep the rest of the author's text.
- To add a reference by DOI, call `reference_from_doi` (served only by `librepaper mcp`, answered on the user's machine) and append its `bibtex` to the `.bib` with `document_propose`. Never write reference metadata yourself.
- Keep the change ids that `document_propose` returns in `effects`. If a pass went wrong, withdraw it with those ids before proposing again. Do not try to "restore" text by proposing the old text over your own pending change.
- Use `validation: "compile"` when the change must still build, and only claim it compiles from a matching render result. That validation needs the in-app Agent panel. With a token or connector, compile the file with `document_compile` after the change instead.

### Step 4: Writes and recovery

- Every write carries `operation: {epoch, id}`. `epoch` is `operation_epoch` from the latest read, copied verbatim. `id` is `v2.<unix ms>.<32 lowercase hex>`, new per write, reused only for that same write.
- On an expired epoch, read again and use the new one.
- **Never resend a write whose outcome is unknown** (timeout, dropped connection). Ask instead: `document_result` with `{"kind":"operation","target_operation":<the same operation object>}`. Report it as unknown if that does not settle it.
- On a conflict, read fresh source and re-target the intended passage. Do not weaken `consistency` to force it through.
- For existing comments, pass the returned `comment_version` as `expected_version`.

## Step 5: Report

End with a short plain-prose answer: what was read, what changed (only effects the server confirmed), and anything incomplete, refused, conflicting or unknown.

Then offer, when they apply:

- Run `lp disconnect` to delete the stored token.

## Source of truth

The live tool list (`python3 "$H" tools` or MCP `tools/list`) wins over this file. In the repository, the schemas are `crates/librepaper-server/src/server/mcp/tools.json`, and the user documentation is `docs/agents.md`.
