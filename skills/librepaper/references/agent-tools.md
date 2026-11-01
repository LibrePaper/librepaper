# Tool reference

Summarized from `crates/librepaper-server/src/server/mcp/tools.json`. The live list (`scripts/lp tools` or MCP `tools/list`) is authoritative and shows only what the token's role allows.

`document_id` appears on every tool: omit it. The endpoint serves one project.

## operation (every write)

```json
{"epoch": "<operation_epoch from the latest document_read, verbatim>",
 "id": "v2.<unix ms>.<32 lowercase hex>"}
```

`authorization_epoch` is a different value and is refused. The epoch is signed and time limited; read again for a fresh one. Reuse an `id` only to retry that exact write after `document_result` says it did not land.

## document_read

Read bounded exact source, search, outline, bibliography, comments and context. Required: `queries`.

- `queries[]`: each has `kind`, one of
  - `manifest`: list the project's files.
  - `context`: permissions, limits, the `view_id`, and surroundings for a `path` or `selection`.
  - `source`: exact text of `path`, bounded by `line`/`end_line` or `start`/`end` (character offsets). Returns `range_id` handles.
  - `section`: one section of `path` (by `id` or `key` from the outline).
  - `outline`: headings of a file, for large files.
  - `search`: `query` text across the project.
  - `bibliography`: entries; `key` for one.
  - `thread`: comments; `comment_id` for one thread, `include: "all" | "unresolved"`.
  - `diagnostics`: build errors and warnings.
  - `changes`: pending tracked changes.
  - `rendered`: rendered output.
- Other query fields: `path`, `selection` (string handle or `{path, exact, prefix, suffix, position}`), `context: "none" | "paragraph"`, `limit` (1 to 100), `cursor` (from `next_cursor`), `revision`, `range_id`.
- `snapshot`: `"latest"` or `{"view_id": "..."}` to keep reading the same view (use it with cursors).
- `budget`: `{max_bytes: 1024..49152, max_tokens: 256..12288}`.

The response carries `view_id`, `operation_epoch`, range handles and, for long results, `next_cursor`.

## document_comment

Create or reply to comments, resolve threads, label versions. Required: `action`, `operation`.

- `action`: `create` (needs `view_id`, `range_id`, `body`), `reply` (`comment_id`, `body`), `resolve` (`comment_id`, `resolved`), `delete` (`comment_id`), `label` (name a version; Edit role only, needs `view_id`).
- Acting on an existing comment: pass its `comment_version` as `expected_version`.
- `body` up to 16000 characters. Counts against the token's hourly comment budget.

## document_propose

Propose replacements for captured ranges. Required: `view_id`, `operation`, `patches`.

- `patches[]`: `{range_id, replacement, find?}`, 1 to 100. `find` must be a unique exact substring within the range.
- `publish`: `"suggestions"` lands at once as pending tracked changes for an editor to accept or reject (Edit role). `"private"` keeps a candidate without changing the document.
- `batch`: `"atomic"` or `"independent"`. Independent only with `publish: "private"`. A published batch is all-or-nothing.
- `validation`: `"source"` or `"compile"` (with `path` of the file to render).
- `dependencies`: extra `range_id`s the patch relies on.

## document_apply

Apply a private candidate as a direct, untracked edit. Only when the user explicitly asked for direct edits and the role is Edit. Required: `candidate_id`, `operation`. Optional `view_id`, `consistency: "exact_tree" | "unchanged_dependencies"` (default exact tree; never loosen it to dodge a conflict). Refuses changed source.

## document_files

Create or delete text files, untracked, Edit role. Required: `operation`, plus at least one of:

- `create[]`: `{path, text}` (text up to 262144 characters), up to 100.
- `delete[]`: `{path}`, up to 100.

Optional `base_tree`, `consistency`. A created file shows up only in a later `document_read`. Binary files cannot be added.

## document_result

Look up a retained result. No required fields.

- `{"kind": "operation", "target_operation": {epoch, id}}`: did that write land?
- `{"kind": "candidate", "id": "..."}`: a private candidate.
- `{"kind": "render", "id": "...", "wait_ms": 0..10000}`: compile result for a candidate.

## Errors worth knowing

- HTTP 401: token expired, revoked, wrong project, or owner changed. Ask for a new one.
- `budget_exceeded`: hourly comment limit spent. Stop commenting; tell the user.
- Stale epoch: read again.
- Conflict on propose or apply: source moved. Read fresh and re-target.
