# Maintainer cost minimization

Status: proposed

LibrePaper is unreleased. These are intentional breaking configuration changes;
removed flags, environment variables, and value spellings have no compatibility
aliases.

Related specifications (available in repository history before their removal):

- `SPEC-01-history-retention.md` defines retained-object accounting, bounded
  history encoding, retention, and garbage collection.
- `SPEC-quota-preferences.md` defines owner-facing storage preferences without
  weakening deployment limits.
- `SPEC-02-diff-display.md` keeps rendering and diff computation in the browser
  and does not retain computed comparisons.

## Purpose

Keep a public or private LibrePaper deployment within an operator-selected cost
envelope under ordinary use, sudden popularity, malfunctioning clients, and
deliberate abuse. Expensive work and traffic must be admitted by explicit
deployment budgets rather than by the availability of CPU, memory, disk, or an
unmetered-looking third-party service.

LibrePaper is intended to run as one binary on a small server. Client-side
rendering avoids a server compilation fleet, but it does not make source,
assets, collaboration traffic, history maintenance, or backups free.
This specification makes those remaining costs visible and bounded.

LibrePaper does NOT retain a rendered copy of any source document. PDF, HTML,
and other generated output are transient client-side results, never retained
deployment artifacts. This applies to the latest render as well as historical,
published, and companion-generated renders.

Publishing always requires authentication through Google or GitHub. Before
sign-in, document access is limited to reading and commenting where the
document's access rules permit them. Anonymous publishing is never supported.

The largest single transfer, the browser TeX distribution, is taken off the
origin entirely: browsers fetch it directly from a static-asset mirror with
free egress, and the origin neither proxies nor pays for it. That change is
Phase A together with transfer accounting for what the origin still serves.
Phase B adds explicit policy reporting, degraded modes, the operator status surface, and backup
reporting. Phase A stands on its own and may be extracted into a separate
specification if Phase B is deferred.

## Goals

- Put enforceable ceilings on durable storage, server work, resident memory,
  requests, and network bytes.
- Prevent anonymous identity renewal from bypassing the deployment-wide cost
  envelope.
- Serve compiler distributions from a static-asset host so a fresh reader's
  cold compile costs the origin nothing.
- Keep ordinary reading and editing useful as a deployment approaches a budget.
- Prefer early, actionable refusal to an unbounded bill or an out-of-memory
  failure.
- Make every effective limit and its origin visible to the operator.
- Attribute enough usage to identify the route, document, owner class, and
  resource responsible for growth without logging document contents.
- Make backup retention and temporary storage part of capacity planning.
- Retain source and input assets without retaining their rendered derivatives.

## Non-goals

- Implement subscription billing, payment processing, invoices, or plan sales.
- Promise a particular cloud-provider bill or translate bytes and CPU time into
  currency inside LibrePaper.
- Replace host, reverse-proxy, CDN, filesystem, or object-store monitoring.
- Support unauthenticated publishing, including on private or local deployments.
- Move authoritative source or private assets to a third party. Package
  requests to the compiler mirror are not private assets; see "Privacy" under
  "LaTeX distribution delivery".
- Move LaTeX, Typst, Quarto, diff, or preview compilation onto the server.
- Promise that any compiler release stays available beyond the ones the
  current LibrePaper build references.
- Treat user preferences as authority to exceed operator limits.
- Guarantee service availability during a denial-of-service attack.
- Add presets, named profiles, or switches that change unrelated limits.
- Widen the flag surface beyond the daily transfer budget. Other new limits
  are documented implementation defaults or advanced configuration-file keys.

## Terms

- **Cost envelope:** Operator-defined limits for durable bytes, temporary bytes,
  network transfer, requests, server work, and concurrency during specified
  time windows.
- **Admission:** The decision made before accepting work or bytes which could
  consume an envelope.
- **Origin:** The LibrePaper server process and its directly attached storage.
- **Edge:** A trusted reverse proxy or CDN in front of the origin.
- **Mirror:** The static-asset host that serves compiler engines, formats, and
  package bundles under digest-named URLs, with CORS headers, directly to
  browsers. The project operates one; an operator may host a copy.
- **Immutable artifact:** Content whose URL includes a verified digest and may
  safely be cached for a long period, such as a compiler module, a font file,
  or a source asset.
- **Anonymous principal:** An unsigned-in reader or commenter, optionally
  identified by a browser-held credential. Such a credential never authorizes
  publishing or source editing.
- **Charged bytes:** Durable physical bytes attributed under
  `SPEC-01-history-retention.md`. Where this document says "storage ceiling"
  it means a ceiling on charged bytes, never on the logical full-tree sum.
- **Transfer bytes:** Response body bytes sent by the origin. Bytes a browser
  fetches from the mirror are not origin transfer.
- **Cold compile:** The bytes a fresh browser must fetch from the mirror to
  compile one typical LaTeX document. Measured in `latex/corpus/MEASUREMENTS.md`;
  it is a browser-experience number, never an origin cost.
- **Export control:** The owner-facing operations that let a user take their
  data out or stop it from spreading: downloading a document tree, exporting
  annotations, revoking share links, and deleting a document or account.
- **Degraded mode:** A bounded state which preserves reads and already-durable
  work while refusing selected optional or growth-producing operations.

## Existing behavior

The migration steps refer to these values as the existing ceilings unless a
step says otherwise.

Flags (`crates/librepaper/src/cli/mod.rs`, defaults in
`crates/librepaper/src/config.rs`):

- `--max-size` 4 MB per document, 8 MB maximum; `--max-assets` 32 MB per
  document; `--quota` 100 MB per publisher; `--storage` 5120 MB per
  deployment; `--max-documents` 50; `--uploads-per-hour` 30 per publisher;
  `--checkpoint` 5 minutes; `--expire-after` never.
- `--history` is documented as "0 keeps only the current text", but the config
  treats zero as no cap and the retention pass applies no hard count when it
  is zero. The effective default is therefore an unlimited checkpoint count
  per document. The help text is wrong and is corrected as part of Phase B.
- `--latex` defaults to the project mirror at
  `https://latex.librepaper.workers.dev/`.
- `--publishers anyone` currently permits anonymous publishing and prints no
  warning on a public listener. This specification removes that permission.

Configuration without flags:

- Rooms: `rooms_max` 200, `rooms_bytes_max` 512 MiB, `peer_queue` 256 frames,
  `updates_per_minute` 3000 per peer. Idle rooms are evicted oldest-first
  when either ceiling is reached, and eviction persists accepted edits first.
  Queue depth is counted in frames, not bytes.
- Checkpoints: `checkpoint_owner_per_hour` 300, `checkpoint_deployment_per_hour`
  10,000.
- Client identity for rate limiting (`server/reply.rs`): the first
  `X-Forwarded-For` entry is believed when the TCP peer is a loopback,
  private, or link-local address. No proxy list is configured. A client that
  reaches the origin directly over a private network can therefore choose its
  own identity.

LaTeX distribution today (`server/latex.rs`, `web/src/lib/latex/`):

- Browsers fetch every distribution file from `/latex/` on their own origin.
  With `--latex DIR` the origin reads a local directory; with an `https:`
  value it proxies the upstream through a 64-slot semaphore, a 128 MiB
  per-file cap, and a 32 MiB manifest cap, streaming in 64 KiB chunks with no
  disk cache, no coalescing, and no retry. Every fresh browser's cold compile
  is therefore origin egress, 2 to 30 MB per the corpus measurements.
- The route is same-origin for three reasons given in the file header: the
  Emscripten loaders resolve their wasm relative to the worker URL, the
  shell's CSP, and keeping package requests on the author's own deployment.
  The CSP in `server/routes.rs` already allows `https:` for scripts and
  connections. `web/src/lib/latex/biber.js` already starts Biber WASM in a
  blob worker using verified assets from the selected mirror release.
- Biber WASM is selected automatically when the release has `engines.biber`.
  The older `--biber-vm` setting configures a separate Linux/v86 fallback,
  implemented in `web/src/lib/latex/vm.js`; it does not configure Biber WASM.
  This specification removes that flag and the VM fallback.
- The browser verifies each fetched file's sha256 and size against the
  manifest before using or caching it, in Cache Storage namespaced by release
  digest, and asks for persistent storage.
- An editor's first compile writes the mirror's default release into the
  shared project settings as `latex.release`, and every collaborator compiles
  with it until someone presses Update in the compiler settings. A document
  pinned to a release the manifest no longer carries fails its compile with a
  message naming the retained releases. This pin is removed by this
  specification.
- Both shapes of `--latex`, the directory and the proxied `https:` upstream,
  are removed by this specification. The README's examples of
  `--latex /srv/librepaper/latex` and `make deploy LATEX=<dir>` go with them.

The project mirror (`wasm-latex` repo): deployed with `wrangler deploy
--assets`, a Cloudflare Worker that is nothing but the files. Its `_headers`
already sends `Access-Control-Allow-Origin: *`, a one-year immutable cache
header on digest-named files, and `no-store` on `manifest.json`. It currently
holds two releases at about 5,800 files and 3.4 GB each, 11,583 files in all,
with the largest file, the LaTeXML wasm, at 21.7 MB.

Fonts route (`server/fonts.rs`): `--fonts DIR` serves font files by family,
immutable for a year, with a 4096-file cap. It is origin egress.

Not present today: transfer accounting of any kind, per-route or per-IP
request limits beyond uploads and comments, a metrics endpoint, an operator
status command, degraded modes, socket byte accounting, or backup
size reporting. `librepaper backup` prints only the backup id and its path,
and every backup is a full copy.

## Cost model and priorities

The deployment treats resources in this order of financial risk:

1. externally billed origin network transfer and request operations;
2. durable primary storage and backups;
3. resident memory and the server size it requires;
4. CPU and disk I/O for history encoding, reconstruction, and garbage collection;
5. release infrastructure and the project mirror, whose constraint is the
   static-asset platform's file-count and per-file limits rather than
   bandwidth.

Client CPU, browser cache, mirror bytes, and local companion work are not
deployment charges. OAuth requests are expected to be minor, but still receive
timeouts and concurrency bounds.

No single quota is presented as a complete cost bound. A storage quota does not
bound repeated downloads, a request limit does not bound large responses, and a
resident-room limit does not bound reconnection traffic.

## Deployment cost policy

Expose one versioned operator policy covering at least:

- total and per-owner charged storage bytes;
- reserved temporary and maintenance bytes;
- document and input-asset size limits;
- response bytes per rolling 24 hours;
- requests per IP prefix, principal, document, and route class;
- WebSocket connections, update rates, queue depth, and resynchronization bytes;
- resident room count and bytes;
- history jobs, queued input bytes, workers, and checkpoint rates;
- concurrent artifact transfers;
- trusted proxy addresses;
- authenticated publishing permissions and public commenting policy;
- document expiry and retained backup expectations.

Policy values come from explicit settings or documented defaults. The only new
control sets the daily transfer budget; renaming existing controls adds none. Limits
must be inspectable at startup and through the operator status surface in
"Observability and operator controls". Secrets, private owner identifiers,
document paths, and source contents must not appear in that output.

### Operator controls

Set storage with the existing `--storage` and `--quota` flags, and daily origin
transfer with the one new control, `--budget-transfer BYTES`. Each setting controls
the resource it names. Publishing permissions, expiry, and history remain
independent explicit settings. There are no presets or named profiles.

Reject removed flags and explicitly set removed environment variables at startup
with a message naming the replacement, when one exists. Do not silently ignore
removed environment settings or map them to the new names.

Rename `--max-assets` to `--budget-document-assets MIB`: the combined size of
all input assets in one document, not a per-file limit or an asset count.
Preserve its existing numeric units (MiB) and default of 32 MiB; for example,
`--budget-document-assets 8` permits 8 MiB of input assets per document.
These bytes also count against owner and deployment storage allowances.
Use `LIBREPAPER_BUDGET_DOCUMENT_ASSETS` as the environment name.
Remove `--max-assets` and `LIBREPAPER_MAX_ASSETS`. Help, startup output, and
examples use the new name.

Rename `--latex` to `--latex-mirror URL` for an optional operator-hosted HTTPS
mirror. Its default remains `https://latex.librepaper.workers.dev/`. It means
direct browser fetch and no longer accepts a directory or proxies the mirror
through the origin. Use `LIBREPAPER_LATEX_MIRROR` as the environment name.
Remove `--latex` and `LIBREPAPER_LATEX`. Help, startup output, and examples
use `--latex-mirror`.
Rename `--fonts` to `--typst-fonts DIR` to make
its compiler-specific purpose explicit. Do not introduce an `--assets-*`
namespace or additional location controls.

Typst's standard fonts remain bundled with its compiler. `--typst-fonts DIR`
optionally supplies additional fonts from a local directory served by the
origin; these bytes count toward the transfer budget. Do not add a remote-font
URL option. This directory contains input fonts, not retained rendered outputs.
Use `LIBREPAPER_TYPST_FONTS` as the environment name. Remove `--fonts` and
`LIBREPAPER_FONTS`; directory validation is unchanged. Help, startup output,
and examples use `--typst-fonts`.

Remove `--biber-vm` and `LIBREPAPER_BIBER_VM` without a replacement flag or
alias. Biber WASM comes from the same mirror release as the LaTeX engines;
operators do not configure a second bibliography runtime or asset location.

`--budget-transfer 10GiB` sets the daily allowance. Bare integers mean bytes;
zero means no ordinary transfer allowance. Omission preserves today's unlimited
transfer behavior and prints a warning. The emergency allowance is
a bounded internal reserve, not another operator setting. There is no separate
hourly byte budget. Burst protection uses documented request and concurrency
limits. The environment name is `LIBREPAPER_BUDGET_TRANSFER`, following existing
CLI-over-environment precedence.

The budget is measured in bytes, not currency. The previously proposed
`--cost-transfer` spelling is not introduced as an alias.

Existing storage, quota, history, and publication flags remain available with
their current names and units. Unspecified settings use their individually
documented defaults. Exhaustive configuration is not required. Validate actual
incompatible settings, not missing optional values.

Request, socket, queue, worker, memory, and maintenance bounds are implementation
guardrails with documented numeric defaults. Operators need not configure each bound or
coordinate overlapping windows. Advanced configuration may override a specific
bound without restating the whole policy. Keep proxy trust in advanced server
configuration as `trusted_proxies`; no new proxy flag is required. An empty list
means forwarded identity is ignored.

### Transparent defaults

Print each effective limit with its units, scope, window where applicable, and
origin (CLI, environment, configuration file, or built-in default). Include
internal guardrails and the emergency allowance, not just user-supplied values.
An unlimited value is printed as unlimited, never hidden behind a policy name.
Documentation lists the same defaults. No setting implicitly changes publishing
permissions, expiry, history retention, or another resource's budget.

Existing deployments retain the individual defaults listed under "Existing
behavior" and receive the applicable warnings. Validate effective values at
startup. Upgrading LibrePaper
must not silently broaden or narrow an existing deployment's effective
envelope.

## Network transfer

### Accounting

Count response bodies as they leave the origin, including successful range and
conditional responses. A `304` carries no body charge; repeated range requests
count the bytes actually sent.

Classify routes at minimum as:

- application shell and renderer modules;
- fonts, when the origin serves them;
- source and checkpoint reads;
- input assets;
- collaboration state and WebSocket updates, including the assistant channel;
- uploads and mutations;
- authentication and administrative traffic.

Maintain rolling deployment totals and bounded-cardinality breakdowns. Route
templates, not raw URLs or document slugs, identify metrics. Per-document and
per-owner investigation uses hashed internal identities with a rotating
operator-held salt.

Admission happens before a known-size response begins: a response whose
`content-length` or file size exceeds the remaining allowance is refused
without opening the file. A response with unknown final length is admitted
incrementally: each chunk is charged to the byte bucket as it is forwarded,
and the stream ends cleanly with a truncated body when the bucket is empty.
The server never reserves a per-file maximum against the budget, and never
reads a large body into memory merely to account for it.

### Enforcement

Apply token buckets or an equivalent bounded rolling-window mechanism at both
the origin and, where configured, the trusted edge. Origin enforcement remains
authoritative because an absent or bypassed edge must not remove the cost bound.

When a transfer budget is exhausted:

- allow small health, authentication, ownership, deletion, export-control, and
  quota-status responses from a separate emergency allowance;
- preserve WebSocket durability acknowledgements for already-accepted edits;
- refuse new large downloads, uploads, and cold room joins with `429` or
  `503`, a retry hint, and a machine-readable reason.

Budgets reset by elapsed rolling windows, not by a restart. Bucket state is
checkpointed to the catalog database at a bounded interval and on clean
shutdown; recovery after a crash resumes from the last checkpoint, which is
conservative because bytes sent after it are forgotten in the deployment's
favour only for that interval. Because the state lives in the catalog, a
backup carries it and a restore resumes the same allowance rather than a
fresh one.

## LaTeX distribution delivery

### Direct fetch

Browsers fetch compiler engines, formats, package bundles, and the mirror
manifest directly from the mirror URL. `librepaper serve` has nothing to do
with delivering them: it publishes the URL through `/api/config` and serves no
distribution bytes, ever. The `/latex/` route, the directory shape, and the
proxied upstream are removed in the same release that ships the loader change.
Browsers refetch each file once under its mirror URL because Cache Storage
entries are keyed by URL; the release-digest namespace is unchanged.

The loader change follows the Biber WASM worker pattern: fetch the engine
loader as verified text, start it in a blob worker, and point the Emscripten
`locateFile` hook at the mirror so the wasm and its companions resolve against
the mirror rather than the blob URL. The shell's CSP needs no change.

`--latex-mirror` takes one shape: an `https:` URL of a static host that serves the
mirror layout with CORS headers. The project mirror is the default value. An
operator who wants their own copy builds it with `make mirror` in the
wasm-latex repo and pushes it to any static host that sends the same
`_headers`: a Cloudflare static-asset Worker, an object-storage bucket, or a
web server in front of a directory. The binary is not one of those hosts. A
directory path or an `http:` URL is refused at startup with a message that
points at the mirror documentation.

There is no fallback in the binary: engines are always fetched from the mirror,
and there is no origin-side cache or digest verification. The browser's
verification against the manifest is the check.

### Biber bibliography support

Resolve Biber automatically from `engines.biber` in the selected mirror release.
Fetch its worker, glue, WASM, and data directly from that mirror and verify their
manifest digests and sizes before use. Keep the Biber version and its biblatex
package pairing in the release manifest, not in separate deployment settings.

Use the local companion when Biber WASM is absent or cannot initialize or run
because of an infrastructure failure. A bibliography input error is reported
to the user without retrying it on another backend. If the companion is also
unavailable, explain that local Biber is required. Do not boot a Linux/v86 VM
or add a server execution fallback. Results remain transient under the
no-retained-renderings policy.

Remove the `biberVm` field from `/api/config`, the VM loader and worker, VM
build/deployment tooling, and VM-only dependencies and checks. Retain Biber WASM
and companion support, including cancellation and bounded execution. Remove
obsolete flag and environment examples from documentation; an explicitly set
legacy environment variable produces a migration message rather than silently
appearing to configure a runtime that no longer exists.

### The project mirror

The project mirror is a static-asset deployment with free egress. It carries
the releases the current LibrePaper build references, and nothing else is
promised. When a new build references a new release, older releases may be
removed at any time to stay within the platform's file-count and per-file
limits. Operators who need a release to stay available copy the mirror to
their own static host. The startup summary says this in one sentence.

### No per-document release

Documents do not pin a compiler release. Every browser compile uses the
mirror's current default, the same way Markdown and Typst documents already
render with whatever the current build carries. The `latex.release` project
setting, the automatic pin on first compile, and the Update and Undo controls
in the compiler settings are removed. The active renderer may display the
release actually used, but no rendered artifact is retained as a
reproducibility record.

An author whose document needs a particular engine or TeX Live, because a
package changed behavior or the mirror never carried what it uses, renders it
locally through the companion with the TeX installed on their own computer.
The resulting output is viewed or exported locally and is not uploaded for
retention or publication. Readers whose browser cannot compile the source
need a suitable local companion themselves. The UI explains this requirement;
there is no stored-render fallback or guarantee that an old source document
will render with the current mirror release.

The mirror layout, headers, and platform limits are documented in the
wasm-latex repo, not here. The platform's current limits, about 20,000 files
per deployment and 25 MiB per file, bound how many releases the project mirror
can hold at once and how large one engine file can grow; they are confirmed
against the provider's documentation before a release is built, not assumed.

### Privacy

`README.md` must include a note under a top-level `# Privacy` section explaining
direct compiler downloads. State that the mirror receives the browser's IP
address and the digest-named files it requests; those requests can reveal
package choices and suggest a document's field or template. Compiler downloads
do not send document source or private input assets to the mirror.

The note must identify the default project mirror and explain that operators
can use `--latex-mirror URL` to keep compiler requests on their own
infrastructure by hosting a mirror copy.

### Edge caching

An edge cache is a shared copy of responses kept by a reverse proxy or CDN.
The compiler mirror may cache its public distribution files. Responses
containing document source, checkpoints, input assets, annotations, or account
data must not be stored in shared proxy or CDN caches, even when a document
has a public share link. There is no authenticated-cache exception.
LibrePaper does not retain rendered outputs anywhere, including at an edge.

Send cache-control headers that prohibit shared caching of document and account
responses, and require deployment proxy/CDN configuration to honor them. Public
application files and deployment fonts may be cached only when access is
independent of any document or user. Private browser caching of source or input
assets remains separate from this rule; rendered outputs are not persisted.

### Client identity behind a proxy

Replace the current private-address heuristic with a configured list of
trusted proxy IP addresses or CIDR prefixes, using `trusted_proxies` in advanced
server configuration. An absent or empty list means use the TCP peer address
and ignore forwarded identity. Loopback and private addresses are not trusted
automatically.

For IP-based admission, resolve `X-Forwarded-For` as follows:

1. If the TCP peer is not trusted, ignore the header and use that peer's IP.
2. Otherwise, parse a bounded list of IP addresses and walk it right to left,
   starting from the TCP peer. Skip trusted proxy addresses and stop at the
   first untrusted address. Use that address as the client identity; entries
   farther left cannot override it.
3. If the header is absent, malformed, exceeds the documented size or hop
   limit, or contains no untrusted address, use the TCP peer address. Never
   use arbitrary header text as a rate-limit key. Normalize equivalent IP
   representations before trust checks and network-prefix grouping.

Support this one forwarding-header convention for admission; do not combine
it with `Forwarded`, `X-Real-IP`, or provider-specific identity headers. Trusted
proxies must overwrite incoming forwarding headers or append the actual
connecting client's IP. They must not pass an unverified header unchanged.

Document a single-proxy example: a reverse proxy connects to LibrePaper from
`127.0.0.1`, and `trusted_proxies` contains only `127.0.0.1/32` (plus `::1/128`
if that proxy uses IPv6 loopback). The proxy appends the actual client IP to
`X-Forwarded-For`. For a connection from `198.51.100.23`, a header containing
`203.0.113.9, 198.51.100.23` resolves to `198.51.100.23`, ignoring the spoofed
leftmost entry. Trust only networks whose members are controlled proxies.

Without this configuration, visitors behind one proxy share its IP-based
limits. Explain that consequence in the deployment documentation. Direct
origin access must either be blocked by deployment or receive the same origin
admission policy. Edge cache-status headers may be used for observability only
when a configured proxy overwrites them; they never control admission or byte
accounting.

## No retained renderings; assets and document reads

LibrePaper MUST NOT retain a rendered copy of any source. This includes PDF,
HTML and its generated bundle assets, DOCX, preview images, thumbnails, and
other generated document outputs. There is no exception for the latest render,
publication, a share link, a companion render, or a rendering cache.

Readers render source on demand in their browser or local companion. Generated
output may exist transiently for the active view or be exported by the user to
their own files; LibrePaper does not persist it in browser storage, origin
storage, an edge cache, source history, or new backups. Compiler distributions
may still be cached. Original input assets remain subject to normal storage
quotas; a PDF supplied as an input is not a retained rendering merely because
of its file type. Render pipelines must not reclassify outputs as input assets
to bypass this policy.

Sharing publishes access to source and input assets, not a frozen rendered
result. A cold reader may need a compiler download, and an unsupported document
may require local tools. If rendering fails, show the failure and keep source
access available rather than serving an older retained output.

This rule supersedes any earlier specification that requires a retained
publication bundle or latest PDF. Source, input assets, and annotations remain
durable; rendering retention is not an operator-selectable option.

Use content-addressed immutable URLs for stored input assets where the
authorization model permits it, but authorization remains checked before bytes
are served. Content-addressed URLs do not make these responses eligible for a
shared proxy or CDN cache; the rule under "Edge caching" applies.

Support conditional and range requests without reconstructing or reading bytes
which will not be returned. Coalesce concurrent reconstruction of the same
historical file. Bound reconstructed output, concurrent readers, and queued
reconstruction bytes as required by `SPEC-01-history-retention.md`.

Rate limits distinguish inexpensive metadata from source trees, input assets,
and cold state transfers. A document made popular through a share link cannot
consume unlimited origin egress merely because it creates no new stored bytes.

## Live collaboration

Keep the existing room-count, resident-byte, peer-queue, and update-rate limits
listed under "Existing behavior". Add explicit ceilings for:

- concurrent sockets per client network, principal, and document;
- aggregate sockets per deployment, counting collaboration, assistant-channel,
  and companion sockets together;
- state-transfer and resynchronization bytes per client and deployment window;
- aggregate queued socket bytes, not only frame count;
- maximum socket lifetime without successful application-level activity.

A slow peer is disconnected before its queue exceeds either the frame or byte
limit. Reconnection is not an unlimited exemption: repeated full-state syncs
consume the state-transfer budget. Editors receive a clear throttled state and
may retry after the named interval.

Room eviction persists accepted edits before releasing memory, as it does
today. Under memory pressure, evict idle rooms and refuse cold joins before
terminating the process. Do not count OS virtual memory as resident-room usage;
measure conservative owned buffers and also export process RSS for operator
comparison.

## Storage and retention

The physical-byte rules, reservations, and eviction order in
`SPEC-01-history-retention.md` are authoritative. This specification adds the
following deployment requirements:

- `--storage` remains a hard admission ceiling on charged bytes, and is
  configured below the filesystem or volume capacity. Its meaning follows
  SPEC-01's migration from the logical full-tree sum to retained physical
  bytes; this document does not preserve the logical meaning.
- Reserve explicit headroom for SQLite pages, indexes, allocator slack, object
  packing holes, temporary files, in-flight encodings, deletion queues, logs,
  and restores.
- Report charged bytes, allocated primary-storage bytes, reserved bytes, cache
  bytes, and free filesystem bytes separately.
- Refuse growth before free space reaches the operator's emergency floor even
  when charged owner quotas would otherwise permit it.
- Finite checkpoint count and age retention are available without requiring
  chunked encoding. Operators may set a finite count explicitly with `--history`;
  omission keeps today's unlimited count and warns.
- Abandoned uploads and permitted regenerable caches are collected
  routinely rather than only under pressure.
- Every newly published document belongs to an authenticated Google or GitHub
  account and is charged against that owner's quota and deployment limits.

The newest committed source remains protected as specified elsewhere. Near a
physical ceiling, optional history capture and input-asset uploads may be refused
while accepted live edits continue through their bounded durability path.

## Authentication and public access

Publishing requires a verified Google or GitHub sign-in on every deployment,
including loopback and private listeners. There is no anonymous-publishing
option or bypass. `--publishers` may further restrict which signed-in accounts
can publish; `any` means any authenticated Google or GitHub account. Remove
the old `anyone` spelling. No value permits unsigned-in publishing.

Before sign-in, users may only read and comment on documents whose access rules
allow those actions. They cannot create documents, upload source or input
assets, edit source, publish through the companion or CLI, change sharing or
ownership, or perform other owner mutations. Reading and commenting do not
require OAuth, but remain subject to document access and commenting permissions.
A read or comment share link does not grant publishing authority.

Enforce this boundary on the server for HTTP, WebSocket, CLI, and companion
paths, not just in browser controls. CLI and companion credentials must resolve
to an account authenticated through Google or GitHub. Reject unsigned-in
publishing with a stable authentication-required reason before accepting
document data. Signed-in users still require the relevant document permission.
If neither provider is configured, reading and commenting remain available;
publishing is unavailable and startup explains how to configure a provider.

An anonymous comment cookie is not a durable cost identity because it can be
discarded. Per-cookie quotas improve ordinary behavior but are not abuse
protection. Per-network request, comment, socket, and transfer limits supplement
deployment-wide budgets. Authenticated publishers also remain subject to owner
and deployment quotas; renewing a cookie or signing in never resets them.

Comment body, reply, total-comment, request-rate, and checkpoint admission
limits apply, since an anonymous comment can cause durable storage and a
checkpoint even when it carries little text. Comment permission authorizes
only comment operations, not source updates carried on the same connection.

## Server work and I/O

Rendering, semantic projection, and diff calculation remain browser work.
Quarto and native TeX execution remain in the user's companion. The server does
not add a fallback compilation service when a client is unavailable.

History hashing, chunking, compression, reconstruction, quota calculation, and
garbage collection use the bounded workers and queues required by
`SPEC-01-history-retention.md`. Admission accounts for:

- input bytes and estimated output reservation;
- worker CPU concurrency;
- temporary memory and disk;
- object and database operations;
- checkpoint owner and deployment rolling-hour budgets.

Maintenance is incremental and interruptible. Each pass has object, row, byte,
and wall-time budgets. It yields to accepted durability work and persists a
cursor rather than repeatedly scanning the entire catalogue, as the account
erasure and deletion-discovery passes in `storage/maintenance.rs` already do.
An operator can schedule expensive verification, compaction, and backup
outside peak hours.

## Authentication and external services

OAuth exchanges and account lookups use strict connect and total timeouts,
bounded concurrency, and no unbounded retry. Cache stable public account
resolution results for an operator-configured period while respecting identity
and authorization changes. Authentication failures must not trigger a tight
polling loop.

LibrePaper does not make maintainer-funded model API calls. The assistant
channel in `server/chat.rs` relays bounded events between a browser and a
runner on the user's own computer; the server holds no model credentials.
Agent integrations must use credentials and budgets belonging to the invoking
user or an explicitly configured organization. Adding server-funded inference
requires a separate specification with per-principal token and currency
budgets, request admission, cancellation, and billing-failure behavior.

## Backups and disaster recovery

Primary storage quotas do not bound backups. Document a recommended backup
policy and allow operators to declare one for reporting. A declaration is
optional and never a startup requirement. Useful policy information includes:

- destination class;
- full versus incremental or deduplicated behavior;
- frequency;
- number or age of retained recovery points;
- encryption and restore expectations;
- estimated retained and transferred bytes.

Today `librepaper backup` prints only the backup id and output path, and every
backup is a complete copy. It will report the logical input bytes, bytes
written, and that the destination is a new full copy. It warns when the output
directory already holds more than a configured number of uniquely named
backups from this deployment, which is the one case detectable without
managing the destination. LibrePaper does not delete operator backups
implicitly. Incremental or deduplicated backups are a later change and are not
required by this specification.

Backup temporary files use a specified directory and capacity reservation.
Creating a backup cannot consume the primary volume's emergency headroom.
Restore requires a fresh destination, as it does today, validates required
capacity before writing, and never overwrites the only usable deployment in
place.

## Degraded modes

Resource pressure has two observable states: **Normal** and **Limited**.
Normal means admitted operations proceed. Limited names the exhausted resource
and refuses only operations that would consume it. Cache eviction, reduced
optional concurrency, and maintenance deferral happen automatically; they are
not separate modes that an operator configures.

Approaching a limit produces a warning without another state transition.
In Limited, preserve already-accepted durability and bounded control, deletion,
and export-control operations through the emergency allowance. Stop growth when
storage is exhausted and refuse large downloads when transfer is exhausted.

One resource entering a restricted state does not imply unrelated damage. For
example, an exhausted download budget must not prevent a small Markdown
document already in memory from being edited and saved.

Every refusal returns a stable reason code, the limiting scope, whether retry is
useful, and a retry time where known. Do not expose deployment-wide usage to an
unauthorized caller beyond what is necessary to explain its own refusal.

## Observability and operator controls

Counters are emitted two ways, and neither requires a new network listener:

- Structured JSON events on standard output, in the existing `event` style
  used by the retention pass, at a bounded interval and on every degraded-mode
  transition. These are the metrics feed; a host agent scrapes the log.
- An operator status surface: a loopback-only HTTP endpoint that refuses any
  peer that is not the loopback interface, and a `librepaper status` command
  that queries it and prints the effective policy, current usage by class, and
  the degraded mode. Durable counters that live in the catalog are also
  readable by the command when the server is stopped.

Emit structured counters and bounded histograms for:

- response bytes by route class and status;
- uploads accepted and refused by reason;
- charged, allocated, reserved, cache, backup-output, and free bytes;
- active sockets, socket queue bytes, state-sync bytes, rooms, and room bytes;
- checkpoint, encoding, reconstruction, retention, and GC jobs and bytes;
- queue delay, worker saturation, process RSS, CPU time, and disk I/O where
  available;
- time spent in each degraded mode.

Do not label general metrics with raw account IDs, slugs, paths, share keys, IP
addresses, source text, annotation quotations, or OAuth data. A separate
operator-only diagnostic command may identify top consumers using salted
identities and explicit access controls.

At startup, print a concise cost-policy summary and warnings for:

- neither Google nor GitHub sign-in configured, making publishing unavailable;
- an unlimited checkpoint count, which is the default today;
- a storage ceiling too close to currently available disk;
- missing transfer limits;
- a room-memory ceiling incompatible with detected or configured host memory;

Proxy trust and backup policy are reported when configured. Their omission is
not itself a startup warning; deployments may have no proxy or use external
backup tooling.

The summary names the mirror in use and, for the project mirror, the one
sentence that no release beyond the current build's is promised.

Configuration validation is side-effect free. It does not contact a paid
service or allocate the configured maximum merely to prove that a limit parses.

## Capacity planning

Document how operators choose explicit storage and transfer limits from their
host capacity and traffic measurements. Examples show actual flag values and
their individual effects; they do not define named configurations or silently
set authentication, history, or expiry policies.

Transfer budgets cover what the origin serves: the shell, input assets, source,
fonts, and collaboration state. Compiler bytes fetched from the mirror are
never in them, and rendered copies are not served by the origin. For example,
`--budget-transfer 10GiB` permits 10 GiB per rolling 24 hours; it changes no storage,
request, or concurrency limit and is not a universal recommendation.

New implementation defaults require measurement of the binary, SQLite, history
workers, and typical documents on the smallest supported host. Limits must leave
headroom for the process, OS page cache, proxy, and maintenance; it cannot
allocate all nominal host memory or disk to rooms and charged objects.

## Migration

Phase A, mirror and transfer:

1. Ship direct mirror fetch: the blob-worker loader with `locateFile` pointed
   at the mirror, `/api/config` carrying the mirror URL, removal of the
   `/latex/` route and of the directory and proxy shapes of `--latex`, and a
   startup refusal for anything but an `https:` URL. Remove the
   `latex.release` project setting, the automatic pin, and the Update and
   Undo controls; existing values are ignored, not migrated. Update the
   README's `--latex` and `make deploy` examples to use `--latex-mirror` for
   direct HTTPS mirror
   fetching, confirm the platform
   limits, and add the no-retention statement to wasm-latex's mirror
   documentation. Add the direct-download disclosure to `README.md` under
   `# Privacy`, including the default mirror and operator-hosted alternative.
   Remove the Biber VM flag, environment setting, config field,
   runtime, and VM-only tooling. Use mirror-provided Biber WASM with the local
   companion as the sole fallback, as specified above.
   Rename the optional local font setting to `--typst-fonts`, remove the old
   flag and environment names, and update help and examples. Keep
   standard Typst fonts bundled and additional fonts origin-served.
   Rename `--max-assets` to `--budget-document-assets`, preserving its MiB
   units and default; remove the old flag and environment names. Update its help to state
   that the allowance covers all input assets in one document combined.
   Remove rendered-output upload, storage, and publication
   paths for every renderer, including companion outputs, and persistent
   client rendering caches. Switch readers to on-demand rendering. Before
   collecting legacy outputs, validate that source and input assets remain
   intact; remove only generated-output references and objects, preserving
   objects also referenced as inputs. Clear legacy browser rendering caches
   on the next client startup. Existing operator backups are not rewritten or
   deleted; new backups omit generated outputs, and restores strip legacy
   rendering references and collect their unreferenced objects before serving.
2. Add observability for route-class bytes, allocated storage, worker queues,
   rooms, and sockets without changing admission. Emit them as structured
   stdout events.
3. Require Google or GitHub authentication on all publishing and source-write
   paths, including CLI, companion, and WebSocket operations. Keep unsigned-in
   reading and commenting where permitted. Use `--publishers any` for any
   authenticated publisher and reject `anyone`. Preserve legacy documents and annotations,
   but anonymous owner credentials no longer authorize publishing or source
   changes. Attaching legacy ownership to a signed-in account requires proof of
   the existing ownership credential as well as successful OAuth; never infer
   ownership from a public share link. Add warnings for missing sign-in providers,
   unlimited history, absent
   transfer budgets, and unsafe disk headroom. Correct the
   `--history` help text.
4. Implement the durable daily transfer budget and internal emergency allowance,
   checkpointed to the catalog. Expose `--budget-transfer`; an explicit value is
   enforced, while omission leaves accounting without a limit.
   Compare application counts with proxy or provider totals. Add the advanced
   `trusted_proxies` configuration key.

Phase B, policy reporting and modes:

5. Enforce document-download, state-resynchronization, socket-byte, and
   deployment-wide request budgets.
6. Report every effective limit and its origin. Document individual defaults
   and advanced overrides. Existing deployments retain their settings and
   numeric defaults; no preset selector or bundled policy is introduced.
7. Add degraded modes and the operator status surface after refusal paths have
   tests proving accepted writes remain durable.
8. Add backup size reporting and policy documentation. Backup deletion remains
   an explicit operator action.

Enforcement should first ship with the ceilings listed under "Existing
behavior" and clear telemetry. A limit is tightened only after representative
public and private deployment measurements show its effect. Security-critical
absolute size and memory bounds remain enforced throughout migration.

## Acceptance criteria

Phase A:

1. A fresh browser's cold compile produces no distribution bytes on the
   origin: the binary has no `/latex/` route and serves only the mirror URL.
2. Every distribution file a browser uses was verified against the manifest's
   sha256 and size before use.
3. A document carrying a `latex.release` value from before this change
   attempts compilation with the mirror's current default. Browser and
   companion rendering, sharing, and publication retain no generated PDF,
   HTML bundle, or other rendered copy, including the latest result. Opening
   the document in a fresh browser renders from source or explains the local
   tool requirement. Migration removes legacy generated outputs without
   deleting source, input assets, or annotations; new backups and restored
   deployments do not retain those outputs.
4. A deployment can set hard transfer budgets and inspect their effective
   values at startup and in the structured event stream.
5. Restarting the server does not reset a durable daily transfer allowance to
   its full value, and a restored backup resumes the allowance it carried.
6. A response known not to fit its remaining byte allowance is refused before
   its body begins; unknown-length streaming charges each forwarded chunk and
   never reserves a per-file maximum.
7. `--latex-mirror` with a directory path or an `http:` URL is refused at startup
   with a message pointing at the mirror documentation, and an operator copy
   on any static host with the documented headers works unchanged.
8. Forwarded client identity is believed only from a configured proxy peer;
   an absent proxy list uses the TCP peer address without preventing startup.
   Tests cover untrusted direct peers supplying spoofed headers, forged
   leftmost entries through a trusted proxy, multiple trusted proxy hops,
   stopping at an intervening untrusted hop, IPv4 and IPv6 normalization,
   missing or malformed headers, over-limit chains, and all-trusted chains.
   Documentation includes the single-loopback-proxy example and explains
   shared IP limits when proxy trust is not configured.
9. Exhausting a download budget does not prevent small control, deletion,
   export-control, authentication, quota-status, or durability responses
   covered by the emergency allowance.
10. Metrics explain network, storage, memory, and work consumption by bounded
    route/resource classes without source content or high-cardinality public
    identifiers.

Phase A also requires Biber WASM to work without any Biber-specific deployment
setting. Verify direct mirror fetch and digest/size checks, companion fallback
for absent or unavailable WASM, terminal bibliography input errors, and a clear
failure when neither backend is available. No path loads a Linux/v86 VM;
`--biber-vm` is rejected as removed and `/api/config` contains no `biberVm` field.

Phase B:

11. A deployment can set hard total storage, request, resident-memory,
    socket, and server-work envelopes and inspect them through the operator
    status surface, which is unreachable from a non-loopback peer.
12. Clearing an anonymous cookie does not bypass deployment-wide storage,
    transfer, request, checkpoint, or work limits.
13. A popular read-only share link cannot produce unlimited origin egress
    under the configured policy.
14. Slow WebSocket peers are bounded by frames and bytes. Repeated reconnects
    consume a state-resynchronization allowance.
15. Memory pressure evicts idle rooms or refuses cold joins before exceeding
    the room budget, while already-accepted edits receive a durability result.
16. Physical disk headroom can stop growth before the filesystem fills even
    when owner quota remains.
17. Temporary encoding, backup, restore, upload, and garbage-collection bytes
    are reserved independently of charged durable bytes.
18. Maintenance passes have bounded rows, objects, bytes, and wall time and
    resume without rescanning the entire catalogue.
19. Refusals identify a stable reason, scope, retryability, and retry time
    without disclosing another owner's usage.
20. Backup output reports input bytes and bytes written, and the documentation
    makes clear that primary quotas do not limit retained recovery points.
21. No ordinary server path invokes maintainer-funded AI inference or
    server-side document compilation.
22. A deployment without explicit overrides retains today's individual
    defaults, and upgrading neither broadens nor narrows any budget or deletes
    source, source history, input assets, annotations, or operator backups.
    Removal of legacy rendered copies and rendering caches is the explicit
    storage migration exception; numeric budget defaults do not change.
    Mandatory publishing authentication and explicit proxy trust are deliberate
    security changes, not preservation of anonymous write access.
23. Only `--budget-transfer` adds a control to the everyday CLI. No presets or named
    profiles are introduced. The obsolete `--biber-vm` flag is removed; other
    than renaming `--fonts` to `--typst-fonts`, `--latex` to `--latex-mirror`,
    and `--max-assets` to `--budget-document-assets`,
    existing asset flag names remain. Removed flags and environment names have
    no aliases: tests verify rejection of `--fonts`, `--latex`, `--max-assets`,
    and their old environment variables with messages naming their replacements.
    Every effective
    limit reports its value and origin; changing one limit does not change
    unrelated settings. Omitted advanced settings do not prevent startup. Transfer
    uses one rolling 24-hour allowance, and status reports Normal or Limited
    with the affected resource.
24. Unsigned-in users can read and comment where permitted, but all publishing,
    source editing, input-asset uploads, and owner mutations require Google or
    GitHub authentication and the relevant authorization. Test HTTP, WebSocket,
    CLI, and companion paths, including `--publishers any`, private listeners,
    and attempts to submit source updates with only comment permission. A
    deployment without either provider cannot publish. Existing anonymous
    credentials and public share links cannot bypass this requirement. The old
    `--publishers anyone` spelling is rejected with a message naming `any`.
25. Typst uses its bundled standard fonts without configuration. Optional
    additional fonts come from `--typst-fonts DIR`, are served by the origin,
    and count toward its transfer allowance. Only `--typst-fonts` and
    `LIBREPAPER_TYPST_FONTS` configure these fonts; no remote-font option is added.
26. `--budget-document-assets` limits combined input-asset bytes per document,
    preserves the 32 MiB default and numeric MiB units, and uses
    `LIBREPAPER_BUDGET_DOCUMENT_ASSETS` as its environment name.
    It does not alter owner or deployment
    storage limits or permit retention of rendered outputs.

## Open questions

- What emergency allowance is sufficient for deletion, export control, and
  durability acknowledgements without becoming a bypass for ordinary traffic?
- Should the application emit standard rate-limit headers for byte budgets as
  well as request budgets?
- Which IP-prefix grouping defaults work acceptably for IPv4 NATs and IPv6
  privacy addresses without penalizing campuses and organizations?
- How should operators declare backup retention so LibrePaper can warn usefully
  without managing or deleting backup destinations?
- Which provider totals can be reconciled automatically without adding
  provider-specific credentials or a billing integration?
- When the project mirror's file count approaches the platform limit, does
  the mirror move to object storage with the same headers, or does the build
  drop the previous release on push?
