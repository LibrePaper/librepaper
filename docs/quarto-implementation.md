# Quarto implementation and compatibility

This accompanies [SPEC-quarto.md](../SPEC-quarto.md). The specification includes
both the first release and later presentation work. This document records the
implementation boundaries rather than treating all Quarto syntax as supported.

The [shared results architecture](results-architecture.md) describes the
engine boundary and compatibility requirements for future computation engines.

## Engine-neutral preview sessions

Per [SPEC-quarto.md §10.3](../SPEC-quarto.md), the local app's managed
preview session is engine-neutral: `POST /librepaper/local/v1/previews`
takes an `engine` field, `"quarto"` (default) or `"calepin"`, selecting the
adapter that runs the hosted workspace. The Quarto adapter is what the rest
of this document describes. The Calepin adapter serves Typst documents with
code chunks: it runs `calepin watch` against the hosted workspace, which
itself preprocesses the chunks and delegates compilation to `typst watch`,
and reports artifact kind `pdf` or `html` the same way the Quarto adapter
does. Preview status and `GET capabilities` both surface which engine is in
play, so reader-facing code never needs to special-case Calepin beyond
offering it as a Tools choice for Typst documents.

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

Per [SPEC-quarto.md §1.1](../SPEC-quarto.md), a Quarto document has exactly two
reader-facing preview modes, chosen under Tools with a check mark on the
active one: Markdown preview (verbatim, execution-free) and Quarto preview
(a live local render). There is no reader-facing results bundle, saved-results
panel, render settings, or frozen/refresh choice, and nothing rendered is
uploaded — the server holds source files only. The bundle format, cell
association, and freshness classification described in the rest of this
section are CLI-/server-side concerns only (a possible future
`librepaper quarto import`), not part of what a reader sees.

The portable format is `librepaper-quarto-bundle/v1`. A bundle has one immutable
render ID, its shared source revision, a computation context, coverage records,
cell outputs, and the full artifact's dependency inventory. Binary identity is
SHA-256; it is separate from cell identity and source identity. Reads require
document access even when the caller knows a digest.

Unique labelled cells can retain their prior results after a code edit. An
unlabelled result requires an unambiguous source fingerprint in that file;
inserting or duplicating an unlabelled cell never transfers a plot by ordinal.
Known input changes invalidate the computational result set together. Imported
HTML cannot establish a computational source revision and remains unknown.

Quarto preview requires loopback pairing, made either on the consent page the
local app serves (`GET /librepaper/local/v1/pair`, opened by the reader in a
popup, which posts the pairing back to the allowed origin) or with the printed
code. The default binding id `hosted` names the workspace the local app keeps
per origin and document: it writes the browser's uploads into that workspace,
removes what an earlier sync wrote that the inventory no longer lists, and
renders there. A machine-local project grant made with
`librepaper local bind-quarto` replaces the workspace with a linked project;
the app then checks the inventory against that project and never overwrites
it with browser uploads, so differences must be synchronized before rendering.
`librepaper serve` also runs the local app in-process for browsers on its own
machine (`--no-local` disables it). While Quarto preview is the selected mode
and this browser is paired with a local app that has Quarto installed, the
browser keeps the hosted workspace current with
`PUT /librepaper/local/v1/workspace` (multipart `manifest` plus `file` parts)
on every edit; the app runs `quarto preview --no-serve` there, re-rendering on
its own, and serves the resulting self-contained rendered page at
`GET /librepaper/local/v1/previews/{id}/page` (ETag/If-None-Match), with an
`x-librepaper-rendering` response header reporting whether a re-render is under
way. The browser polls that endpoint about once a second and paints the
response into LibrePaper's own pane; the previous complete page stays on
screen (with "Rendering…" shown) until the next complete page replaces it, so
figures never flash blank. The pane falls back to the Markdown preview,
with a "Connect" button in the banner, whenever the browser is unpaired or
Quarto is unavailable there.

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

The paragraph above describes CLI-/server-side bundle retention only (see the
note at the top of "Saved results and execution"); a reader's pane never shows
a selected bundle, and there is no reader-facing bundle-selection dialog to
reconcile after a source restore.

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
source span. Comments and highlights on the live-rendered page (Quarto
preview) work the same as on the Markdown preview, through the existing
document frame — there is no separate saved-results comment dialog, render ID,
or output digest for a reader to reference; the pane always shows the current
live render, not a retained one.

PDF is not exposed to readers; only HTML Quarto preview is implemented.

## Managed preview and presentation

Quarto preview is not a separate action from viewing the document: selecting
it under Tools (§ "Saved results and execution" above) is what starts the live
render for as long as it stays selected and the browser is paired. There is no
separate local URL, no "Browse saved pages" dialog, and no distinct
publish/live-preview lifecycle to serialize — the live pane and the source
edits share the same paired session.

Website and book rendering is an explicit project scope for HTML. This is
distinct from deploying a website: no public live server or executable backend
is published, and nothing rendered leaves the reader's machine.

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
