# Log implementation: remaining work

The full specification this implements is not in this file. It is the
564-line `SPEC-server-is-a-log.md` at commit `3391479c`; the section
numbers cited throughout the code (§4.5, §8.4, §9.2 and 40 others) refer
to it. Restoring it beside this list would make those citations resolve
again.

## Priority 1: Correctness — verify and fix

- [ ] Decide how a client learns its work is durable after a reconnect,
  and implement it. Today `join` reports `head_vector`, which includes
  buffered work that is not yet in PostgreSQL. The browser catches up
  from it, exports nothing (the server already covers it), has that empty
  batch acknowledged at once, clears its unacknowledged map, and shows
  `pending: 0` while the work is still only in memory. §6.4's guarantee
  is that unflushed work survives if a surviving client reconnects, so a
  false "saved" that persuades someone to close the tab turns a
  recoverable state into a lost one.
  - The obvious fix does not work: catching up from the durable vector
    instead would re-send everything between durable and head, including
    other editors' operations this client only received by relay, so
    every reconnect re-uploads everyone's recent work.
  - Nor can the acknowledgement path be patched: the buffered work was
    ingested under the previous socket's `peer_key`, so the flush's
    `doc-ack` is addressed to a subscriber that no longer exists.
  - Recommended: carry the durable (log) vector on `Joined` beside the
    head vector, keep catching up from head so nothing is re-uploaded,
    and make the client's safe signal "my vector is covered by durable"
    rather than "my map is empty". The server reports durable advancing
    on flush, either as a field on `doc-ack` or a small `doc-durable`
    frame. This changes §6.1, which is why it is a decision and not a
    defect fix.

- [ ] Test that a closed slow subscriber actually reconnects and
  reconciles. `Sender::force_close` reaches the transport, but the
  end-to-end reconnect path is unproven.

## Measured

`storage::postgres::benchmarks` holds the release benchmarks, each
`#[ignore]`d and gated on `LIBREPAPER_BENCHMARK_POSTGRES_URL`:
`typing_throughput_release_benchmark` (§14.1), and
`compaction_cost_release_benchmark`, which times §8.4 one stage at a time
on the call sequence `worker::compact` uses, with an editor typing
throughout and a 5 ms RSS sampler. `update_import_cost_probe` and
`batched_import_cost_probe` beside them price Loro's import directly and
need no database. Shapes come from `LIBREPAPER_COMPACTION_BENCHMARK_EDITS`
as `edits:seed_kib`; the report goes to
`LIBREPAPER_COMPACTION_BENCHMARK_OUTPUT`.

What those runs establish, as constraints on anything proposed below
(2026-09-20, release, PostgreSQL 16.15, one document on an idle process):
a cold build of a 1 MiB log is about 8 ms and every other compaction stage
is under 35 ms, so compaction is no longer a cost worth designing around.
The transient peak of a build is 3 to 27 MB for logs of 1 to 3.5 MiB,
which §9.2's resident estimate does not cover. The ceiling on a document
is now §5's `MAX_UPDATE_BYTES`, which refuses an 8 MiB upload at the door.

Two rules the harness earned and should keep: a benchmark asserts that its
own run happened, and a failure is a result to record rather than a panic,
which is how an unreadable 2 MiB document was found at all.

