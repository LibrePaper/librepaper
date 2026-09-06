# Plan: Typst documents rendered as PDF

Status: implemented. The stages below record the migration design; the
implementation and validation notes summarize the delivered behavior.

## Implementation and validation notes

- Typst now uses the pinned 0.15.1 PDF exporter in native publishing and WASM.
  `RenderedDocument` is a single typed HTML/PDF result; the ABI reports output
  kind and the worker transfers an owned binary buffer.
- Editors automatically compile Typst PDFs. Readers and historical passage
  lookup use stored PDFs. New source-only documents show an explicit missing
  rendering state. Existing documents acquire a PDF on an editor's next
  successful compile; seeded examples store their native PDFs immediately.
- Native uploads compare a source-input digest, independent of Yjs item IDs,
  with the server's captured tree. A guarded upload validates that identity
  under the room lock before storing bytes. Concurrent changes cannot relabel
  an old PDF as a different current source tree.
- Intermediate Typst previews may appear during continuous typing, but only
  current snapshots update diagnostics and qualify for artifact storage.
- Old HTML figure-region records remain intact and are labeled unavailable
  in PDF when no valid image-coordinate mapping exists. Text-based source
  navigation remains heuristic; exact span-to-page mapping is not introduced.
- Typst's memoization cache is evicted by generation. A 100-revision WASM
  corpus run remained at 34,668,544 bytes of linear memory, instead of growing
  from approximately 35 MB to 57 MB without eviction.
- Independent Poppler checks verify PDF text, A4 geometry, imports, embedded
  images, bibliography, three-page output, repeated edits, and error recovery.
  The corpus page raster matches standalone Typst 0.15.1 pixel-for-pixel with
  system fonts disabled. CJK/emoji glyph coverage remains limited by the same
  embedded fonts as before; switching exporters does not add fonts.
- Browser acceptance covers Firefox and Chromium: initial preview, two visible
  revisions, preserved preview on error, repaired source, delayed artifact
  storage, and a fresh read-link page that never downloads Typst.
- Observed small-fixture edit-to-preview times were approximately 0.2–0.8 s
  with PDF and 0.3–0.5 s with the prior HTML renderer. These are local acceptance
  measurements under varying build load, not a controlled performance study.
  The WASM module grew from 31,790,245 to 33,546,131 bytes; gzip sizes were
  13,813,923 and 14,340,699 bytes respectively.
- `make build` and `make test` rebuild an already-present optional Typst module,
  keeping it in step with the shell. They still do not opt a deployment into
  Typst WASM when it has never been built.

Reproduce validation after `make typst` and `make build`:

```sh
make test
node web/checks/typst-pdf.mjs --soak
node web/checks/typst-viewer.mjs
node web/tools/typst-pdf-browser.mjs target/release/komodoc both
```

The PDF corpus and source-navigation checks require Poppler (`pdfinfo` and
`pdftotext`); browser acceptance requires Firefox and Chromium. No production
document migration or deployment is performed by these checks.

## Outcome and scope

Typst documents use Typst's paged layout and PDF exporter, compiled locally
in a browser worker or by the publishing CLI. Komodoc displays those PDFs
through its existing pdf.js viewer, with selectable text, comments, highlights,
history, and source navigation. Markdown and authored HTML retain their HTML
renderers. The deployment stores artifacts and does not compile documents.

This addresses Typst layout fidelity. It does not establish the cause of the
reported Firefox LaTeX update failure; Firefox editing is a required acceptance
test for this change, not a problem assumed to disappear with PDF output.

Typst's official documentation describes HTML export as experimental and
incomplete, and distinguishes semantic HTML from the fully laid out document
produced by PDF, SVG, and PNG:
https://typst.app/docs/reference/html/

## Existing implementation

| Area | Current behavior | Required change |
| --- | --- | --- |
| `crates/engine/src/typst.rs` | Compiles `typst_html::HtmlDocument`, wraps HTML in the shared page, and suppresses the generic incomplete-HTML warning | Compile a paged document and export PDF; retain source diagnostics and input isolation |
| `crates/engine/src/diagnostic.rs` | `Compiled` carries an optional string page | Represent binary PDF output explicitly without conflating errors, HTML, and bytes |
| `crates/engine/src/abi.rs` | Output storage is bytes, but compile results originate as strings | Add an explicit output-kind contract and return PDF bytes |
| `web/src/lib/renderer-wasm.js` | Decodes every output as UTF-8 | Copy binary results safely and decode only text channels |
| `web/src/lib/renderer-worker.js` | Successful compilation returns `{html, diagnostics}` | Return and transfer PDF output for Typst |
| `web/src/components/Reader.svelte` | PDF frame, artifact persistence, and several readiness branches are tied to LaTeX | Separate output format, compiler availability, compilation cadence, and storage |
| `web/src/entries/viewer.js`, `web/src/lib/pdf/` | PDF rendering and text-layer annotations already exist | Reuse for Typst and verify Typst-specific extraction and recompilation |
| `crates/komodoc/src/server.rs`, `room.rs` | PDF rendering routes, digest validation, quotas, and retention already exist | Audit and generalize for Typst without weakening those rules |
| `crates/komodoc/src/render.rs`, `cli.rs` | Native Typst compilation validates publishing through HTML | Validate with the same PDF backend and publish a matching artifact |

The comments at the top of `typst.rs` say PDFs cannot support annotations.
That predates the PDF text-layer viewer and must be corrected.

## Design decisions

1. **PDF is the normal Typst output.** Do not add a user-facing HTML/PDF toggle
   or silently fall back to HTML after a compile error. Existing stored HTML
   may remain a clearly identified migration fallback, described below.
2. **Keep compiler inputs stable.** Reuse the current world, embedded fonts,
   project files, relative path rules, asset bytes, and host-supplied date.
   Changing output format does not introduce package downloads or broader
   filesystem access. Existing package/font limitations remain limitations.
3. **Typst needs no distribution chooser.** The existing digested Typst WASM
   module loads automatically when an authorized editor needs it. LaTeX's
   chooser and mirror remain specific to LaTeX.
4. **Readers consume stored PDFs.** Viewing a Typst document should not require
   downloading its compiler. Editors produce artifacts; missing artifacts have
   an explicit state and migration behavior, rather than an empty preview.
5. **Source remains authoritative.** A PDF belongs to the exact source-tree
   digest used to compile it. Compilation failures preserve the last successful
   preview and show diagnostics; an old PDF is never marked current.
6. **Use existing annotation records.** Do not replace or bulk-rewrite comments
   to accommodate a different text extraction order. Re-anchor from quotation,
   context, and source selectors, and preserve unresolved anchors for review.
7. **Do not require SyncTeX for Typst.** Retain and test text-based navigation.
   Exact source-to-page mapping through Typst spans is a separate enhancement
   if the existing mapping proves insufficient for formulas and generated text.

## Stage 1: Prove the backend and establish baselines

- Add a small PDF compilation spike using `typst-pdf` at the same pinned
  version as the other Typst crates (currently 0.15.1). Verify the exact API
  against the pinned dependency source before implementation.
- Verify native and `wasm32-unknown-unknown` compilation with the project's
  current WASM profile. Check font embedding and export failures as well as
  successful layout; a native-only success is insufficient.
- Build a committed fixture corpus: ordinary prose; math; tables; columns;
  headers, footers, and pagination; imported chapters; bibliography; embedded
  images; Unicode; and an intentionally broken imported file. Include the
  existing Typst example and a representative long paper.
- Record the current HTML preview's cold-load time, warm compile time,
  edit-to-visible-preview latency, WASM download size, and memory behavior.
  Measure the PDF spike on the same fixtures in Firefox and Chromium.
- Record browser versions, hardware, document size, and median/tail latency.
  Set numerical release budgets from these measurements before switching the
  default; do not borrow LaTeX's debounce merely because both outputs are PDF.

Exit: both native and WASM emit valid PDFs; the existing viewer can draw and
extract their text; measured size and latency make the migration viable.

## Stage 2: Introduce binary compiler results

- Introduce an explicit engine output representation, such as
  `RenderedDocument::Html(String)` / `Pdf(Vec<u8>)`, alongside diagnostics.
  Migrate callers deliberately; avoid a loosely typed byte buffer that every
  caller has to guess how to interpret.
- Implement paged Typst compilation and PDF export. Preserve warnings,
  imported-file locations, UTF-16 diagnostic columns, hints, and traces.
  Represent layout/export failure as no output plus diagnostics.
- Keep title extraction and failure-message channels textual. Remove HTML
  wrappers and the incomplete-HTML warning filter from the PDF path.
- Extend the WASM ABI with an output-kind or capability export. Keep the
  pointer/length binary output and separate diagnostic JSON channel.
- In the JS loader, copy PDF bytes out of WASM memory before another ABI call
  can replace the output or grow memory. Never decode PDF bytes with
  `TextDecoder`, embed them in JSON, or transfer the WASM memory buffer itself.
- Transfer the owned PDF buffer from worker to parent. Preserve an owned copy
  where persistence and iframe delivery both need the bytes.
- Keep backward handling for old HTML modules explicit during development;
  the final release must pair its new loader with digested new module bytes.

Exit: native and browser clients receive a typed PDF result with correct
diagnostics; Markdown HTML and title/failure calls still work.

## Stage 3: Generalize the reader's rendering lifecycle

- Add small renderer capability helpers for output kind and browser compiler
  availability. Keep readiness and compile scheduling separate from output
  kind. Audit each LaTeX conditional rather than mechanically widening it.
- Select the PDF iframe for both Typst and LaTeX, including when the main file
  changes or a checkpoint has a different source format.
- Automatically load Typst for editors. Show loading/compiling/failure states
  without a TeX distribution card. Stored-PDF reading must work on a deployment
  that can store Typst sources but lacks the Typst WASM module.
- Keep at most one preview compilation active and one latest request queued.
  Preserve bounded updates during continuous Typst typing. Do not adopt a
  strict discard-on-any-edit rule that can starve the preview indefinitely.
- Distinguish an intermediate visible preview from an artifact eligible to be
  stored as current. History navigation, main-file changes, and document
  replacement must invalidate in-flight results in either case.
- Generalize snapshot hashing, first-render storage, delayed subsequent
  storage, and checkpoint storage to PDF-producing formats. Keep LaTeX's
  debounce, compiler selection, and optional `.synctex` sidecar separate.
- Retain the last successful page on compile failure. Before the first success,
  display an explicit failure state and diagnostics rather than trying to send
  an HTML failure page to a PDF-only frame.
- Verify iframe readiness, pending-preview replay, worker failure recovery,
  scroll retention, and buffer ownership across repeated recompiles.

Exit: edits produce visible Typst PDF updates in both browsers, including
rapid edits and recovery after invalid source, without stale navigation results.

## Stage 4: Persist PDFs and preserve publishing/history behavior

- Audit rendering PUT/GET/latest routes and PDF frame routing for format
  assumptions. Support Typst under the existing permission, tree-digest,
  payload-size, quota, caching, and retention rules.
- Store the first successful live PDF immediately, later PDFs after the
  existing quiet interval, and the matching PDF when naming a checkpoint.
  Show current/earlier-version/missing states accurately to other clients.
- Keep checkpoint renderings associated with their own tree and format;
  compiling or viewing history must not overwrite the live latest pointer.
- Update native publish validation to use PDF compilation. After publishing
  the complete project tree, obtain the server's canonical digest, ensure it
  matches the compiler inputs, and upload the locally produced PDF.
- Preserve dependency discovery and existing missing-file behavior. A local
  PDF compiled with a sibling that was not uploaded must not be attached to a
  different server tree. If artifact upload fails, report the source publish
  and missing artifact separately and provide a retry path.
- Audit directory publishing, source-only API creation, example seeding,
  export/download behavior, and document title extraction. Define source-only
  creation as valid, with an explicit not-yet-rendered state until an editor
  produces the PDF. Provide PDF download through the same authorized artifact
  path; source archives continue to export the original project.
- Use tree digest as the initial artifact identity, matching the current
  rendering API. Document that it identifies source inputs, not compiler
  version or host date. Do not promise byte-identical PDFs across times or
  compiler upgrades. If provenance is needed for rollout, add metadata without
  silently changing digest or retention semantics.

Exit: a separate reader can see the correct stored PDF without loading Typst;
publishing, checkpoints, permissions, and retention remain coherent.

## Stage 5: Annotations, navigation, and existing documents

- Exercise pdf.js text extraction on Typst ligatures, hyphenation, page breaks,
  columns, repeated running headers, footnotes, equations, and non-ASCII text.
  Verify text offsets against the annotation agent's flattened text, not just
  whether the canvas looks right.
- Test creation, display, reply, resolution, and re-anchoring of text comments
  and highlights after edits. Test source-only annotations and ambiguous quotes.
- Audit region/figure annotations separately: determine whether PDF regions
  refer to a page or an embedded figure, and how existing HTML figure digests
  translate. Do not silently map old figure coordinates to whole-page boxes.
  Preserve and label an unplaceable region rather than dropping it.
- Preserve text-based source navigation in both directions across imported
  files. Measure known failures for math and generated text and keep the
  existing explicit no-match behavior. Exact positional mapping is not a
  prerequisite unless baseline navigation would otherwise regress materially.
- Test switching an existing Typst document with HTML-era comments to PDF.
  Recover anchors using stored quotations, context, and source selectors;
  report unmatched anchors without changing their original records.
- Existing documents have no stored PDF initially. An authorized editor's
  next successful compile creates it. Where a legacy HTML artifact is actually
  available, retain it temporarily as a labeled older rendering; otherwise
  show an honest not-yet-rendered state. Never synthesize an artifact on the
  server or infer that an old checkpoint was rendered.
- Inventory examples and important existing documents before release. Render
  their live versions with a client-side migration helper using normal upload
  validation. Make it resumable and leave source/history/comment records intact.
  Do not bulk-convert historical checkpoints without an explicit need.

Exit: existing comments remain available, anchors are either correct or clearly
unmatched, and old documents have a defined transition rather than blank pages.

## Stage 6: Validation and release

| Layer | Required evidence |
| --- | --- |
| Engine | PDF validity, expected text/page geometry, embedded fonts, local imports/assets, structured errors, native/WASM agreement |
| ABI/worker | Binary round trip, memory growth, repeated calls, diagnostics after failure, transfer ownership, worker recovery |
| Reader | First load, repeated edits, sustained typing, compile failure/recovery, queued work, main-file changes, history races, iframe reload |
| Storage/API | First/latest/checkpoint PDF, stale digest rejection, correct MIME/cache headers, private access, quotas, pruning, unavailable artifacts |
| Publishing | Single-file and directory projects, dependency completeness, canonical digest, failed artifact upload, source/PDF downloads |
| Annotations | Existing HTML-era anchors, new PDF comments, multi-page selections, ligatures/columns, source selectors, region preservation |
| Browsers | Firefox and Chromium: edit, wait for visibly changed PDF text, edit again, reload, and verify a separate reader's artifact |
| Performance | Cold and warm timings, long-paper render time, memory across repeated edits, no growing worker queue or leaked PDF resources |

Use independent expectations: inspect PDF text and geometry against fixtures,
and visually compare a small set of pages with the same pinned Typst compiler
and inputs. Do not rely solely on PDF byte equality or DOM snapshots.

Run the repository's relevant Rust and JS checks, build both WASM renderers,
then run `make test` and the browser checks extended to cover Typst PDF in
Firefox and Chromium. Record measured results and remaining limitations in this
document. If full-page PDF rendering misses the measured latency budget, profile
compile versus pdf.js time before adding viewport rendering or other complexity.

Ship the server, shell, workers, and digested WASM as a consistent build. Keep
old source and stored artifacts readable during rollback; no destructive data
migration is required. A temporary deployment-level rollout switch is acceptable
for validation, but the intended product behavior is a single Typst PDF default.

Update README, history/retention/sync/LaTeX specs where their claims change,
examples, build instructions, and outdated source comments. Remove `typst-html`
from the active build once compatibility paths no longer require it.

## Suggested implementation sequence

1. Backend spike, corpus, and baseline measurements.
2. Typed engine output, PDF export, binary ABI, and worker support.
3. Reader capabilities and complete live PDF preview lifecycle.
4. Artifact storage, CLI publishing, history, and migration handling.
5. Annotation compatibility and navigation fixes found by the corpus.
6. Cross-browser/performance acceptance, documentation, and default switch.

Steps 2–5 should remain reviewable independently but the default should not
switch until reader access, old comments, and publishing work end to end.
Completion means the user can edit a Typst project repeatedly in Firefox or
Chromium, see faithful PDF updates, recover from errors, retain annotations,
and share the correct stored rendering with someone who never loads Typst.
