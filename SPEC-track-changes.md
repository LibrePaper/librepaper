# Track changes in the document

Kata 4sfn. Hard cutover. This replaces proposals (a Loro branch per tracking
session, diff hunks, base and tip, per-hunk decisions, server tables) with
Word's model.

## The model

- A tracked edit goes into the shared document at once, like any edit.
- The document also records it as a **pending change**. Compiles, read links,
  agents and exports see the text with every pending change in it.
- **Accept** drops the record. The text stays.
- **Reject** puts the text back as it was before the change, then drops the
  record.
- A change is one contiguous region: an original span of text `deleted` was
  replaced by the text now between `start` and `end`. Either side may be empty.
- Each change is decided on its own, in any order. Which session made it does
  not matter.
- `run` is an optional tag (an agent pass, for example). It serves filters and
  "accept all from this run". It never makes changes depend on each other.

## Storage: the `changes` root

A new root map in the Loro document, `changes`, next to `files`, `paths`,
`assets` and `meta`. The projection and the render digest ignore it.

Key: a change id (UUID v4 string). Value: a plain Loro value (not a nested
container), written whole with `map.set(id, value)` and removed with
`map.delete(id)`:

```
{
  file:      string,          // file id, a key of `files`
  author:    string,          // display name, as comments show it
  who:       string,          // stable author key: account id when signed in, else the display name; agents use "agent:<name>"
  at:        number,          // ms since epoch of the last edit to this change
  run:       string | null,   // optional group tag
  start:     Uint8Array,      // encoded Loro Cursor (Cursor.encode())
  end:       Uint8Array|null, // encoded Loro Cursor, null when nothing is inserted
  deleted:   string,          // the original text this change removed, in order
  points:    [{ id: string, at: number }]  // optional, omitted when empty
}
```

`points` records foreign deletions that this change swallowed. When an edit
deletes a range that holds another change's deleted text (a pure deletion
point inside the range, or a foreign insertion that the edit deletes whole and
that collapses to a point), the change the editor owns after the edit lists it
as `{ id, at }`. `at` is the number of original characters of `deleted` that
precede that change's point in document order. The stored `at` is always in
UTF-16 units of the owner's `deleted` string. The browser uses it as is. Rust
counts code points internally, so it converts at its read and write
boundaries. A pure deletion that sits at the end of the deleted range, right
after it (`p == edit.to`), is recorded too, with `at` equal to the full length
of the owner's new `deleted` text before it; without it, two deletions
anchored on the same character would lose their order. An id already carried
over from a merged change is not listed twice. A merge carries the points of the changes it absorbs and
shifts each `at` by the deleted text that now lands before it.

Cursors:
- Inserted text `I` at `[from, to)`, non-empty: `start = text.getCursor(from, -1)`
  (attached to the first character of `I`, left side) and
  `end = text.getCursor(to - 1, 1)` (attached to the last character of `I`,
  right side). In the browser, `to - 1` is the start index of the last code
  point, so for a surrogate pair it is `to - 2`.
- Nothing inserted: `start = text.getCursor(from, -1)`, attached to the
  character after the deletion point; at the end of the text Loro returns a
  cursor with no id, which stays at the end. `end = null`.
- Resolving: a Loro cursor resolves to the index of the character it is
  attached to, whatever its side. So `from = pos(start)` and
  `to = end ? pos(end) + w : from`, where `w` is the width of the character at
  `pos(end)` (2 UTF-16 units for a surrogate pair in the browser, else 1). When the attached character was deleted,
  Loro answers with the position it had and an `update` cursor (found by
  walking history, which is slow and fails after a trim); then `to = pos(end)`
  without the `+ 1`. If `to < from`, the inserted text is gone: treat it as
  `to = from`.
- Units: in loro-crdt JS, `getCursor`, `getCursorPos`, `insert` and `delete`
  all use UTF-16, the same as CodeMirror, so the browser converts nothing. In
  Rust, `get_cursor`, `get_cursor_pos`, `insert` and `delete` use Unicode code
  points. The planner works in one unit throughout (UTF-16 in the browser,
  code points in Rust).
- Healing: `prune` also rewrites an entry whose cursor came back with an
  `update`, storing the updated cursor, so a deleted anchor is looked up the
  slow way once. The planner marks every change the edit overlaps as `set`, so
  its cursors are written fresh against live characters.

A change is **orphaned** when its file is gone or its `start` cursor cannot be
resolved. A change is **empty** when `from == to` and `deleted == ""`. Both
are left out of every list. An editor that sees one removes it in its next
local commit (`prune`).

## The planner (pure, shared by JS and Rust)

Input: the resolved changes of one file (`{id, who, author, run, from, to,
deleted}`, sorted by `from`), one edit `{from, to, insert}` in the coordinates
of the text before the edit (the deleted text is `before.slice(from, to)`),
the editor `me = {who, author}`, `run`, `now` and a fresh id source.

Output: the full new list of changes for that file in the coordinates of the
text after the edit, each marked `kept` (unchanged), `set` (new or changed) or
`removed`.

Rules:
1. **Deleting inside any pending insertion only removes characters.** That
   includes another author's insertion: deleting it shrinks their change.
   Nothing is recorded as deleted for those characters.
2. **Deleting original text records it.** Each run of original characters
   (outside every change's `[from, to)`) inside `[edit.from, edit.to)` is text
   that `me` deleted.
3. **Affected own changes** are `me`'s changes whose `[from, to]` touches
   `[edit.from, edit.to]`, endpoints included. A pure deletion (`from == to`)
   touches when its point is inside or at an end of the range.
4. If the edit deletes original text or inserts text, and there is no affected
   own change, a new change is made for `me`.
5. All affected own changes, the original text deleted between them, and the
   new insertion merge into **one** change `M` (the lowest-`from` affected
   change keeps its id; the others are `removed`). `M.deleted` is the
   concatenation, in document order, of: each affected change's `deleted`
   (placed at that change's position) and each deleted original run.
   `M.from` and `M.to` span everything that remains of the merged changes plus
   the inserted text.
6. **Inserting strictly inside another author's insertion** (`other.from <
   edit.from < other.to`) splits theirs: `other` keeps `[other.from,
   edit.from)` and its `deleted`; a new change with the same `who`, `author`,
   `run` and `at` takes `[edit.from, other.to)` with `deleted = ""`. `me`'s
   insertion goes between them.
7. Inserting at an end of another author's insertion, with no own change
   affected, is a new change for `me`; the other change does not grow.
8. Every change shifted by the edit keeps its id and is `kept` (its cursors
   still hold); only `M`, split halves and new changes are `set`.
9. Changes that end up empty are `removed`.
10. A multi-range edit (one CodeMirror transaction with several ranges, or one
    agent request with several edits) is planned range by range, in order,
    each range's coordinates adjusted by the ranges before it, exactly as
    `web/vendor/loro-codemirror/sync.ts` writes them.

Fixtures: `web/tests/fixtures/track-changes.json`, an array of
`{name, before, changes, me, edits, after, expect}` where `before` and `after`
are the file text, `changes` and `expect` are resolved lists (`id`, `who`,
`from`, `to`, `deleted`; new ids are written `"new-1"`, `"new-2"`, ... in order
of creation). Both the JS and the Rust planner run every fixture. At least:
typing into empty text; typing that extends one's own change at either end;
replacing a word (one change, `deleted` = the word); backspacing into original
text before one's own insertion (`deleted` prepended); forward-deleting after
it (`deleted` appended); deleting a selection spanning two own changes and the
text between (one change); deleting all of one's own insertion (removed);
deleting inside another author's insertion (theirs shrinks, nothing recorded);
inserting inside another author's insertion (split); inserting at the end of
another author's insertion (separate change); an edit far from any change
(other changes `kept`, shifted); a multi-range edit; astral characters
(UTF-16 vs code points, so the Rust run converts fixture offsets: fixtures are
in UTF-16).

## Decisions

Both are one local Loro commit by the deciding editor:
- `accept(doc, id)`: `changes.delete(id)`.
- `reject(doc, id)`: resolve; `text.delete(from, to - from)`;
  `text.insert(from, deleted)`; for each entry of `points` whose change still
  exists and is still a pure deletion, rewrite that change's `start` to
  `text.getCursor(from + at, -1)` with `end = null`; `changes.delete(id)`.
  If the change being rejected is itself listed in the `points` of another
  pending change `O` (same file), `O` splits at offset `a`: the part holding
  `deleted[..a]` becomes a pure deletion anchored on the first character of
  the restored text, and the part holding `deleted[a..]` keeps `O`'s inserted
  span, or, with none, anchors on the character after the restored text. `O`
  keeps its id on the left part, or on the right part when the left is empty;
  the other part gets a new id with the same `who`, `author`, `run` and `at`.
  A part with no deleted text and no inserted span is dropped. `O`'s other
  points go left with their offset when `at < a`, else right with `at - a`.
  This keeps the order of two deletions that were anchored at the same
  character, so rejecting them in any order restores the original text.
- `rejectMany(doc, ids)` (Rust `reject_many`) rejects several changes in one
  commit. A selected change whose `points` list another selected change is
  rejected before that one; this repeats until none is left, since owners can
  nest. When rejecting a change splits an owner that was not selected, the
  owner's parts stay pending. It returns the number rejected.
- On `accept`, the `points` go with the record.

They are plain document edits: they sync through the normal update path, need
no server message and no table, and the reviewer's undo takes them back. Two
editors rejecting the same change at the same moment both apply the inverse;
the `deleted` text then appears twice. That race is accepted.

## Browser

New module `web/src/lib/track-changes.js` (the planner, `resolveChanges(doc,
fileId)`, `listChanges(doc)` for every file, `recordEdits(doc, fileId,
preResolved, edits, me, run)`, `accept`, `reject`, `rejectMany`, `prune`). It replaces
`web/src/lib/proposals.js`, which is deleted.

Editor (`web/src/components/Editor.svelte`):
- No fork, no `boundDoc()`, no branch undo managers, no flush, no rebind. The
  editor always writes into `session.doc`.
- Tracking is a flag. When it is on, a CodeMirror ViewPlugin installed right
  after `LoroExtensions` takes each local, user transaction (skip the ones
  annotated by the sync plugin or by undo, the same test as `isOwn` in
  `sync.ts`), plans it against the changes resolved before it, writes the
  `changes` entries with cursors made against the updated `LoroText`, and
  commits. The undo manager merges commits within a second, so the typing and
  its record undo together.
- Insert and the Zotero import write through `recordEdits` when tracking is
  on.
- Pending changes are drawn from the document for the shown file: inserted
  text as a mark tinted by author (`.change-added`, `.change-mine` for
  `me`), deleted text as a struck-through widget at `from`
  (`.change-removed`). The selected change gets `.change-selected`. Redraw on
  every doc change, local or imported.

Reader and Changes panel:
- Rows come from `listChanges(session.doc)`, recomputed on doc changes (keep
  the existing 200 ms coalesce): `{id, file_id, path, author, who, run, at,
  position, before: deleted, after: inserted text}`.
- Accept and Reject call `accept`/`reject` on `session.doc`. Bulk accept and
  reject, select mode, keyboard (J/K/A/R), filters by author and file, and
  reveal stay. Add a filter by run when any row has one.
- Gone: proposal messages and routing, `openProposals`, `decidedHunks`,
  `recomputeReview`, `markContention` and contested groups, stale blocking,
  "Discard change", the read preview of chosen proposals, the recovery
  download, `trackedProposalStatus` and its persistence warnings, the
  stale-suggestion merge editor. `MergeEditor` stays for History.
- Readers without edit rights see the rows with no buttons.
- The run-code-style rule still applies: tracking is off on every open.

Comment suggestions go away entirely: no `editing` motivation, no
suggestion cards, no "suggestions" filter, no writing-pass reject group, no
suggestion painting in the preview frame or the PDF viewer
(`web/src/agent/frame.js`, `frame-overlays.js`, `pages/viewer.html`), no
`suggestion-review` frame message. The preview shows the text as it is.

## Server

Deleted:
- `crates/librepaper-room/src/room/proposals.rs` (move `ActionRefusal` and
  `validate_existing_action` into `comments.rs`; `pending_proposal` goes),
  `proposal_round_trip_tests.rs`, `crates/librepaper-document/src/document/hunks.rs`,
  `crates/librepaper-engine/src/storage/postgres/proposals.rs`.
- Every `proposal-*` message, handler, broadcast and the `label_by_request`
  retry path that only proposals used.
- Suggestion comments: `editing` motivation, `proposed`, `outcome`,
  `AddComment`'s editing path, `RefineSuggestion`, `AcceptSuggestion`,
  `RejectSuggestion`, `BatchSuggestion`, `AgentSuggestionBatch`,
  `enrich_proposed`, `POST /api/documents/{slug}/suggestions`, the `refine`
  room command and message, the `editing` gates in `documents.rs` and
  `socket.rs`.
- The trim refusal while proposals are open.

Migration `crates/librepaper-engine/migrations/postgres/0005_track_changes_in_document.sql`
(0004 is taken by the OIDC branch): delete `kind = 'suggestion'` annotations;
drop the `annotations_suggestion_proposal` and
`annotations_document_proposal_fkey` constraints and `annotations.proposal_id`;
narrow the `kind` CHECK to drop `'suggestion'`; drop
`document_proposal_outcomes`, `document_proposal_hunks`, `document_proposals`.
Update the TRUNCATE lists in `testing.rs` and `deployment_gates.rs` and the
schema regression test.

New: `crates/librepaper-document/src/document/track.rs`, the Rust planner and
the Loro writer, running the shared fixtures:

```rust
pub struct Author<'a> { pub who: &'a str, pub author: &'a str }
/// Applies `edits` (code-point offsets into the file as it is now, planned in
/// order) to the file's text and records them as tracked changes.
/// Returns the ids of the changes it created or changed.
pub fn apply_tracked_edits(doc: &LoroDoc, file_id: &str, edits: &[Edit],
    by: &Author, run: Option<&str>, now_ms: i64) -> Result<Vec<String>, String>;
pub fn resolve_changes(doc: &LoroDoc, file_id: &str) -> Vec<ResolvedChange>;
```

`Edit` is the existing `session::edits::Edit {at, delete, insert}`.

Agents (MCP):
- `document_propose` with `publish: "suggestions"` applies its edits as
  tracked changes through a room command modelled on `AgentPatchCommand`
  (`room/agent.rs`), authority **Editor** (suggesting is an editor's act),
  `who = "agent:<agent name>"`, `author` = the agent's display name, `run` =
  the pass id. Effects become `{kind: "change", id}`.
- `document_comment` loses `refine` and `reject`. `mcp_accept` goes.
- `INSTRUCTIONS`, `tools.json`, `crates/librepaper-companion/src/assistant/`
  (`writing.md`, `context.rs`, `journal.rs`, `registry.rs`, `runtime.rs`
  tests) say "tracked changes" and read `{kind: "change"}`.

## Docs

`docs/architecture/review.md` "Track changes" and
`docs/dev/protocol/room.md` describe this model. Remove proposal and
suggestion wording from `docs/architecture/document.md`,
`local-service.md`, `what.md`, `agents.md`, `backups.md`, `privacy.md`,
`desktop-app.md`, `dev/protocol/chat.md`, `dev/protocol/comments-v1.md`.

## Accepted limits

- Concurrent rejects of one change duplicate its `deleted` text.
- Editing inside another author's insertion is not nested: a deletion there
  shrinks it, an insertion splits it.
- A rejected deletion comes back as new characters; comments anchored to the
  old characters do not reattach exactly.
- After a history trim, a change whose cursor pointed at a character deleted
  before the trim may be orphaned and is pruned.
