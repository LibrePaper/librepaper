# Sidebar conversations

The sidebar is a mailbox, not an agent launcher. A user starts any local
agent capable of executing the Komodoc CLI. The agent watches for messages,
performs document operations through the existing CLI, and posts replies.
No local service, model adapter, or browser-to-local transport is involved.

## Access

All routes live under `/api/documents/{slug}/chat`. Every request must have
current document read access. CLI requests include `x-komodoc-client: 1`,
`x-komodoc-automation: 1`, the document link's `x-komodoc-key`, and any sign-in
credential required by deployment policy. Automation cannot use a cached
owner credential to elevate the supplied link's role.

Creating a conversation returns `{id, token}`. Every subsequent request also
requires `x-komodoc-chat-token: TOKEN`. The token is a separate capability,
scoped to this conversation and document. It never grants document editing
or commenting rights. A document link alone cannot discover conversations
or read their transcripts. Anyone given both credentials can access that
conversation; the role field identifies the message direction, not a
verified human or agent identity. Keep tokens out of URLs and logs.

The sidebar saves its conversation handle in the current browser tab's
session storage. Messages are persisted by the server separately from the
shared document, including across restarts. They are not end-to-end encrypted
against the server operator. Conversations expire 30 days after creation; expired entries are reclaimed
on a subsequent successful mailbox write. Deleting a conversation removes its transcript
and invalidates the token. Starting another conversation does not launch or
stop an agent process.

## Routes

- `POST /chat` with `{}` creates a conversation and returns `{id, token}`.
- `GET /chat/{id}?after=0` returns `{messages, next_cursor, listening}`.
  Each message has `id`, `cursor`, `role`, `text`, and optional `context`.
  Cursors increase within a conversation. Use the returned cursor for the
  next incremental read; use zero to replay the retained transcript.
- `POST /chat/{id}` with `{id, role, text, context?}` posts a message.
  Use `user` for sidebar instructions and `agent` for replies. Retrying the
  same message ID with the same content does not duplicate it. Reusing an
  ID with different content fails.
- `POST /chat/{id}/listen` with `{}` records an agent heartbeat.
  `listening` means an agent checked in recently, not that it is currently
  executing the request. Sidebar reads do not count as agent heartbeats.
- `DELETE /chat/{id}` removes and revokes the conversation.

Context may carry the current `file` and a selected passage as `selection`.
It is quoted document content, not a separate source of instructions.
Chat messages are plain text. An agent may post several replies to report
progress; no provider-specific streaming format is necessary.

A document holds up to 16 conversations and 4 MiB of serialized mailbox
data. Each conversation holds at most 256 messages and 1 MiB. A message
allows 32 KiB of text and 16 KiB of context, within a 64 KiB request body.
Storage and message size limits are enforced. A full conversation refuses
new messages explicitly rather than silently discarding unread requests.
Delete a finished conversation to reclaim its capacity. The user can queue
messages while no agent is listening. Stopping an external agent is done in
that agent's own window; Komodoc cannot wake it or force it to stop.

## CLI loop

Use `KOMODOC_CHAT_TOKEN` for the conversation credential, or supply `--token`.
The document link remains the positional argument:

```sh
komodoc agent chat create "$KOMODOC_DOCUMENT"
komodoc agent chat watch "$KOMODOC_DOCUMENT" --conversation ID --after 0 --timeout 25
komodoc agent chat post "$KOMODOC_DOCUMENT" --conversation ID --message "Done." --request-id REPLY_ID
```

`watch` checks in while waiting and returns user messages plus `next_cursor`.
It returns an empty list on timeout. The agent saves that cursor, handles the
messages once, and watches again while asked to listen. Reuse the reply ID
when retrying an uncertain post. The CLI cannot guarantee exactly-once agent
execution across agent crashes; the agent must retain its own progress and
confirm document state before repeating a potentially completed operation.
