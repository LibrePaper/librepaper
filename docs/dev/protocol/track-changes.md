# Tracked changes in the document

This describes how pending tracked changes are stored in the shared Loro document and how the planner applies edits to them. `crates/librepaper-document/src/document/track.rs` and `web/src/lib/track-changes.js` are twin implementations, and both run `web/tests/fixtures/track-changes.json`. The user-facing model is in `docs/architecture/review.md`.

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

Key: a UUID v4 string. Value: a **map container** (`setContainer`), never a
plain value, so that two peers writing different keys of one record at the
same moment both keep their write. Two kinds, told apart by `kind`:

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

{ kind: "fragment",
  file:   string,
  change: string,          // the change this deleted text belongs to
  text:   string,          // one piece of deleted text
  start:  Uint8Array,      // Cursor on the character right after it, or the end of the text
  rank:   number,          // order among the fragments at one point
  who, author, run, at     // copied from the change, so an orphan can stand on its own
}
```

**Gaps are derived, not stored.** The fragments of a file that resolve to one
offset form the gap at that point, in `rank` order (then by id). No record is
ever moved from one gap to another: a gap merges with its neighbour when the
text between them goes, because their fragments then resolve to the same
point, and it splits when text comes back between them. Each peer writes only
its own new records and the cursor or rank keys it has reason to change.

Ranks: the planner hands a gap's fragments in document order. Fragments
already carrying ranks that rise in that order keep them; the others take a
value between their neighbours (midpoints, or one past the end), so a peer
adding a fragment beside another's does not move it.

Joins and rewrites: the planner never rewrites or drops a stored fragment.
Adjacent pieces of one change join into one fragment only while neither is
stored yet; a stored fragment keeps its id and its rank through a walk, so
writing a gap back changes nothing about it unless the order asks for a new
rank. A stored fragment may be extended by its author's other session, or its
change decided by a reviewer, at any moment, and a rewrite or a drop here
would lose that peer's write. Rows concatenate fragments either way. The
planner never drops a change record either: a span emptied by an edit, the
author's own included, stays as an empty record that prune drops once it has
rested.

A fragment never sits inside a span in one peer's sequence of edits, but a
span its author extends leftward while another peer anchors a fragment on its
first character leaves one there after they sync. On read, such a fragment is
placed at the span's start: its text preceded the span's then first
character.

Cursors and the write discipline:
- Span `[from, to)`: `start = text.getCursor(from, -1)`, `end =
  text.getCursor(to - 1, 1)` (in the browser `to - 1` is the start index of
  the last code point, so `to - 2` for a surrogate pair). Fragment at `p`:
  `start = text.getCursor(p, -1)`. At the end of the text Loro returns a
  cursor with no id, which stays at the end.
- Resolving: a cursor resolves to the index of its character whatever its
  side, so `from = pos(start)`, `to = pos(end) + width`, `fragment.at =
  pos(start)`. When the character was deleted, Loro answers with the
  position it had and an `update`; then `to = pos(end)`, which is the
  exclusive end already. If `to < from` the span is gone: `to = from`.
- A cursor is rewritten only when its side of the span, or the fragment's
  point, would resolve elsewhere than the stored cursor does. A span that
  shrank therefore needs no write: its stale cursor resolves to the right
  place. A span that grew writes the end that moved. So a peer deleting the
  last character of a span and its author typing after it at the same moment
  both keep their write, and the span comes out right. Nothing is written
  over a cursor with null: a span emptied in one view may have grown in
  another; the record stays and prune drops it later if it is still empty.
- A cursor left on a deleted character is sound until text is inserted
  exactly at its point, where it could drift across the new text. Whoever
  inserts there knows it: the planner marks every span with a boundary at an
  insertion point `set`, and reject re-pins the spans at the point where it
  puts text back. The write happens only if the cursor drifted.
- Units: loro-crdt JS and CodeMirror count UTF-16; Rust counts code points.
  Each planner works in its own unit throughout. Fragment texts are strings
  and need no conversion.

Settling: `prune` drops an empty record (a change with no span and no
fragment) only once it has rested empty for a minute, so that its author's
other session has had time to sync what it was doing to it. The rest starts
when a prune first sees the record empty, which it marks on the record
(`emptied`), not at the record's last edit: an old insertion deleted a moment
ago still gets its grace. A record found live again loses the mark. A stale
cursor is healed onto a live character the same way: the first prune that
sees it stale marks the record (`stale`), and a later prune heals it once the
mark is a minute old. The grace is not counted from the record's `at`: the
author may be moving that very end of the span in a session that has not
synced yet, and an immediate heal would race it. A cursor found live again
loses the mark, and a heal removes it. Healing is a forced write (`heal`): the
ordinary suppression of a cursor write whose position would not change does
not apply, so a settled stale cursor is looked up the slow way once and then
rests on a live character.

Orphans: a fragment whose change was decided while the fragment was written
has no change record. It is read as a change of its own, under the fragment's
id, with the author fields the fragment carries, and decided like any other:
accept drops it, reject puts its text back. Nothing is written for that, so a
reader who has never edited sees the same row as the author does, and
`prune` has nothing to repair. The fragment keeps pointing at the change it
had.

Absorbed changes: when an edit joins the author's own deletions into one
change, the records whose fragments it relabels are not deleted. They are
planned `kept` with `emptied`, and `prune` drops them once they have rested
empty, like any other emptied record. Deleting them at once would discard
what the author's other session was adding to them. Spans are never absorbed:
a record whose span survives is still a change of its own, since a span lives
in its record's cursors and cannot be relabelled without a write that would
race the author's other session. A change whose span the edit deleted whole
but whose fragments remain is a deletion from then on, not an empty record.

Rows: a change's `deleted` text is the concatenation of its fragments in
document order, across gaps. Its position is its span, or the gap of its first
fragment. A change whose fragments sit in two gaps (another author's text was
put back between them) is still one row.

Concurrency, stated exactly: two peers editing at the same moment, anywhere,
lose no tracked text and converge. Fragments at one point keep document order
unless the two peers edited the very same point at the same moment, when the
ranks they assigned independently can interleave; both orders are defensible
and nothing is lost. Two editors deciding the same change at the same moment
still restore its text twice, as before.

Records of the earlier model (plain values with no `kind`, a `deleted` string
with `start` and `end` cursors, and an optional `points` list of swallowed
pure deletions with their UTF-16 offset `at` in `deleted`) are **read in
place**: both planners expand them, when resolving a file, into the change
and the fragments they stand for (the owner's `deleted` split at each point's
`at`, the pointed deletions between the pieces, nested points inside their
inner owner; fragment ids `<owner id>:<index>`, rank the index, anchored on
the owner's `start`). Nothing is written on open, so two peers opening an old
document never race. An old record is rewritten as containers (`convertLegacy`:
the owner, the records it points at, and their fragments, under those same
ids) the first time something has to be written to it: a planner write that
changes one of its fragments or its span, or a decision on it. The one race
left is two peers converting the same old record at the same moment, which is
the same race as two peers editing one change at once and is rarer still.

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
3. **Affected own changes** are the first of `me`'s spans touching `[ef, et]`
   (ends included), by position, and `me`'s changes with no span that have a
   fragment in a gap at a position in `[ef, et]`. One change holds one span,
   and spans never merge: a second own span touching the edit is handled like
   any other author's span (rule 1), so it shrinks, splits or moves and keeps
   its record and its text. A span of `me`'s elsewhere stays its own change
   even when a fragment of it sits at the edit, and a span never widens over
   text that is not its own. The affected changes and the new edit make one
   change, which keeps the lowest id by position; the other affected deletion
   changes are `kept` as emptied records (see Absorbed changes), and their
   fragments in every gap of the file, not only the swallowed ones, take the
   merged id (those gaps are `set`). If the edit deletes original text or
   inserts text and no own change is affected, a new change is made.
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
5. A change with no span and no fragment left is `kept` with `emptied`; its
   record waits for `prune`. A gap with no fragment is `removed`.
6. A multi-range edit is planned range by range, in order, each range in the
   coordinates left by the ranges before it, exactly as
   `web/vendor/loro-codemirror/sync.ts` writes them.
7. **Untracked edits are planned too** (`untracked: true`), by the browser
   for every local edit while tracking is off, and by the server for an
   agent's untracked patches (`track::untracked_edits`), when the file has records. An
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
  the rejected change is inserted into the text where the walk stands and its
  record deleted; each run of other changes' fragments is re-anchored on the
  character that now follows it (their ranks unchanged, raised where two gaps
  collapsed onto the span's point). Spans with a boundary at the point are
  re-pinned. Then `changes.delete(id)`.
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
