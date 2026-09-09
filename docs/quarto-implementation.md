# Quarto implementation and compatibility

This accompanies [SPEC-quarto.md](../SPEC-quarto.md). The specification includes
both the first release and later presentation work. This document records the
implementation boundaries rather than treating all Quarto syntax as supported.

The [shared results architecture](results-architecture.md) describes the
engine boundary and compatibility requirements for future computation engines.

## Source and publication

`.qmd` is a source format throughout the editor, publication, downloads, native
diagnostics, and synchronization. Publishing never executes it. A single-file
CLI publication preserves its basename so the local project's entrypoint can
be bound without renaming it. Publish a directory with `--main` to share a
multi-file paper. Pass a directory to `sync` to synchronize the shared project inventory; a file
argument retains the original single-file workflow. `--dry-run` prints the
project inventory without connecting. A scoped local baseline supports reconnect
and three-way reconciliation. Generated outputs, caches, and private files stay
outside synchronization.

Directory publication defaults to editorial source, code, and authored assets.
Generated HTML/Markdown siblings, execution caches, environments, and raw data
are excluded. `.librepaper-share.json` can explicitly include existing relative
paths. Publication does not discover arbitrary dependencies or upload private
data just because document code reads it.

## Saved results and execution

The portable format is `librepaper-quarto-bundle/v1`. A bundle has one immutable
render ID, its shared source revision, a computation context, coverage records,
cell outputs, and the full artifact's dependency inventory. Binary identity is
SHA-256; it is separate from cell identity and source identity. Reads require
document access even when the caller knows a digest.

The browser uses one selected bundle for each format/profile/parameter context.
The Render options controls choose an output format, one optional profile, and
typed scalar JSON parameters. Document default follows the source's format;
an explicit choice overrides it for local rendering. Choices are remembered
for this document in this browser, without editing the source or running code.
Applying them loads that context's saved bundle, and an unavailable context
does not borrow outputs from another context. Parameter names and values are
validated before a job is submitted. Frozen reuse additionally requires verified
local evidence for the selected computation context.
Reveal.js retains its own computation context while using an HTML artifact;
its default preview follows the static HTML isolation rules. The saved artifact
browser can explicitly enable its captured scripts in a separate sandbox.

Unique labelled cells can retain their prior results after a code edit. An
unlabelled result requires an unambiguous source fingerprint in that file;
inserting or duplicating an unlabelled cell never transfers a plot by ordinal.
Known input changes invalidate the computational result set together. Imported
HTML cannot establish a computational source revision and remains unknown.

Local execution requires both ordinary loopback pairing and a machine-local
project grant made with `librepaper local bind-quarto`. The request contains a
binding ID, an entrypoint, typed options, and a shared-input inventory. The
runner checks those files against the bound project, then invokes the installed
Quarto in that project's environment. It does not overwrite the local project
with browser uploads. Synchronize differences before rendering.

Project-default rendering preserves engine cache behavior. Refresh requests
ask Quarto to refresh computations. Neither successful exit nor a requested
refresh proves that all external inputs or engine caches were revalidated.
Execution caches, frozen engine objects, and package environments remain local.
The frozen policy checks a complete matching local freezer before asking Quarto
to reuse it. The adapter accepts a flat entrypoint in a standard project.
Profiles, typed parameters, and recursive includes require the additional
context record written by a successful managed render and an exact match of
all tracked dependencies. Changed source, missing/corrupt cached resources,
hooks, filters, undeclared includes, environment-selected profiles, and
unsupported project metadata are refused before invocation. It uses
`--use-freezer`; combining that flag with `--no-execute` on the tested Quarto
version drops cached display nodes. This restricted operation reuses prior
computations and does not establish current external-data freshness. Capability
negotiation determines whether it can be requested. See Quarto's
[execution and freeze documentation](https://quarto.org/docs/projects/code-execution.html).

Completed browser renders are saved to an IndexedDB outbox before upload. A
reload can resume publication of those same bytes without executing code again.
The outbox holds one pending render per document, at most eight documents, for
24 hours; browser storage failures are visible and the tab retains the result.
Local job admission and completed outputs also survive service restart. A job
interrupted by restart becomes a failed job with its original idempotency key,
so retrying a lost request cannot silently rerun its code.

The collector is a bundled Pandoc Lua filter plus a native normalizer. It reads
post-execution cell/float structures and records static images, text, and table
content. It preserves multiple outputs in order. A label collision, uncertain
cell boundary, unsupported structure, or missing image produces a coverage gap
instead of a guessed association. The actual complete HTML remains available.
R/Knitr and Python/Jupyter integration fixtures are exercised with Quarto 1.10.18;
other installations negotiate available tools and policies and can fail with
explicit compatibility diagnostics.

## Draft fidelity and output isolation

Draft uses the existing Markdown renderer with a bounded Quarto subset: common
metadata, code visibility, standard callouts/divs, references, authorized text
includes, static figures, tables, and literal text output. Incomplete source
remains editable. Inline computation that cannot be mapped remains source text.
Arbitrary filters, extensions, executable diagrams, and widgets do not run in
Draft. Full YAML/project semantics are delegated to Quarto during a real render.

Selected bundles and one associated bundle per retained revision/context are
kept with bounded additional render history. Source restore reselects a retained
matching bundle or explicitly clears the selection. Clearing still advances its
generation, so a job started before the restore cannot overwrite that choice.
Backups preserve bundle objects, the authoritative selection rows, and those
generation records.

Cached HTML table fragments pass through an inert parser and an element,
attribute, and URL allowlist. Text output stays literal. SVG is used as an image,
not inserted as active application markup. Full HTML uses the existing isolated
document frame with a nested opaque sandbox, with scripts and active embeds
removed for static preview. The
original HTML remains downloadable. API downloads are attachments with
restrictive CSP and `nosniff`. Relative artifact dependencies, including nested
CSS imports and fonts, are collected, verified by size and digest, and resolved
against their containing file. A failed resource load preserves the previous
successfully loaded output.

Source comments retain their existing anchors. Generated output has no editable
source span. The Saved results dialog supports comments on individual captured
results and rectangular image regions. Each discussion records its render ID,
cell ID, output ordinal, content digest, and region dimensions where applicable.
Inspect original result retrieves that immutable result, even after a new render
replaces the selected plot. Retained comments pin their referenced bundles.
Generated-result comments cannot become source-edit suggestions. Direct selection
in a full artifact remains disabled where reliable mapping is unavailable.

PDF full artifacts use the existing PDF reader with the saved render's identity;
the editable source remains Quarto. Both PDF and DOCX also retain download links.
PDF text selection does not fabricate a source anchor into the `.qmd`.

## Managed preview and presentation

Start live preview launches an author-only Quarto watcher against the linked
project after checking its shared inputs. The local URL opens separately from
the saved document. Stop it before rendering and publishing immutable results:
the service serializes admission so a watcher and a render cannot own the same
binding concurrently. Preview sessions expire and are not restored after a
service restart. Closing the reader requests shutdown; service expiry also
bounds abandoned sessions.

Browse saved pages / widgets displays captured HTML pages in a separate iframe.
Its default is static. Enable document scripts opts into captured script
execution with an opaque origin, no editor message bridge, blocked fetches,
no child frames, and no forms or popups. It receives no pairing token or app
API context. Inline scripts and bundled script dependencies can run; scripts
requiring a remote service, workers, eval, or uncaptured resources cannot.
The ordinary static preview and original artifact download remain available.
Page selection uses the verified bundle inventory rather than navigating the
application to document-controlled URLs.

Website and book rendering is an explicit project scope for HTML. Captured
pages and their dependencies remain one immutable bundle. This is distinct
from deploying a website: no public live server or executable backend is
published. The saved-results dialog can locate an unambiguous current source
cell by label or exact source fingerprint; it refuses ambiguous or deleted
cells. Discussions still identify immutable captured outputs. Whole-artifact
PDF/HTML selection does not fabricate editable source anchors.

The execution workspace control can copy shared source and explicitly named
local data files into an isolated temporary project. Working-tree execution
remains the default. Snapshot mode does not copy undeclared private files or
package environments; the local runtime must still provide required packages.

Performance targets in the specification are acceptance budgets to benchmark on
an agreed reference machine, not measured product guarantees. Draft updates do
not wait for local execution.

## Verification

The normal Rust and web suites include Quarto format, parser, visibility,
association, MIME/path validation, authorization, publication, and retry checks.
Executable CLI coverage checks that single-file publication preserves its local
name without executing code. Directory-sync CLI tests cover dry-run inventory,
remote additions, edits and deletions, and preservation of private/generated
files. Unit tests cover three-way conflicts and replay after a restart.
Explicit runtime tests exercise R and Python,
different plots from identical code, local-only data, cancellation, stale input
refusal, engine-cache reuse and refresh, HTML dependency import, and publication
against a real durable source checkpoint. They also cover inline values,
profile/include-aware frozen reuse, isolated data inputs, website/book page
closures, and the managed preview lifecycle:

```sh
cargo test quarto
cargo test quarto -- --ignored
node web/tools/quarto-e2e.mjs /path/to/built/librepaper
```

The ignored tests require local Quarto and the corresponding runtime packages.
The browser acceptance check starts an isolated temporary deployment and reads
uploaded results without pairing with a local execution service.

Draft parsing/composition can be benchmarked with `node web/tools/quarto-benchmark.mjs`.
A 200-cell, 17,469-byte fixture on the development machine took approximately
2.3 ms median and 4.3 ms p95 over 20 warmed samples. These measurements exclude
browser layout, asset loading, hashing, and Quarto execution.
