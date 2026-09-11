# Operator cost policy

This document describes the version 1 cost and capacity policy exposed by
`librepaper admin serve`. It is a deployment policy, expressed in bytes, counts, and
time windows. It does not predict a provider bill. The effective policy is
printed at startup as the `cost_policy` event when output is redirected, summarized
briefly on an interactive terminal, and available in full from the loopback-only
status surface.

LibrePaper keeps source, input assets, annotations, session state, and source
history. It does not retain rendered PDF, HTML, DOCX, preview images, thumbnails,
or generated bundles. A browser or the local companion may hold a result while
it is being viewed, and a user may export a result to their own files, but the
origin, browser's persistent document store, edge caches, and backups do not
retain that result. An input PDF supplied as a document asset remains an input
asset and is charged normally.

## Operator controls

The operator CLI has cost controls under `librepaper admin serve`. Existing
storage, document, history, expiry, and publishing controls remain separate.

| Option | Environment | Meaning and default |
| --- | --- | --- |
| `--transfer-budget BYTES` | `LIBREPAPER_BUDGET_TRANSFER` | Origin response-body allowance over one rolling 24-hour window. Bare integers are bytes; `B`, `KiB`, `MiB`, `GiB`, and `TiB` are accepted. Omitted means unlimited and emits a warning. `0` refuses ordinary transfer while retaining the emergency reserve. |
| `--document-assets-limit MIB` | `LIBREPAPER_BUDGET_DOCUMENT_ASSETS` | Combined input-asset ceiling per document, in MiB; default 32 MiB, accepted range 1–1024 MiB. It is not a per-file or asset-count limit. |
| `--document-size-limit MB` | `LIBREPAPER_MAX_SIZE` | Combined source-text ceiling per document; default 4 MB, maximum 8 MB. |
| `--publisher-storage-limit MB` | `LIBREPAPER_QUOTA` | Charged durable storage per owner; default 100 MB. |
| `--deployment-storage-limit MB` | `LIBREPAPER_STORAGE` | Charged durable storage for the deployment; default 5120 MB. Physical headroom is separate and can stop growth first. |
| `--publisher-document-limit N` | `LIBREPAPER_MAX_DOCUMENTS` | Documents per owner; default 50. |
| `--publisher-upload-limit N` | `LIBREPAPER_UPLOADS_PER_HOUR` | Uploads per owner over a rolling hour; default 30. |
| `--history-checkpoint-minutes MINUTES` | `LIBREPAPER_CHECKPOINT` | Quiet period before an automatic checkpoint. The value is in minutes. If omitted, the built-in session quiet period is 30 seconds. |
| `--history-limit N` | `LIBREPAPER_HISTORY` | Checkpoints retained per document. Omission leaves the built-in unlimited count. A finite value is retained as at least one newest checkpoint. |
| `--document-expire-after DURATION` | `LIBREPAPER_EXPIRE_AFTER` | Document lifetime; default never. `--document-expire-from created` selects creation time instead of the last publication. |
| `--latex-mirror URL` | `LIBREPAPER_LATEX_MIRROR` | HTTPS static mirror fetched directly by browsers. The default is `https://latex.librepaper.workers.dev/`. |
| `--typst-fonts DIR` | `LIBREPAPER_TYPST_FONTS` | Optional local directory of additional Typst fonts served by the origin. |

CLI values take precedence over their environment values. The removed
`--latex`, `--fonts`, `--max-assets`, and `--biber-vm` options, and the legacy
environment names `LIBREPAPER_LATEX`, `LIBREPAPER_FONTS`,
`LIBREPAPER_MAX_ASSETS`, and `LIBREPAPER_BIBER_VM`, are rejected with a
migration message. `--cost-transfer` is not an alias. `--publishers anyone` is
also rejected; use `--publishers any` for any authenticated Google or GitHub
account.

Compiler distribution bytes come from the HTTPS mirror and do not count as
origin transfer. Origin transfer includes the shell and renderer modules,
deployment fonts, source and checkpoint reads, input assets, collaboration and
assistant traffic, uploads, mutations, authentication, and administrative
responses. A known response size is admitted before its body is opened. Unknown
length streams are charged as chunks are forwarded and stop when the allowance
is exhausted. A conditional `304` has no body charge; a range response charges
the bytes in the returned range.

The transfer allowance is one rolling 24-hour window. The implementation stores
charges in time buckets rounded upward to the next minute, so expiry can be
conservative by at most 60 seconds. Restarting does not reset the allowance.

## Advanced YAML

`--config PATH` (or `LIBREPAPER_CONFIG`) is optional. It selects a YAML file for
specific guardrail overrides; it does not replace the built-in policy and does
not require every field. Unknown keys are rejected. CLI and environment values
still win for everyday options.

The accepted top-level shape is:

```yaml
cost:
  transfer_bytes: 10737418240
  requests_per_minute: 60000
  requests_per_network_minute: 6000
  requests_per_principal_minute: 6000
  requests_per_document_minute: 12000
  artifact_transfers: 64
  work_concurrency: 64
  request_body_memory_bytes: 268435456

# This key is top-level. cost.trusted_proxies is not accepted.
trusted_proxies:
  - 127.0.0.1/32
  - ::1/128

session:
  rooms_max: 200
  rooms_bytes_max: 536870912
  peer_queue: 256
  inline_state_max: 262144
  updates_per_minute: 3000
  checkpoint_owner_per_hour: 300
  checkpoint_deployment_per_hour: 10000
  history_max: 0
  checkpoint_seconds: 30
  write_after_seconds: 2
  history_interval_seconds: 300

persistence:
  max_encoded_snapshot_bytes: 16777216
  max_queued_payload_bytes: 67108864
  max_staging_bytes: 536870912

sockets:
  deployment_max: 4096
  network_max: 128
  principal_max: 64
  document_max: 256
  queue_bytes_max: 16777216
  state_network_bytes: 268435456
  state_deployment_bytes: 4294967296
  idle_seconds: 1800

backup:
  destination_class: local
  frequency: 86400
  retained_count: 30
  encrypted: true
  warning_count: 10
```

Only the named members of each map are accepted. The `cost` map accepts the
eight fields shown above. The emergency transfer reserve is an internal fixed
guardrail and has no configuration key. The optional `backup` map is a
reporting declaration; `frequency` is seconds, and `warning_count` controls the
completed-backup warning. `trusted_proxies` accepts IP addresses or CIDR
prefixes, with at most 128 entries. An empty or omitted list means the TCP peer
address is authoritative and forwarded identity is ignored; loopback and
private addresses are not trusted automatically.

`destination_class` is a bounded ASCII label (up to 64 bytes), `frequency` is
between one second and ten years, and both count fields are bounded at one
million. `encrypted` records the operator's declaration; it does not cause
LibrePaper to encrypt a copy.

The single-proxy arrangement above is safe only when the proxy overwrites or
appends the actual client address in `X-Forwarded-For`. For a proxy connecting
from `127.0.0.1`, a header such as
`203.0.113.9, 198.51.100.23` resolves to `198.51.100.23`; the forged leftmost
entry is ignored. Without proxy trust, all visitors behind one reverse proxy
share the proxy's network-scoped request and socket limits; the deployment-wide
transfer budget is shared by all clients regardless of proxy configuration.

The effective policy reports the origin of each value: CLI, environment,
configuration file, or built-in default. `trusted_proxies` itself is reported
only in the operator view. Internal guardrails and the emergency reserve are
included even though they are not everyday CLI controls. The emergency HTTP
work pool is an internal fixed concurrency of 16; it has no configuration key.

## Built-in guardrails

These values apply when no advanced override is supplied. They are independent
limits, so changing one does not select a profile or alter the others.

The request controls are token buckets with a 60-second refill period:

| Guardrail | Default |
| --- | ---: |
| Deployment requests | 60,000 per 60 seconds |
| Network requests | 6,000 per 60 seconds |
| Principal requests | 6,000 per 60 seconds |
| Document requests | 12,000 per 60 seconds |
| Concurrent artifact transfers | 64 |
| Concurrent origin work handlers | 64 |
| Emergency origin work handlers | 16 (internal) |
| Emergency transfer reserve | 1 MiB over the transfer window |

Request accounting also keeps bounded route-class and identity keys. The eight
classes are `shell`, `fonts`, `source`, `assets`, `collaboration`, `mutations`,
`authentication`, and `administration`. At most 4096 request keys are resident;
idle keys older than 60 seconds are eligible for eviction when admission needs
room. Identity keys are internal admission state, not metric labels or
diagnostic output.

Live collaboration defaults are 4096 sockets per deployment, 128 per network,
64 per principal, and 256 per document. A socket may queue 256 frames and at
most 16 MiB; the same 16 MiB `sockets.queue_bytes_max` ceiling is also reserved
across all live socket queues, so a full deployment can refuse a new frame even
when the individual socket is below its limit. State transfer is limited to
256 MiB per network and 4 GiB per deployment over a rolling hour; an idle socket
is eligible for closure after 1800 seconds.
The existing room defaults are 200 resident rooms and 512 MiB of conservative
room memory, with a 256-frame peer queue and 3000 updates per peer per rolling
minute. State synchronization is charged before a cold join is built, and a
slow peer is disconnected when its frame or byte queue is full.

Each mutating request reserves four times its known body length before reading;
an unknown-length request reserves four times the configured upload ceiling.
The combined request-body memory default is 256 MiB, and the reservation is
held until the handler finishes. This protects parsing and decoded payload
memory; it is separate from source, asset, storage, and work limits.

Persistence defaults are a 16 MiB encoded snapshot, 64 MiB of queued and
executing journal payload, and 512 MiB for transient staging memory. The
snapshot staging estimate is eight copies. The catalog execution guardrails
are 64 admitted jobs, 16 MiB queued input, two executing jobs, and 128 waiting
producers; a single catalog request may carry at most 4 MiB.

## Storage, maintenance, and backups

`--publisher-storage-limit` and `--deployment-storage-limit` constrain charged retained physical bytes. They do not
include all space needed to operate the deployment. The server also tracks
allocated primary bytes, reserved bytes, cache bytes, backup-output bytes, and
filesystem free bytes. SQLite pages, indexes, allocator slack, logs, deletion
queues, in-flight encodings, and restore scratch need headroom. Growth can be
refused when the primary filesystem reaches its emergency floor even when a
quota remains.

Maintenance is incremental and restartable. A normal deletion pass admits up to
100 jobs, 1000 object requests, and 64 MiB of read bytes. Generated output never
enters the catalogue, so there is no generated-output retirement pass; cleanup
preserves an object when the catalog still identifies it as an input asset.
Accepted edits are persisted before idle rooms are evicted. A graceful shutdown
flushes rooms and runs final local deletion and journal cleanup passes.

Backups are outside the primary storage quota. `librepaper admin backup create` makes a
verified new full copy, including the catalog, retained source objects, secrets,
and deployment identity. Its output reports both:

- logical input bytes: the sum of the source files supplied to the copy; and
- bytes written: those files plus the completion manifest.

The backup destination reserves the logical input estimate plus 64 MiB of
temporary staging. The live primary volume must retain a separate 256 MiB
emergency headroom plus the 64 MiB backup scratch reservation. A backup is
published only after the copy and digest checks complete. The command warns
when its destination contains more than the configured `backup.warning_count`
(default 10) completed, uniquely named backups for the same deployment; it
never deletes old backups. Local backup manifests
carry a 30-day expiry expectation for reporting, but retention is an operator
responsibility. The optional declaration is reporting metadata only; it does
not schedule, encrypt, delete, or otherwise manage backups and is never a
startup requirement. Operators remain responsible for retention and restore
testing. The command also emits a `backup_completed` JSON event with the
backup counter, logical input bytes, bytes written, full-copy mode, and the
declared policy metadata; it contains no filesystem path or secret.

`librepaper admin backup restore` requires a new destination directory. It validates
the backup and required destination capacity before writing, reserves the
logical input estimate plus 64 MiB staging, and publishes the restored tree by
rename. It does not overwrite an existing deployment. Backups contain source
and protected input assets only; generated-output references are not created
and there is nothing to restore from a rendering cache.

## Status and diagnostics

`/api/status` is an operator endpoint available only from a direct loopback
connection. It rejects non-loopback peers and requests carrying forwarded host
or client headers. Query it with:

```sh
librepaper admin status
librepaper admin status --endpoint http://127.0.0.1:8080
```

The command prints the live policy, usage by resource class, and `Normal` or
`Limited` mode. If the server is stopped or unreachable, it reads the last
durable cost checkpoint from the local catalog and labels the result as stopped
with `counters: last durable checkpoint`. If the catalog cannot be read, the
command fails rather than inventing current usage.

Structured JSON usage events are written to standard output every 60 seconds,
after the durable cost checkpoint, and on mode transitions. Clean shutdown
flushes rooms and performs a final cost checkpoint. After a crash, the next
process resumes from the last checkpoint; response bytes sent since then may
be absent from durable counters for at most the checkpoint interval, temporarily
restoring allowance in the deployment's favour. The catalog row is copied by backups, so restoring
a backup resumes the allowance it carried instead of resetting it.

Metrics use the bounded route classes and status groups, plus bounded response
size histograms. General output contains no raw account IDs, document slugs,
paths, IP addresses, source text, annotation quotations, or OAuth data. A
diagnostic can report classes and aggregate counts; it must not turn identities
into unbounded stdout labels.

Status includes charged document bytes, SQLite allocated bytes, filesystem
allocated/available bytes and live write reservations, checkpoint-cache bytes,
room and socket queue bytes, request-body reservations, and persistence staging
usage. `mutation_outcomes` groups uploads and mutations by accepted result or
refusal class. Catalog queue and execution counters include total and maximum
delay. Linux process measurements include RSS, user/system CPU clock ticks,
main-thread runtime in nanoseconds, and physical disk read/write bytes;
unsupported host fields are null. Filesystem allocation includes other users
of that filesystem and is not a document storage charge.

Internal defaults also include two reconstruction workers; 64 MiB per encoded
object/recipe, 512 MiB per reconstructed source, and one million recipe chunks;
100 jobs, 1,000 object requests and 64 MiB of reads per deletion pass; 16
concurrent OAuth requests and eight account lookups, with five-second connect
and fifteen-second total timeouts; and 4,096 optional font files. The catalog
admits 64 jobs, two executing workers, 16 MiB of queued input, 4 MiB per job,
and 128 waiting producers. These appear alongside the configurable values in
the effective policy.

## Capacity examples

See [measured origin costs](cost-measurements.md) for a reproducible cold-reader
workload, memory measurements, and comparison with proxy/provider counters.

Each example changes only the named limit:

```sh
# Allow 10 GiB of origin response bodies per rolling 24 hours.
librepaper admin serve --transfer-budget 10GiB

# Reserve 10,240 MiB of charged deployment storage.
librepaper admin serve --deployment-storage-limit 10240

# Limit one owner's charged storage to 500 MiB.
librepaper admin serve --publisher-storage-limit 500

# Limit combined input assets in one document to 16 MiB.
librepaper admin serve --document-assets-limit 16

# Retain at most 100 checkpoints per document.
librepaper admin serve --history-limit 100

# Wait five quiet minutes before an automatic checkpoint.
librepaper admin serve --history-checkpoint-minutes 5
```

The values are not named profiles and do not imply authentication, expiry,
history, request, socket, or memory settings beyond the one option shown.
Choose storage below the usable volume capacity, reserve room for the primary
emergency floor and maintenance, and estimate transfer from origin-served
source, asset, font, shell, and collaboration traffic. Compiler mirror bytes
and transient rendered results belong outside the origin transfer estimate.
