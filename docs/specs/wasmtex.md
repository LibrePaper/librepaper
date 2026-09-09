# SPEC: WasmTex compilation with local and Biber VM fallbacks

Status: implemented, with three exceptions. Milestone 1's independent
reproduction covers pdfTeX and BibTeX only (in the wasm-latex repository);
XeTeX, LuaTeX, bibtex8 and makeindex are still mirrored builds and the
manifest says `reproduced: false`. Milestone 7's Firefox, Safari and
memory-constrained matrix has not been run. The legacy SwiftLaTeX package
route and its mirror tools have not been removed. The rest of this page is
the design the code in `web/src/lib/latex/`, `crates/librepaper/src/local/`
and `latex/tools/` cites.

Date: 2026-09-07.

## Decision

WasmTex is LibrePaper's sole browser LaTeX compiler foundation. LibrePaper owns a
pinned distribution of its engines, matching formats, packages and fonts,
plus the controller that runs them. The product offers one browser
distribution, with pdfLaTeX, XeLaTeX and LuaLaTeX selected according to the
project's requirements.

TeX and BibTeX run in the browser. When a document requests Biber, or browser
compilation fails, LibrePaper checks whether the local LibrePaper app is available.
The app searches for a suitable local installation and tries the required
work on the author's machine.

If no usable local route is available and browser TeX can produce the Biber
inputs, run real Biber in an on-demand virtual machine inside the browser.
This is the last bibliography fallback and requires no local installation.
WasmTex still performs all browser typesetting.

There are two local operations:

1. Run Biber and return its bibliography output so browser TeX can continue.
2. Compile the complete project with native TeX when browser compilation
   fails, or when a Biber-only operation cannot work with the browser release.

The deployment never compiles documents. An installation is not required for
ordinary browser compilation, VM-backed Biber, editing source, or viewing
stored PDFs. Complete native TeX fallback still requires local tools.

This decision replaces the distribution-selection model and the earlier idea
of running the browser compiler through an extra locally installed JavaScript
runner. No other browser TeX compiler is part of this plan. Browser emulation
is limited to the final Biber fallback, not a second typesetting environment.

## Product behavior

Opening a LaTeX project should lead directly to its pages. A reader with a
stored PDF sees it immediately and downloads no compiler. An author who needs
to compile gets automatic loading, useful progress and an editable source
pane throughout.

Remove the distribution chooser and its book icon. Settings contains:

- Project engine: Automatic, pdfLaTeX, XeLaTeX or LuaLaTeX.
- The project's pinned browser release, with an explicit update action.
- Local compilation connection status, connection controls and diagnostics.
- Compiler cache size and a clear-cache action.

An unobtrusive compile status distinguishes browser and local output and
identifies VM-backed bibliography work when used. A
successful local fallback should explain why it was used without interrupting
editing. Compiler errors remain in Diagnostics with source locations where
available. Preserve both attempts' logs when a browser failure led to a local
attempt.

Do not ask users to choose packages, distributions or download bundles.
Do not require confirmation on each compile after local access has been
connected and authorized for the project. Browser or operating-system
permissions may still require a first connection interaction.

### Routing

| Situation | Required behavior |
| --- | --- |
| Browser TeX and BibTeX succeed | Show the browser PDF. Do not contact the local app. |
| Biber is required | Check the local app; run compatible local Biber and resume browser TeX. |
| Local Biber cannot be used with the browser release | Try a complete native build if suitable local TeX exists; otherwise use the compatible browser Biber VM if browser TeX produced usable inputs. |
| Browser initialization, resource loading or compilation fails | Check the local app and try a complete native build. |
| Local app is available but the required tool is missing | Use the Biber VM if only bibliography work remains; otherwise explain the missing tool and provide setup/path controls. |
| Local app is unavailable and Biber is needed | Start the browser Biber VM when its capability checks pass, then resume WasmTex. Keep local connection controls available. |
| Local app is unavailable and browser TeX itself failed | Preserve the last valid preview and offer connection/setup controls. The Biber VM cannot replace a failing TeX engine. |
| Biber VM cannot load or run | Explain the VM limitation and offer local setup/retry controls. Preserve the last valid preview. |
| Native build also fails | Show its diagnostics alongside the browser failure. Stop retrying that revision automatically. |
| Job is canceled or superseded by an edit | Discard it. Cancellation is not a failure that triggers fallback. |

A source syntax error may fail in both environments. It still receives at
most one automatic full native attempt when local compilation is available;
the controller must not guess that a browser failure is necessarily portable.

BibTeX is never a silent replacement for Biber. No source rewriting changes
the bibliography backend merely to produce a PDF.

VM initialization is an automatic last fallback once local availability has
been checked; there is no VM/distribution chooser. A declined local connection
does not prevent browser Biber from running. Do not wait indefinitely for app
installation or repeatedly request local permission before trying the VM.

## Existing integration points

Keep the project input shape used by the current browser compiler:

    { main, texts, assets }

Texts are strings and assets are bytes. Relative paths, multiple source files,
project-local classes/styles, bibliography databases and figures must survive
both browser and local compilation.

Preserve the public result fields expected by the reader: PDF, SyncTeX, raw
log and normalized diagnostics. Extend results with job identity, backend,
engine and release/tool provenance.

Implementation primarily touches:

- web/src/lib/latex.js and web/src/lib/latex/: orchestration, worker adapter,
  resource/cache handling, diagnostics and local connection.
- web/src/components/Reader.svelte, Settings.svelte and LatexCard.svelte:
  loading, preview, local fallback and removal of distribution selection.
- latex/tools/: release building, resources, manifests and static mirroring,
  including the separately loaded Biber VM runtime/image.
- crates/librepaper/src/cli/mod.rs and new local compilation modules: commands,
  discovery, authenticated local service and native execution.
- Existing history/rendering structures where compiler settings and
  provenance need to be represented.
- Existing browser-test tooling for acceptance checks.

File boundaries can be adjusted during implementation. The behavior and
interfaces in this spec are the contract.

## Browser distribution

### Starting point

Start from the WasmTex source revision and 2026 release evaluated during
spec development (the evaluation tree was removed once these were pinned;
the reproduction record is
[wasm-latex's reproduction notes](../../../wasm-latex/docs/reproduction-2026-pdftex.md)):

- Wrapper/source revision: 44c5861fcdf729838205b00b96ac9509bc7fb677.
- Engine release: 2026-8b7946970153c52e.
- Evaluated package snapshot: 2026-ba38749b8714505a.

Treat these as the first release inputs, not moving dependencies. Verify the
artifacts and preserve their corresponding source and licence notices.
The production browser must obtain resources from LibrePaper's configured mirror;
it must not depend on an upstream project's live service.

Use the engine layer rather than importing WasmTex's editor or complete
headless application pipeline. LibrePaper controls project state, bibliography
runs, cancellation, freshness, diagnostics and publishing.

The upstream release above was the first mirrored input and remains the
comparison baseline. Engines now come from the wasm-latex repository, which
builds them from pinned TeX Live sources, generates their formats, and
stages a release carrying its notices and receipts; `latex/tools/wasmtex.mjs
--release` imports a staged release by its manifest digest. Maintain only
the downstream patches that acceptance tests demonstrate are necessary.

### Coherent releases

A release is an immutable combination of:

- Engine WASM binaries, generated glue and worker controllers.
- Formats generated by the exact binaries that will load them.
- A pinned TeX package tree, fonts, maps, hyphenation and lookup indexes.
- Required support data such as ICU.
- A manifest recording sizes, digests, source/build identities and licences.
- The tested bibliography versions, local-Biber compatibility information and
  compatible Biber VM release identity.

The TeX Live year alone is not a compatibility identity. Do not mix a new
kernel or package tree into an existing engine release. Native format dumps
must not be substituted for WASM-generated formats.

TinyTeX can supply package selection ideas or a pinned input tree. It is a
build-time resource source, not a required user installation or an additional
browser backend.

Build and validate one release atomically. Publish immutable assets first,
then update the release catalog. Retain supported older releases so opening
an existing project does not silently change its typesetting environment.

### Loading and caching

Load only the selected engine and necessary helpers. Prepare a compact
initial resource set using the corpus; fetch the remaining files or small
package groups as needed. Build-time work should remove serial network
round trips for common documents.

The resource catalog must resolve bare filenames, supported extensions,
package dependencies and font files while preserving TeX's relevant lookup
precedence. Project-local files override distribution resources where TeX
normally permits it. Duplicate basenames must not silently resolve to an
arbitrary file.

An unresolved file produces a bounded lookup and an actionable failure.
There is no fallback that downloads an entire distribution.

Persist verified resources in browser storage, namespaced by release digest.
Request persistent storage when appropriate, tolerate refusal and recover
from eviction. Corrupt entries are discarded and refetched; transient network
errors do not become permanent missing-file cache entries.

Keep project files and generated auxiliary state separate from shared package
caches. Closing a project must not expose its files to another project.
Clearing compiler resources must not delete source documents.

Offer offline readiness for the resources a project has actually used.
Do not label a project fully offline-capable while required resources are
missing. Download progress must identify its measured scope rather than
inventing a total for unknown future package requests.

## Project configuration and identity

Engine and browser release are project settings; availability of local tools
is specific to the author's device.

Engine selection follows this order:

1. Explicit project engine setting.
2. A recognized engine directive in the main source file.
3. Conservative detection of known engine requirements.
4. pdfLaTeX when there is no stronger requirement.

Accept only recognized engine names. A directive never becomes a command
line supplied by the document. Respect a requested engine during native
fallback; do not quietly substitute another engine.

Persist the main-file choice, requested engine and pinned browser release
with the project. Extend the canonical project/checkpoint representation as
needed so a change in compile settings cannot accidentally reuse an artifact
identified only by unchanged source text. Preserve old checkpoint identities
when introducing optional fields.

Every compile receives an immutable snapshot and a job identity containing:

- Project identity and source/assets/settings digest.
- Resolved main file and engine.
- Browser release.
- A monotonically increasing local generation.

Generated results carry that identity back unchanged. A result for an older
generation may be retained as an explicitly older preview, but must never
replace a newer successful result or be published as current.

## Browser compilation controller

Keep one running job and one queued snapshot per project. The queue always
contains the newest snapshot. Preserve the existing 1.5-second debounce
initially; tune it from measurements after integration. Manual Compile starts
the latest snapshot immediately.

Run engines in workers. Reuse initialized engines and safe cached resources,
but restore their execution state correctly between passes. Handle project
file edits, removals, renamed roots and deleted generated files explicitly.
Do not let a PDF or XDV left by a prior run turn a failed run into success.

The ordinary sequence is:

1. Stage the snapshot and valid generated state.
2. Run the selected TeX engine.
3. Inspect actual auxiliary files and diagnostics for bibliography/index work.
4. Run browser BibTeX when required, or request Biber locally and then through
   the browser VM when no usable local route is available.
5. Stage generated output and repeat TeX until references settle.
6. Return the validated PDF, SyncTeX, logs and provenance.

Follow included auxiliary files as well as the main auxiliary file.
Support project-local bibliography styles and nested paths. Avoid using
source regexes as the sole authority for required helper work.

Convergence includes relevant auxiliary files and rerun diagnostics, not just
the existence of a PDF. Use a bounded pass count, initially eight TeX passes,
and a configurable total job deadline. Exceeding either is a reported failure
eligible for local fallback.

Bibliography caching must account for the control/auxiliary inputs, database
bytes, styles, configuration and tool/release identity. A prose edit can reuse
a still-valid bibliography. A citation, database or style change must
invalidate the affected result. False reuse is worse than a conservative
extra bibliography run.

Changing source, engine or release while a helper is running must not attach
its output to the wrong snapshot. Preserve raw bytes across every worker and
local boundary, including Unicode bibliography output.

## Local app detection and connection

### What detection means

The browser can discover a reachable local LibrePaper service. It cannot
reliably enumerate installed applications or prove that an application is
absent. An installed but stopped app, a denied browser permission and a
missing app may all be unreachable.

On a Biber request or browser failure:

1. Reuse a healthy authenticated local connection if one exists.
2. Otherwise perform one bounded probe of LibrePaper's documented loopback
   endpoint, subject to the browser's permission rules.
3. If available, establish or resume the authorized project connection and
   request capability discovery.
4. If unreachable, continue through the browser Biber VM when eligible.
   Otherwise show "Local LibrePaper is unavailable" with Open/Connect,
   Retry connection and setup controls.

Do not scan ports or the local network. Use a single documented endpoint,
with a configurable address for users who run it differently. Exact port and
platform activation mechanisms are implementation details to settle when the
bridge is built.

An Open LibrePaper action may use an application protocol where registration is
supported. It must be user-initiated when required by the browser/OS. A
command-line start/connect instruction remains available for installations
without desktop integration.

Do not repeatedly probe or launch the app on every keystroke. Cache a negative
result for the current failure episode; retry on explicit connection retry,
an app connection event or a reasonable bounded backoff.

### App integration

The existing LibrePaper executable owns the local service. Add commands
equivalent to:

    librepaper local start
    librepaper local status
    librepaper local doctor
    librepaper local disconnect

Final command naming can follow the existing CLI conventions. No separate
Node, R or emulator installation is required. The local app uses native
tools already installed on the machine.

Desktop installation should provide app activation and a clear way to keep
the local service available. Starting the public LibrePaper deployment must not
implicitly enable a native compilation service.

### Connection boundary

Bind the service to loopback only. Use explicit origin/host validation and
authenticated requests scoped to the connected project. Establish access
through a deliberate initial pairing/connection; retain it so subsequent
fallbacks are automatic. Support revocation and expiration.

An unauthenticated health response reveals only enough to identify the
service and negotiate the protocol. Tool paths, project contents and jobs
require authentication. Do not reuse server editor credentials as a
general-purpose local execution token.

The bridge accepts structured compilation operations, not shell commands,
executable paths from documents or arbitrary filesystem reads. Protect it
against unrelated websites, cross-origin requests and DNS rebinding.

Use browser-supported local access permissions. Do not require disabling
browser security, installing a root certificate or relying on a permission
bypass. Probe denial is a recoverable connection state.

Exact pairing transport and OS activation details are to be implemented and
tested in the local-bridge milestone, not researched as prerequisites to this
specification.

## Local tool discovery

Discovery is performed by the installed app, never by browser code.

Search configured user paths first, then PATH and conventional installation
locations. Recognize existing TeX Live, TinyTeX, MacTeX and compatible native
tool installations. Preserve paths with spaces and non-ASCII characters.
Resolve executables to explicit paths and inspect versions with bounded
commands.

Report capabilities individually:

- pdfLaTeX, XeLaTeX and LuaLaTeX.
- BibTeX/BibTeX8, Biber and makeindex.
- Required fonts/resource availability where it can be checked.
- Native execution/confinement support.

An installed LibrePaper app does not imply an installed TeX distribution.
Finding Biber does not imply finding a complete TeX installation.

Cache discovery results and refresh on explicit rescan, changed configured
paths or a missing/changed executable. Keep full filesystem paths local;
the browser normally needs tool names, versions and usability.

The first implementation discovers and uses existing tools. It does not
silently install packages, modify PATH or update the user's TeX installation.
If setup is needed, explain the missing capability and provide instructions
or a path override.

## Local Biber operation

Detect an explicit Biber requirement early so connection discovery can
overlap with initial browser compilation. The actual BCF and compile results
are authoritative when source-level detection is inconclusive.

When browser TeX produces a usable BCF:

1. Package that BCF, bibliography files and applicable project configuration
   for the same immutable snapshot.
2. Ask local LibrePaper to find a Biber compatible with the browser release's
   biblatex/control-file format.
3. Run it in an isolated project workspace.
4. Return BBL bytes, BLG/log output, exit status and tool identity.
5. Cache the result under its complete bibliography input identity and
   continue browser TeX.

Version compatibility must be checked; a matching major version alone is
insufficient. Use release compatibility metadata and actual tool diagnostics.
Never convert or patch BCF/BBL version headers to force acceptance.

If browser TeX cannot produce a BCF, local Biber is incompatible, or the Biber
stage fails, attempt a complete native build when suitable local TeX exists.
That build uses its own matching macro packages, formats and bibliography
tools. It must not consume browser-generated auxiliary files blindly.

If no usable local route exists and the browser produced valid Biber inputs,
try the browser Biber VM described below. Otherwise preserve the last valid
preview and identify that the current bibliography could not be rebuilt.
Existing valid BBL output may be reused only while its dependency identity
remains valid.

## Browser Biber VM: final bibliography fallback

### Eligibility and ordering

Prefer a connected local installation. Use the VM when the app is absent,
stopped, unreachable or not authorized, or no compatible local tool route
exists. A successful browser-only document must never download or start it.

The VM requires a valid BCF and its complete bibliography/configuration
inputs from a successful-enough WasmTex pass. Mere existence of a partial BCF
after a failed run is insufficient. If browser TeX cannot produce those
inputs, only complete native TeX can provide the fallback in this plan.

The order is browser TeX/BibTeX, then usable local Biber or native compilation,
then VM Biber when local execution is unavailable and bibliography processing
is the remaining requirement. A real Biber input error is a diagnostic, not
a reason to run identical invalid input through every backend.

Once selected, retain the VM for bibliography work during the current session
while the local route remains unavailable. An explicit app connection can
switch subsequent bibliography jobs to local Biber. Do not interrupt an
in-flight successful job merely because a preferred backend appeared.

### Runtime and image

Ship one selected VM implementation and a minimal guest containing Biber,
its interpreter/libraries and required runtime data. Do not put a complete
TeX distribution in the guest or run TeX there. Build this image ahead of
time and pin Biber to the browser release's biblatex/control-file requirements.

Select the emulator during implementation, using the existing VM experiments
as evidence. Its deployment licence, browser support, startup cost and memory
requirements must fit LibrePaper. No vendor choice or further research is needed
to accept this plan.

Serve the runtime and image through the configured static mirror. Record
digests, sizes, build recipe, source/licence provenance and compatibility in
the release manifest. Load neither the emulator nor the image on the ordinary
TeX/BibTeX path. Use compressed/chunked or range-based image delivery as
appropriate, with integrity verification and persistent caching.

Use a worker and a read-only base image with separate per-project writable
state. The guest receives only the submitted job inputs, no local app token,
server credentials or host filesystem. Network access is restricted to
controlled resource delivery; Biber jobs cannot fetch arbitrary remote input.

### Job contract and lifecycle

Use the same logical Biber request/result contract as the local operation:
immutable snapshot identity, BCF, databases and configuration in; BBL bytes,
BLG/logs, status and backend/tool identity out.

Run the actual Biber executable. Preserve raw bytes and Unicode, nested input
paths and output identity. Write the returned BBL into WasmTex and finish
the usual TeX passes; the VM never returns the final PDF.

Keep one bounded VM job active, with only the latest pending snapshot per
project. Isolate projects and remove canceled/private working files. Reuse a
warm VM and valid bibliography results so prose edits incur no VM execution.
Include image, Biber and configuration identities in bibliography cache keys.

Cancellation, timeout or a poisoned guest must stop the work and retire the
affected state. A late BBL must not attach to a newer snapshot. Release guest
memory after inactivity or under memory pressure; preserve verified static
resources for the next use. A denied persistent-cache request is recoverable.

### Experience and limits

Show "Preparing browser bibliography support" during initial loading and
"Updating bibliography in browser" during Biber execution. Preserve the
preview and editor responsiveness. Explain that connecting local LibrePaper can
speed up bibliography work, without requiring installation to continue.

Measure first-use download/startup separately from warm Biber execution.
Expect this route to be slower than native execution; do not inherit timing
promises from the ordinary browser TeX path. Capability checks and tested
resource budgets must prevent unsupported browsers/devices from entering an
unbounded load or repeated crash loop.

If the VM is unsupported, unavailable, exceeds limits or returns a real
compilation error, stop automatic attempts for that snapshot and show the
specific limitation or diagnostic. Offer local connection/setup and explicit
retry. There is no subsequent emulator or server fallback.

## Complete native fallback

A browser compilation failure triggers local capability discovery and one
native attempt for the same snapshot. This includes engine initialization,
resource failures, unsupported capabilities, crashes, timeouts and TeX errors.
Cancellation and obsolete results are excluded.

The local app stages source and assets into a private per-project workspace,
uses the requested native engine, and runs the required bibliography/index
passes until convergence or the deadline.

Implement a bounded native controller in LibrePaper. Do not require latexmk,
Node or R merely to coordinate executable calls. If a future implementation
uses an available helper, it must preserve the same configuration, execution
and dependency rules without becoming an additional required installation.

Return the PDF and SyncTeX together with all diagnostics and provenance.
Normalize temporary workspace paths back to project-relative source paths.
Record native engine, distribution/kernel and bibliography tool versions;
native output must not be labelled as produced by the browser release.

A local build may differ typographically because its packages or fonts differ.
Show its provenance in compile details, preserve the engine choice, and do
not imply byte-for-byte equivalence with browser compilation.

After a successful complete native fallback, keep that project on the native
route for the current editing session. This avoids repeating an expensive
known browser failure for every edit. Provide "Try browser compilation" to
reset the route. A relevant engine/release configuration change or a new
session also resets it. This device-local route is not a shared project
setting.

Biber-only fallback, whether local or VM-backed, keeps browser TeX active.

Track attempted stages per snapshot. Browser -> local Biber -> browser
continuation -> full native is allowed when necessary, but full native is
attempted at most once automatically for that snapshot. A native failure
does not bounce back into an automatic browser retry loop.

The Biber VM is attempted at most once automatically for a bibliography
input identity when no usable local route exists. It may be followed by
WasmTex continuation, but not by another automatic cycle through the same
failed backends. An explicit retry, changed inputs or a newly available local
capability can begin another attempt.

## Native execution boundary

Collaborators can edit the source that a local compiler executes. Connecting
the app authorizes compilation for the chosen origin/project, not every
document the account can access.

Use argument arrays and app-selected executable paths. Disable shell escape;
ignore executable build instructions from project configuration. Reject
absolute/traversing paths and escaping links when staging a project. Keep
credentials and the app's own files outside the compiler workspace.

Limit elapsed time, output size, logs and concurrent processes. Cancellation
must terminate the whole job and its children. Compiler jobs must not fetch
arbitrary network resources or use package auto-installers without a separate
user action.

Use platform confinement to restrict writes to the job workspace, reads to
the project and selected runtime/resource roots, and network access. TeX flags
alone are not a filesystem sandbox. Validate the actual restrictions with
host-file access and process-execution tests.

The local execution milestone must implement and test this boundary on the
supported desktop platforms. If confinement is unavailable on a machine,
report that capability limitation rather than silently running unrestricted
collaboratively edited code. Platform API/library selection is implementation
work.

## Bridge protocol and lifecycle

The protocol is versioned and limited to discovery, compilation and results:

| Operation | Purpose |
| --- | --- |
| Health | Identify reachable LibrePaper and supported protocol versions. |
| Connect/disconnect | Establish or revoke origin/project-scoped access. |
| Capabilities/rescan | Report or refresh usable native tools. |
| Submit Biber job | Run bibliography processing on supplied inputs. |
| Submit TeX job | Compile a complete project snapshot. |
| Job status/results | Retrieve progress, bounded logs and output artifacts. |
| Cancel | Stop an obsolete or user-canceled job. |

Job requests include protocol version, project authorization, snapshot digest,
generation, requested operation/engine, relative input manifest and byte
digests. The app verifies received content against the manifest before use.
Requests cannot name arbitrary host files or supply an executable command.

Results echo job/snapshot identity and return status, diagnostics, output
digests and tool provenance. Preserve binary bytes exactly. Bound request
and response sizes; use binary transfers or bounded multipart payloads rather
than making unbounded base64 copies of whole projects.

Run at most one native compilation at a time initially, with bounded queued
work and fair handling of multiple tabs/projects. Drop superseded queued
snapshots. A lost client connection must not leave unbounded background work.
Private workspaces and project caches have explicit cleanup and size limits.

No job relay or execution endpoint is added to the public deployment in this
plan. The browser communicates directly with its local app.

## Stored renderings and collaboration

Use the existing rendering upload path for both browser and native results.
The browser receives local output and uploads it using its existing editor
authorization. The local app does not need server credentials for this flow.

Preserve the current rules:

- Only editors can upload rendered artifacts.
- Artifacts belong to the exact matching project/checkpoint identity.
- Existing size, quota, rate and retention limits apply.
- Routine publication follows the existing quiet-minute rule, including the
  existing first-render exception.
- Older PDFs remain visibly older when the source has changed.

Extend artifact metadata as needed for compiler settings and provenance.
PDF and SyncTeX must come from the same successful job. Do not publish new
PDF bytes paired with an earlier source map.

Generated intermediate files remain caches, not source edits. Do not put
BBL/AUX churn into collaborative text or history automatically.

Readers and other collaborators can view a stored rendering without installing
the app, including a PDF whose bibliography was produced in the VM. Authors
can regenerate Biber output using their own local capability or the browser
VM. Complete native builds require their own connected local capability;
another collaborator's app is not remotely commandeered.

## Failure presentation

Keep the current source and last successful preview available during loading,
fallback and errors. Status distinguishes:

- Loading browser compiler/resources.
- Compiling in browser.
- Checking local LibrePaper.
- Local connection needed.
- Running local Biber.
- Preparing browser bibliography support.
- Updating bibliography in browser.
- Compiling locally.
- Current preview ready.
- Compilation failed; previous preview shown.

Connection errors must not erase TeX diagnostics. Explain whether the local
app was unreachable, a required tool was missing, versions were incompatible,
or the native build itself failed.

A local success after a browser error is an ordinary successful preview with
local provenance. Retain the original failure in compile details for
troubleshooting without leaving the interface in an error state.

A successful VM Biber continuation is a browser PDF with VM-Biber provenance,
not a local compilation. Local unavailability does not remain an error banner
after the VM route succeeds.

## Migration

Replace browser distribution selection with automatic WasmTex initialization.
Migrate old browser-only distribution preferences to the new default and
remove obsolete chooser state. Preserve source, stored PDFs and history.

Retain compatibility with existing callers while replacing the compiler
adapter, then remove legacy browser adapters and their production manifest
entries. The evaluation benchmark tree was removed once the release was
pinned; its evidence remains in git history.
There is no production engine-selection fallback to another WASM project.

New projects receive the validated default WasmTex release. Existing projects
without a browser release pin receive one through the normal editable project
configuration path; readers do not mutate projects merely by opening them.
Release updates are explicit and can be reverted to retained versions.

Update docs/specs/latex.md and user documentation to point to this plan and
describe the new browser/local behavior.

## Implementation milestones

### 1. Own the WasmTex release

Mirror the selected 2026 artifacts, preserve source/licence provenance,
generate a coherent resource manifest and make all browser fetches use the
configured LibrePaper mirror. Reproduce pdfTeX/BibTeX and their formats from
pinned inputs. Establish the same release process for XeTeX/LuaTeX.

Acceptance: a clean build can reconstruct the release; no runtime dependency
on an upstream endpoint; missing resources fail without a giant bundle fetch.

### 2. Integrate the browser controller

Implement the WasmTex adapter, file lifecycle, helpers, convergence,
cancellation, release-scoped caching and result identity. Remove distribution
selection from the author flow and connect SyncTeX to existing viewer gestures.

Acceptance: the multifile, ACM and package/BibTeX corpus produces correct
citations and content; edits/deletions/custom styles do not reuse stale output.
The editor remains responsive throughout compilation.

### 3. Add local connection and discovery

Add the local app command/service, pairing, status, version negotiation,
capability discovery and platform activation where available.

Acceptance: reachable, stopped, missing and permission-denied cases produce
accurate states; unrelated origins cannot obtain capabilities or submit jobs;
no repeated prompts/probes during ordinary edits.

### 4. Implement native operations

Implement confined execution, local Biber transfer and the complete native
controller. Preserve byte fidelity, diagnostics, cancellation and matching
PDF/SyncTeX outputs. Support version incompatibility by trying complete native
compilation when possible.

Acceptance: Biber-only and full-native routes work with discovered tools on
Linux amd64/arm64, macOS Intel/Apple Silicon and Windows amd64, matching the
current LibrePaper release targets. Missing tools produce setup guidance, not
implicit installation.

### 5. Connect automatic fallback and rendering publication

Wire the routing table into the reader, including per-snapshot retry limits,
session-native routing after a successful fallback, and existing rendering
upload/freshness rules.

Acceptance: forced browser failures automatically use an authorized local
installation; double failures stop cleanly; obsolete results cannot replace
or publish over current output; readers need no compiler installation.

### 6. Add the final Biber VM fallback

Select and package one browser VM with a minimal, compatible Biber image.
Reuse the bibliography job contract, input invalidation and byte-safe transfer
rules. Add lazy loading, progress, persistent resource caching, cancellation,
project isolation and capability/resource limits.

Acceptance: without a reachable local app, the real-Biber corpus completes
through VM Biber and WasmTex, including Unicode, sorting, related entries and
bibliography edits. Prose-only edits reuse valid BBL output without running
Biber. Ordinary BibTeX projects fetch zero VM resources. VM failures stop
cleanly and preserve source and the previous preview.

### 7. Validate and ship

Run the browser, native and Biber VM acceptance matrices, finish resource prewarming
from observed dependencies, document setup and remove obsolete production
compiler paths. Release only engines/platform capabilities that pass their
tests; unavailable local capabilities remain explicitly reported.

The implementation phases may overlap where independent. Do not expand the
scope into evaluating additional browser compilers.

## Acceptance matrix

Required scenarios include:

- pdfLaTeX with classic BibTeX; biblatex explicitly using BibTeX.
- XeLaTeX and LuaLaTeX documents with appropriate fonts and packages.
- Biber citations, Unicode, sorting and related entries.
- Prose-only, citation-only, bibliography-only and style/configuration edits.
- Nested source/auxiliary paths, custom classes/styles, figures and file removal.
- Browser failure with local success, both failing, and local tools missing.
- Browser-release/local-Biber mismatch with successful complete native fallback.
- No local app with successful VM Biber and WasmTex continuation.
- Local tools missing/incompatible with successful compatible VM Biber.
- Zero VM downloads for ordinary TeX/BibTeX and zero Biber runs on unchanged
  bibliography inputs during prose edits.
- VM cold start, warm use, idle teardown, offline cached use, image corruption,
  unsupported browser/memory limits and cancellation during initialization.
- Switching between VM and newly connected local Biber without stale output.
- Local app stopped, restarted, disconnected or blocked by browser permissions.
- Rapid edits, cancellation, multiple tabs and stale local results.
- Reload, offline cached use, quota refusal, eviction and corrupt resources.
- Invalid paths, unauthorized origins, host-file reads, shell execution attempts
  and time/output exhaustion.
- Matching PDF/SyncTeX, navigation accuracy and stored-rendering freshness.

Use fixed public examples and small targeted fixtures. Compare final
bibliography content/order, citations, Unicode, extracted PDF text and page
counts against suitable native references. Add visual inspection where layout
or source mapping matters. A PDF's existence alone is not a passing result.

Record cold startup, warm prose edit, bibliography rebuild, transferred bytes,
peak memory, native fallback latency and VM startup/warm execution separately.
Test Chromium, Firefox
and Safari, with representative tablet/memory constraints. Do not infer
cross-platform performance from one desktop Chromium result.

Choose concrete product budgets during implementation using these
measurements. The evaluated small fixture's timings are evidence for the
approach, not promised latency for arbitrary projects.

## Boundaries

This plan includes one browser TeX compiler foundation, preferred local
Biber/full TeX fallback, a final browser Biber VM fallback, resource ownership
and the integration needed for a coherent UX.

It does not include server compilation, full TeX emulation, automatic native
TeX installation, arbitrary project shell commands, or a new TeX engine.
Full Overleaf compatibility is not assumed; unsupported workflows receive
diagnostics and the defined local attempt.

Further research into local transport, platform APIs and exact packaging
choices belongs to the corresponding implementation milestone. It must not
delay writing or accepting this plan.

## References

- [Engine reproduction from source (wasm-latex)](../../../wasm-latex/docs/reproduction-2026-pdftex.md)
- [WasmTex source](https://github.com/corca-ai/wasmtex)
- [Existing rendering implementation](../../crates/librepaper/src/server/figures.rs)
- [Browser local-network permissions](https://developer.chrome.com/blog/local-network-access)
- [TeX Live security/configuration changes](https://www.tug.org/texlive/bugs.html)
