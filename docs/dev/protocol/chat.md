# Assistant channel protocol

The assistant channel is a live relay between one LibrePaper browser and one
local runner. It is not a server queue or transcript store. The runner owns
task execution, durable task state, and reconnect recovery.

## Create and connect

- An authenticated reader creates a channel with
  `POST /api/documents/{slug}/chat`. The response is `{ "id": ID,
  "token": TOKEN, "ephemeral": true }`.
- Delete it with `DELETE /api/documents/{slug}/chat/{id}` and the
  `X-LibrePaper-Chat-Token` header. It expires after one hour with both peers
  disconnected. Creation is limited to 16 live channels per document and
  10,000 total.
- Both peers connect to
  `/api/documents/{slug}/chat/{id}/socket`, pass document authentication and
  same-origin checks, then send a `join` frame within ten seconds:

  ```json
  { "type": "join", "token": "TOKEN", "role": "user" }
  ```

- Roles are `user` (browser) and `agent` (runner), with one socket per role.
  The runner also sends a 48-character hexadecimal `binding_nonce`; the
  channel binds it on first join and rejects a different binding on reconnect.
- A successful join receives `ready`; both peers receive `presence` on
  connect or disconnect. The agent's `ready` includes `execution_epoch`.
  The server renews the runner's document/conversation lease every ten seconds;
  the lease expires after sixty seconds without renewal. A replaced or
  detached runner is fenced from further calls under its old epoch. Ordinary
  `presence` frames do not replace the epoch.
- Events are rejected while the recipient is absent and are never replayed on
  reconnect. The local runner reconciles its own task state.

## Relay contract

- Every event has an `id` of at most 128 bytes. Frames are limited to 64 KiB;
  message and answer text to 32 KiB; context to 16 KiB. A channel accepts at
  most 600 new events per rolling minute.
- Successful delivery returns `{ "type": "ack", "id": ID }`. Within the
  current connection pair, retrying the same ID and content is acknowledged
  without redelivery. Reusing an ID for different content returns `409`.
  Relay deduplication is capped at 256 entries and cleared when either peer
  detaches.
- A relay acknowledgement confirms delivery, not task admission or completion.
  The runner persists up to 4,096 compact task admissions and 256 full task
  records for its conversation. If that admission ledger is missing or
  incompatible, recovery is read-only and new work requires a new channel.

## Events

| Event | Direction | Contract |
|---|---|---|
| `message` | either peer | Requires `id`, non-empty `text`, optional `task` and `context`. Task kind is one of `proofread`, `tighten`, `rewrite`, `explain`, `outline`, `respond`, `fix`, `refine`; scope is `selection`, `file`, or `document`. |
| `answer` | agent → browser | Full answer snapshot with `task_id`, positive safe-integer `seq`, `text`, and `truncated`. Browser applies only newer snapshots. |
| `task` | agent → browser | Lifecycle status: `queued`, `working`, `needs_input`, `completed`, `failed`, `cancelled`, or `interrupted`; optional text and context carry detail/results. |
| `cancel` | browser → agent | Requests cancellation by `task_id`. The runner reports the actual outcome; cancellation does not undo accepted document edits. |
| `input` | browser → agent | Responds to a pending input with matching `task_id` and `request_id`, an offered option, or `{ "cancelled": true }`. |
| `capabilities` | agent → browser | Boolean flags describing supported controls. |
| `options` | agent → browser | Current runner options. |
| `set_option` | browser → agent | Sets an option by `option` and `value`. |
| `preview_request` | agent → browser | Requests candidate rendering by identity and revision; source is fetched separately. |
| `preview_result` | browser → agent | Returns result for a known preview request, with matching request/task and revision IDs and bounded diagnostics. |

Permission input is represented in task context with `request_id`,
`kind: "permission"`, a message, and offered options. The browser responds
with an offered option ID; the runner accepts only the current pending request
and binds the answer to both task and request IDs. Up to eight overlapping
permissions are queued in arrival order:

```json
{"type":"input","id":"input-1","task_id":"task-1",
 "request_id":"permission-1","response":{"option":"allow_once"}}
```

Task answers are snapshots: a stable `task_id` identifies the task, while
`seq` orders persisted revisions. The browser replaces an answer only with a
newer sequence. Completed assistant messages carry `context.task_id` so the
browser can upsert instead of duplicating transcript entries. A truncated
answer has an explicit notice and `truncated: true`.

Completed task results may include `effects` with `confirmed`, `refused`, and
`unresolved` operations. Lost MCP responses are reconciled with
`document_result` using the original operation identity. That lookup reports
retained evidence or unknown; it never reruns a mutation. For batch calls,
the aggregate `committed` status does not determine each item's outcome.

## Candidate preview

The relay carries candidate identity, not source bytes. The browser fetches
the manifest and each source from authenticated same-origin endpoints:

```text
GET /api/documents/{slug}/agent/candidates/{candidate_id}
GET /api/documents/{slug}/agent/candidates/{candidate_id}/source?path=paper.typ
```

Private candidates use the short-lived `X-LibrePaper-Candidate-Token` header;
the token is not put in a URL. The browser pins the fetched tree and renders
only when its canonical digest matches `revision`. The live editor need not
still match `base_revision`; preview does not mutate the shared document.
Missing candidate sources or assets produce a verification refusal.

The runner's preview request carries `candidate_id`, `candidate_token`,
`task_id`, `base_revision`, and `revision`; the browser's `preview_result`
echoes the request ID and those revisions with `ok` and `diagnostics`.

Only the agent may send `task`, `answer`, `capabilities`, `options`, and
`preview_request`. Only the browser may send `cancel`, `input`, `set_option`,
and `preview_result`. `message` may travel in either direction. The server
rejects wrong-role events, invalid or oversized values, and delivery without
a connected recipient.

Document edits, comments, and suggestion decisions continue through their
ordinary document operations. The assistant channel has no HTTP message-post,
polling, listen, cursor, or transcript endpoints.

The runner's MCP adapter sends `X-LibrePaper-Runner-Conversation` and
`X-LibrePaper-Execution-Epoch` with leased calls. Mutation commits recheck the
epoch; recovery uses the epoch captured when the effect was prepared.

Implementation: [chat/mod.rs](../../../crates/librepaper-server/src/server/chat/mod.rs),
[chat/hub.rs](../../../crates/librepaper-server/src/server/chat/hub.rs),
[assistant/transport.rs](../../../crates/librepaper-companion/src/assistant/transport.rs),
[assistant/task.rs](../../../crates/librepaper-companion/src/assistant/task.rs),
[agent-client.svelte.js](../../../web/src/lib/agent-client.svelte.js).
