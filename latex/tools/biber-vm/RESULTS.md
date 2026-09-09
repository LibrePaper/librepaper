# Biber VM smoke test results

Date: 2026-09-07T05:21:21.841Z
VM release: `99651f6da4a6c480`
Fixture: hybrid-validation fixture (official biblatex example 91-sorting-schemes.tex); local .bcf control-file version 3.11

## bcf-version compatibility check

- .bcf control-file version in the fixture: `3.11`
- Local TeX Live 2025 biblatex \blx@bcfversion: `3.11`
- Browser release bibliography.control_file (manifest.json): `3.11`
- Match: true
- The .bcf fed to the guest was produced by the developer machine's local TeX Live 2025 biblatex, not by an actual browser-release pdflatex run (that would require driving the full browser engine headlessly, out of scope for this smoke test). If browserReleaseBcfVersion differs from localTexLiveBcfVersion this result does not establish browser-release compatibility.

## Timings

| Measurement | Value |
| --- | --- |
| Boot time | 2.15 s |
| Boot bytes fetched | 12.20 MB |
| Cold Biber run | 24.37 s (exit 0) |
| Cold run bytes fetched | 143.89 MB |
| Warm Biber run (unchanged inputs) | 26.96 s (exit 0) |
| Warm run bytes fetched | 143.89 MB |
| Total bytes fetched (lifetime) | 96472.11 MB |
| .bbl size | 19419 bytes |

**Note on the lifetime total:** it is far larger than boot+cold+warm summed. The
per-phase counters above (`counters.bytes` snapshotted immediately before/after
each guest command) are accurate for what each phase's *foreground* command
waited on, but v86's own 9p client keeps issuing additional HTTP range
requests against `/vm/objects/*` in the background after a guest command
already returned (observed: this smoke harness disables HTTP caching with
`Cache-Control: no-store` to make the counters meaningful per phase, so every
one of those background range reads is a full re-fetch with nothing cached).
This inflates the lifetime total far past the 109 MB the release actually
contains on disk and is a real (if surprising) v86/9p behavior worth noting,
not a bug in this script's request counting -- but it should not be read as
"the browser would download N x the guest size for every job." A production
static mirror serving these objects as content-addressed and immutable would
let the browser's own HTTP cache eliminate the repeat fetches; this harness
intentionally does not, to keep the per-phase counters trustworthy.

## Assertions

- [x] biber --version reports 2.21
- [x] biber --version exit 0
- [x] cold biber exit 0
- [x] warm biber exit 0
- [x] .bbl non-empty
- [x] at least one Unicode name found in .bbl
- [x] bbl contains "Ecclésiastique" byte-exact
- [x] bbl contains "Über das Wesen der Götter" byte-exact

## Result

PASS


