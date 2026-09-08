# SPEC: the writing assistant

An author selects a paragraph, asks for it to be tightened, and gets back a
proposal they can accept or reject. That is the feature. Almost all of it is
already built: `komodoc agent` reads, comments, edits and suggests; the robot
panel opens a private live channel; a suggestion is an annotation an editor
decides, applied through the session with a checkpoint. What is missing is
not a model. It is the material the agent needs to answer well, and the shape
its answers come back in.

## Decision

Komodoc runs no model, holds no provider credential and meters no tokens.
The assistant is the user's own agent, started in their own window, reaching
the document through the link it was given. This is the rule every format
already keeps for compilation -- the client brings the engine -- applied to
writing help.

Four things are added:

1. **Anchored context.** The panel sends the source anchor of the selection,
   not just the words, so a proposal lands on the passage the author meant.
2. **A task vocabulary.** The panel composes named requests; a shipped skill
   says what each one means and what shape its answer takes.
3. **Batched suggestions.** A pass over a document is one operation with one
   result per item, grouped so an editor can decide it as a batch.
4. **Diagnostics an agent can read**, so "explain this error" has something
   to explain.

Everything else -- authorization, attribution, durability, the accept path,
the merge on a stale anchor -- is the existing machinery, unchanged.

## An assistant proposes; an editor decides

Every task whose answer is text produces **suggestions**, never a silent
source edit, even when the link permits editing. A writing pass by a model is
exactly what accept and reject were built for: the proposal is inert, the diff
is shown word by word on the card, and the checkpoint records who accepted it.

`komodoc agent edit` stays what it is, for the case where the author asked for
a specific change and wants it made. The difference is who asked for what, and
it belongs in the skill's instructions rather than in a server rule.

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
working and is the fallback for everything not named here.

## Context: anchored, not quoted

`Agent.svelte` already receives the reader's `pending` selection and sends
`{file, selection:{exact}}`. That object also holds the source anchor the
comment modal uses -- `{path, exact, prefix, suffix, position}` -- and the
reader knows the current checkpoint. Send both.

With an anchor, a proposal does not have to be found again by searching for
its own words. `komodoc suggest` gains `--anchor <json>`, taking the anchor
from the chat context verbatim and posting it unchanged. `--find` stays for
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

`komodoc agent suggest --batch <file.json>` posts an array of proposals in one
request against one snapshot sha. The server takes the room lock once, applies
each item in order and returns one result per item: created with its id,
`anchor-not-found`, or refused with a reason. **Partial success is the normal
outcome and is reported as such**; a batch is not a transaction, because one
bad anchor is no reason to drop thirty good proposals. An empty result set is
an error.

A batch is capped at 100 items, counts against the existing annotation caps
and per-minute rate, and is refused whole when it would exceed either -- the
caps are the document's protection and a batch is not an exemption from them.

Every comment in a batch carries the same `pass` field, a server-assigned id:

```rust
/// The batch this suggestion arrived in, when it arrived in one. Lets a
/// reader group a pass and decide it together; absent on a suggestion made
/// on its own.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub pass: Option<String>,
```

In the comments list, suggestions sharing a `pass` are drawn under one header
saying how many there are and how many are still pending, with **Reject all**
and, for an editor, a way to work through them in order. An editor facing
forty proposals needs a verb that dismisses the pass; without one, a
generous assistant is worse than none.

## Diagnostics for the agent

Diagnostics are computed where the document is rendered, which is the browser,
so there is nothing on the server for the CLI to fetch. `komodoc agent
diagnostics <link>` therefore reads the current source through the existing
snapshot and compiles it locally with `crates/engine`, the same crate the
browser runs, printing severity, path, line, column, message and the offending
source line as JSON. The CLI already does this on `publish` (`cli.rs:678`);
this exposes it against a live document instead of a file on disk.

This covers markdown, typst, html and, when it lands, quarto. LaTeX compiles
in the browser or the local app and its log lives there, so for a LaTeX
document the panel quotes the selected diagnostic into an `explain` task
instead. No server compilation is introduced for either.

## The panel

The context area of `Agent.svelte` gains a row of task chips: Proofread,
Tighten, Rewrite, Explain. A chip is enabled when its scope has what it needs
-- a selection for `tighten`, an open file for `proofread` -- and when
`komodoc agent capabilities` for this link permits the annotations the task
would create; a read link shows them disabled with the reason. Chips obey the
composer's existing rule and are disabled between watch calls, when no agent
socket is listening.

The panel does not claim the agent is working. Presence means a socket is
connected and nothing more, and a pass appears where its results appear: in
the comments list, as the suggestions land. The Diagnostics panel gains one
affordance, **Ask the agent**, which sends an `explain` task carrying that
diagnostic and opens the robot panel.

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

1. Anchored context and `suggest --anchor`, with `revision` carried through.
   This alone makes the loop that exists today usable on a selection.
2. Task chips and the `komodoc-write` skill defining the vocabulary, shipped
   beside `komodoc-document` and `komodoc-pair`.
3. Batched suggestions, the `pass` field, and pass grouping with a bulk
   decision in the comments list.
4. `komodoc agent diagnostics` and **Ask the agent** in the Diagnostics panel.

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
chip and scope, chip enablement from capabilities and selection, and the pass
grouping's counts and bulk transitions.

## Open questions

- **Bulk accept.** Each accept takes a checkpoint and a checkpoint budget
  token (`catalog.md`: 300 per owner per rolling hour). Forty accepted
  proofreading fixes should plausibly be one checkpoint, not forty, which
  means an accept path that applies several suggestions under one lock and one
  mark. Decide this before step 3; it changes `Room::accept_suggestion`.
- **Whether an `explain` answer can persist.** Chat vanishes on refresh. An
  author who asked what a passage assumes may want the answer kept, which
  means a `commenting` annotation. Proposed rule: chat for a question, a
  comment when the author asks for a note. Confirm before the skill is
  written, since the skill is where the rule lives.
- **A whole-document pass on a large file** is bounded by the annotation caps
  rather than by anything the agent pays for. Whether the cap is the right
  bound, or whether a pass should have its own smaller one, needs a number
  from a real document.

## References

- [protocol/chat.md](../protocol/chat.md) -- the live channel and its limits.
- [sync.md](sync.md) -- the automation peer, link-bounded authority, skills.
- [track-changes.md](track-changes.md) -- suggestions, accept, reject, stale.
- [catalog.md](catalog.md) -- annotation caps and checkpoint budgets.
