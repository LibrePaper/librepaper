# Proposed: Incremental Rust-to-Go migration behind a C archive

**Status:** Proposed

**Date:** 2026-10-03

**Scope:** Investigate and, if the platform gate passes, incrementally move selected application behavior from Rust to Go while retaining the Rust/WASM renderers and Rust document authority.

## Summary

This proposal uses Go as an implementation language for selected application behavior while Rust calls exported Go functions through a Go `c-archive`. The first production behavior moved is the remote `list` command. The archive is linked by a small Rust bridge crate. A thin executable composes that bridge with a separate Rust core crate.

The dependency direction is a hard requirement:

```text
librepaper executable
  ├── librepaper-core       (existing Rust application/core code)
  └── librepaper-go-bridge  (Rust FFI wrapper + Go archive build)
```

`librepaper-core` must not depend on `librepaper-go-bridge`, invoke its build script, or import its generated headers. Otherwise changes to Go bridge code can invalidate the large Rust core build. The Go archive and bridge are siblings composed only by the executable. Core Rust code must remain buildable and testable without a Go toolchain wherever that is a supported development workflow.

The proposal is staged. Stage 1 measures a controlled baseline. Stage 2 decides whether this is viable across release targets. Stages 3–6 establish the crate boundary, ABI, and `list` parity. Later stages move related features in coherent slices. A single user-facing executable remains preferred; helper executables are an explicit fallback only if the platform gate fails, not the initial architecture.

This document is a plan, not a commitment that Go will improve build time. No performance gain is asserted before measurement. The recorded test timings include actual test execution and should not be read as a one-line edit or as a prediction of Go savings.

## Observed repository state

- The repository currently has one Cargo package, one library and one `librepaper` binary. See [`Cargo.toml`](../../../Cargo.toml) and [`crates/librepaper/src/lib.rs`](../../../crates/librepaper/src/lib.rs).
- `main.rs` delegates to `librepaper::main()`. CLI parsing and command dispatch live in [`crates/librepaper/src/cli/mod.rs`](../../../crates/librepaper/src/cli/mod.rs); the async `main` dispatches `Command::List` to `list_documents`.
- The current `list` implementation lives in [`crates/librepaper/src/cli/documents.rs`](../../../crates/librepaper/src/cli/documents.rs). It makes an authenticated HTTP request, derives short IDs from configuration, and prints aligned rows.
- The CLI currently also launches the local companion by default and directly dispatches deployment administration, storage backup/restore, moderation, export and MCP commands. It is not a remote-only command crate.
- Companion code is under [`crates/librepaper-companion/src/local/`](../../../crates/librepaper-companion/src/local/). Its `protocol.rs` defines JSON-shaped request/response types. The HTTP service is `local/service/`; local launch/embedding is in `local/cli.rs`, `local/lifecycle.rs` and `local/embedded.rs`.
- The local service currently embeds assistant session state and routes. Assistant and automation modules refer to one another. Moving either one in isolation will require an explicit boundary.
- The Rust document model and collaboration implementation use Loro. Local backup/export code also names Rust projection/archive types. These are not simple CLI leaf dependencies.
- The release configuration currently targets Linux x86_64 musl, Linux ARM64 musl, macOS x86_64/ARM64, and Windows MSVC. See `Cargo.toml` package metadata and release configuration. These target names do not prove that a Go C archive can be built and linked for them.

## Baseline evidence and hypotheses

### Recorded evidence

The current observed timing is:

| Measurement | Time | Recorded condition |
| --- | ---: | --- |
| Test compilation | 79.8 s | Dependency artifacts were cached |
| Additional Nextest preparation | 13.1 s | Cause not yet explained |
| Test execution | 32.8 s | 707 passed, 177 skipped |

The timing report identifies Linux x86-64, 16 reported CPUs/jobs, and Rust 1.99 nightly. It does not establish that the run used the current spec worktree snapshot, so no source revision is attached to these figures. These measurements should be repeated with commands, machine/toolchain versions, clean/incremental state, and per-phase timing recorded. The 13.1-second preparation phase needs attribution before it can guide architecture. Actual execution time is not compile time.

### Hypotheses to test

- A Go archive could reduce edit/build latency for code moved out of Rust, provided Cargo does not rebuild the Rust core when Go changes.
- The benefit may be limited if the edited Rust bridge or executable remains in the same crate as the core. A sibling dependency boundary is required.
- Keeping Rust core dependencies such as Loro, SQLx, TLS and storage in the main core means their compile cost remains. Replacing a command such as `list` does not make those dependencies disappear from builds that still compile the core.
- Go's C archive integration may increase release-build complexity, especially for musl, ARM64, macOS and Windows MSVC. It may cost more than it saves if target toolchains are unreliable.
- The Go runtime is linked into the executable that uses the archive. The Rust core library must not itself contain or link that archive; downstream Rust library consumers must not inherit a Go runtime requirement.

## Goals

1. Establish whether the C archive toolchain works for every supported release target before broad migration.
2. Move behavior in small, reviewable vertical slices with observable parity.
3. Ensure Go implementation edits rebuild the archive, bridge and thin executable without recompiling the heavy Rust core.
4. Preserve one user-facing `librepaper` command and existing command behavior where feasible.
5. Keep Rust as the sole authority for document commits and collaborative state until any future storage/Loro migration has a proven compatibility design.
6. Keep the browser renderer/WASM build in Rust. Do not confuse browser WASM artifacts with Go's C archive.
7. Maintain compatible CLI output, HTTP contracts, on-disk state, authorization boundaries and release behavior during migration.

## Non-goals

- Rewriting the WASM renderers or replacing browser-side renderer artifacts.
- Claiming an overall compile-time improvement based on language reputation or cached-dependency timings.
- Performing dual writes to Rust and Go implementations in production.
- Moving document commit authority, persistence formats, or Loro state before compatibility and crash-recovery evidence exists.
- Splitting command dispatch across user-visible executables as the default design.

## Invariants

### Build and dependency invariants

- The executable is the composition root and may depend on both Rust core and Rust bridge crates.
- The Rust bridge owns the Go archive build/link integration. The core never depends on bridge artifacts or generated bindings.
- A core source change must leave the Go archive fresh; the final executable may still need relinking when its Rust core dependency changes.
- A Go source or Go module change may rebuild Go archive, bridge as needed, and the executable; it must not compile Rust core.
- Core tests and core-only builds must not invoke Go. Bridge/executable builds may require Go and a C toolchain.
- Build artifacts are segregated by target triple, profile and relevant Go build settings. Concurrent Cargo builds must not race over one archive/header path.
- A Rust library artifact intended for external consumers must not contain the Go archive/runtime. If the public crate topology cannot ensure that, the Go archive stays at the binary composition edge.

### Runtime and compatibility invariants

- The first Go call is synchronous from the CLI's point of view, but any blocking C archive call is kept off Tokio executor worker threads. The caller receives bounded completion or a timeout result.
- No Go function calls back into Rust. No cyclic callback protocol is introduced.
- No `os.Exit`, process signal handler ownership, or Go-owned process shutdown is permitted in a library-style archive entry point.
- ABI strings and byte buffers have explicit ownership, UTF-8 rules, maximum sizes, and matching free functions. Rust never frees Go-managed memory directly, and Go never retains Rust pointers after a call returns.
- Errors cross the ABI as stable status codes plus owned UTF-8 messages. Panics/recoverable Go panics must not unwind across C/Rust frames.
- Long-running job APIs, when introduced, have explicit start/status/cancel/shutdown methods and deadlines. Go code must use bounded work and cooperatively honor deadlines/cancellation internally. A Rust wrapper timeout or `spawn_blocking` boundary alone does not stop Go work already running; cancellation must reach and be observed by the Go operation. They do not occupy an executor thread indefinitely.
- Each production request is handled by exactly one selected implementation. Rollback selects the old implementation before an operation begins; no retry against the alternate backend follows an ambiguous mutation.
- No production dual writes. Reads may be shadow-compared only where they have no side effects and sensitive values are handled safely.
- Existing authorization checks, path confinement, origin scoping, grant semantics and data formats remain authoritative.

## Proposed crate and source boundaries

Names below are illustrative until Stage 3 settles packaging. The important property is the dependency graph, not the exact names.

```text
crates/librepaper-core/       Rust library: application and core behavior
crates/librepaper-go-bridge/  Rust library: FFI declarations/wrappers + build.rs
go/                           Go module and packages; c-archive entry package
crates/librepaper/            thin CLI executable / composition root
```

An alternative is to retain the current package name for the executable and create a sibling package named `librepaper-core`; package identity and release publishing need a deliberate decision in Stage 6.

### Core crate

Initially move the existing Rust library modules into the core with behavior unchanged. Preserve existing module APIs for internal consumers and tests. The core may continue to contain CLI helpers during early stages, but the executable must own command parsing/dispatch that chooses the Go list implementation. Over time, remove moved Go-owned paths from core and make any remaining CLI helpers private to the executable or a narrowly scoped Rust support crate.

The core must not have a Cargo dependency on the bridge. It must not include the bridge's `OUT_DIR`, C header, generated Rust bindings, or link directives. Avoid a core `build.rs` that probes for Go.

### Bridge crate

The bridge contains only:

- FFI declarations generated or checked against a stable C header;
- Rust-safe wrappers that accept Rust values and translate to the ABI;
- buffer ownership and error decoding;
- `build.rs` instructions to build/link the Go C archive for the current target;
- no domain policy and no direct dependency on `librepaper-core`.

It may depend on a tiny ABI-types crate if that crate contains only language-neutral identifiers/constants and does not depend on core. Prefer keeping the stable contract in the header plus versioned byte messages to reduce coupling.

### Go module

Keep application Go packages ordinary Go packages. Only a small `package main` with `//export` functions is built with `-buildmode=c-archive`. This limits cgo constraints and keeps logic independently testable with the Go toolchain. The archive entry functions call into those packages, validate ABI inputs and serialize outputs. Do not expose Go structs, slices, strings, interfaces or goroutines as C ABI types.

### Executable

The executable owns Clap parsing and command selection during the first vertical slice. It depends on core for Rust commands and bridge for the selected Go command. `main.rs` stays thin. The executable may link Go runtime; the core crate and its Rust-only test binaries must not.

## Staged implementation plan

Each stage is a separate reviewable change. Stage completion means its acceptance criteria pass; later stages do not begin merely because the previous code compiles.

### Stage 1 — Controlled baseline

Record reproducible timings before changing build topology.

Capture the exact commands and environment for:

- clean dependency build and clean application build;
- incremental Rust core edit/build;
- incremental CLI edit/build;
- unchanged rerun after each baseline build;
- focused test run after a test-only edit, separately from broad test preparation;
- Go-only edit/archive rebuild once a prototype exists;
- full test compile/preparation/execution as distinct phases;
- release cross-builds by target.

Record CPU/OS, Rust version, Cargo version, Go version, C compiler/linker versions, cache state, target triple, profile and whether parallelism is enabled. Attribute the unexplained 13.1-second Nextest preparation phase. Preserve the existing observation (79.8 seconds test compilation with cached dependencies, 13.1 seconds additional preparation, 32.8 seconds actual execution, 707 passed/177 skipped) as one point of evidence, not a universal baseline.

**Acceptance:** a short benchmark note states commands, conditions, phase boundaries and repeatability. No estimate of Go savings is recorded before comparing equivalent workloads.

### Stage 2 — C archive ABI and platform spike

Build a throwaway minimal exported Go function and call it from Rust using the proposed C archive flow. Do not migrate product code. The spike must validate:

- `go build -buildmode=c-archive` output/header generation;
- Rust linking and executable launch;
- explicit input/output ownership and error return;
- panic containment and repeated/concurrent calls;
- Cargo artifact isolation and `rerun-if-changed` behavior;
- clean and incremental archive rebuild timings;
- release target viability for Linux x86_64 musl static, Linux ARM64 musl static, macOS x86_64, macOS ARM64 and Windows Rust MSVC;
- packaging, codesigning/notarization implications where applicable, and the final binary's runtime dependencies.

Use actual release toolchains, not just host-native development builds. Confirm the resulting executable remains compatible with current release constraints. Do not assume cross-target support merely because both Go and Rust list the target.

**Gate:** proceed with one user-facing executable only if all supported targets have a maintainable build and release path. If the gate fails, evaluate an explicit helper-process fallback as a separate architectural decision. Do not silently switch to sidecar-first during the archive plan.

**Acceptance:** a documented target matrix has a working result or a concrete blocker for every target, with a decision to proceed or stop. No product behavior has moved.

### Stage 3 — Isolate core, bridge and composition root

Create sibling Rust packages and relocate existing code with no behavior change. The top executable composes the two siblings. Keep this stage independent of feature migration so build invalidation can be inspected clearly.

The dependency graph must be:

```text
binary -> core
binary -> bridge -> Go archive
```

It must not become:

```text
core -> bridge -> Go archive
```

Move `main`/command composition to the executable. Retain the current public Rust crate surface and `crates/librepaper/tests/` consumers intentionally; update imports with minimal churn. Decide which tests belong to core and which require the executable/bridge. The bridge must not drag core into its own build script or compile Go while building core tests.

**Acceptance:**

- Rust core-only build/test commands complete without Go installed or invoked.
- An edit to Go source does not cause Rust core compilation.
- An edit to core source does not rebuild the Go archive.
- A bridge or executable source change relinks the executable as expected.
- Rust library consumers link without Go runtime symbols.
- Cargo metadata and packaging identify the intended default package/target explicitly.

### Stage 4 — Versioned byte ABI and call lifecycle

Define a small, versioned ABI before product behavior migrates. Prefer `uint32`/`int32` status values and `(pointer, length)` byte buffers over C strings for the general protocol. Treat messages as UTF-8 JSON initially if that simplifies versioning; keep sizes bounded. The ABI must provide a matching release function for every returned allocation, and ownership must be documented on both sides.

Specify:

- ABI version negotiation and unknown-version behavior;
- null pointer and zero-length handling;
- maximum request and response sizes;
- UTF-8 validation and escaping;
- stable error categories and human-readable detail;
- Go panic recovery that returns an error without crossing the boundary;
- memory allocation/free ownership and lifetime;
- reentrancy and concurrent-call guarantees;
- logging policy that excludes tokens and document content;
- request deadlines and cancellation semantics;
- cooperative Go-side deadline/cancellation checks and bounded work; a Rust-side timeout does not terminate an in-flight archive call;
- shutdown semantics and whether process-global Go runtime state is retained.

Initial CLI calls can be short synchronous functions. Rust wrappers should perform any potentially blocking call using the proper blocking boundary (e.g. `spawn_blocking` when called from Tokio). For asynchronous work, define opaque job IDs and explicit start/status/cancel methods rather than holding a C call open. Never let Go call back into Rust to update status.

**Acceptance:** Rust and Go each have focused ABI tests for normal, empty, malformed, oversized, error, panic, free, repeated and concurrent calls. ABI documentation and generated header agree. A bounded-call behavior is demonstrated.

### Stage 5 — Move `list` as the first vertical slice

Move the implementation behind the Go bridge while preserving Rust Clap parsing and credential resolution initially. The current dispatch point is `Command::List` in [`cli/mod.rs`](../../../crates/librepaper/src/cli/mod.rs); the behavior is in [`cli/documents.rs`](../../../crates/librepaper/src/cli/documents.rs).

Rust continues to:

- parse `list` arguments and environment flags;
- resolve the server and stored/explicit bearer token using existing credential behavior;
- pass resolved server, token and suffix configuration to the bridge;
- choose process exit behavior and ensure secrets are not printed.

Go implements the HTTP request/response and formatting path for the list operation. Preserve the current endpoint contract, including POST `/api/list`, request pagination/cursor behavior, authentication headers and response handling. Do not simplify away pagination because current data happens to fit one page. Pass the configured suffix length and alphabet from Rust rather than duplicating configuration defaults in Go.

Parity includes:

- no server and no token diagnostics and exit codes;
- unauthorized/network/malformed response behavior;
- empty list text (`no documents yet`);
- updated date truncation and title/role formatting;
- owner/no-role omission and shared-role labels;
- aligned columns;
- short-ID generation, duplicate suffix handling, ambiguous prefixes, and slug collisions;
- Unicode titles and IDs, including the byte-vs-rune handling differences between Rust and Go;
- stable pagination, ordering and duplicate record handling;
- no credential leakage in stdout/stderr/logging.

Keep the Rust implementation as the rollback backend during rollout or behind a build-time compatibility switch, not as a production retry after an ambiguous request. Read-only shadow comparison may be used in development with redacted diagnostics, but must not double network traffic in production without explicit approval and rate-limit analysis.

**Acceptance:** captured golden cases and integration responses produce matching exit status, stdout and relevant stderr; pagination and error paths are covered; only `list` dispatch changes; other commands remain Rust-owned. The stage records actual incremental timings for Go edit, Rust CLI edit, and core edit.

### Stage 6 — CI, release, package and publication policy

Add CI jobs so Go package tests do not invoke Cargo and Rust core tests do not invoke Go. The bridge/executable job builds both toolchains and runs ABI/list parity checks. Full repository validation remains required at planned integration checkpoints because changes can cross package boundaries.

Update release automation, toolchain setup, caches, target matrices, archive packaging, symbols/debug info, licenses/notices, SBOM/dependency reporting, and failure diagnostics. Verify crates.io behavior and consumers before changing package topology.

Resolve the publication choice explicitly:

1. Consolidate workspace Rust sources into a single published package with no remaining dependencies on unpublished path-only sibling crates. Include the Go module and required source files in Cargo's package `include` allowlist. Users building/installing the packaged mixed-language binary need the documented Go and C toolchains. Test the packaged output separately from a workspace build.
2. Publish the internal Rust core and bridge dependencies alongside the user-facing product, with clear Go/C toolchain requirements and supported versions.

Option 1 preserves one published package by consolidating sources, but gives up the sibling-crate compile isolation for that published source layout unless an equivalent package boundary is retained without unpublished path dependencies. Option 2 retains separate published Rust crates. One installed command and one published package are distinct decisions. Every source-built mixed-language package needs Go and C toolchains.

The preferred outcome is one user-facing/published product unless Cargo packaging makes that impractical. A published Rust library must not unexpectedly require Go when a consumer only wants Rust APIs. A helper binary remains a fallback only if Stage 2 proves archive linking unsuitable.

**Acceptance:** CI and release workflows document Go/C toolchain versions and cache keys; clean release builds work for the chosen target matrix; a `cargo package` artifact is inspected and install/build-tested separately; downstream Rust library linking is understood; package structure is approved before further feature migration.

### Stage 7 — Expand low-coupling remote CLI slices

Migrate remote-only commands one at a time, with the same parse/resolve/bridge boundaries where useful:

1. Identifier resolution and history commands, preserving paging, timestamps, account roles, output shape and auth errors.
2. Login/logout and credential storage, only after choosing whether Rust continues to own credential paths/permissions or Go takes ownership. Do not create two token stores. Preserve restrictive file permissions and atomic writes.
3. Export after separately designing how Go receives/validates source archives and projection metadata. Current `cli/export.rs` relies on Rust projection and source archive types; do not port it as a superficial HTTP command.
4. Local backup only as a coherent backup/restore format slice with migration/version compatibility.

Keep `admin serve`, storage backup/restore, moderation and sweep Rust-owned in this stage. `cli/mod.rs` directly uses Rust configuration, storage and PostgreSQL APIs. Moving those requires a larger ownership boundary, not only command translation.

**Acceptance per command:** its errors, exit status, output, auth rules, pagination and compatibility cases are captured; only that command's dispatch changes; the previous implementation remains available until a release rollback window closes.

### Stage 8 — Companion leaf utilities, then jobs and previews

Treat the companion as a coherent service migration anchored at [`local/protocol.rs`](../../../crates/librepaper-companion/src/local/protocol.rs), the JSON/HTTP seam. First migrate leaf utilities such as tool discovery, diagnostics normalization, safe builder planning, and agent detection where their inputs/outputs are already data-shaped. Then migrate job/preview service behavior in larger slices.

The parity surface includes protocol version negotiation, loopback binding, host/DNS-rebinding protections, CORS and pairing, bearer/origin scope, project grants, workspace staging, path traversal and symlink safety, file/count/size limits, cancellation, per-project generations, queue limits, job status, expiry/reaping, persisted recovery records, previews, tool discovery/cache invalidation, process environment, output/log bounds, and native engine behavior.

Do not treat successful invocation of Typst/Pandoc/Calepin/Quarto as parity. Preserve argument validation, controlled environment, deadlines, cancellation, output caps, workspace confinement and cleanup.

Approval and folder dialogs are OS-specific and currently Rust-owned. For an interim design, Go may issue an explicit request and wait for Rust to perform the dialog, but this must be an explicit request/resume protocol at the process/service boundary. Do not implement a cyclic C callback from Go archive into Rust. If the archive runs inside the same executable, determine a safe orchestration design before including these operations in the Go call surface. Alternatively retain dialog-related routes in a small Rust-owned component until a supported boundary exists.

**Acceptance per slice:** same API and authorization behavior, filesystem safety review, cancellation/deadline evidence, recovery across restart, and platform-specific prompt behavior. Rust code/dependencies are removed only after all consumers move.

### Stage 9 — Move assistant and automation together

Assistant and automation are coupled today. Move them together or first extract a stable language-neutral service contract. Relevant Rust code is under [`assistant/`](../../../crates/librepaper-companion/src/assistant/) and [`automation/`](../../../crates/librepaper-companion/src/automation/); local service routes and state also embed the assistant registry.

Preserve:

- Agent Client Protocol (ACP) process framing, initialization, permission requests and cancellation;
- RMCP/MCP stdio transport, tool schemas and error semantics;
- session identity, restart/recovery, lifecycle and concurrency rules;
- durable journal format, idempotency keys, task limits/status transitions and truncation rules;
- model/provider transport, TLS policy and timeouts;
- document-link parsing, hidden credential handling, role limits and authorization;
- tool-call audit ordering, retry behavior, and exactly-once/at-most-once assumptions;
- assistant route integration with the companion.

Do not split `assistant/runtime` from `automation/peer` by adding callbacks into the core. Expose narrow versioned operations and keep the Rust core/document API behind a clear request boundary. Keep the Rust document/Loro layer as authority.

**Acceptance:** journal recovery and idempotency tests survive process crashes; ACP/MCP interoperability cases pass; concurrency and cancellation semantics are recorded; no credential leaks; local route ownership is explicit.

### Stage 10 — Invert the host only after the archive path is proven

Once CLI, companion and assistant behaviors are stable as Go packages and archive behavior is proven, consider making Go the primary host while retaining a Rust library for document/Loro functionality. This is an architectural decision, not an automatic consequence of migrating more leaf functions.

The Rust library must remain independent of Go and must not link the C archive/runtime. Go may call Rust through a separately designed C ABI only if the topology preserves Rust-only consumers and testing. Avoid a dependency cycle where the Rust core depends on Go bridge while Go calls core symbols. Keep the C ABI thin and data-oriented.

If a Go-hosted executable cannot meet platform/release constraints, retain Rust as host or choose the helper-process fallback. Do not combine host inversion with document-authority migration in one stage.

**Acceptance:** Rust-only library builds/consumers work without Go; Go-host binary owns startup/shutdown cleanly; all release targets pass; incremental build measurements show the intended core isolation; the selected host architecture is documented.

### Stage 11 — Move HTTP edges and selected APIs, preserve document authority

Move stateless or read-only HTTP edge behavior in bounded slices only after identifying authoritative logic. Rust remains the sole document commit authority. Do not let Rust and Go each accept mutations for the same document in production.

For every API slice, document:

- endpoint ownership and route compatibility;
- authentication, authorization, CSRF/origin and rate-limit behavior;
- request validation and error mapping;
- transaction boundary and idempotency behavior;
- pagination/order and cache behavior;
- deployment and rollback routing;
- whether the endpoint reads Rust-owned state or commits through the Rust authority.

Where Go needs a mutation, invoke a narrow Rust-owned API/transaction operation via the chosen boundary. Do not split transaction ownership across languages or attempt compensating dual writes. A Go HTTP server proxying into Rust may be evaluated, but it must not create unbounded synchronous C calls on Tokio.

**Acceptance:** API contract and authorization tests pass; mutation ownership is singular; rollback routes new requests to one backend before execution; ambiguous mutations are never automatically retried against a second backend.

### Stage 12 — Evaluate Loro access from Go

Treat Go access to Loro as an investigation, not a premise. Compare a maintained custom native Rust adapter using upstream Loro against a custom Rust-to-WASM adapter loaded with wazero, and against keeping Loro operations in Rust behind a small boundary. Do not make an official C ABI a prerequisite or imply that one exists. The official npm WASM package has JavaScript glue imports and is not directly usable as an embedded Go library. Running that package in a Node worker is a separate runtime option, not the preferred embedded path.

The browser WASM packages and their JavaScript imports are not directly usable as a Go library. Their module imports/runtime expectations, memory model and browser APIs must be inventoried. A custom Rust-to-WASM/wazero route needs proof for every imported function, serialization compatibility, runtime availability across targets, performance and crash/resource limits.

Required parity covers document creation; update application, encoding and decoding; state encoding; UTF-16 offsets including astral characters; nested text containers; cursor encoding, decoding and resolution; historical fork/checkout; diff; version vectors/frontiers; metadata checks; shallow snapshots; projection, proposal and compaction; malformed inputs; and round trips against the official browser package. Also evaluate text/path access and concurrent updates. Include realistic document sizes, import/export, memory use, startup cost, and concurrent collaboration load. Keep Rust as the only commit authority until the adapter proves identical CRDT semantics and storage compatibility.

**Acceptance:** an options report compares maintained custom native Rust/upstream Loro, custom Rust-WASM/wazero, and retained-Rust-boundary approaches with licensing/support, the required parity corpus, performance results, target support and maintenance cost. No production Loro backend changes from this stage alone.

### Stage 13 — Migrate persistence/collaboration units only after proof

Only after Stage 12 proves a compatible Loro path should individual persistence/collaboration units move. Plan one operation family at a time, with cross-language compatibility fixtures and crash/recovery tests. Never change all state encoding, update application, transactions and storage ownership in one cut.

Require compatibility for existing persisted Loro snapshots/updates and all supported client versions. Test migration from old data, process crash during writes, partial filesystem/database failures, concurrent writers, duplicate updates, retries and restore. Establish a rollback-compatible schema and backup plan before any production write path changes.

After each unit is adopted, remove its Rust implementation and dependencies only when no binaries, tests, fuzz targets, WASM builds or external Rust API consumers still depend on it. The browser renderer WASM remains in Rust regardless of server-side Loro decisions.

**Acceptance:** old/new implementations produce compatible state and updates across the full fixture corpus; restart/restore/concurrency behavior is proven; staged rollout and rollback do not require dual writes; obsolete Rust code is removed after consumers are migrated.

## Validation and rollout policy

- Focused Go package tests exercise Go logic without compiling Rust.
- Rust core tests exercise core without Go.
- ABI tests exercise both sides and allocation/error paths.
- Bridge/executable tests exercise command integration and stdout/stderr/exit behavior.
- Full repository tests, fuzz/fixture checks, and release builds remain mandatory at architecture checkpoints and before release. The goal is to make frequent inner-loop tests independent, not to skip integration validation.
- Record cold and warm build measurements separately. Compare equivalent source edits and cache states; include C archive build and final link time.
- During migration, select backend before request execution. Read-only shadow comparisons are permitted in controlled tests. Do not dual-write production state or retry an ambiguous mutation against the other backend.
- Keep schema, wire and disk formats backward-compatible until old code and rollback releases are retired.
- Release in slices behind explicit dispatch/build switches only where those switches do not create two production authorities.

## Rollback

For each moved read-only command, restore Rust dispatch and remove Go linkage from that command while retaining any compatible data/config format. For a write-capable feature, rollback routes new operations to the established owner before execution; it does not replay uncertain writes in the alternate implementation. Persisted formats remain readable by the rollback version. If a migration changes an on-disk format, migration and reverse/forward compatibility must be designed and tested before deployment.

If Stage 2 platform support fails, stop archive product migration. Reassess a helper-process design as an explicit alternative, preserving the same versioned protocol where useful. Do not keep a partially supported archive path for only developer builds while silently dropping release targets.

## Open decisions

1. Does the complete C archive target matrix work with current static-musl and Windows MSVC release requirements?
2. Can the existing single published package remain the product entry point while core and bridge are internal siblings? Which package owns crates.io metadata and downstream Rust APIs?
3. Which exact Rust APIs are public and consumed externally, especially those currently re-exported from `librepaper`?
4. Should Rust credential lookup remain authoritative after `list`, and when (if ever) should Go own credential storage?
5. What ABI encoding should be standardized: versioned JSON bytes or a more compact binary format? JSON is the initial proposal for inspectability, with strict size bounds.
6. How should a Go-hosted companion request Rust-owned OS approval/folder dialogs without callbacks or blocking the Tokio runtime?
7. What is the supported Go toolchain/compiler matrix and who maintains cross-compilation/release infrastructure?
8. Which build-time measurements are release blockers, and what minimum measured improvement justifies migration cost?
9. Is a helper process an acceptable fallback if one release target cannot link the Go archive, or is one executable a hard product constraint?
10. Which Loro boundary should be maintained: a custom native Rust adapter using upstream Loro, a custom Rust-WASM/wazero adapter, or a retained Rust boundary?
11. How long must old Rust implementations remain available for rollback, and which compatible release retires them?

## Repository workflow

Implement each approved stage in its own reviewable branch/worktree. Follow repository `AGENTS.md`: the main agent coordinates and reviews; delegated contributors own isolated worktrees; verification is centralized after review. Do not commit generated archives or machine-specific toolchain outputs. Keep generated C headers reproducible from Go source and verify the ABI from clean builds.

## References

- [Go build modes](https://pkg.go.dev/cmd/go#hdr-Build_modes) — `c-archive`, cgo and target build behavior.
- [cgo pointer passing rules](https://pkg.go.dev/cmd/cgo#hdr-Passing_pointers) — pointer lifetime and cross-runtime memory constraints.
- [Cargo build scripts: change detection](https://doc.rust-lang.org/cargo/reference/build-scripts.html#change-detection) — `rerun-if-changed` and build-script invalidation.
- [Cargo dependencies: multiple locations](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#multiple-locations) — packaging considerations for path and published dependencies.
