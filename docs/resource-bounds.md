# Resource bounds

This inventories every retained limiter and semaphore in the Rust server
(`crates/librepaper`), following up on REVIEW-BIG-IDEAS.md section 2.1. It
documents what exists after the cleanup that removed egress accounting,
pressure machinery, response histograms and override mirrors, and that
consolidated HTTP rate admission by authenticated principal with a network
fallback for anonymous requests. It does not propose a new resource
management framework; it records what is there, where it applies, and where
it does not.

Everything below was verified by reading the source at the cited line. Where
a statement is an inference rather than something read directly, it says so.

## 1. HTTP-level admission (`server/cost.rs`)

All of these run in `cost::middleware`, which wraps every request before it
reaches a route handler.

| Name | Defined at | Protects | Scope | Enforced at | Anon/auth |
|---|---|---|---|---|---|
| `incoming_memory` semaphore | `server/cost.rs:63,77` | request parsing and decoded-payload memory | deployment-wide, byte-weighted | `cost.rs:222-238` before the body is read | both |
| `work` semaphore | `server/cost.rs:62,75` | concurrent HTTP handlers doing origin work | deployment-wide, `config.cost.work_concurrency` (default 64) | `cost.rs:240-248` | both |
| `control_work` semaphore | `server/cost.rs:63,76` | a reserved lane for control paths when `work` is exhausted | deployment-wide, fixed at 16 | `cost.rs:242-247`, gated by `control()` at `cost.rs:143-150` | both |
| `admit_request` token bucket | `server/cost.rs:56-100` | request rate | per authenticated principal, or per network bucket when anonymous | `cost.rs:257-264` | both, keyed differently |
| `transfers` semaphore | `server/cost.rs:60,74` | concurrent font/artifact/asset transfer bodies | deployment-wide, `config.cost.artifact_transfers` (default 64) | `cost.rs:266-276`, for `/api/fonts/`, `/published/`, `/wasm/`, `/assets/`, and `/api/*/assets/*` paths | both |

The request-rate bucket (`Requests::consume`, `cost.rs:18-49`) is a leaky
bucket keyed on `p:<principal>` when signed in, `n:<network>` otherwise, at
`config.cost.requests_per_principal_minute` (default 6,000/minute). The key
map is capped at `MAX_KEYS = 4096` (`cost.rs:8`) with a sweep of buckets that
have gone idle for 60 seconds when the map is full (`cost.rs:20-22`); if the
sweep cannot make room for new keys, `consume` returns `false` and every new
key is refused admission for the request that carries it (`cost.rs:23-30`),
not just the additional ones past the 4096 cap.

A caller that hits any of these observes a `429` JSON body
(`{"error":"deployment resource allowance exhausted","reason":...,
"scope":...,"retryable":true,"retry_after":60}`, `cost.rs:139-146`) with a
`Retry-After: 60` header and `Cache-Control: no-store`. The `reason` field
names which bound was hit (`request_memory`, `work_concurrency`,
`request_budget`, `transfer_concurrency`) and `scope` says `deployment`,
`network` or `principal`. This is a self-explaining refusal: a client can
tell rate limiting from memory exhaustion from concurrency exhaustion.

Authentication happens before request-budget admission specifically so the
budget can be keyed on the authenticated principal (`cost.rs:249-256`,
comment at the call site). An anonymous caller falls back to
`client_network`, which is the peer address unless the deployment lists
trusted proxy networks (`config.cost.trusted_proxies`, `config.rs:281-291`,
validated by `validate_proxy_network`).

## 2. Per-document log admission (`log/sequencer.rs`)

These run inside `Sequencer::ingest`, per document, under the document's
single-sequencer lock.

| Name | Defined at | Protects | Scope | Enforced at | Anon/auth |
|---|---|---|---|---|---|
| per-principal update rate | `log/sequencer.rs:158-179`, config `session.updates_per_minute` (default 3000/min) | sequencer CPU and the flush path | per principal per document (token bucket keyed on `principal_key`) | `sequencer.rs:178-179` | both; principal key is the account id or, for anonymous sockets, the network bucket (`server/socket.rs:353-364`) |
| `log_quota_bytes` | `config.rs:65-70` (default 64 MiB), checked `sequencer.rs:1073-1077` | one document's log (compaction base plus rows since) | per document | `sequencer.rs:1073-1077` | both |
| `BUFFER_CEILING_BYTES` | `log/sequencer.rs:81` (`FLUSH_TRIGGER_BYTES * 4`), checked `sequencer.rs:1078-1080` | in-memory buffer awaiting a durable flush, when PostgreSQL is unavailable or slow | per document | `sequencer.rs:1078-1080` | both |
| decoded-document memory budget (`log/budget.rs`, `Budget::reserve`/`try_reserve`) | `config.memory_budget_bytes` (default 512 MiB), `cache_expansion` (default 6x) | resident cache entries, in-flight builds, forks, projection output | deployment-wide (one process-wide `Budget`) | `sequencer.rs:1093-1103` grows the cache reservation on every ingest; builds/compactions reserve at admission | both |
| heavy-work admission semaphore (`log/admission.rs`) | `MOST = 4`, `concurrency() = min(cores, 4)` | Loro build/compaction CPU (synchronous, uninterruptible) | deployment-wide, one process-wide semaphore | `log/admission.rs:51-57`, held across `spawn_blocking` for a build or compaction export | both |

The per-principal update bucket is the one bound in this group keyed on a
person rather than on bytes: it stops a client stuck in a resend loop from
filling the buffer faster than a flush can drain it (comment,
`sequencer.rs:147-153`). Deliberately keyed on `principal_key`, not on
`peer_key` (one connection): charging the connection key would hand a fresh
allowance to every reconnect, exactly the behavior a client stuck resending
exhibits (`sequencer.rs:155-165`).

Refusals in this group return `Ingested::Retryable(&'static str)`
(`sequencer.rs:158-159`) with a human-readable reason ("too many updates;
slow down and they will be accepted"; "this document's log is at its quota
and is waiting to be compacted"; "sync_delayed: this server cannot save work
right now"). `server/socket.rs` counts consecutive `Retryable`/`Refused`
answers per socket and closes the connection after
`MAX_CONSECUTIVE_REFUSALS = 3` (`socket.rs:21-26`), asking the client to
reconnect rather than hammer the sequencer forever. The memory-budget refusal
(`Busy`, `log/budget.rs:98-107`) explains itself in its `Display` impl
("this deployment has no memory to read that document right now") and is
counted in a `refused` counter read at `GET /api/status`
(`log/budget.rs:196-197`, `cost.rs:113-135`).

## 3. Live socket admission (`server/socket_budget.rs`, `server/socket.rs`)

`SocketBudget::admit` (`socket_budget.rs:113-158`) runs once per connection,
before a room is loaded, and returns a `SocketPermit` whose `Drop`
(`socket_budget.rs:250-256`) releases every counter it incremented, so
release cannot be forgotten on any exit path including a panic unwind.

| Name | Default | Scope | Refusal |
|---|---|---|---|
| `deployment_max` | 4096 | whole process | `Refusal::Deployment` |
| `network_max` | 128 | per network bucket | `Refusal::Network` |
| `principal_max` | 64 | per principal (`"anonymous"` for unsigned) | `Refusal::Principal` |
| `document_max` | 256 | per document, all roles combined | `Refusal::Document` |
| `document_readers_max` / `_commenters_max` / `_editors_max` | 256 / 256 / 32 | per document per role | `Refusal::DocumentRole(role)` |
| `queue_bytes_max` | 16 MiB | deployment-wide outbound queue bytes across all sockets | `queue_admit` returns `false` (`socket_budget.rs:186-193`) |

`SocketBudget` is explicitly documented as process-local
(`socket_budget.rs:88-90`): a deployment with multiple worker processes must
route all sockets to one process or share these counters at the edge, or the
deployment cap only bounds each process independently. This is read directly
from the comment, not inferred.

Per-connection, `run_socket` (`socket.rs:312-364`) additionally applies:

- `peer_queue` (default 256 frames, `peer_queue * 64 KiB` bytes,
  `socket.rs:333-334`): a bounded outbound channel per socket. A reader that
  cannot take another frame is disconnected rather than queued for
  (`socket.rs:328-331`) and reconciles by state vector on reconnect.
- `update_ceiling` (`socket.rs:456-459`, `max_document * 16 + 1 MiB`): caps
  one accumulated CRDT update a socket may assemble before it is rejected,
  because CRDT history can exceed visible source while peer memory must stay
  bounded.
- `SOCKET_WRITE_TIMEOUT` (5 seconds, `socket.rs:14`): every outbound send is
  wrapped in a timeout so a slow peer cannot pin the writer task.
- `MAX_CONSECUTIVE_REFUSALS` (3, `socket.rs:24-26`): closes a socket whose
  ingest keeps being refused.

These apply to both anonymous readers/commenters (via link) and
authenticated editors; the role ceilings (`document_editors_max` etc.) are
keyed on the socket's effective role at handshake, and the principal key
falls back to the network bucket for unsigned sockets exactly as in section
2 (`socket.rs:353-364`).

## 4. Storage/document quotas (`storage/postgres/repository.rs`, `document/session/sync.rs`, `config.rs`)

| Name | Default | Scope | Enforced at |
|---|---|---|---|
| `storage.total` | 5 GiB | deployment-wide retained bytes | `repository.rs:708` (assets), analogous check in the document-publish path |
| `storage.per_owner` | 100 MiB | per owner | `repository.rs:220,705` |
| `storage.documents_per_owner` | 50 | per owner | enforced in the document-creation path (not re-read here; see `config.rs:462-465` for the default and `server/documents.rs` for the create flow) |
| `storage.uploads_per_hour` (deployment-configured) | 30 | per owner, in-process pre-check | `server/figures.rs:41-56`, an in-memory hour bucket keyed on `who.key`, checked before the body is even read |
| `asset_uploads_per_hour` (durable) | 30 | per owner, durable | `repository.rs:679-683`, inside the same transaction that inserts the asset rows |
| retained per-document asset bytes (`max_assets`) | 32 MiB | per document | `repository.rs:685-690` |
| one asset's bytes (`max_asset`) | 8 MiB | per asset | checked at upload body-read time, `server/figures.rs:60-63` |
| `max_document` | 4 MiB (`DEFAULT_MAX_SOURCE_BYTES`), operator-adjustable up to 8 MiB (`SUPPORTED_MAX_SOURCE_BYTES`) | per document, sum of all texts and keys | many sites: `server/documents.rs:588,652,776,926`, `document/session/sync.rs:323-327`, `room/agent.rs:597` |
| `max_files` | 200 | per document | `server/documents.rs:638-641`, `document/session/sync.rs:285-310` |
| `max_path` | 200 bytes | per file path | `document/paths.rs:22` |
| `max_title` | 200 characters | per document title | `server/documents.rs:604` |
| encoded-snapshot ceiling (`PersistenceLimits`, `E` = 16 MiB default) and staging-memory admission (`M` = 512 MiB, 8x factor) | `config.rs:606-693` | per snapshot write, and a transient process-wide memory estimate | `PersistenceLimits::validate` at configuration time; runtime admission is the `log/budget.rs` budget in section 2 |
| PostgreSQL connection pool | `max_connections: 20`, `acquire_timeout: 5s` | deployment-wide, all catalog access | `storage/postgres/mod.rs:85-86,108-111` |
| `max_uncompacted_updates` / `max_uncompacted_bytes` | 1,000 rows / 512 MiB, clamped by `compaction_count_threshold`/`compaction_byte_threshold` | per document, drives compaction scheduling rather than refusal | `storage/postgres/mod.rs:71-78` |

A caller past `max_document`, `max_files`, `max_title` or the asset ceilings
gets a `4xx` JSON error naming the limit (for example
`server/documents.rs:641`, `"a document may hold {max_files} files"`). The
storage-quota refusals at the repository layer return
`Error::Conflict("account storage quota exceeded")` or
`"deployment storage threshold exceeded"` (`repository.rs:220-224,705-711`),
both self-explaining. These apply to whoever holds editor access to a
document, whether that access came from sign-in or an editor share link;
neither `max_document` nor the storage quotas distinguish signed-in accounts
from editor-role link holders, because both write source the same way.

## 5. Auth and provider-facing admission (`auth/mod.rs`, `auth/device.rs`, `auth/github.rs`)

| Name | Defined at | Protects | Scope |
|---|---|---|---|
| `PROVIDER_REQUESTS` | `auth/mod.rs:48-49`, `PROVIDER_REQUEST_CONCURRENCY = 16` | outbound OAuth exchange/profile calls | deployment-wide, one static semaphore, shared by every provider |
| `provider_slots` (`TokenCache`) | `auth/device.rs:25,98`, `TOKEN_CHECK_CONCURRENCY = 16` | outbound device-token verification calls | deployment-wide |
| `lookup_slots` (`GithubAccounts`) | `auth/github.rs:26,300,315`, `GITHUB_ACCOUNT_LOOKUP_CONCURRENCY = 8` | outbound GitHub profile lookups | deployment-wide |
| `DEVICE_SOURCE_STARTS_MAX` | `auth/device.rs:387`, 10 | device-flow starts per source key, in a rolling window | per source (unauthenticated, pre-identity) |
| `DEVICE_SOURCE_KEYS_MAX` | `auth/device.rs:394`, 4096 | bounded map of source keys tracked for the above | deployment-wide |

`try_provider_request()` (`auth/mod.rs:51-53`) is `try_acquire`, non-waiting:
a caller that cannot get a permit falls through to whatever the caller does
on `None`, which is provider-path specific. These are all deployment-wide,
applying before an identity exists, so they are inherently anonymous-facing
admission (a burst of sign-in callbacks or device-flow polls, by definition
unauthenticated at the moment they are checked).

## 6. MCP tool admission (`server/mcp.rs`)

`Capacity` (`mcp.rs:27-32,35-42`) is one instance held on `Server`
(`server/mod.rs:174,597`), so it is a single deployment-wide set of
semaphores shared by every document and every connected agent, not scoped
per document or per principal:

| Semaphore | Permits | Tool group |
|---|---|---|
| `reads` | 8 | `document_read` |
| `effects` | 8 | everything else (mutations) |
| `results` | 4 | `document_result` (non-cancel) |
| `cancellations` | 2 | `document_result` with `action: "cancel"` |

A caller past its slot gets a JSON-RPC tool result carrying
`{"code":"rate_limited","message":"document tool capacity is busy; retry
with the same operation key","data":{"retry_after_ms":250}}`
(`mcp.rs:290-299`), which is self-explaining. MCP access requires at least
Commenter role (`mcp.rs:259-260`), so this path is authenticated or
link-authorized traffic only; an anonymous reader cannot reach it at all.

## 7. Local companion server admission (`local/service.rs`)

Separate from the main deployment server: `rate_limited`
(`local/service.rs:1589-1604`) is a per-peer-IP sliding window
(`CONNECT_RATE_LIMIT` over `CONNECT_RATE_WINDOW`, both defined nearby in the
same file) guarding local companion connection attempts. This binds a
different process (the local dev companion), reachable only over loopback in
its intended deployment, and was not re-verified against the same anonymous
principal question as the main server because it has no principal concept:
every caller there is, by construction, whoever can reach the loopback
socket.

## 8. Background work (`storage/worker.rs`, `storage/schedule.rs`)

The worker runs compaction, label archives, deletions and the sweep of
superseded compaction bases. None of them has a queue table: each is named
by durable state (`documents.uncompacted_*` over the threshold, a label with
`archive_requested_at` and no `archive_key`, `documents.status = 'deleting'`,
a `superseded_bases.delete_after` in the past), and the worker rediscovers
them by a bounded paged scan. That is what makes every bound here safe to
enforce by refusing rather than growing: refusing a wake-up loses the early
notification, never the work.

| Name | Defined at | Protects | Scope | Enforced at | Overload behavior |
|---|---|---|---|---|---|
| `QUEUE` | `storage/worker.rs:55` | waiting tasks, process memory | one worker, 256 tasks | `Handle::ask` (`worker.rs:109`), `try_send` only | Sets one rescan bit and returns. No waiter is allocated, no wake-up is retried in memory. |
| `DEADLINES` | `storage/schedule.rs:22` | retry and deadline bookkeeping, waiting timers | one worker, 1024 entries | `Deadlines::failed` and `Deadlines::at` | Refuses the new deadline, keeps the earliest refused time, and wakes the worker to rescan durable state then. Counted by `Deadlines::refused`, reported in the `background` section of the operator cost snapshot and in a `log::warn` naming the task. |
| `PAGE` | `storage/worker.rs:300` | one scan's result set and the local queue | one scan page, 64 rows per kind | `Worker::scan` | The continuation cursor sits behind the page's own work, so a million durable rows cost one page of memory at a time. |
| `SWEEP_BATCH` | `storage/worker.rs:88` | one sweep pass, object-store calls | 500 rows | `sweep_superseded_bases` | A full batch asks for itself again rather than raising the bound. |
| backoff and ceiling | `storage/schedule.rs:32,39` | the dependency a failing task keeps hitting | per task, 60s doubling to 1h | `Deadlines::failed` | The ceiling is a floor on retry frequency, not a give-up: durable state still names the task, so the deployment recovers on its own once the dependency does. |

Deduplication is by `Task` identity, which is why `Task::Archive` names only
the label: the HTTP path and the durable scan would otherwise produce two
different values for one archive and neither the queue nor the deadline map
could collapse them.

There is no worker state that grows with the number of documents. The
deadline map is the only per-task memory, and both of the things that used
to grow without a bound (one `tokio::spawn` per retry, one per scheduled
deletion) are now entries in it.

## 9. Gaps

Verified by reading the code:

1. **`config.rate_per_hour` (comment rate, default 20/hour) is defined and
   documented but never enforced anywhere.** `config.rs:92,463` defines and
   defaults it; `server/serve.rs:252-263` documents what it is supposed to
   mean ("what one person may *say* about a document in an hour"), but no
   code path reads `config.rate_per_hour` to check or refuse a comment
   creation. The only other rate concept near comments,
   `share_links.comment_budget` (`storage/postgres/access.rs:52`), is a
   distinct per-share-link budget, not this deployment-wide setting. This is
   a real absence of an authenticated-and-anonymous comment-rate bound: a
   signed-in account or a commenter-role link holder can post comments as
   fast as the request-rate bucket in section 1 allows (6,000/minute by
   default), with nothing more specific in between.

2. **`config.max_comments` (default 500) and `config.max_replies` (default
   100) are defined but not enforced as creation-time caps.**
   `PostgresCatalog::annotation_count` has no caller anywhere in the crate
   outside its own definition, and `max_replies` is grepped nowhere at all
   except its definition and default (`config.rs:139,503`). Both fields read
   as load-bearing limits from their doc comments and are not load-bearing in
   the code. A document's annotation and reply counts are unbounded in
   practice, aside from the general request-rate and body-size bounds in
   sections 1 to 3.

   **Reads no longer assume them.** They used to: `room/comments.rs::load`
   read one 500-row page and called it the document, `find_annotation`
   scanned that same page for one row, and the reply query refused any result
   above 5,000 rows across the whole document. An accepted comment past those
   figures was written and acknowledged and then unreadable and
   unaddressable, and a document past the reply ceiling failed to load at
   all. Both reads now walk the catalogue with the `(created_at, id)` cursor
   the queries already take, in pages of `ANNOTATION_PAGE_MAX` (500) and
   `REPLY_PAGE_MAX` (1,000), and the single-row lookup is an indexed
   `(id, document_id)` read rather than a scan.

   The full-snapshot loader additionally bounds the aggregate collection to
   a 16 MiB memory estimate (twice serialized content plus struct overhead),
   counting replies as they arrive rather than first collecting a whole
   thread. Cache fills serialize with invalidation. Oversized or failed reads
   produce an explicit HTTP/WebSocket/agent error rather than an empty list.
   This is a per-snapshot guard, not a deployment-wide comment-cache budget.
   End-to-end transport pagination and aggregate cache accounting remain
   outstanding; SQL page sizes alone do not bound total residency across
   rooms or the cost of cloning and serializing snapshots.

   Admission is a separate product question: nothing limits how many comments
   one document may accumulate. Enforcing the configured caps at the
   transaction boundary would make new comments refusable; it would not
   remove the need to read pre-existing larger collections.

3. **`config.session.label_deployment_per_hour` (default 10,000) is defined
   but never wired to any enforcement.** Only `label_owner_per_hour` reaches
   `StoragePolicy.versions_per_hour` (`server/serve.rs:265`); there is no
   deployment-wide version-rate check anywhere in `storage/postgres/`. Given
   that `label_owner_per_hour` (300/hour) is well below the deployment
   figure, this is unlikely to matter while there are fewer than roughly 33
   active owners producing versions simultaneously, but the field itself is
   dead.

4. Background retry and deadline state in `storage::worker::Worker` was
   unbounded when this audit was written, which is what REVIEW-BIG-IDEAS.md
   section 2.1 flagged: `Worker::failed` grew a `cooling` map with no size
   check and spawned one sleeping task per retry, and `Worker::schedule_at`
   spawned one timer per call, so repeated observations of one pending
   deletion accumulated timers. **This is now closed**, and the bounds are
   in section 8 above. The audit's reading is kept here because the fix is what
   it describes: one bounded map, [`schedule::Deadlines`], keyed by task
   identity, holding at most `DEADLINES` (1024) entries, with no spawned
   timer anywhere in the worker.

5. **The MCP `Capacity` semaphores (section 6) are global, not scoped per
   document or per principal.** One document or one runaway agent loop can
   exhaust all 8 `reads` or `effects` slots for the whole deployment, and
   every other document's MCP traffic is refused with `rate_limited` until
   a slot frees. This is read directly from `server/mod.rs:174,597` (one
   `Capacity` on `Server`) and `mcp.rs:283-299` (the slots looked up by tool
   name only, not scoped by `slug` or by caller).

6. **Anonymous admission relies entirely on network identity once past
   sign-in gates**, which is a design choice stated by the review and
   confirmed at each admission point (sections 1 through 3): the HTTP
   request budget, the socket budget's principal counters and the
   per-document update-rate bucket all fall back to `client_network` for an
   unsigned caller. This is not a code gap by itself, but it means every one
   of those bounds is exactly as strong as `client_network`'s resistance to
   shared NATs and proxy spoofing, which is bounded by
   `config.cost.trusted_proxies` (`config.rs:270-296`) and was not
   independently re-verified in this pass beyond reading its validation
   logic.

Updated since this audit was written:

- The connection pool (default twenty) bounds concurrent database work across
  requests, editing and background maintenance. Twenty connections versus
  sixty-four HTTP work slots remains a plausible contention case that needs a
  mixed-load measurement. Socket editing adds work after the HTTP upgrade
  releases its permit; it does not invalidate that concern.
- The periodic editing sweep now allows bounded concurrent document flushes.
  [PostgreSQL capacity](postgres-capacity.md) describes the regression test
  and corrected benchmark. Earlier numerical capacity claims are withdrawn
  because their row counts included setup and their rates mixed workload and
  drain intervals. The benchmark omits HTTP traffic and compaction.
- Pool occupancy and transaction-open timing are exposed at `GET /api/status`
  under `database`. Single-statement read waits are not timed, and the
  contention counter is a point-in-time sample, not proof of zero queueing.

## 10. Timers

Four timers remain, three in `server/serve.rs` and one in
`storage/worker.rs`.

| Timer | Interval | Fires a DB query on an idle deployment? |
|---|---|---|
| retention janitor | 3600s, `serve.rs:520-529` | **Yes, when configured.** Only started if an operator sets `retention > 0` (`serve.rs:509`). Every tick calls `delete_expired`, which calls `store.list_result()` (`server/documents.rs:47-56`), an unconditional catalog read, whether or not any document is actually expired or resident. |
| room sweeper | 1s, `serve.rs:540-550` | No. The tick checks `sweeper.rooms.registry().all().await.is_empty()` first and calls `housekeep()` only when a room is resident (`serve.rs:544-547`). The comment at `serve.rs:531-539` states this directly: "an idle deployment ticks this timer but touches no room and issues no query." This was read, not just trusted from the comment: `housekeep()` is gated behind the non-empty check in the same block. |
| per-document flush deadline (`session.write_after_seconds`, default 2s) | in-memory, per resident room | No, by design distinction the review draws explicitly: this is a deadline check against a value already held in memory (when a room last saw a quiet period), not a poll of durable state. It only produces a query when a resident room's own deadline elapses, which requires the room to be resident and to have unflushed writes, i.e., not an idle deployment. Not independently re-read line by line in this pass beyond confirming the field's role in `config.rs:365-370`; this line is otherwise consistent with the sweeper's own gating. |
| the background worker's own deadline | one `sleep_until`, inside `Worker::run`'s `select!` | No. There is exactly one, whatever the worker owes itself: it sleeps until the earliest entry in `Deadlines`, and with the map empty (nothing failing, no deletion inside its grace, no superseded base waiting) `Deadlines::next` returns `None` and the worker blocks on its channel with no timer at all. When it does fire, it runs one task (a deletion, a retry, a sweep pass), which is not a poll: a task that finds nothing to do schedules nothing further. |

The project's rule, stated directly in the sweeper's own comment
(`serve.rs:531-539`), is that an idle deployment issues no query beyond the
lease connection, and that a timer checking an in-memory flush deadline is
not the same thing as a database poll. The room sweeper and the flush
deadline honor that rule as read. The retention janitor does not, but its
own existence is conditional on an operator opting into retention, which the
comment does not claim is exempt; this is worth operator documentation
(retention trades the idle-query guarantee for expiry enforcement) rather
than a code change, since a document-retention policy has no way to know
"nothing is expired yet" without asking.
