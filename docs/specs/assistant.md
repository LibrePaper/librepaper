# SPEC: the writing assistant

An author selects a paragraph, asks for it to be tightened, and gets back a
proposal they can accept or reject. The writing assistant combines an external
agent connection, captured source context, named tasks and reviewable suggestions.
This specification defines the implemented behavior and the order in which its
foundations depend on one another. Bulk acceptance remains deferred.

## Decision

Komodoc runs no model, holds no provider credential and meters no tokens.
The assistant is the user's own agent, started in their own window, reaching
the document through the link it was given. This is the rule every format
already keeps for compilation -- the client brings the engine -- applied to
writing help.

Five things are added:

1. **A usability foundation.** Setup explains how to connect an external agent,
   connection states explain what the user can do, and a lost channel has a
   recovery path. Drafting never depends on an agent listening.
2. **Anchored context.** The panel sends the source anchor of the selection,
   not just the words, so a proposal lands on the passage the author meant.
3. **A task vocabulary.** The panel composes named requests; a shipped skill
   says what each one means and what shape its answer takes.
4. **Batched suggestions.** A pass over a document is one operation with one
   result per item, grouped so an editor can decide it as a batch.
5. **Diagnostics an agent can read**, so "explain this error" has something
   to explain.

Everything else -- authorization, attribution, durability, the accept path,
the merge on a stale anchor -- is the existing machinery, unchanged.

## An assistant proposes; an editor decides

Every task that proposes replacement document text produces **suggestions**,
never a silent source edit, even when the link permits editing. A writing pass by a model is
exactly what accept and reject were built for: the proposal is inert, the diff
is shown word by word on the card, and the checkpoint records who accepted it.

`komodoc agent edit` stays what it is, for the case where the author asked for
a specific change and wants it made. The difference is who asked for what, and
it belongs in the skill's instructions rather than in a server rule.

## Usability foundation

### Connect an agent

The panel leads with **Connect an AI agent you already use** and explains that
the user runs it in their own agent window. The primary action is **Copy setup
prompt**, followed by a short instruction to paste it into that window. The
prompt asks the agent to check the CLI version, install or upgrade it when
needed using the shipped installation instructions, load the pairing and
document skills, check capabilities, and enter the watch loop. It must be
usable by an agent starting without the binary or skills already installed.
Watch/post commands remain available in expandable **Connection settings**.

Connection settings also hold the document link and remain accessible after
the first connection. Do not ask users to understand a share key before they
understand the feature. Explain access in terms of actions: **Can read and
suggest changes**, **Read only**, or **Can edit directly**. Show effective
capabilities verified for the chosen agent link, not the browser user's own
permissions or an inference from the URL. Until verified, show **Access not
checked** and a next step. Read-only access still supports explanation tasks.
Prefer a comment-capable link for writing help when the user can create one;
never silently widen an existing link or create broader access.

The browser checks `GET /api/documents/{slug}/assistant/capabilities` with the
chosen link's `X-Komodoc-Key`. The server resolves that request in link-bounded
automation mode, retaining session identity only for attribution and policy
ceilings. The response exposes `can_read`, `can_comment`, `can_suggest`, and
`can_edit`. Changing the link clears the displayed access immediately; late
responses for a previous link cannot restore it. Failed lookups stay unchecked.

Generated instructions follow the pairing skill's credential handling,
including using `KOMODOC_CHAT_TOKEN` instead of a literal `--token` argument.
The setup prompt necessarily carries connection credentials; keep them out of
the conversation and ordinary status messages.

### Availability and recovery

Use one connection status rather than repeating it in the header and toolbar:

| State | Label and next action |
| --- | --- |
| No agent has joined the live channel | **Connect your agent**; Copy setup prompt. |
| An agent socket is receiving messages | **Ready for your message**; Send is available. |
| The browser channel is live, but an agent that joined is no longer receiving | **Agent isn't listening**; explain that it must resume listening in its own window. |
| The browser connection closed or the channel was revoked | **Connection ended**; offer **Reconnect agent**, creating a fresh channel and fresh setup prompt. |

Presence does not establish that an agent is working, has read a message, or
has completed a task. Never infer a busy indicator from a watch ending. An
agent can post a progress message, but Komodoc cannot wake or stop it.

The textarea stays editable before connection, between watch calls, and after
connection loss. Only submission depends on a receiving socket. Explain why
Send is disabled beside the composer. Preserve drafts on failures and through
channel recovery; never automatically send a draft when an agent reconnects.
This is local composition, not an offline message queue. A revoked channel's
old instructions must not remain the active connection instructions.

Keep the panel mounted when switching sidebar tabs so navigating to Files,
Comments or Diagnostics does not end the conversation. A fresh channel does
not replay earlier messages to the agent. Keep the **Conversation isn't saved**
notice visible after connection; expanded help explains that the external
agent or its provider may retain messages. Move **New conversation** into a
secondary menu and explain before reset that it clears the visible transcript
and requires fresh connection instructions. Reconnection preserves the draft;
an explicit new-conversation reset warns before discarding it.

### Composer and layout

Enter sends; Shift+Enter inserts a newline. Preserve the existing Ctrl+Enter
newline shortcut and IME composition handling. Show the keyboard hint near
the composer. A task chip prepares a request for review; it never sends it
immediately.

Keep setup details collapsible, the attached passage compact, and the composer
reachable in narrow or short panels. Long instructions, quotations and error
messages must scroll without clipping controls. Status updates must be
announced accessibly, and keyboard users must be able to reach setup, context
actions, the composer and review actions.

## Tasks

A task is an ordinary chat message with one extra field:

```json
{"type":"message","id":ID,"text":TEXT,
 "task":{"kind":"tighten","scope":"selection"},
 "context":{"file":"main.md","selection":{...},"revision":SHA}}
```

`kind` is one of `proofread`, `tighten`, `rewrite`, `explain`, `outline`,
`respond`. `scope` is `selection`, `file` or `document`. The server validates
that `kind` and `scope` are known strings within the existing 16 KiB context
budget and relays them; it interprets nothing. An agent that does not
understand a kind falls back to `text`, which always carries a readable
sentence saying the same thing.

What each kind means:

- **proofread** -- grammar, spelling, agreement, word choice. One suggestion
  per passage, no rewriting of arguments. The usual scope is `file`.
- **tighten** and **rewrite** -- one proposal for the anchored passage.
  `rewrite` carries the author's own instruction in `text`; `tighten` does not
  need one.
- **explain** -- a prose answer in the channel and no annotation. Used for a
  diagnostic, an unfamiliar construct or a passage the author is unsure of.
- **outline** -- a prose answer by default; a suggestion only when the author
  asked for headings in the document.
- **respond** -- draft a reply to an open comment thread, posted as a reply
  through `komodoc agent reply`, not as a new suggestion.

The chips in the panel are shortcuts that prefill these; free text keeps
working and is the fallback for everything not named here. Explain and outline
answers stay in chat by default. When the user explicitly asks to keep an
answer as a note, the agent creates an ordinary comment if the link permits it.
The UI must not promise durable chat history.

## Context: anchored, not quoted

`Agent.svelte` receives the reader's `pending` selection, including the source
anchor used by the comment modal. Currently `agent-client.js` reduces the
selection to an exact-text string and omits the revision. Carry the source
anchor -- `{path, exact, prefix, suffix, position}` -- and its revision through
the panel, client, relay and CLI without that loss.

Capture the anchor, source file and revision together when the passage is
attached. Display **Selected passage · introduction.md** with a quotation,
**Remove** and **Replace with current selection** actions. Clearing the browser
selection while focusing the composer must not discard the attachment. Opening
another file must not relabel the attachment or pair its words with the new
file: a selection-scoped request uses the attached source path. Make file or
document scope explicit when that is what the user requests. Once attached,
another selection does not silently replace it.

If source anchoring fails, the quoted passage can still be explained, but show
why Tighten and Rewrite are unavailable. Do not guess an editable occurrence
from a rendered quotation. Preserve the captured revision if the document
changes while the user drafts; do not substitute the latest revision at send
time. Enforce the context budget with a visible error instead of silently
truncating an anchor.

With an anchor, a proposal does not have to be found again by searching for
its own words. `komodoc suggest --anchor <json> --revision <sha>` takes the anchor
and captured revision from the chat context verbatim and posts them unchanged. `--find` stays for
the terminal, where an author types the words they mean; only `--find`
requires a unique match, because only `--find` is guessing. A passage that
legitimately occurs twice is suggestable through `--anchor` and is not
through `--find`.

The anchor's `revision` is stored on the suggestion as it is today, so a
proposal written against a paragraph that has since changed reaches the
existing stale path: refusal with `stale: true`, the merge editor, no silent
application of an out-of-date rewrite. This matters more for an assistant than
for a person, because a pass over a long document takes long enough for the
author to have moved on.

Prefix and suffix are capped by `config.caps.exact`, as any anchor is. Context
remains document content and not instructions; the skills already say so and
must keep saying so.

## Batched suggestions

A proofread pass on a paper is dozens of proposals. Sent one at a time each
costs a request, a room lock and a fresh snapshot, and the twentieth is
written against a document the first nineteen have not changed but may have.

`komodoc suggest --batch <file.json>` posts an array of proposals in one
request against one snapshot sha. The server takes the room lock once, applies
each item in order and returns one result per item: created with its id,
`anchor-not-found`, or refused with a reason. **Partial success is the normal
outcome and is reported as such**; a batch is not a transaction, because one
bad anchor is no reason to drop thirty good proposals. An empty result set is
an error.

A batch is capped at 100 items, counts against the existing annotation caps
and configured caller annotation rate, and is refused whole when it would exceed either -- the
caps are the document's protection and a batch is not an exemption from them.

Every comment in a batch carries the same `pass` field, a server-assigned id:

```rust
/// The batch this suggestion arrived in, when it arrived in one. Lets a
/// reader group a pass and decide it together; absent on a suggestion made
/// on its own.
#[serde(default, skip_serializing_if = "String::is_empty")]
pub pass: String,
```

In the comments list, suggestions sharing a `pass` are drawn under one header
saying how many there are and how many are still pending, with **Reject all**
for users allowed to reject them and **Review next** for an editor to work
through them in order. Preserve the existing word-level diffs and individual
Accept/Reject actions, and report partial failures without hiding successful
proposals. An editor facing
forty proposals needs a verb that dismisses the pass; without one, a
generous assistant is worse than none.

The first batch delivery does not include **Accept all**. Individual acceptance
keeps its existing checkpoint and stale-anchor behavior. Bulk acceptance is
separate work: first specify how multiple accepted proposals share a checkpoint,
how overlapping or stale proposals are handled, and how partial outcomes are
reported. Batch creation and Reject all must not depend on solving bulk accept.

## Diagnostics for the agent

Deliver the browser entry point first. **Ask assistant** on a diagnostic opens
the assistant panel and prepares an `explain` request with its severity, file,
line, column, message, available source excerpt and render revision. Preserve
the diagnostic's provenance rather than pairing an old error with the latest
source revision. The user can inspect and send it when the agent is listening;
opening the request must also work before setup. Replacing a nonempty draft
requires an explicit choice. This works with diagnostics already in the
browser and does not depend on a new CLI command.

Diagnostics are computed where the document is rendered, which is the browser,
so there is nothing on the server for the CLI to fetch. `komodoc agent
diagnostics <link>` therefore reads the current source through the existing
snapshot and compiles it locally with `crates/engine`, the same crate the
browser runs, printing severity, path, line, column, message and the offending
source line as JSON. The CLI already compiles locally on `publish`;
this exposes it against a live document instead of a file on disk.

This covers markdown, typst, html and, when it lands, quarto. LaTeX compiles
in the browser or the local app and its log lives there, so for a LaTeX
document the browser entry point supplies the diagnostic rather than asking
the CLI to reconstruct its log. No server compilation is introduced for either.

## The panel

The selection toolbar gains **Ask assistant** alongside the existing annotation
action, including for readers who can ask for explanations. It opens the panel
with the selected passage attached. If a request is already being drafted,
offer an explicit replacement choice instead of overwriting its text or context.

The context area of `Agent.svelte` gains a row of task chips: Proofread,
Tighten, Rewrite, Explain. Show the scope before submission: **Selected passage**,
**Current file · main.md**, or **Whole document**. A chip is available when its
scope has what it needs -- an anchored selection for Tighten and Rewrite, an
open source file for file-scoped Proofread -- and verified capabilities permit
the annotations it would create. Explain disabled actions, including unknown
capabilities. A read link permits Explain but not rewriting suggestions.
Connection availability gates Send, not local task preparation.

Suggestions remain in the comments list, where the existing review machinery
lives. Provide a **Review suggestions** action in the assistant panel when
linked results arrive, opening the relevant suggestion or pass. Derive that
action from actual result identifiers and document annotations, not a prose
claim that work was completed. `komodoc agent chat post --results <json>` puts
`{suggestions:[ID,...],pass:ID}` in `context.results`; omit `pass` for a single
proposal. The relay validates the shape, and the browser resolves IDs against
visible suggestion annotations. Do not
guess that every new annotation belongs to the current request. The first
selection workflow must lead all the way from request to Accept/Reject without
requiring the user to discover a different sidebar tab unaided.

## Attribution

An agent's suggestion is attributed the way anything through that link is: to
the signed-in account behind it, or to the link's pseudonym. Nothing here
grants authority, and no flag says "a model wrote this", because Komodoc
cannot verify such a flag -- anyone can post through the CLI. What an editor
sees instead is true and derived: a suggestion in a pass says so, and the
skill instructs the agent to put a short note on its proposals. That is the
honest limit of what the server knows.

## What is not done here

- No model in Komodoc: no server-side inference, no provider credentials, no
  token accounting, no plan tiering around generation.
- No execution of anything the assistant writes.
- No transcript. `protocol/chat.md`'s rule stands: the channel is relayed,
  never stored, and a refresh starts empty. An answer worth keeping is a
  comment, made through the ordinary annotation path.
- No autonomous or scheduled agent. The user starts it, and it watches.
- No inline completion in the editor. Ghost text as you type needs a model in
  the browser or a server proxy, and both break the decision above.
- No evaluation of assistant quality. What a model writes is the user's
  business and their provider's.

## Order of work

1. **Usability foundation.** Clear setup and verified access, distinct connection
   states, fresh-channel recovery, editable drafts, keyboard behavior and
   accessible layout. Done when a new user can connect, understand when sending
   is possible, and recover without losing their draft or copying dead credentials.
2. **Reliable selection context.** Anchored context and `komodoc suggest --anchor`,
   with source path and captured revision carried through. Done when
   repeated passages target the selected occurrence, switching files cannot
   corrupt the attachment, and stale proposals remain protected at acceptance.
3. **Complete the writing loop.** Selection entry point, scope-aware task chips,
   the `komodoc-write` skill shipped beside `komodoc-document` and `komodoc-pair`,
   and a handoff to actual review results. Define the capability and result
   metadata needed by the panel. Done when Select → Tighten → Review →
   Accept/Reject is understandable end to end. Ship selection tasks first;
   file/whole-document proofreading becomes generally available with step 5,
   once its potentially numerous results can be managed as a pass.
4. **Explain browser diagnostics.** Ask assistant prepares an inspectable request
   from an existing diagnostic, including before connection. Done when a user
   can ask about an error without manually copying it; no CLI diagnostics
   dependency.
5. **Manage writing passes.** `komodoc suggest --batch`, the `pass` field, grouped
   counts, partial-result reporting, Reject all and Review next. Done when an
   editor can review or dismiss a proofreading pass without losing track of
   failures or pending items. Keep bulk acceptance out of this delivery.
6. **Independent CLI diagnostics.** `komodoc agent diagnostics` compiles supported
   source locally and returns structured diagnostics. Done when an agent can
   obtain them without a browser handoff, with unsupported formats explained.

Use the existing top-level `komodoc suggest` command for both `--anchor` and
`--batch`; do not introduce an inconsistent `komodoc agent suggest` spelling.
`komodoc agent diagnostics` provides independent compilation diagnostics.
All six work items are implemented; the checks below cover their contracts.

## Tests

Rust, `crates/komodoc/src/tests/assistant.rs`: an anchored suggestion on a
passage occurring twice lands on the anchored occurrence; a batch returns one
result per item including a not-found anchor among successes; a batch over the
item cap, the annotation cap or the rate limit is refused whole; every comment
in a batch shares its `pass`; a read link is refused; a comment link may
suggest but not edit; `revision` is recorded and a later accept of a stale
item still refuses with `stale`.

Rust, `crates/komodoc/src/tests/assistant_cli.rs`: `--anchor` and `--batch`
against a test server, the JSON shapes, exit codes, and `agent diagnostics`
on a document with a known error.

Web, `web/checks/assistant.mjs`, as pure functions: the task message shape per
chip and scope, chip enablement from verified capabilities and selection,
unchanged anchor/path/revision propagation, diagnostic context, result-id
mapping, and the pass grouping's counts and bulk transitions. Test that lack
of a listening socket disables sending without disabling request preparation.

Extend `web/checks/agent-client.mjs` and `web/checks/agent-browser.mjs` beyond
the connected happy path:

- Setup before any agent joins, read-only and unknown access, settings after
  connection, and setup instructions for an agent without prerequisites.
- Distinguish a watch ending from browser channel loss. After loss, recover
  with fresh credentials and preserve the draft without automatic sending or
  replay. Failed sends keep the text; no status falsely claims work is underway.
- Select in file A, open file B, and send: the attachment still names and
  targets A. Clearing the browser selection preserves the attachment; Remove
  and Replace behave explicitly. Edits during drafting preserve its revision.
- Enter sends, Shift+Enter and Ctrl+Enter insert newlines, and IME confirmation
  does not accidentally submit. Drafting remains possible between watch calls.
- Ask assistant from a selection or diagnostic before setup; preserve or
  explicitly replace an existing draft. Review suggestions opens the referenced
  result, and sidebar navigation keeps the live panel mounted.
- Reset consequences and the unsaved-conversation notice remain discoverable
  after connection. Draft loss on explicit reset is explained before it happens.
- At narrow widths and short heights, including with long passages and errors,
  setup and composer controls remain reachable. Verify keyboard navigation,
  accessible status updates and scrolling rather than only component mounting.

Keep the existing checks for escaped agent text, chat tokens staying out of
socket URLs, acknowledgement handling and no transcript replay.

## Open questions

- **Deferred bulk accept.** Each accept takes a checkpoint and a checkpoint budget
  token (`catalog.md`: 300 per owner per rolling hour). Forty accepted
  proofreading fixes should plausibly be one checkpoint, not forty, which
  means an accept path that applies several suggestions under one lock and one
  mark. Resolve checkpoint accounting, stale or overlapping items and partial
  outcomes before adding Accept all. This does not block step 5's individual
  review and bulk rejection.
- **A whole-document pass on a large file** is bounded by the annotation caps
  rather than by anything the agent pays for. Whether the cap is the right
  bound, or whether a pass should have its own smaller one, needs a number
  from a real document.

## References

- [protocol/chat.md](../protocol/chat.md) -- the live channel and its limits.
- [sync.md](sync.md) -- the automation peer, link-bounded authority, skills.
- [track-changes.md](track-changes.md) -- suggestions, accept, reject, stale.
- [catalog.md](catalog.md) -- annotation caps and checkpoint budgets.
