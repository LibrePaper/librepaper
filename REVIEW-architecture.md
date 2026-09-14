# The document model is the architecture

*2026-09-13. Started as "would this have been better in Go?" That turned out
to be the wrong question, and the right one is underneath it.*

## The wrong question

The server language is the least consequential decision in this system.

The two hard problems -- the document model and typesetting -- both have to run
in a browser, so both are forced to JS or wasm. Once they are, what remains on
the server is durability, authorization, fanout, identity and billing, which
any competent language handles. Arguing Rust versus Go versus Node for that
layer is optimizing the easy part.

See "Is this simpler? Is it more robust?" below before acting on any of this.

The consequential decision is whether **the document model is one artifact or
a compatibility surface**. Today it is a compatibility surface. Everything
below follows from that.

## First principles

### 1. What must the server actually do?

Rendering happens in the browser, through wasm. Native TeX happens on the
author's machine, through `local`. Merging is a CRDT, which is by definition
something peers do without a coordinator.

Strip those out and the server's irreducible jobs are durability,
authorization, fanout, identity and billing. That is a small, boring server.

Ours is ~67k lines:

```
local     16292   loopback service, tool discovery, subprocess running, pairing
server    15799   HTTP, routes, quota, cost, serve
cli       10461
room       8341   <- Yjs
storage    6472   Postgres, blob, worker, maintenance
document   4744   <- Yjs
auth       2627
```

This is not a language problem. The server was *able* to know about documents,
so over time it did. `document/session.rs` exposes ~35 functions over the
`Doc`, called from ~125 server-side sites.

### 2. The browser is the only hard constraint

A browser runs JS or wasm and nothing else.

The document model -- what a document is, how paths and texts and assets map
onto Y types, repair, merge -- is needed by three hosts: the browser, the
server, and the command line. Anything three hosts need should be **one
artifact**, not three implementations kept in step.

`document/session.rs` describes itself as "a compatibility surface: what is
encoded here is decoded there," and `tests/yjs.rs` exists to run a real browser
Yjs against it. The vendored word diff shared across the `wasm-*` repos is the
same problem in a second place. Those tests are the price of having answered
this question the other way.

The document model's language is therefore forced. Every other language choice
in the system is downstream of it.

### 3. Do not own the CRDT

CRDT bugs are silent, they corrupt data, and they do not reproduce. This is the
one component where a defect unnoticed for a month is unrecoverable.

Use an implementation with years of adversarial use behind it -- Yjs or yrs.
Treat any third option as risk taken in the worst available place.

### 4. Authorization cannot live in a structure clients write to

An invariant, not a preference.

From `room/revisions.rs`:

> Revision values deliberately live in the Yjs `revisions` map as JSON
> strings. This keeps the browser and Rust representations byte-compatible,
> while allowing the server to validate transitions before relaying an update.
> Decision transitions are performed by the server command path; clients must
> not be able to forge accepted/rejected history in a raw `y-update`.

The forging surface exists because the decisions are in the CRDT. Byte
compatibility was the convenience; server-side transition validation inside a
`TransactionMut` is the bill.

### 5. A room is a pure function of a log

Durable ordered log, plus stateless fanout. That is infrastructure, not
application code.

From `room/mod.rs`:

> One process, so the mutex plays the part a single-threaded actor would.

A documented scaling ceiling, and a hand-built actor standing in for one the
language does not provide.

### 6. Enforcement does not have to be synchronous

`admit_update(doc, update, ceiling, max_files)` parses a document on the hot
path to enforce a quota. A byte ceiling does not need a parsed document, and
structural limits do not need per-update precision -- they need the abuser
stopped within seconds.

## The system that falls out

**One document artifact.** The document model over `yrs`, compiled to wasm and
published beside `wasm-typst`, `wasm-markdown`, `wasm-bibliography` and
`wasm-helpers`. The browser loads it. Every server host loads the same bytes.
Drift stops being something we test for and becomes something that cannot
happen.

**A small stateless server.** Auth, log, blob, fanout, quota, billing.
Perhaps 10-15k lines once it knows nothing about documents. Chosen for ops and
contributors, because at that size nothing else distinguishes the candidates.

**Postgres holds what must not be forged.** Review decisions, permissions,
quota, publication state.

**Three clients, one artifact.** Browser, command line, and the local TeX
daemon. `local` is a separate product -- different users, different platform
matrix, different security model, different release cadence -- and belongs in
its own binary and probably its own repo.

## Consequences, in order of independence

These are derivations, not a wish list. The first three need no decision about
languages or runtimes and are worth doing regardless.

1. **Move review decisions out of the CRDT into Postgres.** (From principle 4.)
   Deletes the forging surface rather than defending it. Cost: offline edits
   can no longer carry decisions, which may be a product constraint we want.
2. **Split quota into wire-level and structural.** (From principle 6.) Count
   bytes on the socket synchronously; materialize periodically in a worker for
   `max_files` and structural limits.
3. **Split `local` into its own binary.** (From the system above.) Also the
   direct fix for the build loop: the 12-minute suite, the timeout flakes
   under load, the worktree strategy and the `debug = "line-tables-only"`
   profile are all symptoms of one very large test binary. Decomposition fixes
   that; a debuginfo flag manages it.
4. **Resolve anchors on the client.** (From principle 1.)
   `sticky_index_at_path` and `offsets_of_sticky_indices` resolve against live
   state server-side, but we already store a TextQuoteSelector -- `exact`,
   `prefix`, `suffix` -- per the W3C model `room/mod.rs` follows. That anchor
   is durable and position-independent, and the browser already holds the
   document.
5. **Make the room a log plus stateless fanout.** (From principle 5.) Removes
   the single-process ceiling.
6. **Publish the document model as wasm.** (From principle 2.) The one that
   needs a real decision, because it is the one that reopens the server
   language for the other 80% of the code.

Note the closing move: after (2), the server never parses an update on the hot
path. It needs the document only for batch work -- publish, restore,
checkpoint, export. At that point wasm execution cost is irrelevant, in-process
versus sidecar is irrelevant, and the server language is irrelevant. The
sequence is self-unblocking.

## Is this simpler? Is it more robust?

Written last, and less flattering than the rest of the document. The honest
answer is **more correct, not much simpler.**

### Three of the six simplify. Three are trades.

Simplifying:

- **(3) Split `local`.** 16k lines leave the compile unit, the one very large
  test binary breaks up, the build loop stops hurting. Unambiguous.
- **(1) Decisions to Postgres.** Deletes code rather than moving it: the
  forging surface and the validation defending it both go.
- **(2) Quota split.** Deletes a document parse from the hot path.

Trades, and they should not be described as simplifications:

- **(5) Log plus fanout** replaces a mutex with a distributed log. A single
  process holding a mutex cannot have split-brain, duplicate delivery, replay
  bugs or compaction. All four arrive with the log, in exchange for a ceiling
  we may not be near.
- **(6) Wasm artifact** relocates the document logic behind a boundary rather
  than removing it, and adds a server-side wasm packaging pipeline plus
  cross-repo version coordination to a project that already has ABI drift held
  together by convention.
- Total line count probably barely moves. Fewer lines in the server, more
  parts in the system.

### Robustness is bought, not free

Removed as classes of bug: drift between the browser and server
implementations, forged review decisions, the documented single-process
ceiling.

Added: every failure mode a distributed log has, and a host boundary that can
version-skew. Consequence 2 also trades exact quota enforcement for eventual
-- a window in which someone exceeds a limit.

### What to actually do

- **(3) first, and alone.** It is the largest practical improvement in this
  document, and it is a build-speed fix rather than an architectural insight.
- **(1) and (2) next.** Small, self-funding.
- **(4) when convenient.**
- **(5) not yet.** Insurance against a problem we may never have, priced in
  permanent operational complexity. Revisit when the ceiling is real.
- **(6) is the only genuine bet**, and the case for it is not the language
  question. It is that the drift problem is already real: a vendored word diff
  coupled across repos by convention, and an interop suite that exists because
  two implementations must agree. If that is costing us now, do it. If it is
  not, the elegance of the argument is not a reason.

## Appendix: the language post-mortem

Kept because the reasoning was wrong in instructive ways.

**Go with a Node sidecar.** Dismissed at first on the grounds that ~125
server-side call sites touch the document, so the boundary would have to sit
between the server and the document rather than between the server and the
socket. True of the current structure, false of the one above. Sorting the
calls shows a clean seam:

- *Hot path, per update or per connect:* `apply_update`, `encode_state`,
  `encode_vector`, `encode_diff`, `admit_update`, `rehearsed_encoded_len`.
  This is the sync protocol, and `y-websocket` already implements it.
- *Coarse and latency-tolerant:* `texts_of`, `assets_of`, `text_of`,
  `put_text`, `restore`, `repair`, `replace_text`, `put_asset`, `paths_of`,
  `main_path`. Publish, restore, checkpoint, export, seed.

`room` + `document` is 13k of 67k, and consequences 1 and 4 move part of that
out. A Node component would be ~6-8k lines, roughly 10% of the system -- a
component, not the app.

**Typst is not an obstacle.** The server never compiles Typst. Its entire use
of the engine crates is `is_font`, `families_in`, `render_body`, `no_assets`
and one `citations::compile` -- pure functions over bytes. PDF compilation is
already in the browser.

**Go does have Yjs.** An earlier claim that it would need a second runtime was
false. `reearth/ygo` is pure Go, with every Y-type, v1 and v2 wire formats,
subdocs, y-protocols awareness, a byte-level conformance suite run in CI
against `yjs@13.6.30` fixtures in both directions, and a differential
convergence fuzzer. `dbesio/ygo` and `ProlificLabs/autosync` wrap y-crdt's own
`yffi` over cgo, so the CRDT is the `yrs` we already run.

Two caveats. cgo costs exactly what Go was for: `CGO_ENABLED=1` means
per-target toolchains, the problem rustls-over-OpenSSL was chosen to avoid.
And `ygo` documents neither `OffsetKind::Utf16` nor StickyIndex, which
`session.rs` names as the whole of what interoperability required -- UTF-8
offsets silently misplace edits past non-ASCII characters and panic past
astral ones. Principle 3 says not to take the risk regardless.

**The real alternative to Rust was TypeScript on Node**, not Go -- Yjs native,
one language across server and browser. What decided against it was assumed to
be Typst, and Typst turns out not to decide anything.
