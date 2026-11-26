# Tool reference

The helper's task commands (`read`, `comment`, `suggest`, `insert`, `create`) cover ordinary edits and comments. This reference is for everything else.

Summarized from `crates/librepaper-server/src/server/mcp/tools.json`. The live list is authoritative and shows only what the token's role allows. Fetch it with `scripts/lp tools` (it uses the full schemas; `scripts/lp tools <name>` prints one tool), or with `GET https://<origin>/api/documents/<slug>/tools?schema=full` and the bearer token. Without `?schema=full` the response is the bridge's version, which omits `operation`.

`document_id` appears on every tool: omit it. The endpoint serves one project.

## operation (every write)

```json
{"epoch": "<operation_epoch from the latest document_read, verbatim>",
 "id": "v2.<unix ms>.<32 lowercase hex>"}
```

`authorization_epoch` is a different value and is refused. The epoch is signed and time limited; read again for a fresh one. Reuse an `id` only to retry that exact write after `document_result` says it did not land.

## document_read

Read bounded exact source, search, outline, bibliography, comments and context. Required: `queries`.

- `queries[]`: one to eight entries. Each has `kind`, one of
  - `manifest`: list the project's files.
  - `context`: permissions, limits, the `view_id`, and surroundings for a `path` or `selection`.
  - `source`: exact text of `path`, bounded by `line`/`end_line` or `start`/`end` (character offsets). Returns `range_id` handles.
  - `section`: one section of `path` (by `id` or `key` from the outline).
  - `outline`: headings of a file, for large files.
  - `search`: `query` text across the project.
  - `bibliography`: entries; `key` for one.
  - `thread`: comments; `id` for one thread with its replies, `include: "all" | "unresolved"` (default `all`). Each comment on a passage carries an `anchor`: `path`, `exact` (the passage as it reads now), `prefix` and `suffix` (up to 32 characters either side), `status` (`exact`, `modified`, `pending_deletion`, `deleted`, `unresolved` or `stale`) and a `range_id` for that passage in this read's view, usable in `document_comment` and `document_propose`. When the passage cannot be placed, `path`, `exact` and `range_id` are absent and `original_exact` holds the words the comment was made on. The anchor is the whole account of where a comment sits; the browser's own records of the passage are not sent with it.
  - `diagnostics`: build errors and warnings.
  - `changes`: pending tracked changes.
  - `rendered`: rendered output.
- Other query fields: `path`, `selection` (string handle or `{path, exact, prefix, suffix, position}`), `comment_id` (with `source`, `context` or `section`: the passage that comment is on, in place of `path` and a range), `context: "none" | "paragraph"`, `limit` (1 to 100), `cursor` (from `next_cursor`), `revision`, `range_id`.
- `snapshot`: `"latest"` or `{"view_id": "..."}` to keep reading the same view (use it with cursors).
- `budget`: `{max_bytes: 1024..49152, max_tokens: 256..12288}`.

The response carries `view_id`, `operation_epoch`, range handles and, for long results, `next_cursor`.

## document_compile

Ask the project owner's open browser tab to compile a file with the app's own engines, then return the status, diagnostics and log. Nothing compiles on the server. The tab must be open, because the owner's session does the work. Every role can call it. It takes no `operation`.

When a tab answers `run_unavailable`, `run_disabled` or `busy`, the server tries the owner's next open tab, newest first, before reporting.

- `path` (required): the file to compile, for example `paper.tex`, `index.qmd` or `main.typ`.
- `output`: `"pdf"` or `"html"`. Omitted, the tab uses its own setting for that format.
- `run`: default `false`. `true` runs the code chunks (Quarto and Typst). It needs an Editor token, the desktop app, a Run directory, and the owner's "Let agents run code" setting for the project. Otherwise the result is `run_unavailable`. A token without the Editor role is refused with `forbidden` before anything is sent.
- `wait_ms`: 0 to 25000, default 20000. How long the call waits inline.

The result has `compile_id`, `status`, `path`, `output` (`"pdf"`, `"html"` or `"none"`), `revision` (the tree the tab compiled), `current`, `diagnostics`, `log` (the last 16 KiB of the compiler log) and `provenance`. Each diagnostic has `severity`, `file`, `line`, `column`, `message` and `source`. `current` is `true` when `revision` is the tree the server holds for this call. If it is `false`, the tab compiled a different tree, so compile again before trusting the result.

Statuses:

- `succeeded`: the file compiled. `output` says what was made.
- `failed`: the compile ran and failed. Read `diagnostics` and `log`.
- `unavailable`: nothing compiled. `error` gives the reason (`run_unavailable`, `run_disabled`, `busy`, `no_renderer`, `failed`, `timeout`) and `message` says what to do.
- `pending`: the inline wait ran out. Fetch the same object later with `document_result` `{"kind": "compile", "id": "<compile_id>", "wait_ms": N}`.

If no eligible tab is connected, the call fails with the tool error `renderer_unavailable`, not with a result. Tell the user to open the project in a browser signed in as its owner, then try again.

Workflow: edit the file, call `document_compile` on it, read the diagnostics (each has a `file` and `line`), fix those lines with a new edit, and compile again. Stop when the status is `succeeded` with no errors.

## document_comment

Create or reply to comments, resolve threads, label versions. Required: `action`, `operation`.

- `action`: `create` (needs `view_id`, `range_id`, `body`), `reply` (`comment_id`, `body`), `resolve` (`comment_id`, `resolved`), `delete` (`comment_id`), `label` (name a version; Edit role only, needs `view_id`).
- Acting on an existing comment: pass its `comment_version` as `expected_version`.
- `body` up to 16000 characters. Counts against the token's hourly comment budget.

## document_propose

Propose replacements for captured ranges. Required: `view_id`, `operation`, `patches`.

- `patches[]`: `{range_id, replacement, find?}`, 1 to 100. `replacement` is at most 60000 characters. `find` is at most 8192 characters and must be a unique exact substring within the range. A larger change should be split into several patches.
- `publish`: `"suggestions"` lands at once as pending tracked changes for an editor to accept or reject (Suggest role or higher). `"private"` keeps a candidate without changing the document.
- `batch`: `"atomic"` or `"independent"`. Independent only with `publish: "private"`. A published batch is all-or-nothing.
- `validation`: `"source"` or `"compile"` (with `path` of the file to render).
- `dependencies`: extra `range_id`s the patch relies on.

## document_withdraw

Withdraw your own pending tracked changes. Suggest role or higher. Required: `operation`, `change_ids`.

- `change_ids[]`: the ids that `document_propose` returned in `effects` as `[{"kind":"change","id":"..."}]`.
- Only your own pending changes. Changes by others, or already accepted or rejected, are refused.
- Result: `effects: [{"kind":"withdrawn","id":...}]`. CLI: `lp withdraw <change_id> [<change_id> ...]`.

## document_apply

Apply a private candidate as a direct, untracked edit. Only when the user explicitly asked for direct edits and the role is Edit. Required: `candidate_id`, `operation`. Optional `view_id`, `consistency: "exact_tree" | "unchanged_dependencies"` (default exact tree; never loosen it to dodge a conflict). Refuses changed source.

## document_files

Create, rename or delete text files, untracked, Edit role. Required: `operation`, plus at least one of:

- `create[]`: `{path, text}`, up to 100 entries. `text` is at most 262144 characters. For a whole new file, use this tool rather than a propose patch.
- `rename[]`: `{from, to}`, up to 100 entries. The file keeps its identity, so its comments and tracked changes stay attached. Rename a file rather than deleting it and creating another. `to` must be free, and a renamed path cannot also appear in another operation of the same call.
- `delete[]`: `{path}`, up to 100 entries.

Optional `base_tree`, `consistency`. A created or renamed file shows up only in a later `document_read`. Binary files cannot be added.

## document_result

Look up a retained result. No required fields.

- `{"kind": "operation", "target_operation": {epoch, id}}`: did that write land?
- `{"kind": "candidate", "id": "..."}`: a private candidate.
- `{"kind": "render", "id": "...", "wait_ms": 0..10000}`: compile result for a candidate.
- `{"kind": "compile", "id": "compile_...", "wait_ms": 0..10000}`: the result of a `document_compile` that returned `pending`. Results are kept for an hour.

## Errors worth knowing

- HTTP 401: token expired, revoked, wrong project, or owner changed. Ask for a new one.
- `budget_exceeded`: hourly comment limit spent. Stop commenting; tell the user.
- Stale epoch: read again.
- Conflict on propose or apply: source moved. Read fresh and re-target.
- `renderer_unavailable` from `document_compile`: no browser tab of the owner is open. Ask the user to open the project signed in as its owner.
