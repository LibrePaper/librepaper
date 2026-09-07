# SPEC: track changes

Two features, built in this order. Both live on top of what exists: the
Yjs session, source anchors, checkpoints, the word diff in `crates/text`, and
the merge editor.

1. **Suggestions.** A commenter proposes a replacement for a passage. The
   proposal is an annotation, inert until an editor accepts it, at which point
   the server applies it to the live source through the session and records a
   checkpoint. Rejecting resolves it without applying.
2. **Redlines.** The reader shows what changed since a checkpoint inline in
   the rendered document: insertions underlined, deletions struck through, with
   who made them when the timeline can say.

Tracked editing inside the CRDT (an editor's own keystrokes held in suspense)
is deliberately out of scope.

## Vocabulary

- *Suggestion*: a comment whose `motivation` is `editing` (the W3C term).
- *Proposal*: the text the suggestion wants in place of the passage. Empty
  means delete the passage.
- *Outcome*: what an editor decided: `accepted` or `rejected`. Empty while
  pending.

## Data model

`Comment` (in `crates/komodoc/src/room.rs`) gains two fields, both
serialized and both sent to clients:

```rust
/// The text this suggestion wants in place of the passage, when the
/// motivation is `editing`. `Some("")` proposes deleting the passage.
/// Absent on every other motivation.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub proposed: Option<String>,
/// `accepted` or `rejected` once an editor decided; empty while pending
/// and on every comment that is not a suggestion.
#[serde(default, skip_serializing_if = "String::is_empty")]
pub outcome: String,
```

`editing` joins `commenting` and `highlighting` in the default
`motivations` list in `config.rs`.

A suggestion replaces the passage its **source anchor** names, never the
rendered quotation: the source is what is versioned and what the session
holds. `comment.exact` remains the rendered quotation for display. The
proposal is therefore always relative to `source.exact`. A suggestion may
arrive without a source anchor (the browser could not place the passage in
the source); it is stored and shown, but accepting it fails with a clear
error and the editor applies it by hand.

Size caps: the proposal is cleaned and capped with `config.caps.exact` the
way `exact` is. A suggestion needs no body; `body` is an optional note.

## Room messages

The room's existing message envelope (`Message`) gains one field,
`proposed: Option<String>`. Three shapes matter:

**Create.** `{"type":"comment","motivation":"editing","exact":..., "prefix":...,
"suffix":..., "position":..., "source":{...}, "proposed":"...", "body":"optional
note", "temp_id":...}`. Authorization is that of any comment (commenter or
above). A `comment` with motivation `editing` and no `proposed` field is
refused: "a suggestion needs a proposal". A `proposed` on any other motivation
is dropped. The broadcast is the usual `{"type":"comment","comment":{...}}`.

**Accept.** `{"type":"accept","comment_id":...,"request_id":...}`. Editor or
above (the room's `is_owner` flag, which is "at least editor"). Behaviour:

1. Refuse when the comment is unknown, is not a suggestion, is already
   accepted, or has no source anchor ("this suggestion has no source anchor;
   apply it by hand"). A rejected suggestion may be accepted; that reverses
   the rejection.
2. Locate `source.exact` in the live text of `source.path`. When it occurs
   once, that is the place. When it occurs more than once, choose the
   occurrence whose surrounding text best matches `source.prefix` and
   `source.suffix` (longest common suffix of the prefix and longest common
   prefix of the suffix), ties broken by distance from `source.position`.
   Apply one `komodoc_text::Edit { at, delete: len16(exact), insert: proposed }`
   to that `Y.Text` in one transaction and collect the update.
3. When it occurs zero times, fall back to a three-way merge with the
   suggestion's `revision` checkpoint as base: base = that checkpoint's text
   at `source.path` (through `checkpoint_texts`), remote = base with the
   proposal applied at the anchor (located as in step 2, in the base), local
   = the live text. `komodoc_text::merge(base, local, remote)`. With no
   conflicts, apply `komodoc_text::diff(live, merged.text)` to the live
   `Y.Text` and collect the update. With conflicts, or when the anchor is not
   found in the base either, refuse with `{"type":"error","stale":true,
   "comment_id":..., "message":"the passage has changed since this was
   suggested", "request_id":...}` and change nothing. The browser opens the
   merge editor on that error.
4. Broadcast the collected Yjs update to every socket as
   `{"type":"y-update","update":<base64>}`, the way `handle_restore` does.
5. Take a checkpoint with `why = "accept"` by the acting identity.
6. Mark the comment `resolved`, `resolved_at`, `resolved_in = <that sha>`,
   `outcome = "accepted"`, save, and broadcast `{"type":"accept",
   "comment_id":..., "resolved_in": sha, "resolved_at":..., "request_id":...}`.

Accepting must not run inside `Room::apply` (it holds the state lock and a
checkpoint needs it too). Add a `Room::accept_suggestion(comment_id, by,
author) -> Result<(Vec<u8>, String), AcceptError>` beside
`restore_and_checkpoint`, and dispatch `accept` and `reject` to it from both
places the server accepts room messages: the socket loop in `run_socket` and
the REST `POST /api/documents/{slug}/comments` handler (whose response is
the same JSON the socket would send). Take the same `restore_write` lock the
restore takes, so two accepts cannot interleave with a restore.

Retrying an accept whose `request_id` was already applied is a no-op that
answers with the recorded outcome and `"noop": true`, mirroring `resolve`.

**Reject.** `{"type":"reject","comment_id":...,"request_id":...}`. Editor or
above. Sets `resolved`, `resolved_at`, `resolved_in = <current checkpoint sha>`
(the same value `resolve` records), `outcome = "rejected"`. Broadcast
`{"type":"reject","comment_id":..., ...same fields as resolve...}`. Idempotent
the way `resolve` is.

**Resolve** on a suggestion: `resolved: true` on a pending suggestion behaves
as reject. `resolved: false` on a rejected suggestion reopens it and clears
`outcome`. `resolved: false` on an accepted suggestion is refused: "an
accepted suggestion cannot be reopened; restore the checkpoint instead".

**Delete** keeps its rules. Deleting an accepted suggestion leaves the text
as it is.

## Checkpoint reason

`accept` joins the `why` vocabulary (`history.rs` doc comment,
`History.svelte`'s `WHY` map as "accepted a suggestion"). Like `comment`, it
is decided by the server and never accepted from a client's `y-checkpoint`.

## Exports

- JSON-LD: a suggestion's `body` becomes an array with the note (when any)
  and `{"type":"TextualBody","purpose":"editing","value":<proposed>}`.
  `outcome` is emitted as `"komodoc:outcome"` when set.
- Markdown: `**Suggested:** “…”` after the quotation, then the note.
- Response: the heading reads `editing, accepted in 4f2a91c` or
  `editing, rejected`; the body shows `**Suggested:** “…”`. The existing
  **Now** line stays.

## CLI

Three commands, each taking the same identifier `comment` takes (a short id,
a slug, or a pasted link) and the same `--key` and `--server` flags:

```sh
komodoc suggest c9k --find "with 95% probability" --replace "in 95% of samples" [--path main.md] [--note "..."]
komodoc accept  c9k <comment-id>
komodoc reject  c9k <comment-id>
```

`suggest` fetches the current source through the existing endpoints the
export uses (`text_as_it_stands` / the current tree), finds `--find` in the
named file (the main file when `--path` is absent; refuse when it occurs zero
or more than one time, saying how many), builds a source anchor with 32
characters of prefix and suffix and the UTF-16 position, and POSTs a
`comment` with motivation `editing`, `exact` = the found text, `proposed` =
`--replace`, `body` = `--note`. Prints the comment id. `accept` and `reject`
POST the corresponding message and print the outcome and, for accept, the
checkpoint sha. Exit non-zero with the server's message on refusal; a stale
accept prints the message and exits 3 so an agent can tell it apart.

## Browser: suggestions

**Composing.** The selection tools gain `editing`, labelled "Suggest" on the
selection bar and in the tool chooser, with its own tint in `agent.js`'s
`TINTS` (hue around 0, low saturation). Clicking the bar with that tool opens
the comment modal in a suggest variant: a textarea pre-filled with
`pending.source?.exact ?? pending.exact` (the source slice when the passage
was placed, the rendered words otherwise) and an optional note field. When
`pending.source` is null the modal says the passage could not be located in
the source and an editor will have to apply the suggestion by hand. Submit
sends the `comment` message above with `proposed` = the textarea. The
optimistic card is drawn as for any comment.

**Cards.** `CommentCard` shows a suggestion as the quotation followed by a
word-level diff of `source.exact` (or `exact`) against `proposed`, computed
with `history.wordDiff` and rendered as struck and underlined runs (deletion
in full when `proposed` is empty). Editors (`canModerate`) see Accept and
Reject buttons while the suggestion is pending; everyone sees "Accepted in
<short sha>" or "Rejected" once decided. The Resolve button is hidden on a
suggestion (Reject is the resolve). The comments list keeps its ordering.

**Deciding.** Accept sends `{"type":"accept"}` optimistically marking the
card busy, not resolved; the `accept` broadcast settles it, the `error`
with `comment_id` clears busy and shows the message. On `stale: true`, the
reader opens `MergeEditor` for `source.path` with `oldText` = the proposal
applied to the text of the suggestion's `revision` checkpoint (fetch it the
way `openFileDiff` fetches a checkpoint file), the live `Y.Text` on the
right, and a header saying this suggestion no longer applies cleanly. After
the editor takes what they want and closes, they reject or leave the
suggestion pending.

**Inline.** In the frame, a pending suggestion's highlight is struck through
and, on its last segment, carries `data-proposed` with the proposal;
`mark[data-proposed]::after { content: attr(data-proposed) }` draws the
proposed text after the passage, underlined, in the tint. CSS content adds
no text node, so the frame's offset tables are untouched. A decided
suggestion is painted like a resolved comment.

## Browser: redlines

In the History panel, beside "What changed since", a toggle "Show in
document". While on, the reader sends the frame
`{"type":"redlines","items":[...]}` with one item per hunk of the diff it
already computes for the panel: `{start, end, kind:"insert", who}` for text
present now, and `{at, kind:"delete", text, who}` for text no longer there.
`who` is the `by` of the checkpoints after the baseline when they agree and
"several people" otherwise. The frame paints inserts as underlined marks
through the same segment machinery `highlight` uses and deletes as an empty
mark at `at` whose `::before` content is the deleted text, struck through.
`title` carries `who`. Turning the toggle off, leaving the panel, or changing
the baseline clears them. Redlines and comment highlights coexist.

The toggle is disabled with an explanation for a document rendered to PDF
(typst, latex), where the frame has no text to paint.

## Tests

Rust, `crates/komodoc/src/tests/suggestions.rs`: creating a suggestion with
and without a proposal; a reader link refused; accept by a commenter refused;
accept applies the edit to the session text, broadcasts a `y-update`, takes an
`accept` checkpoint and marks the comment; accept after the passage moved
elsewhere in the file still applies (unique match); accept after the passage
changed refuses with `stale`; accept with a concurrent unrelated edit
merges; reject; resolve semantics on suggestions; retry with the same
`request_id` is a no-op; exports carry the proposal and the outcome.

Rust, `crates/komodoc/src/tests/suggest_cli.rs`: the three commands against a
test server, including the zero-and-many `--find` refusals and exit code 3.

Web, `web/checks/suggestions.mjs`: the card's diff runs, the modal prefill
rule, and the accept/error state transitions, as pure functions. Web,
`web/checks/redlines.mjs`: hunks to redline items, including `who`.
