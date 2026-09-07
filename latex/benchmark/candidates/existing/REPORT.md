# Existing adapter failure diagnosis

This is an isolated diagnosis of the checked-in SwiftLaTeX/BusyTeX/TeXlyre
adapters. The scripts and candidate copies here do not modify `web/src`, the
shared mirror, `latex/benchmark/browser.mjs`, or benchmark results. Browser
experiments used the requested local ports 8703 (HTTP) and 9703 (Chromium
driver). Timings are exploratory single runs under concurrent repository work;
the compatibility outcomes are the useful measurement.

## ACM / xkeyval

Baseline TeXlyre BusyTeX `acm-conference` fails at:

```
(/tmp/texlive_remote/26_xkeyval.sty
Package: xkeyval ...
! I can't find file `xkeyval'.
```

The adapter's remote resolver fetched `xkeyval.sty` but did not make the same
file's sibling `xkeyval.tex` available to TeX's bare `\input xkeyval` search.
This is an adapter/resolver protocol failure. The ACM class and pdfTeX engine
are already running, so this is not evidence that ACM or pdfTeX cannot work in
the browser.

`experiment.mjs` ran the following controlled project-tree variants against
the same TeXlyre release and mirror:

| Variant | Result | Next diagnostic |
| --- | --- | --- |
| Original tree | failed | bare `xkeyval` missing |
| Add project `xkeyval.tex` | failed | bare `xkvutils` missing |
| Add project `xkeyval.tex`, `xkvutils.tex`, `xkvtxhdr.tex`, `keyval.tex` | PDF produced | resolver chain repaired for this case |

The injected files were copied from the current package mirror only to expose
the lookup defect. This does not prove a coherent production snapshot: the
mirror combines package assets and engine releases from different provenance,
and the project injection bypasses normal package resolution entirely.

The portable implementation direction is to normalize bare TeX lookups in the
remote-file bridge (probe the exact name plus the format's valid extension
variants, and cache the returned bytes under the name TeX will search), or to
ship the complete matching xkeyval package closure in the engine's bundle.
Blindly adding one file to the mirror is insufficient, as the controlled chain
demonstrates.

## Real Biber

Baseline TeXlyre `biber-sorting` fails before Biber runs:

```
Error: No biber module factory found. Ensure biber.js is loaded.
```

The cause is visible in the adapter copies. `busytex_biber.js` constructs
`BusytexBiber`, which later looks for the Emscripten factory named `biber`.
The shared `busytex.js` module loader exports only `BusytexPipeline`,
`BusytexBiber`, and `busytex`; the module-local `biber` factory from
`biber.js` is never copied to `self`. Therefore TeXlyre's pipeline receives
the Biber request but has no constructible backend.

`biber-experiment.mjs` runs a candidate-only copy in a module Worker with one
change: expose `biber` in the loader's global export list. Against the same
TeXlyre assets and `91-sorting-schemes.tex`, the candidate produced a one-page
PDF and its log contains both `$ biber 91-sorting-schemes.bcf` and
`$ biber --output-format=bbl ...`. It contains no “No biber module factory”
error. This proves the loader fix reaches actual Biber, not a BibTeX
substitution, but it is only a partial fix: `pdftotext` compared with the native
reference shows `Ecclésiastique` rendered as `EccleÌ�siastique` and `Über ...
Götter` as `UÌ�ber ... GoÌ�tter`, with corresponding U+0081/U+0088 missing-glyph
warnings. The candidate PDF therefore remains `needs-review`; Biber backend
availability is fixed while byte/encoding fidelity is not established.

## Recommendation

The two defects have small, portable adapter fixes, but the release/mirror
boundary remains the larger risk. The ACM experiment needed a transitive bare
file closure, and the benchmark notes already show package/kernel and
engine-directive mismatches. A mature TeX Live engine with a versioned,
coherent mirror and a narrow resolver shim has a smaller maintenance surface
than importing a young whole framework whose package catalog, engine assets,
and backend loading still require local glue. Before production, pin one
TeX-Live-year snapshot, derive the package catalog and engine formats from that
same snapshot, fix bare-name resolution, export the Biber factory, and rerun
the full corpus with the normal worker harness.

## Files

- `experiment.mjs`: ACM resolver experiment and results in ignored JSON.
- `biber-experiment.mjs`: worker-based candidate Biber experiment and ignored
  JSON result.
- `candidate/`: isolated copies of the adapter files; only its `biber` export
  line differs from the checked-in source.
- `candidate-server.mjs`: isolated static server for the candidate worker.
