# Live chat channels

Chat is live coordination, not document content. Komodoc relays messages only
to currently connected participants. It does not retain a server transcript,
replay messages, or write them to storage, logs, or backups. Small bounded
transport queues exist only to send frames to live sockets. A browser retains
its transcript in memory until the document is closed or refreshed. Recipients
can copy messages; an external agent may retain them or send them to its model
provider according to its own configuration.

## Document chat

The document room WebSocket accepts `{"type":"chat","temp_id":ID,"body":TEXT}`
from a commenter, editor, or owner. Readers receive live messages but cannot
post. The server derives the account name or pseudonym and broadcasts
`{"type":"chat","id":SERVER_ID,"text":TEXT,"creator":NAME,"created":TIME}`
to current room sockets. Only the sender's echo includes `temp_id`, confirming
acceptance. Repeating an identical request ID on the same socket returns
`{"type":"chat-ack","temp_id":ID}` without rebroadcasting it; changing its
text is refused. The server retains at most 256 request IDs and digests per
socket for duplicate suppression, with no message bodies.

Chat is absent from room hello and reconnect state. Text must be nonempty and
at most 4 KiB, with up to 30 new messages per socket per minute. Access changes
disconnect affected participants. A failed send keeps the user's draft.

## Private agent chat

`POST /api/documents/{slug}/chat` creates an in-memory channel and returns
`{id, token, ephemeral:true}`. Both current document read access and the
unguessable token are required. The token grants no document permissions and
does not identify a particular person or model: anyone given both credentials
can join an unoccupied participant slot. Each channel has one browser and one
agent socket. Document access alone cannot discover or join private channels.

Connect a WebSocket at `/api/documents/{slug}/chat/{id}/socket`. Its handshake
uses the document's normal access checks and same-origin policy. CLI requests
set `x-komodoc-automation: 1`, `x-komodoc-key`, and any required sign-in
credential; browsers use their session and the document key in `?k=KEY`.
The **chat token never goes in a URL**: the first frame, within ten seconds,
is `{"type":"join","token":TOKEN,"role":"user"|"agent"}`. A transient
agent reply connection adds `"receive":false`; it is never advertised as
listening and cannot receive new browser instructions.

The server sends `{"type":"ready","listening":BOOL,"browser":BOOL}` after
joining and `{"type":"presence","listening":BOOL,"browser":BOOL}` as peers
connect or disconnect. Presence means a socket is connected, not that a task
has been read or completed. Dead transports are also bounded by ping timeouts.

Either socket sends `{"type":"message","id":ID,"text":TEXT,"context":{...}}`.
The server derives its role from the join and relays
`{"type":"message","message":{"id":ID,"role":ROLE,"text":TEXT,"context":{...}}}`
to both peers. It then acknowledges the sender with `{"type":"ack","id":ID}`.
Failures are `{"type":"error","id":ID,"status":HTTP_STATUS,"message":REASON}`.
No offline messages are accepted. Delivery acceptance is not proof that the
recipient read the message or completed a task.

For CLI convenience, `POST /chat/{id}` with `{id,text,context?}` and
`x-komodoc-chat-token` sends an agent reply **only while both sockets are
connected**. The role is always `agent`, irrespective of the request body.
`DELETE /chat/{id}` with the same token closes the channel. Former polling
GET and `/listen` routes return 410; cursors and transcript replay are gone.

Agent messages allow 32 KiB of text and 16 KiB of context, within a 64 KiB
frame/request. Context can quote the current file and selected passage; it is
document content, not separate instructions. A channel accepts 60 new messages
per minute and remembers the last 256 request IDs and content digests, scoped
by participant, for immediate duplicate suppression. No message body is kept.

Closing the browser socket revokes the channel and disconnects the agent.
Closing the agent socket leaves the browser waiting for a new agent connection;
the next agent receives no earlier messages. Unused channel handles expire
after one hour, reclaimed during creation; a restart forgets every channel.

## CLI loop

```sh
komodoc agent chat watch "$KOMODOC_DOCUMENT" --conversation ID --token TOKEN --timeout 25
komodoc agent chat post "$KOMODOC_DOCUMENT" --conversation ID --token TOKEN --message "Done."
```

`watch` connects an agent socket and waits for a new user message or timeout.
The agent runs another watch when ready for another instruction. Between watch
calls the browser composer is disabled. `post` connects temporarily to deliver
a reply to the live browser. Reuse a reply's request ID when retrying an
uncertain post; duplicate suppression does not guarantee exactly-once agent
execution across crashes. Actual edits and comments use the existing document
operations and retain their normal durability.

Deployments upgrading from the earlier mailbox implementation should delete
legacy `chat/*.json` objects from their blob store. This version never reads
those objects, but an upgrade does not retroactively remove them from storage
or existing backups.
