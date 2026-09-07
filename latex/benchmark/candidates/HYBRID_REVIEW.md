# Hybrid implementation review

Intent: compile the bibliography fixture with direct WASM TeX and emulated
Biber, preserve correct output across edits, and measure the two runtimes
separately. This is an isolated experiment, not a production integration.

## Findings addressed during implementation

- The previous CheerpX bibliography-update check searched for literal spaces.
  The PDF contained the updated title split across lines. Whitespace-aware
  checks are necessary; the underlying Biber update had succeeded.
- The first hybrid draft selected Biber using a caller-provided phase flag.
  The pipeline needs to decide from actual bibliography inputs, so that a
  prose edit reuses output while citation and data changes invalidate it.
- The first draft used a project-loading API that flushes the engine cache.
  That would erase auxiliary files and undermine the intended edit workflow.
- The draft also wrote files only into the headless wrapper's virtual tree,
  then invoked its engine directly. Direct invocation must write to the
  engine's filesystem, including the returned bibliography bytes.
- A produced PDF alone is insufficient: nonzero process exits, stale outputs,
  unconverged references, and Unicode defects must fail validation.

## Required evidence before accepting the experiment

1. A native reference for each exact fixture mutation, with recorded versions.
2. Matching normalized PDF text in order, page counts, and bibliography bytes
   for initial compilation, prose edit, citation addition, and title edit.
3. Negative checks demonstrating that stale outputs do not pass the verifier.
4. No Biber invocation on the prose-only edit; an invocation for changed
   bibliography inputs. Separate initialization, process, and transfer timing.
5. A fresh browser run after reviewing the final implementation, with no
   competing benchmark, and explicit limits on performance conclusions.

## Scope and review method

Review focuses on the implemented data flow and measured outputs. Existing
staged benchmark work and production adapters are outside this change. The
code-reviewer skill supplied the checklist; no generalizable skill change is
needed for this review.

## Final review and evidence

The final parent-run benchmark passed all four per-phase native comparisons:
normalized PDF text in order, page counts, Unicode markers, and byte-identical
BBL files. The bibliography-only mutation leaves the TeX source unchanged.
The prose phase made zero Biber calls; the other phases each made one.
The native verifier self-test rejected stale citation outputs, and the browser
rejected an invalid TeX edit without returning the previous PDF.

Additional corrections during review included guest export permissions,
length-framed cache keys, separate Biber timings, and a switch to the frozen
TinyTeX package tree after the original package endpoint caused TeX errors.
Actual engine asset bytes are checked against the recorded WasmTex manifest.

No critical issues remain for this fixed-fixture experiment. The resource
resolver's simplified duplicate-name precedence, fixed project lifecycle,
limited browser coverage, and CheerpX deployment licensing prevent treating
it as a production integration. Those limits are explicit in the report.

The strongest implementation choices are the independent per-phase native
oracle, raw byte bibliography transfer, and content-based invalidation. They
made it possible to distinguish valid cached output from an accidentally stale
PDF and to isolate bibliography work from prose edits.

No author clarification is outstanding. Verdict: accept the isolated
experiment; do not merge it into production rendering as-is.

See [hybrid/REPORT.md](hybrid/REPORT.md) for the measured results.
