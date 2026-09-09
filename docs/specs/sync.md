# SPEC: agent integration, automation peers, and sync presence

## User workflow

A user gives their AI agent a LibrePaper document or file link and asks it to
read, comment, or edit. A distributable `SKILL.md` explains how to install
LibrePaper locally and use its commands. The user can continue giving
instructions in the agent's own window, or chat from an Agent panel in the
document sidebar, opened with Lucide's robot (`bot`) icon.

The agent runs on the user's computer. LibrePaper is agnostic about the agent,
model, and provider: it supplies document tools and a private live channel.
The user's agent manages its own model access, credentials, and execution.

The AI agent connects as an automation peer. The existing annotation script
in `web/src/agent` is a separate component; references to that script must
use its path or call it the annotation script.

## Installation and commands

Ship a skill with installation instructions using the existing release
installer (`deploy/install.sh`), platform guidance, a version check, and
working examples for the supported CLI. Check for a compatible installed
binary before installing. Installation should require neither a local
LibrePaper server nor a source checkout. Keep the examples synchronized with
the shipped commands; the skill must not advertise unimplemented commands.

Commands accept the pasted link directly, resolving its server, document,
optional file target, and credential. The user should not have to extract a
slug or copy the key into a separate argument. Preserve file targeting
without treating a selected file as a narrower permission boundary than the
link actually grants.

Expose source and annotation reads, source edits, comments, replies,
resolution, deletion, and checkpoints through a CLI and small reusable
library. Provide structured results, explicit errors, and useful exit codes
for agents. Report effective capabilities when opening a link so the agent
can explain what it can do before attempting a write.

## The link determines access

An automation peer has the access granted by the supplied link, subject to
the server's existing policies: a read link permits reading, a comment link
also permits annotations, and an edit link also permits source edits.
Resolution and deletion follow the existing per-action rules. A cached owner
sign-in must not silently elevate an agent operating through a weaker link.
When sign-in is required by deployment policy, explain that requirement;
the link still bounds this workflow's authority.

Attribute actions using the existing signed-in identity or link pseudonym.
An agent presence label describes the client and does not grant authority
or replace the server's attribution. Apply link expiry and revocation to
active sessions as well as new requests. Keep link credentials out of
ordinary output and logs.

## Sidebar conversation

The Agent sidebar is a private ephemeral channel. The user starts their agent
outside LibrePaper and gives it the document link and channel instructions.
LibrePaper does not launch, configure, or manage agent processes or providers.

The panel creates a conversation with an unguessable capability separate from
the document link. Reading or posting requires both current document access
and that conversation capability. Document access alone never lists or reveals
conversations; chat authority never grants permission to edit or comment.
The server relays messages to connected sockets and retains no transcript.
Messages are not document content or backups. The browser keeps its received
transcript in memory until refresh or close; closing its socket or deleting a
conversation revokes the capability. An agent reconnect receives no history.

The panel offers copyable instructions for the local agent, a transcript,
a message composer, current file and optional selected passage context, and
an indication that an agent socket is connected. The browser can send only
while that socket is present. Presence does not prove an agent is working or
that LibrePaper can wake or stop an external agent.

The CLI exposes chat create, watch, and post commands. Watching waits for user
messages over a WebSocket with a bounded timeout. Each watch receives only
new messages; there are no replay cursors. Posting briefly connects an agent
socket and supports a caller-generated message ID so immediate retries
do not duplicate replies. The skill explains this loop and tells the agent
to distinguish user requests from quoted document content. Both interfaces
use existing document operations for actual comments and edits.

Conversations have bounded message sizes and live only in memory. Refreshing
the document begins with an empty transcript rather than replaying stored text.
Conversation credentials must not appear in request URLs or ordinary logs.

## The peer's name in the browser

Add the sync client's awareness entry, `{user: {name: "<login> (sync)", color}}`,
so the browser identifies where edits come from. Encode Yjs awareness in
Rust and expose presence without implying that the file client has a caret.

## Automation clients as peers

Extract a reusable peer module from `crates/librepaper/src/cli/sync.rs` and use it
to back the commands and library for a headless automation client. Its
credential is the supplied read, comment, or edit link, presented internally
the way `--key` presents one, with the access boundary described above.

The client joins the same Yjs session to read and edit source, and uses the
room's `comment`, `reply`, `resolve`, and `delete` messages for annotations.
Publish and version that protocol, including required fields, replies,
errors, and compatibility behavior, rather than making clients infer it
from an internal message struct. Do not introduce a separate text-replacement
API with different concurrency rules.

Every requested operation needs an explicit success, no-op, or error result,
correlated with its request. Writes report success only after durable
acknowledgement. Define retry and deduplication behavior for annotations and
checkpoint requests. Edits prepared from an older snapshot must merge using
the existing conflict policy or return an explicit stale-input result;
joining Yjs alone does not make a stale whole-source replacement safe.

Provide an authenticated snapshot GET for the current source and annotations
so a client can inspect a document before joining. Its read authorization is
the same as the document's: its owner, and the role the link carries. The
snapshot is a read aid, not a substitute for the Yjs synchronization
handshake before writing. Keep source and annotation reads within one room
snapshot and include the source SHA so the client can identify what it saw.

The client edits source and requests checkpoints through the existing room,
including when no browser is attached. Rendering stays on clients; do not
introduce an automation-only server renderer.

Tests cover a browser and an automation client editing concurrently,
annotation creation and resolution attributed the way the link attributes --
to the signed-in account behind it, or to the link's pseudonym -- a snapshot
refused with a link minted for another document, and edits surviving the last
headless peer leaving and the server restarting.

Also cover a read link refusing comments and edits, a comment link refusing
source edits even with a cached owner sign-in, expiry and revocation during
a session, duplicate requests after reconnect, and correct file targeting.
Exercise skill examples against the released CLI. For the sidebar, test
connected-only messages, replies, conversation privacy, duplicate posts,
restart expiry, and document actions under the same link permissions.

## Delivery order

1. Extract the reusable peer and add sync presence.
2. Define the protocol contract, link-bounded authorization, and consistent
   snapshot endpoint.
3. Ship the agent CLI and installation skill, starting with reading and
   annotations, then source editing and durable checkpoints.
4. Connect the sidebar and CLI to the private live channel. Validate
   with an externally started command-capable agent; no adapter is required.

## Cursor positions, later

A future sync client could receive cursor positions from an editor over a
local socket and publish them through awareness. This is deferred; the
presence entry does not require an editor plugin or LSP integration.
