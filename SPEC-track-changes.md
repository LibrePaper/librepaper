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
- A change is one author's edit of one region: text it inserted (a **span**)
  and original text it deleted. Either may be empty. Replacing "cat" with
  "tabby" is one change.
- Deleted text lives in **gaps**. A gap is one point in the text where text
  was taken out, and it holds the deleted texts that sat there as an ordered
  list of **fragments**, each owned by a change. There is one gap per point.
  Several authors deleting around the same place put their fragments in one
  gap, in document order, so whatever is accepted or rejected, and in whatever
  order, the text comes back in the order it had.
- Each change is decided on its own, in any order. Which session made it does
  not matter.
- `run` is an optional tag (an agent pass, for example). It serves filters and
  "accept all from this run". It never makes changes depend on each other.

## Storage: the `changes` root

A root map in the Loro document, `changes`, next to `files`, `paths`,
`assets` and `meta`. The projection and the render digest ignore it.

Key: a UUID v4 string. Value: a plain Loro value (not a nested container),
written whole with `map.set(id, value)` and removed with `map.delete(id)`.
Two kinds, told apart by `kind`:

```
{ kind: "change",
  file:   string,          // file id, a key of `files`
  author: string,          // display name, as comments show it
  who:    string,          // stable author key: account id when signed in, else the display name; agents use "agent:<name>"
  at:     number,          // ms since epoch of the last edit to this change
  run:    string | null,   // optional group tag
  start:  Uint8Array|null, // Cursor on the first inserted character, null when nothing is inserted
  end:    Uint8Array|null  // Cursor on the last inserted character, null when nothing is inserted
}

{ kind: "gap",
  file:      string,
  start:     Uint8Array,   // Cursor on the character right after the gap, or the end of the text
  fragments: [{ change: string, text: string }]   // deleted texts in document order, each owned by a change
}
```

A change with no span and no fragment in any gap is empty. A gap with no
fragment is empty. Both are left out of every list and removed by `prune`.

A replacement's gap sits on the first character of its span. Text another
author inserts right before the span then lands before the gap, and the
deleted text comes back where the span was.

Cursors:
- Span `[from, to)`: `start = text.getCursor(from, -1)`, `end =
  text.getCursor(to - 1, 1)` (in the browser `to - 1` is the start index of
  the last code point, so `to - 2` for a surrogate pair).
- Gap at `p`: `start = text.getCursor(p, -1)`. At the end of the text Loro
  returns a cursor with no id, which stays at the end.
- Resolving: a cursor resolves to the index of its character whatever its
  side, so `from = pos(start)`, `to = pos(end) + width`, `gap.at =
  pos(start)`. When the character was deleted, Loro answers with the position
  it had and an `update`; then `to = pos(end)`. If `to < from` the span is
  gone: `to = from`.
- Units: loro-crdt JS and CodeMirror count UTF-16; Rust counts code points.
  Each planner works in its own unit throughout. Fragment texts are strings
  and need no conversion.
- Healing: `prune` rewrites a record whose cursor came back with an `update`,
  so a deleted anchor is looked up the slow way once. The planner marks every
  record the edit touches as `set`, so its cursors are written fresh.

Rows: a change's `deleted` text is the concatenation of its fragments in
document order, across gaps. Its position is its span, or the gap of its first
fragment. A change whose fragments end up in two gaps (another author's text
was put back between them) is still one row.

Records of the earlier model (no `kind`, a `deleted` string with `start` and
`end` cursors, and an optional `points` list of swallowed pure deletions with
their UTF-16 offset `at` in `deleted`) are converted by the browser's `prune`.
Each becomes a change record. Its `deleted` text goes into the gap at its
anchor, split at each point's `at`, with the pointed change's text between the
pieces, and a pointed change's own points expanded inside it the same way; a
point that another point of the same owner also lists is expanded only inside
that inner one. A record nobody points at that has `deleted` text gets its own
gap. The server ignores old records until an editor has converted them.

## The planner (pure, shared by JS and Rust)

Input: the resolved state of one file, `{changes: [{id, who, author, run, at,
from, to}], gaps: [{id, at, fragments: [{change, text}]}]}` in the
coordinates of the text before the edit, the edits `{from, to, insert}`
(ascending, non-overlapping, each adjusted by the ones before it), the editor
`me = {who, author}`, `run`, `now` and a fresh id source.

Output: the same shape in the coordinates of the text after the batch, each
change and gap marked `kept`, `set` or `removed`.

Rules, for one edit `[ef, et)` with `insert`:
1. **Spans.** Deleting inside any span only removes characters, another
   author's included: their span shrinks, nothing is recorded for those
   characters. A span of another author that the edit cuts in two with inserted
   text splits: theirs keeps the left part, a new change with the same `who`,
   `author`, `run` and `at` takes the right part. A span shifted by the edit
   is `kept`.
2. **Original text** inside `[ef, et)` and outside every span is what `me`
   deleted. Each run of it becomes a fragment owned by `me`'s change.
3. **Affected own changes** are `me`'s changes with a span touching `[ef, et]`
   (ends included), and `me`'s changes with no span that have a fragment in a
   gap at a position in `[ef, et]`. A span of `me`'s elsewhere stays its own
   change even when a fragment of it sits at the edit: one change holds one
   span, and a span never widens over text that is not its own. The affected
   changes and the new edit merge into one change, which keeps the lowest id
   by position; the others are `removed`, and their fragments in every gap of
   the file, not only the swallowed ones, take the merged id (those gaps are
   `set`). If the edit deletes original text or inserts text and no own change
   is affected, a new change is made.
4. **Swallowed gaps.** Every gap at a position in `[ef, et]` merges into one
   gap with the new fragments. Walking `ef..et`, the gaps at `p` come first
   (in their order; an untracked edit elsewhere can have left two on one
   point), then the original character at `p`. Adjacent fragments of one change join.
   With nothing inserted the merged gap sits at `ef`. With text inserted it
   sits at the merged span's start, and the gap that sat exactly at `et`
   stays separate after the span, at `ef + insert.length`, since what it held
   came after the deleted text. That gap merges before the span instead when
   it holds one of `me`'s fragments, or when a span of `me`'s starts at `et`:
   then `me`'s deleted text belongs with the new change. A gap placed at a
   position that already has a gap (the span's start after a merge with an
   earlier own span) joins that gap after its fragments. There is one gap per
   position.
5. A change with no span and no fragment left is `removed`. A gap with no
   fragment is `removed`.
6. A multi-range edit is planned range by range, in order, each range in the
   coordinates left by the ranges before it, exactly as
   `web/vendor/loro-codemirror/sync.ts` writes them.
7. **Untracked edits are planned too** (`untracked: true`), by the browser
   for every local edit while tracking is off, when the file has records. An
   untracked edit records nothing and makes no change: spans still shrink,
   move and split around it, and the gaps it collapses merge in order at
   `ef`. Without this, deleting the text between two gaps leaves two gaps on
   one point with no order between them.

Fixtures: `web/tests/fixtures/track-changes.json`, an array of
`{name, before, state, me, edits, after, expect}`, plus `untracked: true` for
an untracked edit. `state` and `expect` are
`{changes: [{id, who, from, to}], gaps: [{at, fragments: [{change, text}]}]}`;
new change ids are written `"new-1"`, `"new-2"`, ... in order of creation and
gap ids are not compared. Offsets are UTF-16; the Rust runner converts. Both
planners run every fixture.

## Decisions

Both are one local Loro commit by the deciding editor, both work on the
rows' change ids:
- `accept(doc, id)`: remove the change's fragments from every gap (a gap left
  empty is deleted), then `changes.delete(id)`. The text stays.
- `reject(doc, id)`: resolve the file. The gaps to put back are every gap
  holding one of the change's fragments and, when the change has a span, every
  gap from the span's start to the character after it, which all collapse onto
  one point when the span goes. Gaps beyond the span come back first, at
  their positions, then the span's text is deleted, then the gaps at and
  before it. Putting a gap back: walk its fragments in order; each fragment of
  the rejected change is inserted into the text where the walk stands; each
  run of other changes' fragments becomes a new gap anchored on the
  character that now follows it. Then `changes.delete(id)`.
- `rejectMany(doc, ids)` rejects each in turn in one commit and returns the
  number rejected. Order does not matter.

They are plain document edits: they sync through the normal update path, need
no server message and no table, and the reviewer's undo takes them back.

The fuzz that pins this: random deletions, insertions and replacements by
random authors, then (a) every change rejected in random order gives the
original text; (b) every change accepted or rejected in random order leaves
each original character iff its first deleter was rejected, each surviving
inserted character iff its author was accepted, and original characters in
their order. A third run interleaves tracked edits, untracked edits and per-author
decisions, authors editing again after being decided, with one state per
character as the oracle. Both planners carry seeded versions in their unit
tests.

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
- The rendered preview does not paint pending changes. The code pane and the Changes panel are the two views, and both are exact; the source-to-rendered matching the comments use is a heuristic and is not used for changes.
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
- Two editors deciding different changes with fragments in one gap at the
  same moment race on that gap's record; the later write wins and the other
  decision's fragments may come back or vanish twice. Rare, and the same
  class of race as above.
