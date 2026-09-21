# Bounded comment transport (`librepaper.comments.v1`)

How a comment collection of any size is read, over HTTP, over the room
socket, by an agent and by `librepaper export`, without any consumer ever
assembling the whole thing.

This replaces the whole-snapshot protocol: the `hello` frame that carried
every comment, the `{"type":"comments"}` broadcast that restated every
comment, `Room::comments()`, `Room::snapshot_for()`,
`room::comments::load()` and the 16 MiB `COMMENT_SNAPSHOT_BYTES_MAX` read
guard that refused a collection larger than one process wanted to hold.
None of those exist any more. There is no compatibility path: this is an
unreleased protocol change and every consumer in the tree was moved.

This is a transport change only. Nothing here limits how many comments a
document may accumulate; admission (`config.max_comments`,
`config.max_replies`, `config.rate_per_hour`) is a separate, still
undecided product question recorded in REVIEW-BIG-IDEAS.md.

## 1. Pages and cursors

Two independently bounded traversals.

**Annotations** are ordered by `(created_at, id)` ascending, which is the
`annotations_timeline` index and is stable: neither column is ever
rewritten. `id` is the deterministic tie-breaker, and it is load-bearing:
several comments written in one transaction share `now()` to the
microsecond.

**Replies** of one thread are ordered by `(created_at, id)` over
`replies_timeline`, cursored per thread. A thread with a hundred thousand
replies costs one reply page, not one annotation page: the annotation page
carries at most `REPLY_PREVIEW` replies per comment and a per-thread
cursor for the rest.

A cursor is base64url of

```json
{"v":1,"d":"<document uuid>","t":"<thread uuid>"|null,"o":"<options hash>",
 "at":<microseconds since the epoch>,"id":"<row uuid>"}
```

and is refused unless every one of `v`, `d`, `t` and `o` matches the
request it is presented with. `o` is a hash of the query options that
change which rows are eligible -- today just `suggestions`, whether the
caller may see `kind='suggestion'` rows. A cursor is a position, not a
capability: authorization runs in full on every page request, before the
cursor is even decoded, so a cursor lifted from another session, another
document or another thread buys nothing.

## 2. Row and byte bounds

| Bound | Value | Where |
|---|---|---|
| `COMMENT_PAGE_DEFAULT` | 50 comments | `room/comments.rs` |
| `COMMENT_PAGE_MAX` | 200 comments | requested `limit` is clamped |
| `REPLY_PREVIEW` | 10 replies per comment in an annotation page | |
| `THREAD_PAGE_DEFAULT` / `THREAD_PAGE_MAX` | 50 / 200 replies | thread route |
| `PAGE_BYTES_MAX` | 512 KiB of encoded content per page | both |

Rows are not fetched and then measured. A page is chosen in two steps:

1. a *sizing* query reads `(created_at, id, octet_length-sum)` for up to
   `limit` candidate rows. That result is bounded by construction,
   200 rows of three fixed-width values -- whatever the rows contain.
2. the byte budget is spent over that prefix, and only the rows that fit
   are read in full.

So a page never materializes a row it is going to discard. The existing
field sizes this accounts for are the ones with no server-side ceiling on
already-stored data: `body`, `exact`, `prefix`, `suffix` and the three
`rendered_*` columns.

**One row always comes back.** A single stored comment can exceed 512 KiB
on its own -- nothing ever refused one -- and a page that returned nothing
would make the rest of the collection unreachable. So a page that cannot
fit even its first row returns exactly that row and sets `"oversize":
true`. Peak per-page memory is therefore `PAGE_BYTES_MAX` plus one row,
not `PAGE_BYTES_MAX`.

## 3. Consistency during concurrent change

**The contract: a bounded keyset traversal, not a point-in-time
snapshot.** No transaction, connection or Postgres snapshot is held across
a page boundary. Each page sizes and fetches rows in a short repeatable-read
transaction, so concurrent edits or deletions cannot invalidate its memory
estimate. The connection is returned before the response is written.

What that guarantees, over the whole traversal:

- a row that existed when the traversal began and still exists when it
  ends is returned **exactly once**;
- no row is ever returned twice, at a page boundary or anywhere else,
  because the cursor is a strict `>` on an ordering no row ever moves
  within;
- a row **created** during the traversal can be returned if it becomes
  visible ahead of the cursor before the final page is read. Creation time
  is a transaction timestamp, not a commit-order guarantee; concurrent
  creations are not guaranteed to appear in this traversal;
- a row **deleted** during the traversal simply stops appearing;
- a row **edited** or **resolved** during the traversal is returned in
  whatever state the page that reaches it read. `created_at` does not
  change, so the edit cannot move the row across a boundary.

That is weaker than a snapshot and the protocol says so out loud rather
than pretending otherwise: every page carries a `state` block with the
authoritative `total`, `open`, `replies` counts and a `revision` token
over them plus `max(updated_at)` of the document's annotations and
replies. A client that sees `revision` change between pages knows the
collection moved under it; what it does about that is in §5.

`complete` is explicit and is never inferred from a short page: it is
true exactly when the sizing query proved there is no further row.

## 4. Live events and the page traversal

The room socket is the invalidation channel; it is not a second copy of
the collection.

- `hello` carries the **first page** plus `state`, and is sent *after*
  the socket is registered as a subscriber. Every mutation committed from
  that instant on is relayed as its own bounded event, so the handover
  from "loading pages" to "live" cannot drop one: a create that races page
  1 arrives either in a later page or as a `comment` event, and the client
  keys on comment id, so the overlap is a no-op rather than a duplicate.
- the per-mutation events (`comment`, `reply`, `resolve`, `delete`,
  `refine`, `accept`, `reject`) name one comment or one reply and carry
  viewer-specific authoritative `state` counts, including mutations outside
  the browser's loaded prefix.
- the old `{"type":"comments", "comments":[...everything...]}` restatement,
  which an agent's multi-item suggestion batch sent, is now
  `{"type":"comments-changed","state":{...}}`. It says the collection moved
  and how large it is; it carries no rows.
- `{"type":"attachments"}` re-anchoring frames are chunked at
  `ATTACHMENT_FRAME_MAX` (200) entries.

## 5. What the browser keeps

The reader holds a **prefix** of the collection: page 1 from `hello`, and
whatever further pages the reader asked for by pressing *Load more*.
Nothing drains pages automatically, and a thread's replies past the
preview are fetched by *Show more replies* on that card.

- counts come from `state`, never from `list.length`, so the panel header
  says `7 open · 412 total · 50 loaded` without having loaded 412 rows.
- a failed page request changes nothing on screen: the already-visible
  comments stay, and an inline error with a *Retry* action appears under
  the list.
- the four states are distinguished: `loading` (nothing yet),
  `empty` (`total == 0`), `partial` (`complete == false`) and
  `complete`.
- every request carries a monotonic generation and the slug it was issued
  for. A response whose generation is stale, or whose slug is not the one
  currently open, is dropped rather than applied. (Document switch is a
  page reload in this client, but the guard is cheap and the reload is
  asynchronous.)
- on `comments-changed`, or on a reconnect that delivers a fresh `hello`,
  the client re-reads from page 1 forward over as many pages as it
  currently holds, capped at `REFRESH_PAGES_MAX` (8). Beyond the cap it
  truncates and shows itself as partial rather than silently claiming to
  hold the rest.

## 6. Exports

`librepaper export DOCUMENT` streams. It walks the annotation cursor, and
for every thread whose replies are incomplete it walks that thread's reply
cursor, writing each comment to the output as it is rendered. The three
renderers (`jsonld`, `markdown`, `response`) were split into
header/item/footer so nothing collects a `Vec<Comment>` of the whole
document on either side of the wire.

- the server holds one page's rows and one pooled connection per request,
  released before the body is written, so a slow consumer cannot pin a
  database connection;
- output goes to a temporary file, written as each comment is rendered and
  renamed into place only after the traversal finishes. A failed page
  aborts with a non-zero exit and no output file at all, not even the
  temporary, so a partial export can never be mistaken for a complete one.
  A pipe cannot be taken back: to stdout the bytes go as they are rendered,
  and what says the export is not complete is the message on stderr and the
  non-zero exit status;
- consistency is §3's contract. The export also compares the rows it
  wrote against the authoritative `total` the first page reported, and
  prints a warning naming both figures when they differ, which is what
  "the collection changed while this ran" looks like from the client.

## 7. Agents

`document_read` captures an immutable **source** view -- tree, texts,
digest -- and that is unchanged: `view_id` and `range_id` safety, and
`document_propose` / `document_apply`, all depend on the source capture
and on nothing else. What the view no longer holds is the comment
collection; it never needed to, because every edit-safety check
(`expected_version` on comment, reply, resolve, delete, refine, reject,
and accept's `comment_version` comparison) already read the comment live
from the catalogue rather than from the view.

So a `thread` query resolves a bounded page from the catalogue at request
time and the result says which: `complete`, `next_cursor`, and the
`comment_state` revision the page was read at. Continuation cursors bind
to that revision exactly as they bound to the old `comment_digest`, so an
agent that keeps paging across a change is told the cursor no longer
matches instead of quietly interleaving two different collections.

`comment_version`, the optimistic-concurrency token, is computed from the
annotation row plus its reply count and newest reply stamp -- never from
however many replies a particular page happened to carry -- so it does not
depend on page size.

## 8. Server memory, end to end

| Where | Bound |
|---|---|
| database result buffering | one sizing page (≤200 fixed rows) plus ≤512 KiB of content, +1 oversize row |
| per-room cache | no comment list. An attachment cache of at most `ATTACHMENT_CACHE_MAX` (1024) entries and `ATTACHMENT_CACHE_BYTES` (4 MiB), oldest evicted first, per room |
| concurrent page requests | `cost.work_concurrency` (64) HTTP work slots x one page each |
| snapshot clones / serialization | one page per response; no room-held list to clone |
| reply expansion | `REPLY_PREVIEW` per comment in a page; `THREAD_PAGE_MAX` per thread request |
| anchor reattachment | the attachment cache only; re-resolution is bounded by its size, not by the document's comment count |
| websocket payloads | `hello` = one page; every other comment frame names one row; attachment frames chunked at 200 |
| export buffering | one page in the server, one page in the client |
| agent capture storage | source only; no comments stored in a view |

Remaining limitations, stated honestly:

- the attachment cache is per room and there is no deployment-wide cap
  across rooms: N resident rooms can hold N x 4 MiB of cached anchors.
  Rooms are dropped when nothing holds them (`Rooms::housekeep`), so this
  is bounded by concurrent activity rather than by the catalogue.
- an evicted attachment costs a fork at the comment's frontier the next
  time that comment is paged in. This is the same cost a cold process
  already paid; eviction just makes it possible more than once.
- `state` counts every annotation and reply of the document. On a
  document with very many replies that is an index scan per page, `hello`,
  mutation event and invalidation. It is not performed for source-update frames.
- a page whose single first row exceeds the byte budget is served anyway
  (§2). There is no upper bound on one stored row's size, because nothing
  ever refused one.

## 9. Measured

Structure is an argument; these are the numbers. From
`room::comment_paging_tests::page_cost_does_not_grow_with_the_collection`,
which walks the same traversal over three collections of identical 4 KiB
comments and reports the *peak* cost of any one page:

```text
comment traversal: 50 comments   -> 1 page(s),  peak  50 rows, peak 218600 encoded bytes per page
comment traversal: 400 comments  -> 4 page(s),  peak 101 rows, peak 441572 encoded bytes per page
comment traversal: 3200 comments -> 32 page(s), peak 101 rows, peak 441572 encoded bytes per page
room attachment cache after a 3,200-comment traversal: 1024 entries
```

An eightfold larger collection costs the same page, to the byte: 101 rows
and 441,572 bytes both times. The 50-comment run is one short page and its
peak is the size of that document rather than the size of a page, which is
why the test compares the two multi-page runs and not that one. The cache
line is the other half: walking 3,200 comments leaves the room holding
`ATTACHMENT_CACHE_MAX` anchors, not 3,200.

Two more tests carry the rest of the claim rather than restating it:
`a_collection_past_the_old_snapshot_budget_is_still_readable` walks 40 MiB
of bodies -- well past what the old whole-snapshot guard refused outright --
and checks each page against the budget; and
`the_room_cache_stays_bounded_while_a_large_document_is_paged` pages a
document larger than the cache and checks the cache did not grow to match
it.
