# Collaboration scaling measurements

Measured 2026-09-12. Three localized changes reduce repeated server work:

- Reuse the exact pre-update snapshot length already encoded for revision
  validation when sizing quota reservations and initializing the encoded bound.
  Author validation, quota formulas, and rejection semantics are preserved.
- Share an immutable `Utf8Bytes` payload across broadcast queues and pass it
  directly into the WebSocket writer. Each recipient still consumes its own
  queue allowance; sharing memory cannot bypass deployment backpressure.
- Send the latest local awareness state at most once per 100 ms during normal
  cursor activity, and stop echoing remote awareness. Join/reconnect and local
  removal announcements bypass the delay. Document edits are not delayed.

## Room workload

An optimized baseline at commit `02a99e038bab427f9c74d65ad15cb58f1c6b6ce2`
was compared with the same revision plus only these Rust changes. Both used
the identical `single_room_scale_workload` test. Concurrent changes elsewhere
in the working checkout were excluded from this performance comparison.

The fixture creates 32 independently seeded CRDT peers, each generating 16
one-character insertions. It submits the resulting 512 updates in round-robin
order through `receive_update`, using real catalog quota reservations and
bounded production delivery queues. Every update is broadcast to the other
31 healthy recipients. One additional recipient never drains its queue.
After persistence, the fixture checks each peer's durable sequence and reopens
the storage layers to verify the complete source.

Five trials per variant and source size alternated baseline/improved order.
The following values are medians of the five trial totals:

| Measurement | Baseline | Improved | Reduction |
| --- | ---: | ---: | ---: |
| Admission of 512 edits, 100 KiB source | 147.643 ms | 115.894 ms | 21.5% |
| Admission of 512 edits, 1 MiB source | 814.971 ms | 560.576 ms | 31.2% |
| Broadcast enqueue work, 100 KiB source | 1.214 ms | 0.935 ms | 23.0% |
| Broadcast enqueue work, 1 MiB source | 1.785 ms | 1.325 ms | 25.8% |

All 20 runs delivered 15,872 healthy-recipient frames, removed only the slow
peer, and recovered every acknowledged edit. The current integrated working
checkout also passed this workload in the debug profile.

These are elapsed server-path times, including quota storage waits, not CPU
utilization measurements. The harness excludes network transport, browser
rendering, awareness traffic, simultaneous task scheduling, and client update
generation. It is a saturation micro-workload, not a 32-user latency test or a
claim about the maximum supported editor count. Persistence timing was noisy
(one baseline save took 3.34 seconds); no persistence-speed claim is made.
Raw measurements are in [scaling-measurements.json](scaling-measurements.json).

## Component measurements

The isolated revision-validation comparison performs the old validation plus
another full-state encode, versus validation returning the already computed
length. Sixteen alternating repetitions on sources built with 100 incremental
extensions took 2.062 ms versus 1.369 ms for 100 KiB, and 21.247 ms versus
14.202 ms for 1 MiB: about one third less elapsed time in this component.
It does not measure the entire admission path.

The outgoing-buffer benchmark includes constructing one shared payload per
broadcast, enqueueing into 64 production queues, draining them, and releasing
their reservations. Three alternating trials of 1,000 broadcasts per mode
gave the following mean trial times:

| Payload | String copies | Shared payload | Reduction |
| --- | ---: | ---: | ---: |
| 128 bytes | 6.983 ms | 5.900 ms | 15.5% |
| 4,096 bytes | 8.327 ms | 5.937 ms | 28.7% |

This component benchmark excludes deployment-wide metrics callbacks and
network writes. A separate test verifies that all eight tested recipients
share the same payload allocation while reserving eight recipients' worth of
queue bytes. It checks rejection at the aggregate limit and release after
in-flight delivery is dropped.

Running the awareness frame counter against old and new client code produced:

| Activity | Baseline frames | Improved frames |
| --- | ---: | ---: |
| 100 local cursor changes within one throttle window | 100 | 1 |
| Applying one remote awareness update | 1 echo | 0 echoes |

This is a deterministic burst measurement. Ordinary spaced typing will have
a smaller reduction. Tests decode the emitted update to check the final
cursor and cover reconnect, repeated sync, immediate removal, and timer cleanup.

## Reproduction and validation

Host: AMD Ryzen AI 7 PRO 450, 8 cores / 16 threads, Linux; Rust
`1.99.0-nightly (11177f223 2026-08-02)`. Rust measurements used the repository's
release profile. Commands from the repository root:

```sh
cargo test --release --lib single_room_scale_workload -- --ignored --nocapture
LIBREPAPER_SCALE_BYTES=1048576 cargo test --release --lib single_room_scale_workload -- --ignored --nocapture
cargo test --release --lib revision_validation_snapshot_size_benchmark -- --ignored --nocapture
cargo test --release --lib benchmark_shared_fanout -- --ignored --nocapture
node web/tools/collab-awareness-benchmark.mjs
node web/tools/collab-awareness-benchmark.mjs /path/to/baseline-checkout
```

For room A/B measurements, copy the same workload into the baseline before
compiling. `LIBREPAPER_SCALE_PEERS`, `LIBREPAPER_SCALE_ROUNDS`, and
`LIBREPAPER_SCALE_BYTES` control its dimensions. The two component benchmarks
compare old and new operations within one executable.

Validation completed:

- `cargo clippy --offline --workspace --all-targets --all-features -- -D warnings`
- `cargo fmt --check` and `git diff --check`
- Outgoing queue tests and revision/admission/room-lock regression tests
- `npm run check`, the new awareness test, and `npm run build`
- Editor browser integration test
- Full Rust workspace attempt: 1,084 passed, 30 ignored, four failures.
  All four failures passed focused reruns on the current checkout. They were
  an agent-transaction assertion in concurrently changing code, process
  cancellation, and two local Calepin preview timeouts. A single all-green
  full-suite run was not obtained.

Review corrected repeated UTF-8 validation in the first buffer proposal,
expanded accounting and awareness lifecycle coverage, and released the
temporary validation encoding as soon as the scratch document was populated.
No skill files were changed; no general skill-workflow correction was needed.
