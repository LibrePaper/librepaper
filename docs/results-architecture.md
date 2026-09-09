# Shared computation results

LibrePaper currently supports Quarto as its only computation engine. This
separation prepares the code for another engine, such as Calepin, without
detecting Calepin documents, executing Calepin, or loading its runtime.

## Three independent identities

A document has a source representation, a computation engine, and an artifact
format. These answer different questions:

| Property | Purpose | Current examples |
| --- | --- | --- |
| Draft format | Choose the source preview renderer | Markdown, Typst, LaTeX, HTML |
| Execution engine | Interpret computational inputs and produce results | None, Quarto |
| Artifact format | Present one completed render | HTML, PDF, DOCX |

The legacy `source_format: quarto` remains supported. Its draft format is
Markdown and its execution engine is Quarto. Plain Typst remains Typst with no
computation engine. Import text alone does not enable an additional engine.

Schema migration 18 backfills document results metadata. Insert and source
format update triggers keep the supported pairs synchronized, including
restores. Current clients may supply the engine and draft format explicitly,
but they must agree with the source format; this release does not enable
arbitrary engine overrides. Legacy documents with no source format retain
their HTML preview.

An execution engine is not a local execution grant. Pairing, the bound project,
the document and origin, and the job's supported options still constrain every
local invocation. A future engine must obtain its own explicit authorization;
it cannot inherit a Quarto grant because the result transport is shared.

## Shared responsibilities

The results layer owns immutable bundle records, content-addressed resources,
publication retries, selection generations, retained history, and output
discussion identities. The browser results modules also own artifact loading,
resource cleanup, and the completed-render outbox.

These operations do not parse computational source or decide whether an engine
cache is valid. They consume an engine's normalized result bundle. A figure's
binary digest, a cell's identity, and a computation fingerprint remain distinct.

An engine adapter owns source inventory and fingerprinting, supported render
policies, invocation planning, capture, and the project's sharing exclusions.
The existing Quarto parser, capture filter, and freezer checks stay specific to
Quarto. Shared job management retains cancellation, limits, progress, durable
recovery, and idempotency.

## Compatibility boundaries

Internal names can change without renaming persisted data. In particular, this
refactor preserves:

- The `librepaper-quarto-bundle/v1` schema and the existing Quarto HTTP routes.
- Blob and manifest object keys, selection tables, and selection generations.
- Existing source revisions, computation fingerprints, and context IDs.
- The browser outbox database, record keys, and pending publication payloads.
- Output comment anchors and their immutable render/content references.
- Existing local job requests, binding files, and restart recovery records.

Changing a document's current format must not make its historical result
bundles or output discussions unreadable. Publication validates the current
engine; access to an existing result follows document read permissions.

An old bundle without an engine discriminator means Quarto, not an arbitrary
default engine. New unsupported engine values must fail explicitly. Default
metadata must not change the serialized identity of a legacy manifest: old
object digests and retries against an existing render ID still matter.

Compatibility entry points remain available for existing Rust and browser
callers while application code moves to the shared names. They delegate to one
implementation rather than maintaining two result stores or two outboxes.

## Resource roles

Display resources belong to a completed artifact or its captured outputs.
Draft dependencies are generated files an engine may eventually need to add to
a source preview's virtual filesystem. Calepin's Typst runtime and generated
binding files are examples of the latter, but are not supported in this release.

Reserving a distinct role does not authorize injecting arbitrary files into the
editor's source tree. A future draft adapter must validate its runtime version,
logical paths, dependency closure, and collisions with authored files. It must
also establish which generated resources are safe to share. Execution caches,
private configuration, and local datasets do not become public draft resources
merely because they live beside a results file.

## Adding Calepin later

Start with a small import fixture: a real Calepin document, its saved results,
the required runtime and bindings, and generated figures. Compile that complete
virtual tree with LibrePaper's actual WASM Typst renderer. Cover explicit chunk
calls, fenced code, included files, and changed source before deciding whether
the original document can be rendered without an additional wrapper.

Only then define the Calepin adapter and enable its engine value. Keep uncertain
chunk association and freshness explicit. A shared Calepin model/scanner crate
may be useful, but extracting it and adding local execution are separate work.
This refactor does not change the Calepin repository.

Enabling Typst plus Calepin also requires making the stored pair independently
authoritative: replace the current source-format derivation and synchronization
triggers, and carry the pair through document reads, updates, and restores.
The present backfill is a compatibility foundation, not engine selection UI.
