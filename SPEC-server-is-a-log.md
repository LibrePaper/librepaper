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
  reconciles. The close now reaches the transport (below), but the
  end-to-end reconnect path is unproven.

## Priority 2: Measure compaction performance

- [x] Measure reconstruction, export, verification, compression, upload and
  database activation separately in release builds; record peak memory and
  editing latency on long-lived documents.

  `storage::postgres::benchmarks::compaction_cost_release_benchmark` runs
  §8.4 one stage at a time on the same call sequence `worker::compact`
  uses, with one editor typing at 120 ms throughout and a 5 ms RSS sampler
  over the compaction window. Shapes are `edits:seed_kib`; the small-seed
  and zero-edit shapes are controls that separate byte cost from op cost.
  Measured 2026-09-20, release, PostgreSQL 16.15, filesystem blob store.

  **One stage is the whole cost, and it is reconstruction.** On a 1 MiB
  document with 500, 2000 and 8000 accumulated edits: reconstruction
  3.82 s, 3.92 s, 3.82 s. Every other stage, at every shape: flush 6 to
  15 ms, export 0.1 to 2.7 ms, verification 0.1 to 1.7 ms, compression 0.06
  to 0.29 ms, upload 0.1 to 0.2 ms, activation 2.1 to 3.4 ms. Compaction is
  reconstruction plus about 8 ms.

  **The cost is the byte count, not the op count, and it is roughly
  quadratic.** Zero-edit documents, one upload each: 64 KiB rebuilds in
  16 ms, 256 KiB in 219 ms, 1 MiB in 3.74 s, 2 MiB in 15.2 s. Four times
  the bytes is 14 to 17 times the work. The op history is nearly free by
  comparison: 2000 edits on a 16 KiB seed rebuild in 9.5 ms, and going from
  500 to 8000 edits on a 1 MiB seed does not move the number at all.

  **A 2 MiB document is already unreadable.** Its cold build takes 15.2 s,
  past the 10 s `BUILD_DEADLINE`, so §9.3 marks it unreadable. Compaction
  then cannot run on it, so §9.1's quota eventually refuses its updates and
  nothing ever clears them. This is a correctness problem found by the
  measurement, not a tuning question: the ceiling is about 1.3 MiB of text,
  while `MAX_UPDATE_BYTES` admits 4 MiB and `MAX_EXPANDED_BASE_BYTES`
  allows a 128 MiB base.

  **Peak memory is three orders of magnitude over §9.2's estimate.** Peak
  RSS above the pre-compaction baseline: 0.65 MB at 64 KiB, 24.5 MB at
  256 KiB, 401 MB at 1 MiB, 1617 MB at 2 MiB. `DEFAULT_EXPANSION` is 6, so
  a 1 MiB log reserves about 6 MB against a transient 400 MB. The budget
  bounds what a document costs while resident, and this is what the build
  costs while producing it; on a small VPS the second is what runs out.

  **Reconstruction stalls typing for its whole duration.** Ingest and
  `build` take the same `inner` lock, and `build` holds it across
  uninterruptible Loro work: ingest p50 is 43 to 60 µs idle, and the one
  keystroke that lands during a 1 MiB compaction takes 3.80 to 3.90 s.
  Export does not stall anything measurable (2 ms).

  Rebuilding the same document from the base compaction just wrote takes
  0.55 to 4.3 ms, against 3.8 s from the rows: compaction is worth roughly
  a thousandfold on read cost, which is why the shape of the fix matters
  more than whether to compact.

  Raw report: `LIBREPAPER_COMPACTION_BENCHMARK_OUTPUT` writes the JSON,
  and `LIBREPAPER_COMPACTION_BENCHMARK_EDITS` picks the shapes.

## Priority 3: Performance changes — only where measurements justify them

The measurements above answer all three conditions, and none of them the
way the list expects.

- [ ] Reconstruction, not export, is what stalls editing, so exporting a
  durable prefix off the lock fixes the 2 ms stage and leaves the 3.8 s one
  alone. What the numbers point at instead is the quadratic import: a
  1 MiB text insert replayed as an update costs 3.7 s and 400 MB, while the
  same document loaded from a snapshot costs 3 ms. Find out whether this is
  `import_batch` over a large single op, confirm it against Loro upstream,
  and fix or work around it there before tuning anything around it.
- [ ] Raise or remove the gap between what ingest admits (`MAX_UPDATE_BYTES`
  4 MiB) and what a build can survive (about 1.3 MiB at the current
  `BUILD_DEADLINE`). Until reconstruction is fixed, a document that crosses
  it is permanently unreadable and permanently uncompactable, which is data
  a person can no longer edit.
- [x] Verification and compression do not block async workers: 1.7 ms and
  0.3 ms at their worst, both already behind §9.3's semaphore. Nothing to
  move.
- [ ] Repeated full-history rewrites do not dominate: the rewrite is cheap
  once the entry is built, and the entry has to be built anyway. Thresholds
  are not the lever; reconstruction is.

## Done

Implemented, `clippy -D warnings` clean, and covered by tests that were
shown to fail against the unfixed code. The PostgreSQL-gated suites and the
browser suite have not been re-run since the last of these landed.

- [x] Compact unopened documents discovered by the startup scan.
  `Worker::compact` admits through `Registry::get` instead of dropping the
  task, which at startup was every document.
- [x] Trigger compaction after threshold-crossing semantic commits as well
  as ordinary flushes. Both call sites ask now; a command writes its row
  through `insert_log_row` without passing through `flush`, so one trigger
  could not have covered both.
- [x] Retry dropped or deferred compaction tasks without a restart or
  another edit. `Sequencer::recheck_compaction`, called from
  `Registry::housekeep`, with a one-minute floor so a slow compaction
  cannot flood the 256-slot queue and cause the very drop it repairs.
- [x] Coordinate base activation, covered-row deletion and sequencer
  metadata with concurrent joins and cache builds. `CompactionGate` takes
  `transaction` then `inner`, matching the existing lock order. The race
  was worse than reported: a join saw `has_base == false` and answered
  with the full head vector, no base and no rows, so a fresh client
  believed itself synced and never asked again.
- [x] Serialize browser join processing. `rows()` awaits hydration itself,
  because a bare `doc-rows` can be the whole join reply, and is chained
  behind a `start()` still fetching a referenced base.
- [x] Terminate slow subscribers' transports even when their queues cannot
  accept a close frame. `Sender::force_close` uses a raw slot that ordinary
  admission never claims. Previously the more overloaded a subscriber was,
  the less likely it was to be told to disconnect.
- [x] Bound updates per principal with a token bucket. §9.1 specifies one;
  the code counted within `now_unix() / 60`, which admitted twice the
  allowance across a boundary and was not monotonic. The flaky rate-limit
  test was that bug reporting itself. Now a continuous-refill bucket on a
  monotonic clock, with a test seam, deterministic over five runs.
- [x] Bound updates per PRINCIPAL rather than per socket. The limit was
  keyed on `peer_key`, which is the per-connection acknowledgement
  identity, so a client in a resend loop got a fresh allowance by
  reconnecting -- exactly the workload the bound exists for. `peer_key`
  could not simply change, since it namespaces `client_seq`; the two
  concerns are separated now.
- [x] Prove acknowledgements cannot mark missing or buffered work durable
  after a refused update, causal gap or empty catch-up. Traced and holds
  in all five cases, and now held by two regression tests, each shown to
  fail against deliberately broken production code. A refused batch is
  never merged into `head_vector`, so the peer's next batch gaps rather
  than being accepted past the hole. The doc comment that claimed the
  reason was socket closure is corrected.
- [x] Notify readers when source changes with no decoded cache resident.
  The frame carries `digest: null` rather than being withheld; nothing
  builds a document on the typing path. Two bugs: readers were never told,
  and the one-per-second throttle never engaged while cold.
