# SPEC: Quarto source, cached outputs, and local rendering

Status: design and implementation contract. This document specifies intended
behavior, explores alternatives, and identifies implementation gates. See the
[implementation and compatibility notes](docs/quarto-implementation.md) for
implemented behavior and remaining presentation work. Written 2026-09-09.

This specification supersedes the architectural recommendations in
[the earlier Quarto draft](docs/specs/quarto.md), particularly its prohibition
on local execution and its assumption that an individual cell's source hash
proves the validity of its output. The earlier draft remains useful background
for the Markdown dialect work. Where the documents disagree, this proposal is
the intended direction for future Quarto work.

## 1. Product objective

An author should be able to publish a `.qmd`, collaboratively edit its source
in LibrePaper, and see a useful preview immediately. A collaborator should be
able to read the document without installing Quarto, R, Python, or the
author's packages, using the Markdown preview. An author with LibrePaper and
Quarto installed locally should be able to switch to Quarto preview and see
the document rendered in its real project environment, on their own machine,
as they edit.

The central workflow is:

1. Publish the Quarto source and its intended shared resources.
2. Edit prose collaboratively in the browser. Readers without a paired local
   Quarto installation see the Markdown preview.
3. Switch to Quarto preview to run the document with Quarto on the reader's
   own computer through the local app, re-rendering on every edit.
4. Continue writing even when the local app disconnects; the pane falls back
   to the Markdown preview.

The `.qmd` remains the source of truth. Nothing rendered is ever uploaded to
LibrePaper: the server holds source files only. Editing does not create a
second independently editable version of the paper.

### 1.1 Two preview modes, one pane

A Quarto document has two preview modes, chosen under Tools. Exactly one is
active at a time (a check mark marks it), the choice persists per document,
and the default is Quarto preview.

| Mode | What it shows | Runs code | Needs |
| --- | --- | --- | --- |
| Markdown preview | The source rendered as Markdown in the browser: front matter dropped, `:::` divs and code chunks shown verbatim | Never | Nothing |
| Quarto preview | The document run with Quarto on the reader's own computer, through the local app | Yes, locally | A paired local app with Quarto installed |

Both modes paint into the same frame, so comments and highlights work the same
way regardless of which is active.

Quarto preview mechanics: the reader's browser keeps the local app's hosted
workspace current by calling `PUT /librepaper/local/v1/workspace` on every
edit. The app runs `quarto preview --no-serve` against that workspace with
embedded resources, and Quarto's own watcher re-renders on source changes.
LibrePaper never talks to Quarto directly; it polls
`GET /librepaper/local/v1/previews/{id}/page` (ETag) and paints whatever
complete page comes back into the pane. The endpoint only ever serves a
complete rendered page: the previous complete page stays visible until the
next one finishes, so figures never flash blank. The response header
`x-librepaper-rendering` reports whether a re-render is under way, and the
pane shows "Rendering…" while one is, alongside the still-visible last page.

If the browser is not yet paired with a local app, choosing Quarto preview
opens the app's consent page for a single Allow click (see §9.4). If no local
app is available at all, the pane shows the Markdown preview and the banner
shows a "Connect" button.

Nothing rendered by Quarto preview is ever uploaded to LibrePaper: the server
stores only the `.qmd` source and its declared shared resources. There is no
results bundle, no saved-results panel, no render settings, no frozen/refresh
options, and no separate rendered-output view — the live pane described above
is the only rendered view a reader ever sees.

The remainder of this document (§§4-19) is written from an earlier design
that included a published, browser-visible results bundle with per-cell
freshness tracking. That reader-facing bundle no longer exists: readers who
cannot run Quarto see only the Markdown preview (§1.1), and Quarto preview is
always a live local render, never a previously published artifact. Passages
below that describe a bundle, cache, or capture mechanism as CLI- or
server-side storage (for example an eventual `librepaper quarto import`) may
still apply; passages that describe a reader seeing saved/cached results,
freshness badges, or a separate rendered-output view are superseded by §1.1
and §10.

### 1.2 Scope

The first useful release supports one main `.qmd`, common academic Markdown,
static figures, tables, text results, an optional linked local project, and
full HTML rendering. Includes and configuration may make that document a
multi-file project without making LibrePaper a website or book editor.

Later increments cover richer source mapping, PDF, interactive widgets, and
managed live preview. Full notebook-style cell execution, cloud kernels,
package installation, dependency inference for arbitrary programs, and complete
browser emulation of Quarto are outside the initial scope.

## 2. Existing architecture and required changes

The following observations describe the repository before implementation:

| Existing component | Evidence | Proposed use or change |
| --- | --- | --- |
| Source format detection and render dispatch | [renderers.js](web/src/lib/renderers.js) | Add `quarto`, routed to the Markdown family for draft rendering |
| Source format versus output kind | [renderers.js](web/src/lib/renderers.js) | Preserve the distinction; Quarto draft HTML and full HTML/PDF cannot be represented by one fixed format mapping |
| Optional bibliography engine | [bibliography-engine.js](web/src/lib/bibliography-engine.js) | Reuse existing loading and citation support rather than assuming citations must enlarge the base Markdown module |
| Local pairing and structured jobs | [local protocol](crates/librepaper/src/local/protocol.rs) | Add an explicitly negotiated Quarto capability and job contract |
| Browser local-service client | [local.js](web/src/lib/latex/local.js) | Extract reusable connection logic without importing TeX fallback policy into Quarto |
| Revision-aware rendered PDF handling | [rendering-store.js](web/src/lib/reader/rendering-store.js) | Generalize artifact handling; the current implementation is PDF-specific |
| Local file synchronization | [sync.rs](crates/librepaper/src/cli/sync.rs) | Extend to `.qmd` and eventually project resources, preserving peer/merge semantics |
| Room and storage guarantees | [invariants.md](docs/specs/invariants.md) | Preserve durable publication, authorization fencing, accounting, and retained-history guarantees |

At the design baseline there was no `.qmd` branch in `formatOf`. The linked
implementation notes distinguish that baseline from the implemented feature.

The existing local TeX path provides useful transport infrastructure, but its
execution policy is not sufficient for Quarto. Scientific code intentionally
reads data, uses packages, may access services, and can write arbitrary files.
Quarto execution therefore needs its own locally granted project capability.

## 3. Non-negotiable behavior

1. Opening, editing, synchronizing, importing, or reading a `.qmd` never by
   itself authorizes execution on another person's machine.
2. The hosted LibrePaper service stores and distributes source and artifacts;
   it does not execute Quarto, document code, filters, or project scripts.
3. The draft renderer does not execute computational cells, inline expressions,
   or project-provided JavaScript.
4. Every published output bundle identifies the input revision supplied to its
   render and the limitations of that association.
5. An output can remain useful and visible after it becomes potentially stale.
   Visibility and freshness are separate decisions.
6. Identical cell code does not imply identical output. Cell identity, source
   equality, execution context, and binary content identity are distinct.
7. A late or failed job cannot overwrite the current source or silently replace
   a newer selected output bundle.
8. A source edit never waits for code execution to finish.
9. Generated outputs do not become collaborative source text or trigger a
   synchronization loop.
10. A published bundle is either complete for its declared coverage or absent.
    Readers never receive a manifest that refers to unfinished uploads.
11. Unsupported or ambiguous output mapping fails visibly rather than attaching
    a plausible-looking result to the wrong cell.
12. A successful render proves that an invocation completed, not that the
    analysis is correct or that all external inputs were freshly recomputed.

## 4. Source and draft rendering

### 4.1 Format and round trips

Register `.qmd` as `quarto` throughout publication, editor language selection,
main-file selection, downloads, history, sync, and format validation. Route its
draft render through the Markdown infrastructure. Preserve ordinary `.md`
behavior: executable-looking fences in a Markdown file do not implicitly turn
it into a Quarto project.

Preserve source bytes except for deliberate user edits and the synchronization
system's established text handling. Parsing must not reserialize front matter,
rewrite cell options, inject persistent labels, or replace shortcodes on save.
The title may come from front matter or the first heading without modifying it.

Treat incomplete YAML, unfinished fences, and half-written attributes as normal
editing states. Produce a partial preview with localized diagnostics, and avoid
throwing away the whole document because one construct is incomplete.

### 4.2 Dialect support

| Construct | Initial draft behavior | Fidelity limit |
| --- | --- | --- |
| Paragraphs, headings, lists, links, images, tables | Existing Markdown behavior | Document supported Pandoc differences |
| YAML title, authors, abstract, date | Render a title block | Dynamic metadata may differ from Quarto |
| Math | Reuse compatible math rendering | Numbering and macros require explicit support |
| Citations and bibliography | Reuse bibliography infrastructure | Unsupported styles receive a diagnostic |
| Fenced divs and bracketed spans | Parse attributes and nesting | Arbitrary Quarto filter behavior is unavailable |
| Callouts | Render standard callout families | Match semantics before exact styling |
| Figure/table labels and references | Stable targets and draft numbering | Final Quarto numbering may differ |
| Columns, tabsets, margin content | Readable structural approximation | Responsive presentation may differ |
| Computational fences | Syntax-highlighted code plus cached results | No execution |
| Inline computation | Cached value when mapped; source otherwise | Unknown or stale values are identified |
| Includes | Resolve authorized project text with cycle/depth limits | Missing local-only includes cannot be expanded |
| Conditional content | Evaluate only supported conditions for draft HTML | Unknown profiles/conditions remain diagnostic |
| Shortcodes | Small explicit allowlist; preserve others visibly | No arbitrary extension execution |
| Mermaid, Graphviz, Observable | Static code or stored output initially | Browser execution is a separate feature decision |

Use parser-aware transformations or syntax tree support. A sequence of regex
replacements across the full source is insufficient: nested examples, quoted
code, long fences, escaped delimiters, and inline code must not be reinterpreted
as live cells. Maintain source spans through any preprocessing so comments and
diagnostics still refer to the user's original text.

The earlier draft's concrete comrak extension choices are hypotheses to verify
against the pinned Markdown renderer, not an instruction to enable extensions
without compatibility tests. The source model should be reusable by native and
browser builds of that renderer.

### 4.3 Cell presentation

Parse the cell language, label, option block, source span, and code separately.
Resolve the supported project, document, and cell option hierarchy. Unknown
options remain preserved and make freshness classification conservative.

Quarto supports retained execution intermediates through `keep-md` and
`keep-ipynb`. Its raw `output: asis` mode can remove normal output wrappers,
which is one reason generic HTML or Markdown scraping cannot guarantee cell
boundaries. These are adapter inputs and edge cases, not LibrePaper storage
formats. [Quarto execution options](https://quarto.org/docs/computations/execution-options.html)

The proposed draft presentation contract is:

- `echo: false` hides displayed code, not its source or its identity.
- `include: false` hides the cell's displayed code and outputs. It is not an
  instruction to skip execution during a real render.
- `output: false` suppresses displayed results.
- `eval: false` receives no missing-result placeholder. Previously stored output
  remains historical and is not shown as its current result.
- `code-fold` affects presentation only where the draft supports it.
- `fig-cap`, `tbl-cap`, alt text, and dimensions are represented separately from
  the output bytes when the adapter can establish that separation.
- Multiple outputs retain their order and individual types.

Hidden output is not private output. Suppressed code, results, or intermediate
objects must not be uploaded merely because a parser found them. The collector
publishes display artifacts selected by policy; a diagnostic must not reveal
the contents of a hidden cell's result.

## 5. Cached outputs: what is stored and why

Per §1.1, none of this reaches a reader today: there is no results bundle
painted into any pane, and Markdown preview never mixes in saved output.
Sections 5-8 describe a per-cell capture/bundle/freshness model that no longer
has a reader-facing surface; they are kept only as a design record for a
possible future CLI-side import/bundle mechanism (§1.1, design note 3), not as
a description of what a reader sees.

### 5.1 Three distinct caches

| Cache | Owner and location | Key or association | Purpose |
| --- | --- | --- | --- |
| Execution cache | Quarto/engine on author's machine | Engine-defined | Avoid repeated computation |
| Published result bundles | LibrePaper storage | Render ID, input revision, output manifest | Share and retain results |
| Downloaded assets | Browser | Content digest plus authorized document access | Avoid transferring unchanged bytes |

LibrePaper does not upload R environments, Python pickles, Jupyter kernel
memory, or package caches to make the draft preview work. It needs the resulting
display content, not a portable copy of the computation process.

Quarto offers engine caching and project freezing. `_freeze` preserves
computational results for reuse; ordinary single-document rendering bypasses
the usual project freeze behavior. Changes in data or other external inputs can
require manual refresh. These mechanisms remain under Quarto's control.
[Quarto execution management](https://quarto.org/docs/projects/code-execution.html)

### 5.2 Published result bundle

A bundle is an immutable record containing:

- The exact shared input tree digest and durable revision reference, if known.
- A unique render ID independent of that tree digest.
- The entrypoint, selected output format, profiles, and effective parameter
  fingerprint, with private values excluded.
- The local runner's declared tool versions and execution policy.
- A full rendered artifact when available.
- A mapping from source cell identities to normalized display outputs.
- Asset descriptors, content hashes, relative paths, MIME types, and byte sizes.
- Coverage information distinguishing captured, intentionally hidden,
  unsupported, ambiguous, and unavailable outputs.
- Provenance for imported versus managed results and known versus unknown inputs.
- Optional generated-content/source mappings.

Two renders of the same source can produce different bundles because the data,
randomness, time, environment, or external services changed. Never use the source
digest as the sole storage identity of a result.

### 5.3 Initial output types

Support raster images, static SVG through the existing safe image path, plain
text streams, and a constrained representation of tables or sanitized static
HTML. Each output has a type and its own content digest. A table requiring
unavailable styles must either carry supported styles or fall back to readable
table content with a fidelity notice.

SVG is active-capable content and must not be inserted unchecked into the
application DOM. HTML fragments need sanitization and URL handling. Large
tables should have explicit preview limits with a link to the full artifact;
silent truncation could change the meaning of an analysis.

Interactive widgets are a later output class. They need dependency bundles,
isolated execution, network policy, sizing, accessibility, and version handling.
Initially show a static fallback if one was produced, otherwise a link to the
full output and an unsupported-inline-output notice. Do not claim that every
widget can be automatically converted to a static image.

### 5.4 Asset naming and deduplication

Store bytes by SHA-256 and maintain a per-bundle mapping from logical paths to
those blobs. Repeated use of the same figure reuses bytes but retains separate
cell associations. A generated path can change across renders without forcing
an upload if its content is unchanged.

Content addressing does not grant access. Every asset resolution must remain
scoped to authorized document or artifact access. Do not expose an unrestricted
global lookup that allows guessing a private figure by its hash.

Raw data and local directories are not automatically shared. Only the declared
render outputs and required publishable dependencies enter the bundle. Resolve
relative resource references within the collected output root; reject traversal,
unexpected absolute paths, and symlink escapes. External URLs are explicit
dependencies, not evidence of a self-contained bundle.

## 6. Cell identity and output association

### 6.1 Identity is not a code hash

Consider two occurrences of `plot(x)`, separated by a reassignment of `x`.
Their code text is identical and their results differ. Consider one unchanged
`plot(x)` after a changed data-loading cell. Its identity is unchanged and its
old result is potentially stale. Content hashes alone solve neither problem.

The identity model has four distinct values:

| Value | Meaning |
| --- | --- |
| Cell ID | Which source cell this is |
| Cell source fingerprint | Exact code, options, language, and relevant syntax at capture |
| Context fingerprint | Ordered computation-related inputs for the document |
| Output content hash | Which bytes were produced |

Use explicit unique cell labels scoped by source file wherever possible. In a
live collaborative document, unlabelled cells may receive IDs in sidecar CRDT
metadata anchored to their source ranges. Do not insert synthetic labels into
the user's `.qmd` without an explicit editing action.

Offline imports and filesystem edits need reconciliation. Preserve a mapping
only when label, structural context, and source matching establish a unique
association. If two candidates are equally plausible, leave them unmapped.
Line numbers and ordinal positions are useful evidence but not durable identity.

Repeated includes need an occurrence identity in addition to the included file
and cell label. Renames, label changes, and moves between files require a
documented migration rule or conservative loss of mapping.

### 6.2 Matching and invalidation are separate

Moving a uniquely labelled cell can preserve its association with an old plot.
Moving executable code also changes execution order and therefore marks the
computation context potentially stale. Both statements must be represented.

Duplicate labels produce a diagnostic and disable ambiguous associations.
Duplicating a cell does not duplicate the claim that the new cell was executed.
Deleting a cell removes its output from the current draft, while retained
historical bundles continue to contain it.

Do not trim code or ignore comments to establish execution equality. Whitespace
inside strings, indentation, language directives, and option expressions can
matter. Hash canonical structured records with explicit schema versions;
preserve code bytes and include execution-affecting syntax.

### 6.3 Inline values

Inline computation needs expression identity, source location, and result text.
It shares the document's computation freshness state. A stale computed number
inside a sentence is less visually obvious than a stale plot: provide a subtle
indicator and an accessible explanation without inserting status words into
the author's prose or exported source.

If capture cannot map an inline value reliably, show the original expression
in the draft. Do not substitute values by searching for matching numbers in
the rendered document.

## 7. Freshness and invalidation

Superseded by §1.1: there is no reader-visible bundle of saved/cached results
to classify, so none of the "suggested wording" or per-cell staleness display
below is shown to a reader. Quarto preview is always a live local render (with
its own "Rendering…" state, §10.2); Markdown preview never shows a computed
result at all. This section is retained only as a record of the freshness
model an eventual CLI-side import/bundle mechanism could reuse internally.

### 7.1 Two axes

Track association and freshness separately.

Association states: `mapped`, `ambiguous`, `unmapped`, `hidden`, `deleted`.

Freshness states:

| State | Meaning | Suggested wording |
| --- | --- | --- |
| `matches-recorded-inputs` | Known tracked inputs match the captured invocation | “Results from this version” |
| `source-compatible` | Recognized presentation/prose edits only | “Showing saved results” |
| `potentially-stale` | A known computation-related input changed | “Results may be outdated” |
| `unknown` | Provenance or comparison is insufficient | “Imported results; freshness unknown” |
| `missing` | No display result was captured where one is expected | “No saved result” |

“Results from this version” must have details explaining what was recorded.
It is never a claim that external data, dependencies, or services are unchanged.

The full artifact has its own exact source-revision status. A prose edit can
make the complete HTML older than the source while the embedded draft figures
remain source-compatible. One boolean `current` cannot represent both.

### 7.2 Conservative context fingerprint

For the initial implementation, fingerprint the ordered cells and inline
expressions, their exact code and options, the entrypoint, relevant shared
includes, selected format/profile, parameters, and tracked configuration and
dependencies. Any unknown configuration change is computation-relevant.

Exclude only recognized ordinary prose and a narrow allowlist of presentation
metadata. If custom filters or engines may read that prose or metadata, downgrade
the result to unknown/potentially stale rather than claiming equivalence.
Code can inspect its own `.qmd`; therefore even a prose-only compatibility
classification is a user-facing heuristic, not a proof of reproducibility.

The initial policy invalidates the document's computational result set together
when computation-related source changes. This avoids building an incomplete
dependency graph that quietly leaves downstream results marked current.

### 7.3 Change matrix

| Event | Keep mapped output visible? | Freshness action |
| --- | --- | --- |
| Edit ordinary paragraph | Yes | Source-compatible, subject to unknown extensions |
| Edit known presentation-only caption | Yes | Recompose caption; saved plot remains visible |
| Change cell code or execution options | Yes | Mark computational set potentially stale |
| Add, delete, or reorder an executable cell | Yes for remaining mapped cells | Mark computational set potentially stale |
| Move cell without changing its label | Yes if association is unique | Order change still invalidates context |
| Change a cell label | Only if identity reconciliation is unique | Recompute references and association |
| Change tracked data/config/include | Yes | Mark computational set potentially stale |
| Change untracked local data | Yes | Local runner can report it; browser alone cannot detect it |
| Change profile, engine, or parameters | Only matching-context output by default | Other-context results stay in history |
| Parse error during editing | Keep unambiguous prior mappings | Unknown until classification recovers |
| Delete cell | No in current draft | Preserve in history |
| Set `eval: false` | No old result as current cell output | Preserve historical result only |
| Refresh completes successfully | Yes | Publish a new bundle with actual policy/provenance |

Changing caption text and changing figure dimensions are different operations.
The latter may change the rendered bytes or analysis code's behavior and is
computation-relevant by default. A caption that contains executable inline code
is also computation-relevant.

### 7.4 Stale outputs remain useful

Default to retaining mapped stale results with one document-level status and
available per-output details. Avoid covering every plot with an intrusive
warning. Users may hide stale results, but changing code must not cause a page
full of useful plots to vanish immediately.

Do not silently build a result set from whichever individual cells happen to
match across unrelated runs. Use one selected bundle per execution context.
Historical per-cell browsing can be added later with explicit provenance.

## 8. Capturing results from Quarto

### 8.1 Preferred capture contract

Introduce a local, versioned collector that turns a Quarto invocation into the
LibrePaper bundle schema. The collector receives the source inventory before
execution and captures outputs after a successful render. The collector must
identify the tested Quarto and execution-engine versions it supports.

Candidate mechanisms require an implementation spike:

| Mechanism | Strength | Limitation |
| --- | --- | --- |
| Retained executed Markdown | Close to draft renderer input | Missing/altered boundaries, raw output, engine differences |
| Retained notebook | Structured code/output records for supported engines | Not universal; mapping to original source still required |
| Quarto filter or extension emitting a manifest | Can capture semantic nodes and dependencies | Filter order, engine coverage, and source mapping need verification |
| Final HTML extraction | Complete visible output exists | Hidden code, generated headings, widgets, and duplicated IDs complicate association |
| Existing `_freeze` data | Useful for already-rendered projects | Internal schema/version coupling and uncertain source provenance |

Preferred direction: a maintained adapter using structured intermediates and,
where necessary, a small companion Quarto extension. Do not promise that one
Lua filter alone can recover every source cell and inline expression.

The capture spike must demonstrate labelled and unlabelled cells, hidden code,
multiple plots, no-output cells, generated tables, raw output, includes, and
inline computation in both R and Python before cell caching is called complete.

### 8.2 Reject heuristic slicing as the primary protocol

The earlier draft proposes taking output after a code fence until the next
source paragraph. That boundary is not reliable. A cell can print a paragraph
identical to source prose, suppress its code, emit arbitrary headings, or share
supporting resources with other outputs.

Heuristic import may be offered for best-effort recovery, but it must declare
its uncertainty. Ambiguous content remains accessible in the full artifact;
it is not injected into the draft under a guessed cell.

### 8.3 Importing existing results

Support an import operation that reads existing render artifacts without
executing anything. Prefer a valid LibrePaper manifest when present. Otherwise
try only recognized adapters for retained intermediates or freeze data.

A file modification time is not proof that the output belongs to the current
source. A digest of the source at import time must not be misrepresented as the
source digest at execution time. If execution provenance is missing, store
`source_tree: null`, record an import-time comparison separately, and mark
freshness unknown.

If only complete HTML is available, publish it as a full artifact with unknown
or declared provenance. If only images are available without trustworthy cell
associations, publish them as named assets or leave them unassociated. Do not
invent the missing associations.

### 8.4 Completion and partial success

Quarto success and extraction success are separate outcomes. A valid complete
HTML render can be published with cell coverage marked incomplete. Failure to
map a widget need not discard every static figure.

Conversely, nonzero process exit must not cause the collector to gather an old
HTML file left from yesterday and call it a successful result. Use a job-specific
output location or a verified before/after output inventory. Partial outputs
from failed jobs remain local diagnostic material initially.

If the project permits execution errors and Quarto completes, mark the bundle
as completed with diagnostics. A zero exit code does not erase captured errors.

## 9. Local execution and project binding

### 9.1 Capabilities

Extend local capability discovery to report Quarto presence/version, supported
collector versions, available target formats where known, and runtime checks.
Finding Quarto is not proof that the document's kernel or R packages are ready.
The doctor view distinguishes missing Quarto, missing runtime, project readiness
unknown, collector unsupported, and connected/ready states.

The browser discovers a reachable paired service; it cannot establish that an
application is absent from the computer. A failed probe produces connection
instructions and a retry action, not an incorrect “Quarto is not installed.”

### 9.2 Bind to a local project

Every document has a binding without anyone granting one: the hosted
workspace. The local app keeps a directory of its own per deployment origin and
document, under its cache, and writes the files the browser uploads with each
job into it before rendering, removing what an earlier job wrote that the
document no longer holds and leaving Quarto's own caches alone. The browser
names this binding with the fixed id `hosted`. It exposes no path of the
author's machine and reaches nothing the document does not share, which is
why it needs no grant. A standalone `librepaper local start` and the local app
embedded in `librepaper serve` both serve it; a service constructed without a
workspace base refuses the id. While a live preview runs against this
workspace, the browser keeps it current by issuing the same `PUT workspace`
call on every edit, rather than a one-shot render request.

An explicit local binding maps a remote document ID and origin to a locally
selected project root and entrypoint, for a project that keeps data or an
environment the document does not share. The browser receives an opaque binding
ID, not a filesystem capability to choose arbitrary absolute paths, and enters
it in the render settings, where it replaces the hosted workspace for that
document.

Keep machine-specific paths, environment configuration, credentials, and
execution grants local. Project settings shared with collaborators contain
portable choices such as the entrypoint and profile, not `/home/...` paths.

The initial execution mode uses a linked project to preserve relative paths,
data access, environment selection, extensions, and existing caches. The runner
inherits the configured execution environment of that binding; it must not
silently choose a different Python installation or install missing packages.

### 9.3 Working tree versus isolated snapshot

| Mode | Advantages | Costs |
| --- | --- | --- |
| Linked working tree | Matches existing research workflow and caches | Source/data can change during rendering; code can mutate the project |
| Isolated project snapshot | Stronger source identity and concurrent editing story | Large/private data, symlinks, absolute paths, environments, and caches are difficult to reproduce |

Start with linked working-tree mode and honest provenance. Before launching:

1. Reconcile shared files using the existing sync peer semantics.
2. Establish a durable shared revision and verify local shared-file hashes.
3. Record the agreed tracked input inventory and selected local context.
4. Serialize renders for that binding.
5. Pause LibrePaper's own writes of shared source to disk during the invocation,
   while continuing to accept collaborative updates in memory.
6. Record an end inventory and reconcile queued source changes afterward.

External editors and code can still mutate files. A detected tracked-input
change during execution makes provenance unstable and prevents claiming an
exact source match. Start/end equality cannot prove no intermediate change
occurred; expose the provenance level as “working-tree verified,” not “isolated.”
Offer strict snapshot execution later for projects that can supply a complete
input closure. Neither mode proves that remote services were unchanged.

The synchronization pause is bounded by the render lifetime. Cancellation,
failure, or local-service restart releases it and reconciles pending changes
without replacing newer text with the old render snapshot.

### 9.4 Execution authority

Pairing is the execution grant for the hosted workspace: a person at the
machine allows a named site and document, either by clicking Allow on the
consent page the local app serves on its own loopback origin (the reader opens
it in a popup, and the pairing returns by `postMessage` to the allowed origin
only) or by entering the code the app printed. The consent form is accepted
only when posted from the app's own origin, so no site can pair itself. An
explicit project binding is granted separately, by the local user, for a
particular root. In both cases the Render action executes the captured shared
revision, including collaborators' changes visible at launch. An editor
elsewhere cannot cause execution on that machine merely by editing the document
or sending an ordinary server message.

Execution can involve code cells, filters, project scripts, and access allowed
by the local environment. Structured arguments prevent command injection at
the bridge but do not make document code harmless. Preserve the existing
origin/project-scoped pairing model, and define filesystem/network confinement
as a separately reported property of this capability.

Automatic rendering is off initially. A later grant may allow rendering after
source changes for this binding, with an explicit owner, visible running state,
bounded debounce, and an immediate stop control. Permission to edit a document
and permission to consume another user's local compute remain separate.

## 10. Render and preview operations

### 10.1 Operations

The only choice exposed to anyone is which preview mode is active (§1.1),
under Tools. There is no Share results, Refresh computations, Use frozen
results, or Import existing output action, and no render settings to
configure. Quarto preview always runs the document with the project's own
defaults; the app does not accept a typed render policy from the browser, and
there is no job to queue, cancel, or supersede — only "is the live render
current" and "is one under way."

### 10.2 Live pane mechanics

Quarto provides a preview service via `quarto preview --no-serve`. While a
browser has Quarto preview selected and is paired with a local app that has
Quarto installed, the app runs that command against its hosted workspace,
kept current by the reader's `PUT /librepaper/local/v1/workspace` call on
every edit; Quarto's own watcher re-renders on source changes.

The app serves the resulting self-contained rendered page at
`GET /librepaper/local/v1/previews/{id}/page` (ETag/If-None-Match). The
reader polls that endpoint about once a second and paints the response into
LibrePaper's own frame — never Quarto's page framed or linked directly — so
comments and highlights work on the live page.

Complete-page rule: the endpoint only ever serves a complete rendered page.
The `x-librepaper-rendering` response header reports whether a re-render is
under way; while it is, the pane shows "Rendering…" and keeps showing the
previous complete page until the next complete page replaces it. Figures
never flash blank mid-render.

This applies only in a pane that has Quarto preview selected and is paired
with a local app that has Quarto installed; an unpaired browser, or one with
Markdown preview selected, always shows the Markdown preview. No loopback URL
is ever framed or shown to a reader.
[Quarto preview CLI](https://quarto.org/docs/cli/preview.html)

Use one owner for watching/rebuild scheduling: Quarto's own preview watcher
owns re-rendering while the pane is polling its page. Nothing else rebuilds
the same workspace at the same time; running both without coordination can
execute the same changes twice and create a rebuild loop. Stop or expire
unused preview processes and invalidate their page endpoints once a document
is closed or Markdown preview is selected instead.

### 10.3 Engine-neutral preview sessions

The local app's managed preview is not Quarto-specific: one hosted workspace
per document, one preview session per workspace, and an engine adapter
underneath that knows how to run that workspace and report progress. Quarto
is one adapter; Calepin, for Typst documents with code chunks, is another.
`POST /librepaper/local/v1/previews` takes an optional `"engine": "quarto" |
"calepin"` (default `quarto`); Calepin's typed options are `"calepin": {
"binding_id": "hosted", "main": "doc.typ", "format": "html"|"pdf" }`. For
Calepin the app runs `calepin watch doc.typ doc.pdf --format pdf` (or
`--format html -- --no-serve`) against the hosted workspace; Calepin
preprocesses the chunks and delegates the actual compilation to `typst watch`
itself, and its `.calepin` artifacts survive workspace syncs the same way a
Quarto `_freeze` cache would.

Every adapter produces one of two artifact kinds, `html` or `pdf`, and the
same complete-output rule from §10.2 applies regardless of engine: `GET
previews/{id}/page` serves only a finished render (`content-type` `text/html`
or `application/pdf`, `x-librepaper-kind: html|pdf`, ETag, and
`x-librepaper-rendering` reporting whether a re-render is under way), never a
partial one, and the pane keeps the previous complete page on screen until
the next one replaces it. Preview status reports the session's `engine` and
`kind`; `GET capabilities` reports a `calepin` entry alongside `quarto`, with
whether the command was found and its version. As with Quarto, no loopback
URL from either adapter is ever framed or shown to a reader.

## 11. Full HTML, PDF, and output isolation

Full HTML is the first target because it supports the existing flow-document
reader and provides the author's actual Quarto presentation. Preserve its
styles and required assets within the document isolation boundary, rather than
inserting its page-level CSS into LibrePaper's application shell.

Quarto's `embed-resources` option can package resources into an HTML file.
This is useful for simple exports, but should not be the only storage strategy.
[Quarto HTML documentation](https://quarto.org/docs/output-formats/html-basics.html)

Prefer separate content-addressed assets internally for deduplication, bounded
transfers, and history. A collector may support self-contained HTML as an import
and download format. It must inventory unresolved external dependencies rather
than assume every runtime fetch was embedded.

Full output must use an isolated origin/frame consistent with LibrePaper's
existing document model. No rendered script receives application cookies,
pairing tokens, arbitrary authenticated fetch ability, or direct editor access.
Any annotation/navigation message bridge validates origin, message type, and
document/render identity. A widget script is still document code even when it
arrives in an artifact rather than a `.qmd` cell.

Preserve exact full-artifact figure numbers and captions. The draft owns its own
numbering over current source. Store captured caption metadata separately where
possible to avoid rendering “Figure 2: Figure 2: ...” or attaching an obsolete
caption after a prose edit. Arbitrary preformatted outputs whose captions cannot
be separated remain captured fragments with a fidelity limitation.

PDF is a later target using the existing PDF reader. The source format remains
`quarto`; artifact descriptors carry `html` or `pdf`. Format is part of execution
context, so PDF results cannot be silently reused as HTML computation results.
DOCX can initially be a downloadable artifact without an editable preview.

Books, websites, presentations, and dashboards need separate navigation and
resource contracts. A local render may successfully produce them before
LibrePaper can present them completely. Declare target support explicitly.

## 12. Proposed storage and protocol shape

The following JSON is illustrative, not an implemented API. Angle-bracketed
values stand for real identifiers or SHA-256 values. Field names must be finalized
with the existing rendering and storage protocols before implementation.

```json
{
  "schema": "librepaper-quarto-bundle/v1",
  "render_id": "<unique-render-id>",
  "document_id": "<document-id>",
  "source": {
    "revision": "<durable-revision-reference>",
    "tree_sha256": "<input-tree-digest>",
    "main": "paper.qmd",
    "verification": "working-tree-verified"
  },
  "context": {
    "id": "<context-id>",
    "fingerprint_version": 1,
    "computation_sha256": "<ordered-input-digest>",
    "format": "html",
    "profiles": [],
    "parameters_sha256": "<public-parameter-digest>"
  },
  "provenance": {
    "kind": "managed-local-render",
    "quarto_version": "<detected-version>",
    "collector_version": "<collector-version>",
    "policy": "project-defaults",
    "computation": "cache-use-unknown",
    "external_inputs": "not-fully-observed",
    "started_at": "<UTC-timestamp>",
    "completed_at": "<UTC-timestamp>"
  },
  "artifact": {
    "kind": "html",
    "entrypoint": "paper.html",
    "sha256": "<html-digest>"
  },
  "cells": [
    {
      "id": "paper.qmd#fig-trend",
      "source_path": "paper.qmd",
      "label": "fig-trend",
      "source_sha256": "<cell-digest>",
      "coverage": "captured",
      "outputs": [
        {
          "ordinal": 0,
          "kind": "image",
          "asset": "paper_files/figure-html/fig-trend-1.png"
        }
      ]
    }
  ],
  "assets": [
    {
      "path": "paper_files/figure-html/fig-trend-1.png",
      "sha256": "<image-digest>",
      "mime": "image/png",
      "size": 12345
    }
  ],
  "coverage": { "full_artifact": true, "cell_outputs": "partial" }
}
```

The asset list inventories dependencies; the artifact entrypoint blob is also
an upload/publication dependency even though it has its own descriptor. Real
schemas need bounded arrays, output dimensions where useful, diagnostic and
inline-result records, and optional provenance for locally tracked inputs.
Do not send secrets or low-entropy secret hashes as public parameter metadata.
Use opaque private-context IDs when a parameter cannot be disclosed safely.

### 12.1 Local job request

A Quarto request should carry a negotiated protocol/capability version, typed
job kind, origin/project scope, binding ID, shared revision, input manifest,
relative entrypoint, format, selected public profile/parameters, render policy,
deadline, and idempotency key. It must not carry executable paths, arbitrary
environment variables, or shell fragments chosen by the browser.

Local environment choices come from the binding. Validate entrypoints against
the authorized root, including symlink resolution. File uploads cannot overwrite
arbitrary local paths. Unknown job kinds and unsupported policies fail explicitly
without falling back to a more permissive execution mode.

### 12.2 Atomic publication

1. Validate schema, ownership, scope, source reference, declared sizes, and quota.
2. Reserve storage using existing accounting semantics.
3. Upload missing immutable blobs, verifying byte counts and content hashes.
4. Validate the complete manifest and its referential closure.
5. Recheck write authority and document existence after asynchronous work.
6. Commit manifest, retention references, and artifact-selection event durably.
7. Notify readers only after commit.

Retries are idempotent. If source has advanced, publish as an older-revision
artifact or reject current selection; never change its recorded input identity.
If the uploader loses permission, the final mutation is fenced even if all blobs
already arrived. Staged orphan blobs enter bounded cleanup with correct charges.

No compile, upload stream, or external process runs while a room lock is held.
Preserve the documented room lock order and journal authority instead of adding
a parallel uncoordinated artifact database.

### 12.3 Retention, quotas, and recovery

Retain the selected bundles, named checkpoints' bundles, bundles referenced by
retained comments/history, and in-flight publication dependencies. Apply a
bounded policy to other successful renders. Physical deduplication must not
make quota accounting or cross-document access ambiguous.

Limits cover total artifact bytes, individual blob size, file count, manifest
size, logs, and output count. Existing source/PDF limits cannot be assumed
sufficient for a Quarto page with many plots. Report which limit failed and
keep source edits and the last successful bundle available.

Backup/restore includes manifests, blobs, selection events, and provenance.
Restoring source history must restore the corresponding output selection when
retained; if missing, say so. Garbage collection follows the full reference
graph and the existing conservative deletion guarantees.

## 13. Synchronization, CLI, and filesystem behavior

The intended CLI workflow extends existing verbs. The syntax below is proposed;
it must not be advertised as implemented until the corresponding code ships.

```sh
librepaper publish paper.qmd
librepaper sync <document> paper.qmd
librepaper local start
librepaper local doctor
```

Publishing a `.qmd` uploads source without executing it. If recognized outputs
are present, collection may attach them under the provenance rules. Missing
outputs are informational, not publication failure.

The exact CLI for binding a local project, requesting a render, and importing
an external render remains open. Prefer a small typed extension of existing
commands over requiring a Makefile or an arbitrary command hook. A manual
`quarto render` followed by an explicit import remains a supported escape hatch.

Classify project files into shared source, shared authored assets, generated
display outputs, local computation inputs, local caches/environments, and
private configuration. This is a policy manifest, not an assumption that every
file under the project root should be uploaded.

Generated output directories and collector sidecars must be excluded from
ordinary collaborative source watching. Prefer a collector completion event to
“upload everything whenever a filesystem event arrives.” External render import
must wait for a coherent inventory and detect files still changing.

The source watcher and result collector operate independently. A figure update
does not rewrite `.qmd`, and a remote result download does not trigger another
render. A lost network connection can leave a complete local bundle pending
upload; reconnection retries publication without rerunning code.

Ordinary local edits remain three-way merges through the sync peer. If a project
script modifies shared source during render, reconcile it as a real edit and
mark provenance unstable; do not overwrite those changes with an old snapshot.

## 14. Comments, history, and navigation

Prose comments stay anchored to source text through the existing collaborative
model. Renderer-added titles, reference numbers, and generated bibliography
text must not be mistaken for editable source spans. In Quarto preview,
comments and highlights land on the current live-rendered page (§10.2); there
is no separate render ID or bundle for a comment to outlive, since the pane
always shows the latest complete render, not a retained one.

The remainder of this section's per-cell output-comment model (render ID,
cell ID, output digest, retained comments on stale results) belongs to the
superseded reader-facing bundle (§5) and applies only if a future CLI-side
import/bundle mechanism restores that concept.

Output comments carry render ID, cell ID where available, output ordinal, and
output content digest. A region comment on a plot also stores the artifact's
coordinate system and dimensions. Updating a plot does not automatically move
the comment to the same pixel coordinates on a different chart.

When the source cell still exists but the output changed, show the old comment
as referring to a previous result, with a way to inspect that result. A user may
explicitly carry a discussion forward, but the system must not silently assert
that a statistical conclusion on an old table applies to a new table.

Switching between draft and full output preserves navigation through source
maps where available. Without a reliable map, use headings or cell targets as
best-effort navigation and say when a precise anchor is unavailable. Do not
promise pixel-exact synchronization before the mapping adapter exists.

Generated table cells and inline values are not directly editable in the draft.
An edit action navigates to their source code or associated caption. Figure
replacement is a source/asset operation, not an invisible mutation of a saved
computation result.

## 15. User interface and diagnostics

The Markdown preview / Quarto preview choice lives under Tools, with a check
mark on the active mode (§1.1); there is no other reader-facing render
control, no logs panel, no cancel button, and no render policy settings. The
pane always paints whichever mode is active into one frame, and switching
modes never itself causes execution — Quarto preview only runs while it is
the active mode and the browser stays paired.

| Situation | Expected presentation |
| --- | --- |
| Quarto preview selected, not yet paired | Consent page opens for one Allow click |
| Quarto preview selected, no local app available | Markdown preview shown; banner shows a "Connect" button |
| Quarto preview selected, app paired, first render pending | "Rendering…" until the first complete page arrives |
| Quarto preview selected, re-render after an edit | Previous complete page stays visible with "Rendering…" until the next complete page replaces it |
| Quarto exists but the project fails to run | Runtime-specific diagnostic from the local app; Markdown preview remains available as a fallback |
| Markdown preview selected | Source rendered as Markdown, verbatim divs/chunks, no execution, no connection needed |

Diagnostics for a failed local render are local-app concerns: stage, severity,
and a concise actionable message shown near the "Connect"/mode control. Full
logs stay on the author's machine; nothing about a local run is uploaded or
shown to other readers.

Keyboard users can switch previews, render, cancel, and inspect status. Staleness
must not be communicated through color alone. Cached figures retain alt text;
figures without useful alternatives are reported as an accessibility limitation.

## 16. Performance and resource targets

Draft rendering should remain independent of execution latency. Reuse parsed
source structure and cached blob URLs when feasible, perform expensive parsing
in the existing worker infrastructure, and avoid fetching unchanged output
assets on every keystroke.

Provisional acceptance budgets, to calibrate against existing Markdown baselines:

- A representative 10,000-word paper with 30 already-downloaded static outputs
  updates prose within 300 ms at the 95th percentile on the agreed reference
  machine after warmup.
- Prose edits cause no local execution and no figure reuploads.
- Opening a saved document without Quarto requires no engine/cache download.
- Identical figure bytes across consecutive renders require no second blob upload.
- Logs, queued jobs, collectors, and disconnected preview processes remain bounded.

Measure first-open loading separately from warm editing. Establish actual fixture
sizes and hardware before making these product promises. Do not optimize the
dependency classifier by weakening freshness rules.

## 17. Validation and acceptance scenarios

### 17.1 Parser and draft fixtures

Use real `.qmd` fixtures containing nested fences/divs, code examples that look
executable, YAML errors, braces in strings, labels, unlabelled duplicates,
includes, inline expressions, citations, cross-references, captions, and hidden
cells. Verify source round trips and source spans, not just pretty HTML.

Test each visibility option independently from execution relevance. Verify
that `eval: false` does not display a stale historical plot and that a hidden
cell can still invalidate downstream results when its source changes.

### 17.2 Collector compatibility corpus

Run managed render fixtures against the declared supported Quarto versions and
both Knitr and Jupyter workflows. Cover multiple outputs, interleaved text and
plots, empty results, duplicate labels, `output: asis`, formatted tables,
warnings/errors, Unicode paths, spaces in paths, includes, and supported static
SVG. Keep widget cases as explicit unsupported/fallback fixtures initially.

Verify that normal cache use, refresh, and freezer reuse produce accurate
provenance. A successful render with a stale engine cache must not become a
claim of fully refreshed computation. A failed render with an old HTML file
present must not publish that file as new.

### 17.3 Freshness regressions

Per the §5 note, freshness classification has no reader-facing surface today;
these scenarios apply only if a CLI-side import/bundle mechanism is built.

| Scenario | Required result |
| --- | --- |
| Two identical `plot(x)` cells after different assignments | Distinct associations and correct images |
| Change only a paragraph | Figures remain visible; full HTML marked older |
| Change upstream data-loading cell | All computation results marked potentially stale |
| Reorder labelled cells | Associations survive where unique; context invalidated |
| Duplicate an unlabelled cell | No guessed shared result |
| Change dimensions or parameters | Context invalidated |
| Modify tracked input data without changing source | Local change detected and result status updated |
| Modify untracked external data | No false browser claim that it was checked |
| Reuse frozen computations in new HTML | Rendering time and computational provenance remain distinct |
| Import old HTML beside newer source | Freshness unknown; no fabricated execution revision |

### 17.4 Concurrency and durability

Exercise revision changes during execution, out-of-order job completion, two
authors rendering the same revision, network loss after upload, retry after
commit response loss, quota failure mid-upload, revocation before manifest commit,
document deletion during render, and local-service restart.

Verify bundle atomicity, authorization fencing, idempotency, reference retention,
backup/restore, and garbage collection of staged versus retained blobs. Confirm
that no source update is lost while local sync writes are paused.

### 17.5 Isolation and resource tests

Reject path traversal, escaping symlinks, forged MIME types, oversized manifests,
decompression expansion if archives are accepted, and unauthorized blob lookup.
Verify HTML/SVG cannot access application or local pairing credentials. Exercise
bounded logs, process-group cancellation, job deadlines, and orphan preview
cleanup. Test loopback pairing in the browsers actually supported by LibrePaper.

### 17.6 End-to-end release gate

An R author publishes a paper with a generated plot and table. A collaborator
with no local tools sees the Markdown preview: verbatim divs and code chunks,
no figures, no execution. The author pairs a local app, switches to Quarto
preview, and edits prose and analysis code while the pane keeps repainting the
latest complete rendered page, never flashing blank between renders. The
author disconnects; their pane falls back to the Markdown preview. Comments
placed on the live-rendered pane remain inspectable throughout. Repeat with
Python and after reconnecting from a network interruption.

This workflow is the gate for claiming Quarto preview support, rather than a
demo that renders only a static `.qmd`.

## 18. Implementation sequence

This phasing predates §1.1's decision to drop the reader-facing results
bundle in favor of two preview modes: Markdown preview needs only Phase 1;
Quarto preview needs the live-pane mechanics of §10, not the bundle work in
Phases 2-3. Phases 2-3 remain relevant only if a CLI-side import/bundle
mechanism (§1.1, design note 3) is later built.

### Phase 0: settle capture and contracts

Build the collector spike and fixtures first. Establish what each engine/version
can map reliably, decide on a companion extension, finalize cell identity and
bundle schema, and inventory existing HTML isolation and storage assumptions.
Resolve the local execution grant and project binding before exposing Render.

Exit criterion: an actual R and Python result bundle can be produced and mapped
without heuristic paragraph slicing, with explicit coverage gaps.

### Phase 1: source plus static draft

Add `.qmd` recognition, editor support, common dialect rendering, source spans,
diagnostics, and source publication/sync. Include clear draft labeling. This is
a useful standalone increment but is not the complete cached-output feature.

### Phase 2: portable saved results

Implement static output bundles, cell association, conservative freshness,
atomic upload, content-addressed assets, import, retention, and full HTML storage.
Support the central browser-only writing workflow with already-generated output.

### Phase 3: integrated local rendering

Extend capability negotiation, project binding, source reconciliation, typed
jobs, cancellation, collection, upload retry, and render selection. Add normal
render and refresh policies; add frozen-result rebuilding only after adapter
fixtures verify its semantics for supported versions.

Phases 2 and 3 may share implementation work, but import must remain execution-free
and reading bundles must not require a running local app.

### Phase 4: richer presentation

Add managed live preview, PDF, stronger source-to-artifact navigation, selected
widgets with static fallbacks, and supported project types. Add isolated project
snapshots where practical. Consider per-cell execution only as a separate design
with an explicit kernel lifetime and dependency model.

## 19. Decisions, alternatives, and remaining questions

### 19.1 Recommended decisions

| Topic | Recommendation | Reason |
| --- | --- | --- |
| Editable representation | Original `.qmd` | Preserves the author's workflow and round trips |
| Immediate preview | Existing Markdown infrastructure with a defined Quarto subset | Low latency and no local installation |
| Cached results | Include static figures/tables/text early | Essential to writing around analysis results |
| Cache freshness | Conservative document context plus explicit provenance | Avoid false per-cell validity claims |
| Local computation | Delegate to installed Quarto in a bound project | Retains real engines and environments |
| Initial execution trigger | Explicit Render | Predictable behavior during collaboration |
| Output storage | Immutable bundle plus deduplicated assets | Atomicity, history, and efficient transfer |
| Initial artifact | HTML | Useful full preview and existing flow-reader fit |
| Kernel/cache ownership | Quarto and its engines | Avoid a second execution system |
| Unknown association | Leave unmapped | Wrong scientific results are worse than an explicit gap |

### 19.2 Alternatives not selected initially

Browser-hosted R/Python would introduce runtime size, package compatibility,
resource limits, and a new execution environment. It does not solve reproducing
the author's local project.

Uploading only rendered HTML preserves appearance but loses the native source
editing workflow. It remains a useful import route, not the primary model.

Executing every changed cell independently assumes a kernel/dependency model
that this proposal deliberately does not provide. A cached display fragment is
not an executable notebook cell.

Requiring exact source equality before showing any old output would make every
prose edit erase figures. Retaining stale mapped outputs gives a much better
writing experience while keeping status explicit.

Treating freeze internals as the permanent wire format would couple browser and
server behavior to engine-specific files. A versioned local adapter contains
that dependency and allows the shared bundle schema to remain stable.

### 19.3 Open decisions to resolve before implementation

1. Which tested Quarto versions and execution engines form the first support
   matrix? How is a collector compatibility failure reported after an upgrade?
2. Can one capture adapter produce reliable cell and inline mappings for both
   engines, or are engine-specific collectors necessary?
3. What sidecar identity model survives whole-file sync rewrites and repeated
   includes without modifying source? Which ambiguities intentionally lose mapping?
4. Which existing artifact APIs can generalize to HTML bundles without breaking
   PDF rendering, checkpoint identity, or authorization semantics?
5. What is the exact shared/local file policy and binding CLI? How are renamed
   entrypoints and changed project roots reconciled?
6. What execution confinement modes are practical across supported platforms,
   and how does a local user grant required data/network access?
7. Which presentation-only edits are safely classified for the initial subset?
   How do unsupported filters weaken that classification?
8. What policy chooses among independent authors' successful renders, and what
   user interface exposes a deliberate selection override?
9. How much historical output is retained outside checkpoints and comments,
   and how is deduplicated storage charged fairly?
10. Which generated-content comments can reuse existing anchors, and which
    require new artifact-region records?
11. Can local preview be embedded reliably under the current frame and browser
    connection policies, or should it initially open in a separate local tab?
12. What static widget fallbacks are supplied by real projects, and which
    dependencies can safely be supported in isolated full output?

These questions should be answered with the Phase 0 corpus and concrete protocol
examples. They do not change the intended product behavior: edit the real source,
retain useful saved results, execute through the author's local Quarto when
requested, and accurately identify what every displayed result represents.
