# Candidate assessment

**Subsequent experiment:** the [TinyTeX-based v86 prototype](tinytex-v86/REPORT.md)
runs conventional Linux TeX/Biber in the browser and reproduces native text and
bibliography bytes on the multifile and Biber examples. Its cached edits took
12–33 seconds, so this establishes an additional compatibility route but not a
production-ready live-preview backend. The assessment below describes the
earlier direct-WASM candidate comparison.

The parallel experiments did not establish a production-ready replacement.
They support keeping TeX Live as the compatibility foundation, while treating
the browser port, package delivery, and compilation pipeline as engineering
work LibrePaper must either verify upstream or maintain itself.

## Evidence

| Candidate | What was demonstrated | Unresolved requirement |
| --- | --- | --- |
| [Siglum](siglum/REPORT.md) | Eight public cases attempted; multifile produced PDF and SyncTeX, with edits and cache reuse | That PDF omitted bibliography content; seven other cases failed. No BibTeX/Biber runner in the inspected revision; LuaLaTeX selection takes the XeLaTeX branch. Package proxy was disabled in this experiment. |
| [WasmTex](wasmtex/REPORT.md) | Multifile produced PDF, figures, and SyncTeX; cached reinitialization worked | Classic bibliography was unresolved. Documented full Biber requires a server; browser bibliography subset is insufficient. Other focused cases timed out without a result. |
| [Existing TeXlyre adapter, controlled experiments](existing/REPORT.md) | Injecting a missing package's filename closure let ACM produce a PDF. Exposing the Biber factory reached real Biber. | Injection bypassed normal resolution and did not establish a coherent release. Biber output had corrupted Unicode; PDF production is not a compatibility pass. |

These are different experiments, not a performance leaderboard. Siglum's
recorded first compile excludes initialization; WasmTex recreates its compiler
for the edit phase. Runs overlapped on the same machine. Neither can be ranked
against the original cold/edit/reload baseline on those timings. A failed
configuration also does not prove that its underlying TeX engine cannot work.

## Best current hypothesis

Build one LibrePaper-controlled TeX Live release from an existing, understandable
WASM port. Keep the browser-specific patch set small and reviewable. Own the
static resource catalog, matching formats/fonts, persistent cache, and thin
compile orchestration. Deliver engine and bibliography modules only when needed.
Provide real Biber for documents that require it; preserve project engine/release
settings. The [architecture note](ARCHITECTURE.md) describes the intended UX.

This is an architectural hypothesis, not a selected implementation. We have
not reproduced an independent engine build during this evaluation, demonstrated
the full corpus passing, or measured Safari/mobile memory. Downloading and
hashing published WASM assets is not the same as rebuilding them.

The next decision should follow one bounded build experiment: reproduce an
engine and matching resource snapshot from pinned sources, then compile the
ACM and real-Biber fixtures through normal package resolution with correct
text, references, and encoding. Record the actual patches needed. Expand to
all eight cases before selecting the product default. If this requires a
substantial compiler fork, revisit the compatibility scope and maintenance
budget before building more editor UI around it.

## What maturity means here

TeX Live's established engines do not establish the maturity of their browser
adaptations. Self-hosting protects against an upstream service disappearing;
it does not correct bugs or demonstrate maintainability. Adoption needs an
independent build, understood patches, coherent release inputs, automated
compatibility checks, and a workable update process.

Under the no-install/no-server-compilation constraints, the remaining cost is
client download/memory plus maintaining and delivering this browser runtime.
The [bundle audit](PACKAGING.md) identifies avoidable overlap and deployment
limits, but establishes neither a minimum download size nor a performance
target. No current result justifies promising complete Overleaf compatibility.

All work here is isolated evaluation tooling. Production adapters, offered
distributions, and the shared mirror were not changed by these experiments.

## Subsequent TinyTeX emulation experiment

The [TinyTeX-based v86 prototype](tinytex-v86/REPORT.md) runs conventional Linux
TeX and real Biber entirely in a browser worker. Across two recorded image
revisions, the multifile paper, official Biber sorting example, and ACM
conference sample produced PDFs matching native page counts and normalized
extracted text, with byte-identical bibliography files. This establishes a
working compatibility path without an installed user app.

The measured cached prose edits took 12–33 seconds, with no further downloads.
The first multifile PDF needed about 20 MB of compressed downloads despite a
roughly 692 MB static filesystem. This experiment therefore shifts the main
concern from initial distribution size to execution latency. Full v86 emulation
is not a suitable live-preview default on these measurements. A direct-WASM
TeX/emulated-Biber hybrid or another emulator needs its own measurements.

## Subsequent hybrid result

The [hybrid experiment](hybrid/REPORT.md) now passes the bibliography fixture
and all three edit cases against their native references. Direct WASM XeTeX
uses the frozen TinyTeX package tree; CheerpX runs Biber only when its inputs
change. A prose edit took 0.74 seconds without Biber. The first build took
49.50 seconds, and citation/data updates took 15.53–15.71 seconds, dominated
by roughly 13-second cached Biber commands.

This is evidence for a responsive ordinary editing path, with bibliography
processing still a bottleneck. It is a single-fixture Chromium experiment,
not a production selection or a complete compatibility result. The original
WasmTex package endpoint failed this case before switching to TinyTeX's frozen
packages; ownership of consistent resources remains part of the solution.
