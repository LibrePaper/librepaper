# HTML diff display and tracked changes

Status: proposed

Dependencies: immutable tree reads, stable logical tree digests, and capture of
a complete current tree. Chunked storage and physical-quota accounting are not
prerequisites; this work may ship before `SPEC-01-history-retention.md`.

## Purpose

Use one rendered HTML pipeline to display historical differences and proposed
source changes for Markdown, Quarto, Typst, LaTeX, and authored HTML. The
historical source is exact; its preview is regenerated on demand with the
currently supported HTML rendering pipeline, honoring the effective release
selection defined below.

This specification covers rendering, semantic projection, diff calculation,
change attribution, inline redlines, navigation, and suggestion display. Source
history storage and retention are specified in
`SPEC-01-history-retention.md`. The history-panel behavior preserved below
supplies the comparison endpoints and controls used here.

## Goals

- Present changes in the formatted document for every supported source format.
- Use one projection, diff, and redline contract after format-specific HTML
  rendering.
- Keep source revisions and source quotations authoritative.
- Avoid differences caused solely by generated HTML markup and layout wrappers.
- Handle prose, headings, lists, equations, citations, figures, code, and tables
  predictably.
- Render only the comparison endpoints required for the current view.
- Preserve source comparison when rendered comparison is unavailable.
- Keep the diff engine replaceable behind a small deterministic interface.

## Non-goals

- Reproduce historical rendered bytes, pixels, fonts, or compiler binaries in
  comparisons. Only the latest published PDF is retained for ordinary viewing.
- Edit the generated HTML or adopt a rendered-document editor model.
- Store pairwise rendered diffs in checkpoint history.
- Make HTML reproduce PDF pagination or typography.
- Infer author intent or guarantee move detection in the first implementation.
- Accept or reject arbitrary historical hunks as though they were suggestions.

## Terms

- **Source checkpoint:** An immutable project tree retained by history.
- **Endpoint:** The baseline or target of a comparison.
- **Contemporary preview:** Historical source rendered by the currently supported
  HTML pipeline with the effective renderer/release selected from that tree's
  settings. It is not a reproduction of a stored historical PDF.
- **Projection:** A deterministic semantic token stream plus mappings between
  tokens and rendered DOM locations.
- **Hunk:** One insertion, deletion, or replacement produced by comparing two
  projections.
- **Redline:** The inline visual representation of a hunk.
- **Suggestion:** A durable annotation proposing a source replacement. It is
  inert until accepted by an authorized editor.

## Existing behavior

LibrePaper already has most of the presentation path:

- the history controller selects immutable comparison endpoints;
- a shared bounded Myers implementation returns UTF-16 text edits;
- history code adds context and coalesces nearby edits into readable changes;
- ranges of up to 12 checkpoints are rendered and diffed step by step for
  attribution, refining the initial redlines asynchronously;
- the reader frame maps flat offsets across DOM text nodes;
- insertions are wrapped in marks and deletions are inserted at target offsets;
- file-level source comparisons use the existing CodeMirror merge interface;
- suggestions store source quotations and replacements and reuse the rendered
  annotation interface.

The principal inconsistency is how visible historical text is obtained. Flow
formats are rendered to HTML, while Typst and LaTeX history may use saved PDF
text extraction. Raw `textContent` also lacks explicit semantic boundaries and
special handling for structured content.

Typst HTML export and LaTeXML rendering of arbitrary project trees already
exist in the browser. The historical passage path does not request those HTML
modes. Its existing cache holds up to 64 visible-text results; the HTML,
versioned semantic projection, and cache lifecycle below are new work.

Checkpoint `by` records the event actor; quiet checkpoints currently use the
last update sender. This is not evidence that only that person edited the
interval, particularly in collaborative sessions or imported/synchronized edits.

## Architecture

### Shared pipeline

Every rendered comparison follows the same pipeline:

```text
source checkpoint A                    source checkpoint B
         |                                      |
         +------ current format renderer -------+
                         |
                    HTML documents
                         |
              canonical DOM projection
                 |                  |
              tokens A           tokens B
                 +--------+---------+
                          |
                    bounded diff
                          |
              semantic display hunks
                          |
        history list + navigation + DOM redlines
```

The source-to-HTML renderer and its semantic metadata adapter are format-specific.
Projection, diffing, attribution, navigation, and painting use shared contracts.

### Browser/server boundary

Comparison rendering, semantic projection, bounded source/token diffing, and
redline display execute in browser workers or isolated frame tasks. The server
supplies authorized complete source files/trees and captured asset reads. Its
storage layer performs FastCDC chunking, SHA-256 hashing, zstd compression,
verified reconstruction, retention, and GC under SPEC-01. Chunk recipes and
compression parameters are not part of this renderer/worker ABI.

Do not add a browser history-encoding module, download a codec to compare history,
or move HTML compilation/diff computation to the server as a consequence of the
storage change. Existing renderer WASM dependencies remain independently loaded
and bounded. Server encoding benchmarks measure neither browser rendering time
nor whole-application concurrency, and cannot replace the performance budgets
below. A whole-file/chunked storage switch must be invisible to comparison
identity, source quotations, and cache keys.

### Renderer and diff dependencies

The existing HTML paths do not supply a uniform contract for citation keys,
equation source, asset identity, or source ranges. Add a versioned optional
metadata contract and capability declaration for each renderer:

- source path, file digest, and UTF-16 source range for mapped passages;
- equation source/semantic identity and readable text;
- citation keys plus authored locators, prefixes, and suffixes;
- figure asset identity and separately mapped caption content;
- stable structural keys where available.

Validate metadata against the captured tree and rendered generation. Asset
digests can be resolved from project paths or generated URL-to-asset mappings in
LibrePaper; do not require upstream changes where a local adapter suffices.
Renderer-produced metadata that cannot be derived locally requires coordinated
releases of the affected external WASM/compiler repositories and corresponding
module/release pins (including `wasm-modules.lock` where applicable). These are
implementation dependencies, not capabilities assumed to exist already.

The shared text diff lives in the external `wasm-helpers` code used by the
renderer modules. A token diff export requires coordinated native/browser ABI
and worker-protocol changes, pinned releases, and compatibility checks. Preserve
the small Markdown-module preference for format-independent diffing; do not
load Typst solely to gain a new diff export when the Markdown module is available.
Unsupported module versions must report capability failure or use a compatible
bounded fallback, never silently call a mismatched export.

### Source remains authoritative

Rendered HTML and projection offsets are display data. They must never replace:

- source checkpoint identity;
- source paths and file identities;
- source quotations and revision preconditions on comments or suggestions;
- source edits used to accept a suggestion or restore a version.

When rendered and source location disagree, source identity governs mutations.
The UI may still show the rendered passage when it can be located safely.

## Rendering comparison endpoints

Render each endpoint using its captured project files, assets, and
source-affecting settings through the currently supported HTML pipeline.
For LaTeX, honor the tree's pinned release when it provides LaTeXML; otherwise
use the mirror's default HTML-capable release, matching the existing HTML
preview policy. Do not change the tree's PDF engine or release pin. Other
formats use their supported renderer's documented settings resolution.

Resolve both renderer configurations against one captured module/mirror
manifest generation for the comparison. Endpoints with different pins may use
different effective releases; a changed compile setting is a real input change.
Record the actual effective renderer, release, dependencies, and configuration
in cache keys and diagnostics, including any fallback from a pin. Never key
the cache only by the requested pin or the word "current".

Historical rendering is independent of the user's normal preview choice. A
user viewing the current Typst or LaTeX document as PDF still receives HTML
when opening a history comparison.

For an ordinary comparison between adjacent checkpoints, render exactly those
two checkpoints. For “Compare with current,” capture an immutable current tree
and render it as the target. Later collaborative edits do not alter that target
until the user explicitly refreshes it.

Capture asset identities and bytes as well as source/settings, or hold bounded
read leases until all required objects are materialized. An endpoint must not
lose its inputs to concurrent history pruning or live asset replacement.

Paint the clean target preview as soon as it is ready, without waiting for the
baseline render or attribution. On a single-slot LaTeXML queue, schedule the
target first. Reusing cached adjacent versions normally leaves one uncached
conversion per navigation step; both endpoints can be unavailable for a document
whose constructs LaTeXML does not support. Keep source-only review usable.

Do not render every intermediate checkpoint merely to display a range. In
particular, detailed author attribution must not turn one LaTeX comparison into
a chain of compiler jobs.

Historical previews answer “how this source renders now.” They do not claim to
reproduce what a browser displayed when the checkpoint was created. Renderer
identity belongs in cache keys and diagnostics, but routine history UI need not
present it as version metadata.

### Rendering failures

Treat failure of either endpoint distinctly from an empty diff:

- Keep the successfully rendered endpoint if it remains useful.
- State which endpoint could not be rendered.
- Offer the file-level source comparison.
- Keep restore and source inspection available.
- Never substitute a PDF, live document, or different checkpoint silently.
- Never report “No changes” when projection could not be completed.

Missing assets are an explicit incomplete-render condition. A renderer may show
labelled placeholders for missing display-only images if it can still render
safely, but must mark the result incomplete and retain the missing asset identity.
Missing compiler inputs, unknown semantic objects, or other omissions prevent a
complete no-changes verdict. Never substitute a live asset with the same path.
Offer source comparison and a retry; do not cache transient failures indefinitely.

Only the latest successfully published PDF is retained under
`SPEC-01-history-retention.md`; named versions do not archive PDFs. A selected
tree can expose that PDF only when the rendering identity matches. Other versions
use source inspection and contemporary HTML, with no historical PDF download
promised. PDF availability does not imply HTML conversion will succeed. Leaving
history restores the ordinary preview mode and the latest publication bundle.

## Canonical HTML projection

### Contract

Projection returns an immutable value conceptually shaped as:

```js
{
  version: 1,
  tokens: [
    { kind: "word", value: "Results", display: "Results" },
    { kind: "block", value: "paragraph" },
    { kind: "math", value: "<stable equation identity>", display: "E = mc²" }
  ],
  locations: [
    { token: 0, start: { node, offset: 0 }, end: { node, offset: 7 } },
    { token: 1, element },
    { token: 2, element }
  ],
  text: "Results\nE = mc²"
}
```

The concrete representation may avoid storing DOM nodes outside the frame, but
it must preserve equivalent token-to-DOM and token-to-display-offset mappings.
Projection results crossing the frame boundary use serializable element keys
and offsets rather than DOM objects.

Locations are valid only for their document/frame generation. Persisted caches
store tokens and rebindable location descriptions, not live node references or
element keys reusable across newly parsed documents. Painting must validate or
rebuild mappings for the displayed target before applying hunks.

Each token has:

- a `kind` used by comparison and display policy;
- a deterministic comparison `value`;
- a comparability flag, defaulting to true; unsupported semantic objects use
  `comparable: false` and make the comparison incomplete;
- readable display text when deletion requires content not present in the
  target DOM;
- a target DOM location when the token exists in the target;
- a UTF-16 display span where existing annotation and navigation interfaces
  require flat offsets.

### Included content

Project visible document content in reading order. Include:

- headings and paragraphs;
- list items and definition terms;
- quotations and captions;
- table cells and row boundaries;
- code blocks and inline code;
- footnote content at its rendered location;
- meaningful image alternative text;
- equations, citations, figures, and other semantic objects;
- generated numbering only when it is part of what a reader normally reads.

### Excluded content

Exclude:

- `script`, `style`, and `template` contents;
- diagnostics and LibrePaper preview controls;
- injected deletion/suggestion text and annotation controls; unwrap annotation
  marks while preserving the original document content beneath them;
- hidden accessibility duplicates when equivalent visible content is present;
- renderer bookkeeping and source maps;
- navigation repeated outside the document body;
- generated IDs, classes, inline styles, and attribute order;
- layout-only wrappers and pagination scaffolding.

### Whitespace and boundaries

DOM `textContent` alone is not the projection because adjacent blocks can
collapse into ambiguous text and renderers use different whitespace wrappers.

Use explicit semantic boundary tokens for blocks, list items, cells, rows,
code blocks, captions, and footnotes. Normalize layout-only whitespace while
preserving authored spaces and line breaks that change inline prose or code.

Repeated rendering of the same source by the same renderer and projection
version must produce identical tokens. Equivalent inline wrapper changes must
not change the token sequence.

### Tokenization

Use one bundled, version-pinned Unicode word/grapheme segmentation algorithm and
data set for every format and both endpoints. Treat word segments as word tokens,
punctuation/symbol segments as separate grapheme tokens, and normalized prose
whitespace as explicit space/break tokens. Preserve code whitespace exactly.
Combining sequences and emoji must not be split within grapheme clusters; DOM
and display spans remain UTF-16 offsets. Do not normalize away authored character
differences. The projection version includes the segmentation rules/data version;
unversioned browser `Intl.Segmenter` output cannot define comparison identity.

Join inline text runs before tokenizing so wrapper boundaries do not split words.
Block-boundary tokens include their semantic type and have a readable deletion
representation, such as a line break or a labelled paragraph boundary. Structural
changes must remain navigable even when they contain no word tokens.

### Structured atomic tokens

Treat a structured object atomically when diffing its internal generated DOM
would be noisy or misleading.

#### Equations

Prefer stable renderer-provided source metadata. Otherwise normalize MathML by
semantic content while excluding presentation attributes. Use readable math
text or source as deletion display. A changed equation is initially one
replacement hunk; sub-expression diffing is a later enhancement.

Without source metadata or usable MathML, use a trustworthy accessible math label
as an atomic fallback. If none exists, emit an explicitly uncomparable object
with a readable "Equation" placeholder and offer source comparison; identical
placeholder text must not establish equation equality.

#### Citations

Represent a citation cluster using stable citation keys when available. Keep
the rendered label as display text. Renumbering caused by another citation must
not make every later citation appear edited when stable keys establish that
their identity is unchanged.

Include authored locators, prefixes, and suffixes in comparison identity so
changing a cited page still produces a change. Without stable keys, compare the
normalized visible cluster label and accept possible renumbering noise; do not
guess bibliographic identity from the number alone.

#### Figures

Use the referenced asset digest, meaningful alternative text, and caption
projection. Treat replacement of the asset as a figure change even when its
caption is unchanged. Caption prose may be diffed independently.

Resolve local asset identity through the captured tree and renderer URL mapping;
blob URLs themselves are not stable identities. For generated figures use a
verified generated-content digest when available. If no trustworthy identity is
available, compare readable alt/caption content but mark the figure's visual
comparison unavailable. Never claim unchanged image pixels from unchanged prose.

#### Tables

Emit explicit row and cell boundaries. Sequence diff over these tokens supplies
the default alignment; stable renderer-provided row/column keys may refine it.
Diff cell contents as ordinary projected content. Adding one row must not merge
the complete table into a single text replacement.

#### Code

Ignore syntax-highlighting spans. Preserve code whitespace and line boundaries.
Use a line-oriented refinement for multiline code only if the ordinary token
diff produces unreadable results.

### Projection versioning

Increment the projection version when comparison values or boundary rules
change. Cache entries include this version. Because projections and diffs are
derived on demand, no checkpoint migration is required.

## Diff calculation

### Initial engine

Keep the existing bounded Myers implementation for the initial unified
pipeline. Preserve the existing source-oriented `{at, delete, insert}` API for
synchronization, three-way merge, and source suggestions.

Add a review-oriented token interface rather than serializing semantic tokens
into an ambiguous string. Its logical result is:

```js
{
  fromA: number,
  toA: number,
  fromB: number,
  toB: number,
  deleted: projectionA.tokens.slice(fromA, toA),
  inserted: projectionB.tokens.slice(fromB, toB)
}
```

The implementation may reuse the same Myers core over token comparison values.
Compare both token kind and comparison value for comparable tokens. Unsupported
objects cannot compare equal merely because their placeholder values match;
report those regions as unverified, not as proven edits or unchanged content.
Results are sorted, non-overlapping, and deterministic for fixed inputs and
resource limits.

### Bounds

The existing text diff has a fixed trace bound and coarse-replacement fallback;
the token interface and richer limits are new work. Bound token diff work by
token count, edit distance, deterministic work/trace budget, and output hunks.
When the bound is exceeded, return one coarse replacement for the affected
region and mark it as simplified. Do not freeze the reader or silently omit the
change.

A separate wall-clock watchdog may cancel a worker and show a retry/source-view
state, but must not choose different "deterministic" hunks merely because one
device is slower. Benchmark and expose limits in diagnostics before release.

Strip equal leading and trailing token runs before allocating diff trace state.
Do not render or diff unrelated files merely because they belong to the same
checkpoint tree.

### Hunk refinement

Coalesce nearby word edits into one readable passage using existing context
rules. Preserve separate structured-object and block-boundary changes unless
joining them makes the rendered passage clearer.

A replacement hunk retains both its deleted and inserted sides. Empty inserted
or deleted sides represent pure deletion or insertion respectively.

Move detection is optional. Until implemented reliably, a move appears as one
deletion and one insertion. The UI must not label an inferred move without a
stable structural match.

### Library policy

Do not adopt ProseMirror solely for diffing. LibrePaper source and Yjs remain
the editing model. Do not use a DOM mutation diff as the review contract:
mutation operations do not provide semantic tokens, readable deletions,
source identity, author attribution, or stable navigation.

A ready-made diff engine may replace the Myers core after benchmarks, provided
it accepts LibrePaper tokens, enforces resource bounds, produces stable ranges
in both endpoints, has a compatible free-software license, and improves actual
academic documents. It must not own rendering, history, or UI state.

## Redline display

### Insertions

Wrap inserted target content in one or more marks derived from its DOM mapping.
A change crossing inline element boundaries uses multiple visual marks but
remains one navigable hunk.

Use underline or another non-color cue. Author color supplements the insertion
style and is never its only meaning.

### Deletions

Insert deleted content at the corresponding seam in the target projection.
Render prose deletions as struck-through text. Structured atomic deletions may
use a compact struck-through representation containing their readable display
text and type.

Do not insert untrusted historical HTML fragments into the target document.
Rich deleted-fragment rendering requires an explicit sanitizer and is outside
the initial implementation.

### Replacements

Display deletion immediately before insertion at the shared target seam. Keep
both sides associated with one hunk for navigation, summary, and attribution.

### Styling

Redlines must remain understandable in light and dark themes, high contrast
mode, print styles, and for common forms of color-vision deficiency. Selected
changes receive a visible focus treatment independent of author color.

The painter must coexist with comment highlights and pending suggestion marks.
Clearing or repainting one annotation class must not remove the others.

## History comparisons

### Preserved panel behavior

Keep the existing newest-first timeline, day grouping, expandable sessions of
unlabelled checkpoints by the same actor, and the named-versions filter. Runs
split at milestones and gaps of at least fifteen minutes; labelled versions
remain individually visible. UI grouping does not delete history and need not
use the UTC buckets of storage retention.

Keep naming/renaming versions, checkpoint links, changed-file inspection, and
the authorized restore flow. Restoring requires the existing explicit user
action, preserves current work under the existing checkpoint/merge rules, and
creates a new restore event rather than rewriting an old tree. The new comparison
pipeline must not remove these already implemented behaviors. Their preservation
is part of this spec, independent of older history-panel proposal files.

### Retention and saved-state expectations

The default Balanced policy in SPEC-01 retains routine events at five-minute
density during the first hour, hourly from 1–24 hours, six-hourly from 1–7 days,
and daily thereafter, using UTC-aligned buckets. It does not preserve every
checkpoint from the last 24 hours. Display the effective policy supplied by the
server (including overrides and limits), not a separately implemented UI policy.
Use the selected display timezone for labels without changing UTC retention.

Explain that ordinary edits are saved promptly while history keeps selected
recovery points. Bucket density is not live-save cadence, minimum spacing between
events, or a guarantee that every bucket is populated. Named versions and
milestones are preferential, subject to count/byte limits; they never retain
older PDFs. Show pending history admission separately from live-save durability
if background encoding or quota admission delays a checkpoint.

Tighter retention makes ancestry gaps normal, including inside the first hour.
Show the actual retained range; do not imply continuous per-save attribution or
keep a hidden source/PDF archive merely to refine redlines. Bounded comparison
leases protect active reads, not indefinite history retention. If an unleased
selection or link has been pruned, refresh the timeline and report that the
version is no longer retained; never substitute another checkpoint silently.
An already captured, authorized comparison may finish under its existing bounded
lease/cache lifetime, but cannot resurrect an evicted version in server history.

### Ordinary version selection

Selecting a checkpoint compares its immediate retained predecessor with that
checkpoint. The earliest retained checkpoint has no predecessor and displays a
clean preview labelled “First retained version,” not a claim that it is the
document's original creation state.

Ordinary selection resets any prior arbitrary baseline. The panel identifies
the selected version and comparison baseline by name or timestamp.

A retained predecessor need not be the original predecessor. Consume ancestry-gap
metadata when available; missing legacy evidence is unknown, not proof of
adjacency. Changed-file summaries must describe these actual endpoints, not a
cached summary against a deleted parent.

### Compare with current

Capture the current complete tree when the user invokes this action. Use the
historical checkpoint as baseline and the captured tree as target. If live
source subsequently changes, show “Newer edits available” and refresh only on
explicit request.

### Arbitrary ranges

Keep arbitrary checkpoint-to-checkpoint comparison as a secondary control.
Render only its two endpoints. Clearly label both endpoints and do not imply
that displayed author attribution reconstructs every intermediate edit.

### Show changes

The Show changes toggle controls redline painting without changing the selected
source or regenerating HTML. Turning it off shows the clean target preview.
Turning it back on reuses the current projections and diff when still valid.

### Navigation and summary

Provide:

- previous and next change controls;
- explicit “Change N of M” status (an expansion of the existing bare count);
- an explicit no-changes state;
- a concise expandable prose summary;
- changed-file links and source comparisons;
- scrolling and focus to the complete hunk, including multi-mark insertions.

Navigation order follows target reading order. Pure deletions at the same seam
retain deterministic diff order.

## Author attribution

Do not render intermediate checkpoints to recover attribution. Preserve useful
detail through bounded source-only refinement after the two rendered endpoints
have supplied the initial hunks.

### Event actor versus content authorship

Keep `by` as the checkpoint event actor, labelled accordingly in the timeline.
For automatic checkpoints it may be the last update sender. It must not by
itself assign authorship to all content since the previous checkpoint. Add an
interval authorship summary with `single`, `multiple`, or `unknown` evidence,
maintained from reliable source-edit provenance since the original parent.
One authenticated sender of a batched sync/import is not proof of one author.
Legacy intervals without evidence are `unknown`.

Contributor identity remains subject to existing account-erasure and authorization
rules. Stable account IDs stay catalogue-only; expose only the display evidence
needed by the reader. These summaries and original-adjacency/gap metadata are new
dependencies for precise attribution, not prerequisites for showing unlabelled
redlines or shipping HTML comparison before the storage work.

### Refinement and fallback

1. For proven original adjacency and a proven single contributor to the interval,
   use that contributor. Retained adjacency after pruning is insufficient.
2. For a short complete chain, fetch changed source files at intermediate trees
   and diff them without invoking renderers. Initially retain the existing cap
   of 12 intervals, with additional source-byte and diff-work limits. Propagate
   source ranges and their supported contributor evidence through those edits.
3. Map those source ranges to baseline/target rendered hunks using validated
   renderer source ranges. Exact unique quotation-and-context anchoring may map
   ordinary prose only when it establishes an unambiguous source-to-display
   correspondence. It locates already established provenance; a text match does
   not establish who wrote it. Generated citations, repeated passages, transformed
   markup, and ambiguous matches remain coarse unless stronger metadata exists.
4. Attribute a whole hunk to one person only when evidence covers all its changed
   parts. Otherwise use "Several people" when multiple contributors are known,
   or leave authorship unspecified when evidence is unknown. A range-wide single
   author requires a complete chain of proven single-author intervals agreeing
   on the contributor, with no pruned gap or unknown interval.
5. Uncheckpointed changes in a captured current target remain unattributed unless
   the captured collaborative provenance supplies equally reliable evidence.

Pruning `A → B (Alice) → C (Bob)` into `A → C` must not credit Alice's surviving
edits to Bob. Missing checkpoints, byte/work caps, mixed authors within one
checkpoint, or unmappable source spans cause coarse/unknown fallback. Refinement
may update attribution only for the still-current comparison; it must not change
hunk identity, navigation position, or the captured target. No fuzzy text matching
or intermediate HTML rendering is permitted to fill an evidence gap.

## Suggestions and tracked changes

### Durable representation

A suggestion continues to store:

- the source path and stable file identity;
- its source checkpoint or revision;
- an exact source quotation with context or an equivalent durable range handle;
- the proposed replacement source;
- creator, timestamps, discussion, status, and outcome.

Do not store generated HTML as the suggestion's mutation target.

### Display

To display a suggestion:

1. Resolve its source anchor against the applicable source revision or current
   source using existing conflict rules.
2. Render that source tree to current HTML.
3. Locate the corresponding rendered passage through stable renderer source
   metadata when available, with quotation anchoring as fallback.
4. Project the passage and proposed result through the same semantic token
   rules used by history.
5. Display its deletion and insertion using the shared redline vocabulary.

Ordinary pending suggestions are painted over the existing source preview,
where proposed text is absent. The shared painter therefore has two insertion
forms: marks on content present in a target DOM, and synthetic inert insertion
text at a validated seam. For a suggestion, mark the existing mapped passage as
deleted and insert the proposed display tokens after it; a pure insertion uses
only the seam. Do not apply the history painter's target-span insertion rule to
text that is not there. Render synthetic content using text nodes and fixed
semantic labels, never arbitrary proposal HTML.

Keep suggestion-specific source anchors and mark identities. Several pending
suggestions are independent proposals against the applicable source, not a
silently composed candidate. Overlapping or unlocatable proposals open a review
card with the exact source replacement instead of misleading inline placement.
A fully rendered candidate may use ordinary target-DOM marks in a separate
candidate preview, clearly identified as such.

Suggestion display may use a focused passage render or deterministic source
projection when rendering the complete proposed document would be too costly.
It must not imply that a proposal compiles unless a complete candidate was
actually rendered and verified.

Without a safe rendered mapping, display the exact source proposal in the
review card. A deterministic source projection must identify itself as a source
excerpt and cannot claim equation/citation semantics that require compilation.

### Acceptance and rejection

Accepting applies the proposed source change through the authoritative Yjs and
merge path with revision preconditions. It creates the existing history event.
Rejecting resolves the annotation without changing source.

If the source anchor is stale or ambiguous, open the existing merge/review flow.
Rendered similarity alone must never authorize applying a proposal to a new
source location.

### Relationship to history

History redlines describe differences between source checkpoints. Suggestions
describe proposed source changes. They share projection, diff presentation,
styles, and navigation primitives, but retain separate state and actions.

## File-level source comparison

Keep CodeMirror Merge as the detailed source comparison for changed text files.
It remains available when:

- HTML rendering fails;
- the change concerns non-rendered source or configuration;
- users need exact markup or code differences;
- a historical passage has no safe rendered location;
- an editor needs to bring selected source changes into the live document.

Binary and asset changes show type, path, size, digest, and available preview
metadata rather than pretending to be text diffs.

## Caching and concurrency

Cache contemporary HTML and projections by:

```text
document storage identity
tree digest
effective renderer/release identity and configuration
projection version
```

Use bounded browser memory and optional IndexedDB storage. Every cache entry is
disposable and does not count as durable history. Do not send credentials or
share keys into cache keys, logs, rendered HTML, or compiler inputs; partition
private cache access by an opaque local authorization scope where necessary.

Include the effective compiler release, dependency manifest/module digests,
renderer metadata capabilities, and completeness state in the appropriate cache
identity. Captured current trees use the same logical identity rules as stored
trees. This is a new cache, not permission to put projections into the existing
URL-only Cache Storage wrapper. If that wrapper is extended, its key must encode
the full versioned identity and authorization partition. Rebind DOM mappings on
load as required by the projection contract.

Comparison work carries generation identities. A late render, projection, or
diff must not replace a newer selection. Closing history cancels or retires
work where supported. Current edits cannot mutate a captured target.

Deduplicate concurrent requests for the same endpoint and projection identity.
Render independent endpoints concurrently when both compilers can safely do so;
respect single-worker queues for engines such as LaTeX.

## Security

- Continue rendering generated documents inside the existing isolated document
  frame and content-security policy.
- Treat messages and projection data arriving from the frame as untrusted.
- Project the baseline in an inert parsed document or a scriptless sandbox with
  resource fetching disabled. Never attach baseline HTML to the live application
  DOM or execute its scripts to obtain a projection. The target follows the
  existing isolated-frame policy. Define shared visibility rules from renderer
  semantics and explicit hidden attributes so baseline projection does not need
  live layout, arbitrary script execution, or network access.
- Validate token counts, offsets, element keys, and text lengths before
  painting.
- Do not insert historical HTML fragments for deletions.
- Do not expose source, cached HTML, projections, or checkpoint metadata across
  document authorization boundaries.
- Invalidate private in-memory projection caches when the reader changes link,
  signs out, or loses access.
- Partition persistent private caches by opaque authorization scope, clear the
  scope on sign-out/link change/observed revocation, and revalidate access before
  reopening cached private content. Late jobs must not repopulate a retired scope.

Frame payload validation is new work beyond existing message-tag/type dispatch:
validate the full shape, bounded arrays/strings, integer ranges, token-to-location
consistency, and the active frame/generation before accepting or painting data.

## Accessibility

- Announce comparison loading, failure, no changes, and the current change
  position through appropriate live status text.
- Give insertions and deletions programmatic labels in addition to visual
  styling.
- Make Show changes, endpoint selection, summary disclosure, and previous/next
  navigation keyboard accessible.
- Preserve a logical reading order for injected deletions.
- Do not depend on color to distinguish additions, deletions, authors, or the
  selected change.
- Restore focus predictably after rerendering or closing a comparison.

## Performance budgets

Set measured budgets before release for:

- HTML rendering latency by format;
- maximum projected DOM nodes and tokens;
- diff edit distance and trace memory;
- redline mark count;
- cached HTML and projection bytes per document;
- time from selecting a cached version to painted redlines;
- time from selecting an uncached version to either painted redlines or a
  useful loading/failure state.

Do not choose final thresholds without representative Markdown, Quarto, Typst,
and LaTeX papers. The UI must remain responsive while compilation and diffing
run in workers or asynchronous frame tasks.

## Migration

1. Route historical Typst/LaTeX passage reads through their existing HTML modes;
   preserve captured settings/assets and effective release selection. Paint the
   clean target before waiting for comparison work.
2. Introduce the shared metadata/capability contract, explicit fallbacks, inert
   baseline projection, pinned segmentation, and validated DOM mappings. Ship
   required upstream renderer changes with compatible native/browser pins.
3. Add the bounded token diff interface through coordinated WASM/worker releases;
   preserve the existing source-text API and small-module loading policy.
4. Compare selected versions with semantic tokens while retaining flat visible
   text as a compatibility adapter. Run corpus checks for both representations.
5. Add bounded versioned HTML/projection caches with authorization lifecycle and
   stale-generation handling. Remove PDF extraction from historical comparison
   and rendered-passage lookup, preserving ordinary latest-PDF viewing. This
   enables retirement of older PDFs under the latest-only storage policy.
6. Replace intermediate rendering for attribution with bounded source-only
   refinement and evidence-based fallback; add contributor/gap metadata support.
7. Move suggestions onto shared projection and painting, including synthetic
   insertions and explicit source-only review cards.
8. Remove obsolete branches only after format, browser, and ABI compatibility
   coverage passes. Preserve the panel behaviors specified above.

No checkpoint rewrite is required. Old source histories are rendered through
the current pipeline when selected.

Coordinate rollout with the tighter retention profile: consume ancestry-gap and
unavailable-version states before presenting sparse history as adjacent editing
steps. The preference preview/confirmation process controls destructive policy
changes; opening the history panel does not opt an owner into new retention.

## Acceptance criteria

1. Markdown, Quarto, Typst, LaTeX, and authored HTML comparisons all use the
   same projection, diff, and redline interfaces after rendering.
2. Typst and LaTeX historical comparison and passage lookup do not retrieve or
   extract text from PDF; explicit latest-PDF viewing remains independent.
3. Historical HTML is rendered on demand from exact checkpoint source with the
   effective supported renderer/release; no stored historical rendering is required.
4. Switching the ordinary Typst or LaTeX preview between PDF and HTML does not
   alter history hunk results.
5. Generated IDs, styles, wrapper elements, and syntax-highlighting spans do
   not create false changes.
6. Block boundaries prevent adjacent paragraphs, list items, and table cells
   from being diffed as one concatenated string.
7. Citation renumbering does not mark unchanged citations when stable citation
   keys are available.
8. Equations and figures with trustworthy identities behave as atomic changes
   and produce readable deleted representations. Unsupported objects explicitly
   degrade to source comparison rather than false equality.
9. Insertions spanning inline element boundaries paint as one navigable change.
10. Replacements show their deletion and insertion together using non-color
    visual cues.
11. Adjacent comparisons distinguish event actors from proven contributors;
    pruned gaps and multi-author/unknown intervals cannot acquire a single author
    merely from the target's `by` field.
12. Arbitrary ranges render only their two endpoints. Bounded intermediate source
    diffs refine attribution when provenance and source-to-display mapping support
    it, otherwise remaining coarse or unknown.
13. Compare with current remains stable while collaborators edit and changes
    only after explicit refresh.
14. Renderer failure is distinguishable from an empty comparison and leaves
    source comparison and restoration available.
15. Suggestions remain source-anchored, use shared redline presentation, and
    cannot be applied based only on a rendered match.
16. Comment highlights, suggestion marks, and history redlines can be painted
    and cleared independently.
17. Stale asynchronous results cannot replace a newer selection or current
    preview.
18. Projection and painter validation rejects invalid frame offsets and element
    references without corrupting the displayed document.
19. Focused automated coverage includes Unicode/UTF-16 offsets, inline markup,
    whitespace, math, citations, figures, tables, code, renderer failures,
    cancellation, author attribution, suggestions, and cache invalidation.
20. Browser coverage verifies the main history, Show changes, navigation,
    compare-with-current, suggestion, keyboard, and screen-reader status flows.
21. Tests cover different LaTeX pins, missing HTML support in a pin, effective
    release cache invalidation, single-worker target-first rendering, and missing
    assets without substitution or false no-changes results.
22. Unicode/punctuation segmentation is stable across supported browser engines;
    metadata-free citation/math/figure fallbacks and citation locator edits have
    explicit expected results. Table row insertion preserves useful cell alignment.
23. Suggestions cover pure synthetic insertion, replacement, overlap, unavailable
    source mappings, and clearing one annotation class without removing another.
24. Reparsed cached HTML has valid fresh DOM mappings. Inert baseline projection
    cannot execute scripts or fetch document resources. Authorization changes and
    late asynchronous results cannot resurrect private cache entries.
25. Tests cover pruned `A → B (Alice) → C (Bob)` ancestry, multiple contributors
    in one checkpoint, imported edits with unknown authorship, repeated quotations,
    source-only refinement limits, and account erasure of contributor metadata.
26. Existing session grouping, named filtering, naming, checkpoint links, and
    authorized restore remain available. Naming a checkpoint does not retain its
    PDF, and the latest PDF never substitutes for another tree's comparison.
27. Server-side whole-file/chunk encoding changes do not require browser codecs,
    change cache/source identities, or move comparison compilation/diffing to the
    server. Reads verify reconstructed source before it enters the browser pipeline.
28. Tests cover same-five-minute-bucket replacement, hourly/six-hourly transitions,
    earliest-retained-version labels, pruned selections/links, and active comparison
    leases. Effective retention density and pending checkpoint state are distinct
    from ordinary saving; display timezone never changes retention selection.

## Open questions

- Which concrete attribute names/adapter payloads implement the shared metadata
  contract in each renderer, and which upstream release first supports them?
- Which projection and diff limits preserve good results for the largest
  admitted documents?
