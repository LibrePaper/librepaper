# Snapshot and result contract inventory

*2026-09-14. Field-level inventory of the structures that describe source or
build output at a moment. The purpose is to converge vocabulary where meanings
match without collapsing distinct authority and lifecycle boundaries.*

## Contract overview

| Contract | Owner | Lifetime | Authority | Primary consumers |
| --- | --- | --- | --- | --- |
| Browser render tree | `collab.js::tree`, `capturePreviewTree` | Mutable capture used for one operation | Local observation; not durable authority | Renderers, previews, downloads, bundle builder |
| Checkpoint tree | `document/history.rs::Tree` | Immutable and durable | Server source history | Restore, history, snapshot API, source archives |
| Agent source tree | `room/agent.rs::SourceTree` | Immutable request candidate | Validation input; room command commits separately | Patch validation and agent receipts |
| Renderer result | `renderers.js::render` return value | Transient | Local computation result | Reader preview and bundle preparation |
| Local job status | `local/protocol.rs::JobStatus` | Transient, queryable until job cleanup | Companion report | Browser renderer adapter and diagnostics UI |
| Quarto collector bundle | `local/quarto.rs::QuartoBundle` | Transient wire input | Untrusted collector output until validated | Local Quarto adapter |
| Validated result bundle | `results/mod.rs::BundleManifest` | Transient validated computation evidence | Host-validated result contract | Quarto reuse and result consumers |
| Bundle manifest | `server/bundle.rs::BundleManifest` | Immutable and durable | Server activation transaction | Bundle storage and reader delivery |

## Source tree fields

| Meaning | Browser render tree | Checkpoint `Tree` | Agent `SourceTree` | Decision |
| --- | --- | --- | --- | --- |
| Main source path | `main` | `main` | `main` | Canonical term: **main path**; keep wire field `main` |
| Text bodies | `texts[path]` | Stored separately by entry digest | `files[path].text` | Different lifecycle; do not force one shape |
| Stable text identity | `files[path].id` | `files[path].id` | `files[path].file_id` and canonical tree | Standardize prose on **file ID**; retain existing wire compatibility |
| Asset identity | `digests[path]`; bytes optionally in `assets[path]` | `files[path].sha` | Canonical checkpoint only | Canonical concept: content digest; projection differs by consumer |
| File kind | `files[path].kind` | `files[path].kind` | Implicit: source files are text | No new common enum until a consumer requires cross-boundary decoding |
| Byte size | Optional `files[path].size` | `files[path].size` | Computed from text | Keep size with durable manifests and transfer admission, not every source view |
| Compile settings | Optional `settings` | Optional `settings` | Canonical checkpoint | Same meaning; browser digest deliberately mirrors checkpoint serialization |
| Tree identity | `snapshotDigest(tree)` | `Tree::digest()` | `SourceTree::digest()` delegates to canonical tree when present | Canonical term: **source tree digest** |

The browser `snapshotDigest` intentionally serializes the checkpoint-tree
shape, including stable file IDs, asset sizes, and compile settings. This is
already a real shared contract implemented through equivalent serialization
and fixtures. Replacing it with a network or wasm call would add a boundary
without changing the contract.

The agent `SourceTree` is intentionally text-oriented: it supports expected
passages and patch validation. Its `canonical` checkpoint retains asset and
stable-identity information without making binary assets part of patch input.

## Source identity names

Several existing fields contain SHA-256 values but identify different things:

| Field | What it identifies | Scope |
| --- | --- | --- |
| Checkpoint `Tree::digest()` | Canonical source manifest: main path, file metadata, settings | Complete shared project source |
| Browser `snapshotDigest` | The same canonical manifest serialization when metadata is complete | Captured render input |
| Agent `base_tree`, `revision` | Expected before/after canonical source tree digests | Patch conflict protocol |
| Local request `snapshot` / `inputRevision` | Browser job identity, normally the captured tree digest | Job correlation and stale-result rejection |
| Quarto `shared_tree_sha256` | Durable shared source inventory supplied to local execution | Verification against a companion workspace |
| Result `source.tree_sha256` | Shared source tree used for a validated result | Computation provenance |
| Bundle `source_sha256` | Canonical source tree from which display bytes were built | Published-source freshness |
| Cell `source_sha256` | One executable cell's normalized source | Cell reuse; not a project revision |
| Bundle `bundle_sha256` | HTML/object descriptor | Display package identity |
| Bundle `render_config_sha256` | Renderer configuration identity | Freshness independent of source |
| Result `computation_sha256` | Computation context | Quarto output reuse |

Use **source tree digest** in documentation and internal variable names for the
complete project identity. Use **cell source digest**, **display bundle
digest**, **render configuration digest**, and **computation digest** for the
other scopes. Do not use bare “revision” or “snapshot” in new contracts when a
digest scope is intended.

Existing public fields remain stable. Rename them only in a versioned protocol
migration with compatibility evidence.

## Artifact and asset fields

| Meaning | Renderer result | Local job | Validated result bundle | Bundle |
| --- | --- | --- | --- | --- |
| Primary bytes | `html`, `pdf`, or `artifact` | Raw endpoint selected by `outputs`/`artifact` descriptor | External blob named by `artifact.sha256` | `html` object plus asset objects |
| Kind | `artifactKind`; otherwise inferred from `html`/`pdf` | Job `kind`, output-map key | `artifact.kind` enum | HTML fixed; asset MIME types |
| Entrypoint/path | Source tree supplies it | Output-map key or request entrypoint | `artifact.entrypoint` | Asset `path`; HTML is root object |
| Digest | Usually computed by adapter/caller | `OutputEntry.sha256` | `ArtifactDescriptor.sha256` | `BundleObject.sha256` |
| Size | Byte array length | `OutputEntry.size` | `ArtifactDescriptor.size` | `BundleObject.bytes` |
| MIME type | Usually implicit | Implicit in output route/key | Optional `ArtifactDescriptor.mime` | Required on every object |

These shapes should remain distinct. A local job describes retrievable
temporary outputs, a validated result describes computation evidence, and a
bundle describes immutable public display objects. A universal artifact
structure would either lose lifecycle information or acquire optional fields
for most consumers.

Convergence should happen in adapters and terminology:

- adapters should expose `{kind, bytes, diagnostics, provenance}` to the
  reader regardless of browser or companion backend;
- use `size` for in-memory/internal byte counts and translate to the existing
  bundle wire field `bytes` at its boundary;
- use `sha256` for serialized fields and “digest” in explanatory prose;
- require MIME types only at storage/delivery boundaries.

## Diagnostics

| Contract | Location fields | Extra semantics |
| --- | --- | --- |
| Browser renderer diagnostic | `file`, `line`, `column`, optional ranges/hints | UI-oriented, normalized by `diagnosticContext` |
| Local protocol `Diagnostic` | `file`, `line`, `column`, end range, hints | Companion wire contract matching browser diagnostics |
| Result-bundle `Diagnostic` | `source_path`, `start_line` | Durable computation coverage evidence |

Browser and local protocol diagnostics intentionally share the UI shape. The
result-bundle diagnostic is smaller because it records computation coverage,
not an editor selection. Do not replace either with a union full of optional
fields. Normalize local diagnostics at the renderer adapter before the reader
aggregates them.

The reader's render-diagnostics controller owns aggregation of renderer,
bibliography, and local-tool streams. It deduplicates the UI shape only after
each source has been normalized.

## Provenance

There are two distinct provenance concepts:

1. **Build provenance** answers which backend, builder, engine, preset, and
   installed tool versions produced the artifact shown in this browser.
2. **Computation provenance** answers how Quarto computations were obtained,
   including collector version, execution policy, external inputs, and start
   and completion times.

`local/protocol.rs::JobStatus` carries build provenance. The Quarto collector
wire bundle and `results::BundleManifest` carry computation provenance, with a
deliberate validation/conversion boundary between them.

These must not be merged. The local protocol Rust type is named
`BuildProvenance` to make the distinction visible while retaining the existing
serialized `provenance` field. The result type remains `results::Provenance`
inside its computation-specific module.

Bundle manifests currently retain source and render-configuration
identity but not either detailed provenance record. Persisting build or
computation provenance would be a product/schema decision, not a vocabulary
cleanup, and is outside this simplification pass.

## Consolidation decisions

### Consolidated now

- Render, bibliography, and local-tool diagnostic ownership moved out of
  `Reader.svelte` into one render-diagnostics controller after normalization.
- The local job protocol type is renamed from ambiguous `Provenance` to
  `BuildProvenance`; JSON remains unchanged.
- Documentation now uses scoped digest names instead of treating every SHA as
  a source revision.
- Protocol v2 local builds carry a `source` descriptor with the collaborative
  source-tree digest, main path, and digest of the exact input manifest. The
  companion verifies it before staging the immutable runner workspace, and
  build provenance returns the main path and materialized-manifest digest.
  A bound Quarto project remains the user's local project, while each one-shot
  build executes from a verified temporary copy so the runner cannot mutate
  editable state.

### Keep separate

- Mutable render tree versus durable checkpoint tree
- Patch-oriented agent source tree versus asset-complete checkpoint tree
- Temporary local job output versus validated computation result
- Computation provenance versus build provenance
- Result bundle versus bundle display bundle
- UI diagnostics versus computation-coverage diagnostics

### No new contract

The inventory does not justify another common package or top-level manifest.
Existing adapters already sit at the meaningful lifecycle boundaries. Future
work should remove conversion code only when two adjacent representations have
identical authority, lifetime, and consumers.
