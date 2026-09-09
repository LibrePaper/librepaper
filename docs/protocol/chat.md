# Assistant channel protocol

The assistant channel connects one LibrePaper browser to one local runner. It
is a live relay, never a server queue or transcript. The runner owns model
execution, local task queues, and reconciliation after reconnecting. The server
keeps only socket handles and a bounded set of event digests for duplicate
suppression.

## Create and connect

An authenticated reader creates a channel with:

```http
POST /api/documents/{slug}/chat
```

The response is `{ "id": ID, "token": TOKEN, "ephemeral": true }`. The token
is a capability for this channel and is never put in a URL. The channel expires
after one hour with both peers disconnected, or can be explicitly closed with:

```http
DELETE /api/documents/{slug}/chat/{id}
X-LibrePaper-Chat-Token: TOKEN
```

Both peers connect to `/api/documents/{slug}/chat/{id}/socket`, pass the usual
document authentication and same-origin checks, then send this first frame
within ten seconds:

```json
{ "type": "join", "token": "TOKEN", "role": "user" }
```

`role` is `user` for the browser and `agent` for the local runner. There is one
socket per role. A successful join receives `ready` and both connected peers
receive `presence` whenever either socket connects or disconnects:

```json
{ "type": "ready", "browser": true, "agent": true }
{ "type": "presence", "browser": true, "agent": false }
```

Closing a socket temporarily detaches that peer and leaves the channel alive
until expiry or explicit deletion. Events sent while the other peer is absent
are rejected; the local runner queues and reconciles work itself. A document
access revocation closes affected sockets. No event is replayed on reconnect.

## Events

Every event has an `id` (at most 128 bytes). The server relays accepted events
to both peers and returns `{ "type": "ack", "id": ID }` to the sender. A retry
with the same ID and identical content is acknowledged without another
delivery during the current connection. Detaching either peer clears relay
deduplication; the runner uses its persisted task IDs to prevent reexecution.
An acknowledgment confirms relay delivery, while a task event confirms local admission. Reusing an ID for different content returns `409`. Events are
bounded by the 64 KiB WebSocket frame limit; text is at most 32 KiB and context
is at most 16 KiB. A channel accepts at most 600 new events per rolling minute;
identical retries do not consume that budget.

The browser sends an assistant request as `message`:

```json
{
  "type": "message", "id": "request-1", "text": "Tighten this paragraph",
  "task": { "kind": "tighten", "scope": "selection" },
  "context": {
    "file": "paper.typ",
    "selection": { "path": "paper.typ", "exact": "...", "prefix": "...", "suffix": "...", "position": 120 },
    "revision": "rev-7"
  }
}
```

Supported task kinds are `proofread`, `tighten`, `rewrite`, `explain`,
`outline`, `respond`, `fix`, and `refine`. Supported scopes are `selection`,
`file`, and `document`. The server validates the vocabulary and forwards the
request without interpreting document content.

The runner sends assistant output using `message` with the same shape. It
reports task lifecycle separately:

```json
{
  "type": "task", "id": "event-2", "task_id": "request-1",
  "status": "working", "text": "Reading the selected section"
}
```

Valid statuses are `queued`, `working`, `needs_input`, `completed`, `failed`,
and `cancelled`. `text` and optional `context` explain a status or carry
structured results such as suggestion IDs.

The browser requests cancellation with:

```json
{ "type": "cancel", "id": "cancel-1", "task_id": "request-1" }
```

The runner reports `cancelled` when the model confirms interruption. If the
model finishes before interruption takes effect, it reports the actual completed
result. Cancellation does not undo edits already accepted by the user.

A `needs_input` task carries `context.input` with `request_id`, `kind`
(`approval` or `question`), `message`, and `questions`. The browser responds:

```json
{
  "type": "input", "id": "input-1", "task_id": "request-1",
  "request_id": "permission-1", "response": { "decision": "accept" }
}
```

Approval decisions are `accept` or `decline`. Question responses use
`{"answers":{"question-id":{"answers":["chosen answer"]}}}`. The runner
matches the request to a pending model RPC before forwarding the response.
Completed assistant messages carry `context.task_id`; clients upsert by that
identity so reconciliation does not duplicate answers.

The runner advertises the controls it supports:

```json
{
  "type": "capabilities", "id": "cap-1",
  "capabilities": { "steer": false, "cancel": true, "preview": true, "input": true }
}
```

For browser-side candidate rendering, the runner asks for a preview:

```json
{
  "type": "preview_request", "id": "preview-1", "task_id": "request-1",
  "base_revision": "base-tree-sha", "revision": "candidate-tree-sha",
  "files": { "paper.typ": "candidate source" }
}
```

The browser returns diagnostics tied to that candidate revision:

```json
{
  "type": "preview_result", "id": "preview-1-result",
  "request_id": "preview-1", "task_id": "request-1",
  "base_revision": "base-tree-sha", "revision": "candidate-tree-sha", "ok": true,
  "diagnostics": []
}
```

The browser captures its full source tree and assets, checks `base_revision`,
overlays the supplied changes to existing text files, and computes the
canonical candidate tree digest. It renders only if that digest equals
`revision`. Verification never mutates the shared document. Source changes
since the base snapshot and unavailable assets cause a verification refusal.

Only the runner may send `task`, `capabilities`, and `preview_request`. Only
the browser may send `cancel`, `input`, and `preview_result`. `message` is permitted in
both directions. The server rejects wrong-role events, malformed values,
oversized payloads, and delivery when the recipient is disconnected.

There are no HTTP message-post, polling, listen, cursor, or transcript
endpoints. Document edits, comments, and suggestion acceptance continue to
use their ordinary authenticated document operations.
