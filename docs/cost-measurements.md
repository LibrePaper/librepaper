# Origin capacity measurements

Measured September 11, 2026 on Linux x86-64 with the debug binary, SQLite,
Chromium, and a local HTTPS copy of the current compiler mirror. The origin
process ran with a 4 GiB virtual-address-space limit (`ulimit -v 4194304`).
The host had 54.6 GiB physical RAM; this is a constrained-process smoke test,
not a measurement of a dedicated 4 GiB machine under sustained load.

The workload published `latex/corpus/e2e/biber`, opened its read link in a
fresh browser, compiled its bibliography with Biber WASM, drew the PDF,
exported its transient bytes, and checked that rendering routes returned 404.
Publishing used a locally signed test OAuth account; no provider was contacted.

| Measurement | Fresh origin | After publication and cold reader |
| --- | ---: | ---: |
| Origin RSS | 110,497,792 bytes | 129,585,152 bytes |
| SQLite allocated pages | 626,688 bytes | 626,688 bytes |
| Charged document storage | 0 bytes | 1,935 bytes |
| Resident room estimate | 0 bytes | 1,120 bytes |
| Persistence staging peak | 0 bytes | 6,296 bytes |
| Completed catalog jobs | 13 | 86 |
| Catalog execution time, cumulative | 70,196 µs | 381,174 µs |
| Catalog maximum queue wait | 365 µs | 365 µs |
| Process disk writes, cumulative | 815,104 bytes | 1,404,928 bytes |

The reader drew its one-page PDF in 3.5 seconds and exported 54,217 bytes.
Origin shell/renderer-module response bodies totaled 2,765,731 bytes; source
responses totaled 1,376 bytes and collaboration totaled 1,184 bytes. TeX and
Biber distribution traffic went directly to the mirror. None of the PDF's
54,217 bytes were uploaded to or downloaded from the origin. Status polling
itself is administrative traffic and is included in the transfer counters.

Reproduce the browser workload with a built binary and populated mirror:

```sh
node web/tools/latex-e2e.mjs target/debug/librepaper browser \
  latex/corpus/e2e/biber 300 /path/to/wasm-latex/mirror
```

The harness prints aggregate status snapshots before and after the workload.
It requires Chromium, OpenSSL, and sqlite3. To repeat the address-space cap,
pass a wrapper executable that sets `ulimit -v 4194304` and then executes
the binary with its arguments. Browser and mirror processes run separately.

Keep room, persistence, request-body, socket-queue, and cache budgets separate
when sizing a host. Their defaults are ceilings, not startup allocations.
Reserve additional memory for the executable, response buffers, allocator,
SQLite, operating-system page cache, and proxy. This smoke test establishes
normal-operation costs; it does not justify raising existing ceilings or
claim that every permitted concurrency can be sustained on a 4 GiB host.
Measure representative documents and concurrent uploads before changing a
guardrail. Use process/container memory enforcement alongside the application
policy for a hard whole-process limit.

For reconciliation, application transfer totals count response bodies and
WebSocket payloads, while a proxy or provider may also count headers, framing,
compression, retries, and TLS. Compare the same time window and account for
those differences. The range/conditional tests compare charged bytes with
the exact body received, including zero charge for 304 responses.
