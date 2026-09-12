# Build tools and the local companion

Status: Proposed

## Purpose

Make LibrePaper's build choice explicit and extensible. Authors can render in
the browser or with tools installed on their computer, see which choices are
available, and use local build tools such as `latexmk`, Tectonic, Typst,
Calepin, Pandoc, and Quarto without sending an executable command line from a
document to the companion.

The existing **Compiler** settings category becomes **Build** because several
supported choices are build drivers or document processors rather than
compilers. The existing **Local app** category continues to manage the
companion itself.

## Product decisions

Present one grouped build-tool selector. Group choices by where they execute,
using **Browser** and **Local companion** sections instead of prefixing every
choice with “Browser” or “Local.” Keep unavailable local choices visible and
disabled so users can discover the feature and understand what is missing.

The selected build tool and its portable options belong to the document when
they can be honored by collaborators. Whether this browser uses the local
companion, its pairing token, a folder binding, and any machine-specific preset
remain local to the user and device.

The companion accepts structured build requests and locally defined preset
identifiers. A document or website never supplies an executable path, shell
fragment, unrestricted argument list, or environment map.

## Current behavior

LibrePaper already has several parts of this design, but they appear through
different interfaces:

| Source | Browser | Local companion |
| --- | --- | --- |
| LaTeX | WebAssembly pdfLaTeX and XeLaTeX | Native pdfLaTeX, XeLaTeX, and LuaLaTeX, principally as fallback; native BibTeX, Biber, and MakeIndex helpers |
| Typst | WebAssembly Typst | Calepin managed preview; no general native `typst compile` job |
| Markdown | WebAssembly Markdown | No ordinary Markdown build job |
| Quarto | Markdown draft | Native Quarto render and managed preview |

The companion already discovers direct TeX engines and helpers, Quarto, and
Calepin. It reports availability and versions without exposing executable
paths. The browser already submits project snapshots as bounded manifests and
downloads bounded, identified outputs.

The current **Compiler** category is offered only for editable LaTeX documents
and selects `auto`, `pdflatex`, `xelatex`, or `lualatex`. It does not let an
author distinguish browser XeLaTeX from local XeLaTeX or select a build driver
such as `latexmk`.

## Settings organization

### Build

Offer **Build** for editable LaTeX, Typst, Markdown, and Quarto documents. It
contains the choices that affect how the document is rendered:

- **Build tool**: one grouped selector described below.
- **Engine**: shown only when the selected tool supports an engine choice.
- **Output**: shown when the tool supports more than one relevant preview or
  export format.
- Tool-specific portable settings, such as a Quarto profile and parameters,
  plus a clearly identified device-local preset choice.

The first option in the selector is **Automatic**. Automatic prefers the
normal browser renderer for formats that can render there and may use a paired,
available local adapter according to the fallback rules for that format. The
result always identifies the actual backend and tool used.

For LaTeX, present:

```text
Automatic

Browser
  pdfLaTeX
  XeLaTeX

Local companion
  pdfLaTeX
  XeLaTeX
  LuaLaTeX
  latexmk
  Tectonic
  Custom preset…
```

For Typst, present:

```text
Automatic

Browser
  Typst

Local companion
  Typst
  Calepin
  Custom preset…
```

For Markdown, present:

```text
Automatic

Browser
  Markdown

Local companion
  Pandoc
  Quarto
  Custom preset…
```

For Quarto, the browser group contains **Markdown draft** and the local group
contains **Quarto** and applicable custom presets. Existing Quarto profile and
parameter settings move under **Build**.

Use accessible grouped controls. A native `<select>` may use `<optgroup>`;
another component must preserve equivalent group names, keyboard operation,
disabled states, and accessible descriptions.

### Conditional settings

Do not treat a build driver and a TeX engine as the same setting. Direct
pdfLaTeX, XeLaTeX, and LuaLaTeX choices need no second engine row. `latexmk`
may expose an engine row containing the engines that the detected adapter can
drive. Tectonic owns its compilation strategy and does not show an unrelated
TeX engine choice.

Calepin is a first-class Typst build tool with managed-preview support. Native
Typst is a separate tool using the installed Typst CLI. Choosing one must not
silently substitute the other.

### Local app

Keep machine management under **Local app**:

- connection and pairing status;
- companion address and launch action;
- folder bindings;
- discovered tools and versions;
- rescan and setup checks;
- local preset management;
- permissions, disconnection, and revocation.

A user who selects or inspects a local build choice can complete the required
companion setup without first finding this separate category. The Build page
links directly to the relevant Local app action or displays it inline.

## Companion availability

The browser distinguishes these actionable states:

| State | Build-page message | Actions |
| --- | --- | --- |
| No loopback response | “Local companion unavailable. Open it if installed, or install it to use tools on this computer.” | **Open companion**, **Install companion**, **Retry** |
| Launch attempted, still unavailable | “LibrePaper could not find the local companion.” | **Install companion**, **Troubleshoot**, **Retry** |
| Running, not paired | “Connect this document to use local build tools.” | **Connect** |
| Paired, capability scan pending | “Checking tools on this computer…” | none |
| Connected | Show detected choices and versions | **Rescan** when appropriate |

A failed loopback probe alone does not prove the companion is uninstalled. The
browser first offers `librepaper://launch`, probes again for a bounded period,
and then emphasizes installation. Wording must not claim that an app is absent
when it may merely be stopped or blocked by the browser's local-network policy.

Keep the **Local companion** group visible in every state. Disable its build
choices until the companion reports that adapter as available. Give the group
one explanation rather than repeating “Companion required” in every option.
When connected, identify individual missing or incompatible tools in the
option's description.

If a previously selected local tool disappears, retain the selection, mark it
unavailable, and explain how to rescan or choose another tool. Never silently
change the document's requested build tool.

## Capability discovery

The companion discovers and version-probes these initial tools:

- LaTeX: `pdflatex`, `xelatex`, `lualatex`, `latexmk`, `tectonic`, `bibtex`,
  `bibtex8`, `biber`, and `makeindex`;
- Typst: `typst` and `calepin`;
- Markdown and Quarto: `pandoc` and `quarto`.

Ordinary discovery resolves an executable locally, performs a bounded version
probe, validates compatibility, and derives supported features. Executable
paths never cross the bridge. Discovery must not install packages, mutate a
toolchain, initialize a project, or run document code.

An explicit **Check local setup** action may run bounded smoke tests. Report
that a smoke test can initialize caches or fetch packages before running one
when the underlying tool has that behavior.

Replace the protocol's growing set of special-case tool fields with an
additive adapter-oriented capability list. During migration, retain the old
fields for older clients:

```json
{
  "builders": [
    {
      "id": "latexmk",
      "available": true,
      "version": "4.87",
      "source_formats": ["latex"],
      "outputs": ["pdf"],
      "engines": ["pdflatex", "xelatex", "lualatex"],
      "operations": [{ "kind": "build", "workspace_modes": ["snapshot"] }],
      "preview": false,
      "presets": true,
      "note": ""
    },
    {
      "id": "calepin",
      "available": true,
      "version": "0.4.0",
      "source_formats": ["typst"],
      "outputs": ["html", "pdf"],
      "engines": [],
      "operations": [
        { "kind": "build", "workspace_modes": ["bound"] },
        { "kind": "preview", "workspace_modes": ["bound"] }
      ],
      "preview": true,
      "presets": true,
      "note": ""
    }
  ]
}
```

Capability IDs and option values are stable protocol vocabulary. Display
names may change without changing stored settings or requests.
The `operations` list is authoritative for supported operation/workspace
combinations; the `preview` summary must agree with it. Availability describes
tool support, not whether the current document has the required grant.

Maintain one canonical catalog of supported built-in builders, their stable
IDs, display names, source formats, browser/local variants, and minimum
protocol requirements. The browser ships this catalog so it can show disabled
local choices before connecting. The catalog describes client support, not
installed tools or permission to execute them. Settings and request validation
use the same vocabulary; contract tests check it against companion adapters.

Overlay verified companion capabilities on the catalog. An absent adapter is
unavailable, never implicitly supported. For protocol version 1, a compatibility
mapper derives only the operations the old fields actually support; managed
preview support does not imply a one-shot build operation. Keep newer built-in
choices visible but disabled with “Update companion” when the negotiated
protocol cannot provide them. Hide unsupported subordinate controls. Unknown
companion IDs do not become executable choices until the client supports their
contract. Preset entries come from authenticated companion discovery; a saved
but missing preset remains visible as unavailable on its original device.

## Build requests and adapters

Generalize the local execution boundary around build adapters. Admission,
authentication, upload limits, queueing, cancellation, workspace lifetime,
and output download remain shared service responsibilities. Each adapter owns:

- discovery and compatibility checks;
- typed options and their validation;
- command construction without a shell;
- input staging and confinement requirements;
- progress, log, and diagnostic normalization;
- output discovery, MIME type, size, and digest validation;
- preview/watch support, when offered;
- provenance containing the adapter, tool version, engine, and preset identity.

The initial built-in adapters are direct TeX, `latexmk`, Tectonic, native
Typst, Calepin, Pandoc, and Quarto. BibTeX, Biber, and MakeIndex remain helper
stages used by applicable LaTeX adapters rather than user-facing top-level
build tools.

Use a general structured request instead of adding a top-level optional object
for every new tool:

```json
{
  "protocol": 2,
  "kind": "build",
  "project": "paper-id",
  "origin": "https://example.org",
  "snapshot": "source-revision",
  "generation": 12,
  "builder": "latexmk",
  "workspace": { "mode": "snapshot" },
  "entrypoint": "paper.tex",
  "output": "pdf",
  "options": {
    "engine": "xelatex"
  },
  "manifest": []
}
```

Every adapter defines its accepted option schema. Reject unknown builders,
options, outputs, and engine combinations. Do not fall through from an unknown
builder to a native command.

### Workspace and authorization

Every version 2 build or managed-preview request declares exactly one workspace
mode: `{"mode":"snapshot"}` or `{"mode":"bound","binding_id":"opaque-id"}`.
Snapshot mode uses a fresh service-owned workspace populated only from the
validated upload manifest. Bound mode resolves an existing local binding; no
request may supply a directory path. Reject a binding ID in snapshot mode and
reject bound mode without one. The adapter must advertise support for the
requested mode and operation.

Pairing authenticates the origin and document; it does not grant access to a
working directory or authorize arbitrary document code. Apply these initial
authorization requirements before staging inputs or starting a process:

| Adapter or operation | Workspace | Required authorization |
| --- | --- | --- |
| Direct TeX and bibliography/index helpers | Snapshot | Existing scoped pairing and native-execution policy |
| Built-in `latexmk`, Tectonic, native Typst, and Pandoc | Snapshot | Scoped pairing and the restricted adapter policy below |
| Quarto builds and previews; Calepin builds and previews | Bound | Existing explicit local execution grant and scoped folder binding |
| Custom preset | Snapshot or bound, as locally configured | Explicit preset grant; bound mode also requires its scoped binding |

Other combinations are unsupported in the first release. In particular, the
general request does not introduce ungranted snapshot execution for Quarto or
Calepin. Existing version 1 authorization remains in force throughout migration.

Resolve each binding and grant against the authenticated origin and project,
the requested adapter or preset, operation, and entrypoint. Preserve existing
canonical-root, manifest-consistency, and concurrent-preview checks. A request
body's `origin` is not a substitute for authenticated scope. Recheck grants
before queued execution; revocation prevents subsequent execution and follows
the existing cancellation policy for active jobs and previews. Selecting a
tool, discovering it, or receiving a collaborator's preference cannot create
a local grant.

### Restricted adapter policy

Typed arguments alone do not prevent a tool from executing configuration found
in a project. Each built-in adapter must explicitly define which configuration,
hooks, filters, plugins, and helper commands it reads or starts, along with its
filesystem, environment, network, and child-process policy. Preserve existing
native confinement behavior, including failure when an expected sandbox cannot
be applied; do not silently broaden access to make a new adapter work.

The snapshot `latexmk` adapter disables automatic RC loading with `-norc`,
does not accept uploaded RC files through `-r` or Perl expressions through
`-e`, and fixes its engine/helper commands locally with TeX shell escape
disabled. Its version probe also suppresses RC loading and uses a service-owned
working directory. RC files execute Perl, so ordinary option validation does
not make them safe; see the [upstream manual](https://www.cantab.net/users/johncollins/latexmk/latexmk-488.pdf).

The other snapshot adapters likewise must not load document-selected executable
filters, hooks, or plugins or start undeclared external helpers. Tool-native
document evaluation stays within the adapter's declared confinement policy.
Package fetching remains subject to the no-automatic-installation boundary.
Quarto and Calepin retain their existing granted-project execution policies.
Additional executable project configuration, such as a `latexmkrc` or Makefile,
requires a locally configured preset whose explicit grant covers that behavior.
Apply configuration restrictions to probes and setup checks as well as builds.

Protocol version 1 TeX, Biber, Quarto, and managed-preview requests continue
to work during migration. The browser negotiates the protocol advertised by
the companion and disables unavailable operations according to the catalog
rules above; it never sends a version 2 request to a version 1-only companion.

## Local presets and customization

Named presets allow authors to customize builds without giving a website a
general process-execution API. Presets are created and stored by the companion
on the user's machine. The browser receives only their stable ID, display
name, compatible source formats, base adapter, and portable option schema.

A preset may select a built-in adapter and locally configure checked arguments,
environment values, output rules, or a wrapper executable. Its complete
command and environment remain local. A build request can name the preset and
supply only values declared by that preset's typed schema.

Using a preset for a new origin and document requires an explicit local grant.
The grant identifies the preset and whether it operates on an uploaded
snapshot or a bound working directory. Editing a preset invalidates grants
whose execution meaning changed; renewal requires explicit local approval.

Folder-bound presets may support existing project build files such as a
Makefile, but only through a locally configured, explicitly granted preset.
The document must not be able to choose an arbitrary make target or command.

The first release should ship native adapters for common tools rather than
modeling `latexmk`, Tectonic, Calepin, Pandoc, and Quarto as arbitrary custom
commands. Their invocation, cancellation, diagnostic parsing, output capture,
and confinement requirements differ enough to warrant explicit adapters.

## Shared and local state

Store a shared, portable build preference where collaborators benefit from the
same intent, for example:

```json
{
  "format": "latex",
  "selection": "tool",
  "tool": "latexmk",
  "engine": "xelatex",
  "output": "pdf"
}
```

Store these on the current browser or companion instead:

- backend selection and any complete local build override;
- pairing credentials and companion address;
- local preset IDs and definitions;
- folder binding IDs and paths;
- locally discovered versions and capability cache.

When the shared preference cannot be honored on a collaborator's machine,
show the mismatch and offer compatible available choices. Do not overwrite the
shared preference merely because one collaborator lacks its tool. A local
override must be visibly identified and must not affect other collaborators.

### Selection representation and precedence

The shared record has `selection: "automatic" | "tool"`, `format`, `output`,
and portable options. For `selection: "tool"`, `tool` is a stable catalog ID.
Direct TeX uses `tool: "tex"` with its `engine`; `latexmk` has its own ID and
engine option. Automatic LaTeX may retain an engine constraint for migration.
No shared record contains a backend, preset ID, binding, or credential.

The device record is keyed by origin and document. It contains
`backend: "auto" | "browser" | "local"` and, optionally, a complete portable
preference override plus a local preset ID. When a backend choice accompanies
the shared preference, save the shared preference revision it applies to.
If another collaborator changes that revision, clear that backend choice to
`auto` and explain that local execution may need to be selected again. A
complete local override remains in effect until the user clears it. Persist
device records across reloads, without synchronizing them to collaborators.

Default selector edits change shared portable intent and the editing device's
backend. Offer an explicit **Only on this device** mode for a complete override.
The selected value is derived from the effective preference and backend:

| User choice | Portable preference written | Device backend |
| --- | --- | --- |
| Automatic | `selection: "automatic"`; clear explicit tool and engine constraint | `auto` |
| Browser XeLaTeX | `selection: "tool", tool: "tex", engine: "xelatex"` | `browser` |
| Local XeLaTeX | Same portable fields as Browser XeLaTeX | `local` |
| Local `latexmk` | `selection: "tool", tool: "latexmk"` and selected engine | `local` |
| Other named tool | `selection: "tool"` and its catalog ID and portable options | Chosen group |
| Custom preset | Complete device override using its base adapter; shared record unchanged | `local` |

Automatic clears any previous device override or preset in the default mode;
in **Only on this device** mode it stores an automatic override. Provide
**Use document settings** to remove the override and reset backend selection
to `auto`. Discard options incompatible with the newly selected tool.

Resolve a complete device override first, otherwise use the shared record.
Apply the matching device backend next, then availability and authorization.
With backend `auto`, a named tool uses its browser implementation if one exists,
with the same no-local-fallback rule as explicit browser selection;
a local-only tool requires an explicit local choice on this device, except for
the legacy direct-TeX routing preserved below. Show a mismatch/setup action
while that choice or grant is missing. Receiving shared intent cannot by itself
enable Quarto, Calepin, Pandoc, `latexmk`, Tectonic, or a preset locally.

### Existing LaTeX settings

When no new shared build record exists, interpret the legacy engine setting
as follows. Migration is idempotent and does not force a backend or rewrite
preferences merely when opening the document.

| Legacy engine | Effective shared preference | Routing with device backend `auto` |
| --- | --- | --- |
| Missing or `auto` | Automatic LaTeX, PDF, no engine constraint | Existing automatic engine detection and fallback |
| `pdflatex` | Automatic LaTeX, PDF, engine constrained to `pdflatex` | Browser first, existing native fallback |
| `xelatex` | Automatic LaTeX, PDF, engine constrained to `xelatex` | Browser first, existing native fallback |
| `lualatex` | Automatic LaTeX, PDF, engine constrained to `lualatex` | Existing native route and authorization; no browser substitution |

Show migrated engine constraints beside Automatic until the author changes
them. A new build record takes precedence over the legacy field; old-client
compatibility must not overwrite it on reconnect. Protocol version 1 transport
support is independent of this settings migration.

## Automatic selection and fallback

An explicit device backend is strict. Browser selection never triggers a local
build or local bibliography helper; report a missing helper with an action to
choose Automatic or local execution. Local selection never falls back to a
browser renderer or another tool. Failure or loss of availability retains the
selection and reports the problem. These rules also apply to Quarto: its draft
fallback below is allowed only with backend `auto` and Automatic selection.
Changing the effective selection cancels or stops an incompatible active job
or managed preview and prevents its late results replacing the new result.

Automatic behavior is deterministic and reported to the user:

- LaTeX starts with a compatible browser engine, except for the migrated
  LuaLaTeX constraint described above. It may use the corresponding
  native direct-TeX adapter after an infrastructure, missing-package, or engine
  failure under the existing fallback policy. A source error remains a source
  error and must not cause repeated execution through every installed tool.
- A device that explicitly selects local `latexmk` or Tectonic does not run
  the browser compiler first.
- Typst starts with browser Typst unless the local user explicitly
  selects native Typst or Calepin. Calepin is never substituted for Typst
  merely because it is installed.
- Markdown starts with browser Markdown. Pandoc or Quarto execution requires
  an explicit local choice.
- Automatic Quarto preview may use its existing explicitly granted local
  execution path and fall back to its non-executing Markdown draft when the
  companion is unavailable, while clearly identifying that the displayed
  result is a draft.

Show the actual builder, engine, version, backend, and relevant preset in build
status and diagnostics. A successful fallback explains why it happened.

## Browser organization

Move the companion client out of the LaTeX-specific module because it already
serves TeX, Biber, Quarto, and Calepin. The target organization is:

```text
web/src/lib/companion/
  client.js
  connection.js
  capabilities.js
  jobs.js
  previews.js
  builders/
    latex.js
    typst.js
    markdown.js
    quarto.js
```

Connection and pairing state must have one owner. Builders use the common job,
manifest, polling, cancellation, and output-verification functions rather than
copying them. Format renderers translate document settings into a builder
request and translate normalized results into the existing preview surfaces.

The settings registry offers **Build** according to source format and renders
the selector from the canonical catalog overlaid with browser capabilities and
negotiated companion capabilities. It must not maintain its own duplicate tool
list. Catalog presence alone never enables local execution.

## Companion organization

Separate service mechanics from tool adapters:

```text
crates/librepaper/src/local/
  service/
    jobs.rs
    previews.rs
    capabilities.rs
    authorization.rs
  builders/
    latex.rs
    latexmk.rs
    tectonic.rs
    typst.rs
    calepin.rs
    pandoc.rs
    quarto.rs
  presets/
    config.rs
    validation.rs
    grants.rs
```

This is a responsibility boundary, not a requirement to move every file in
one change. Preserve the existing hardened service behavior while introducing
the adapter interface and migrate one tool at a time.

## Scope boundaries

- No server-side native compilation. Local tools run only in the companion.
- No executable paths, arbitrary argument vectors, shell source, or arbitrary
  environment maps supplied by the website or document.
- No automatic installation or upgrade of TeX, Typst, Calepin, Pandoc,
  Quarto, Tectonic, or their packages.
- No promise that every collaborator produces byte-identical output with
  different local tool versions. Provenance makes differences inspectable.
- No silent publication of local generated files. Existing output collection
  and publication rules continue to apply.
- No automatic execution of code-bearing Markdown or Typst content merely
  because a capable local tool was detected.

## Delivery plan

1. Rename **Compiler** to **Build**, offer it for all supported source formats,
   and add the grouped selector with browser choices and companion setup states.
   Preserve the existing LaTeX setting through migration.
2. Generalize capability reporting while retaining protocol version 1 fields.
   Add discovery for `latexmk`, Tectonic, native Typst, and Pandoc.
3. Make direct native LaTeX an explicit choice in addition to its existing
   fallback role. Add `latexmk` and Tectonic adapters with typed engine and
   output options.
4. Add native Typst and Calepin build adapters, preserving the existing managed
   Calepin preview lifecycle. Add Pandoc for Markdown.
5. Migrate Quarto settings and execution to the common builder contract and
   consolidate the browser companion modules.
6. Add companion-local preset creation, validation, grants, and management.
7. Remove compatibility fields and obsolete module paths only after supported
   clients have migrated.

## Tests

Browser settings tests verify grouped labels, keyboard and screen-reader
semantics, format-specific options, disabled local choices, companion setup
messages, retained unavailable selections, and shared versus local state.
Cover each selector-to-state mapping, reload persistence, remote preference
changes, override removal, all four legacy engine values, and idempotent
migration. Explicit browser builds must never issue native or helper requests;
explicit local failures must not start browser rendering. Selection changes
must stop incompatible previews and reject stale results.

Capability tests distinguish available, missing, incompatible, timed-out, and
changed tools without exposing paths. Rescan updates choices without requiring
a page reload. Older companions continue to expose their current TeX, Quarto,
and Calepin capabilities.
Test the catalog with no companion, an unpaired companion, and a version 1-only
companion: built-in choices remain visible, unsupported operations stay disabled,
and preview-only capabilities never enable one-shot builds.

Adapter tests verify typed option rejection, exact argument construction,
cancellation, deadlines, confinement, input manifests, output bounds, digests,
MIME types, diagnostic parsing, and provenance for every built-in adapter.
Unknown tools and invalid engine/tool combinations never execute a process.
Adversarial manifests containing executable RC files, hooks, or filters cannot
cause undeclared execution in snapshot adapters. In particular, an uploaded
`.latexmkrc` cannot write a marker or replace an engine command during either
discovery or compilation. Tests cover each adapter's declared configuration
and confinement policy, including child processes.

Authorization tests reject missing, revoked, cross-origin, cross-document, and
wrong-entrypoint bindings and grants, including revocation while queued. Pairing
alone cannot authorize bound execution. Quarto and Calepin cannot bypass their
existing grants through version 2 snapshot requests or shared settings changes.

Integration tests compile representative projects with browser XeLaTeX,
native XeLaTeX, `latexmk`, Tectonic, native Typst, Calepin, Pandoc, and Quarto
when those tools are installed. Missing optional tools skip only their real-tool
integration case; request validation remains covered without them.

Preset tests prove that a request cannot change the executable, inject an
argument, select an undeclared target, or add an environment variable. Changing
a preset invalidates the applicable grant. Revocation prevents subsequent
execution while leaving document files intact.

## Acceptance criteria

- A LaTeX author can explicitly choose browser XeLaTeX or local XeLaTeX and
  see which one produced the current preview.
- Explicit browser selection cannot execute local helpers or fallback builds;
  explicit local selection cannot silently render with a browser tool.
- Selector choices survive reloads, local overrides remain device-local, and
  legacy engine preferences retain their existing routing during migration.
- Uploaded executable configuration cannot bypass a restricted built-in
  adapter. Bound execution and presets require their scoped local grants,
  including after request migration to protocol version 2.
- A connected author with `latexmk` or Tectonic installed can select it; an
  author without it sees the supported choice as unavailable with a useful
  explanation.
- A Typst author can choose browser Typst, native Typst, or Calepin when those
  local tools are available. LibrePaper does not silently substitute among
  them.
- A Markdown author can keep the safe browser preview or explicitly build with
  installed Pandoc or Quarto.
- Build choices appear under **Browser** and **Local companion** sections;
  option labels do not repeat the section name.
- When the companion does not answer, the Build page offers Open, Install, and
  Retry actions without falsely asserting that the app is uninstalled.
- When the companion is running but unpaired, the Build page offers connection
  in context. Once paired, detected tool versions appear without a reload.
- Local choices are enabled only from verified companion capabilities. Rescan
  finds a newly installed tool, and a removed selected tool remains visible as
  unavailable.
- Supported built-in choices remain discoverable without a companion and with
  an older companion; unsupported operations are disabled with setup or upgrade
  guidance rather than inferred from catalog presence.
- `latexmk` exposes an engine choice only when supported. Tectonic and Calepin
  do not show irrelevant TeX-engine settings.
- A local preset can customize a build after an explicit grant, while the
  browser cannot supply or alter its executable, unrestricted arguments, or
  environment.
- Collaborators can honor a portable shared build preference or apply a clear
  local override without changing another person's companion configuration.
- Every result and fallback reports its backend, builder, engine when
  applicable, version, preset when applicable, and source snapshot.
- Existing protocol version 1 LaTeX fallback, Biber fallback, Quarto render,
  and Calepin managed preview continue to work throughout migration.
