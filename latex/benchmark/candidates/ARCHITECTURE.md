# Provisional direction for Komodoc's browser TeX runtime

The best current hypothesis is an existing WASM TeX foundation with a
Komodoc-controlled release pipeline, package catalog, and browser orchestration.
The engine supplier remains an evaluation question. The product should expose a
dependable compilation environment rather than a menu of experimental runtimes.

## What Komodoc would own

1. **One coherent release identity.** Pin engine sources/toolchain, compiled
   modules, formats, package archives/database, bibliography tools, font indexes,
   and font maps. Produce native reference outputs from the same snapshot. Keep
   a project's chosen release and engine in project metadata, shared by authors.
   A newer package from a live CTAN mirror must never silently enter an old release.
2. **Static delivery.** Build a broad runtime file catalog ahead of time, with
   content hashes, filename aliases, and package dependencies. Start with a small
   core for the selected engine; deliver fonts and additional packages in modest
   compressed groups as needed. Core selection should follow measured dependency
   traces and common templates. A missing optional configuration file is not a
   request to download the entire distribution.
3. **Preparation at build time.** Generate formats using their matching WASM
   engines, as well as font-name databases, font maps, language configuration,
   and common dependency sets. TinyTeX's package lists can seed this selection;
   they do not supply the browser engine or define the compatibility limit.
4. **A persistent resource cache.** Cache immutable compiler/package bytes across
   reloads, rehydrate the worker's filesystem, and record the files each project
   used for its next prefetch. Keep mutable auxiliary files isolated per project.
   Preserve correctness when storage is unavailable or evicted. Browser storage
   is best-effort unless persistence is granted; it cannot be promised forever.
   [Storage behavior](https://developer.mozilla.org/en-US/docs/Web/API/Storage_API/Storage_quotas_and_eviction_criteria).
5. **A complete compile pipeline.** Honor explicit engine selection, retain the
   correct auxiliary files between passes, run real BibTeX/Biber/makeindex when
   their inputs change, and rerun TeX until references settle with a bounded pass
   count. Biber should be a separately loaded component with a compatible
   biblatex version. A limited JavaScript bibliography implementation does not
   meet this requirement.

## The author experience

Open a project and immediately display its stored PDF. Prepare the matching
compiler in the background, with clear progress when resources must download.
For a new project without an explicit requirement, choose a supported default;
honor project settings and engine directives before applying detection heuristics.

Compile after a typing pause while keeping the editor responsive and the previous
PDF visible. Publish only the newest applicable result. Diagnostics, reliable
source/PDF navigation, and download progress matter more than exposing upstream
distribution names. Cache/offline controls belong in browser settings; engine and
TeX release belong in project settings.

## What the measurements currently support

The original benchmark proves that local edit latency can be usable on one
document, and that the existing mirrors/adapters have substantial compatibility
and reload problems. It does not identify a winning engine. The
[packaging audit](PACKAGING.md) confirms that the TeXlyre bundles contain
overlapping trees and multiple engines' resources; it does not establish a
minimum achievable download size or browser memory requirement.

The first implementation priority should be a correct ACM paper and a real Biber
workflow from a coherent snapshot. Next expand to the full eight-case corpus,
including the long thesis and Unicode fonts. Then optimize initial downloads and
cache rehydration. Advanced preamble/heap snapshots can follow after correctness
and invalidation rules are established.

Separate-build modules and incremental resource delivery are preferred because
they let a simple paper avoid the cost of other engines and large font families.
The specific boundary between single files and small archives should follow
network measurements. Deployment remains static hosting or object storage/CDN;
compilation stays in the browser, including bibliography processing.

## What still determines the choice

- Whether a candidate reproduces native output for the corpus without changing
  its source or silently reducing bibliography functionality.
- Whether engine and package artifacts can be rebuilt and served independently
  of the candidate's live infrastructure.
- Whether the unresolved failures are tractable adapter/resource fixes or require
  substantial engine-port maintenance.
- Memory and behavior in Firefox and Safari, plus repeated cold/reload tests on
  realistic networks. Concurrent exploratory runs should not be used to rank
  candidates by speed.

Using upstream source while building/hosting the artifacts ourselves is one form
of independence. Avoiding TeXlyre-derived source entirely is a separate constraint
and may particularly affect the choice of Biber port.
