# Run environments

A project can render inside an environment that a tool such as Nix, a dev
container, pixi or conda provides. The desktop app runs the usual render
command behind a prefix, for example `nix develop path:. --command`, that
each person chooses for each project on their own machine.

This replaces option 3 of issue #96, which handled dev containers only.

## Problem

**Run code** in the desktop app renders with whatever Quarto, R, Python,
LaTeX and Typst are on the person's PATH. Coauthors with different versions
get different output, and a project that needs a package someone lacks just
fails. Researchers already solve this with environment tools: a
`flake.nix`, a `devcontainer.json`, a `pixi.toml`, an `environment.yml`.
LibrePaper ignores all of them.

Almost every one of these tools has the same shape: a command that runs
another command inside the environment.

| Tool | Runs `quarto render paper.qmd` inside the environment |
|---|---|
| Nix flake | `nix develop path:. --command quarto render paper.qmd` |
| Dev container | `devcontainer exec --workspace-folder . quarto render paper.qmd`, after `devcontainer up --workspace-folder .` |
| pixi | `pixi run quarto render paper.qmd` |
| conda | `conda run -n paper quarto render paper.qmd` |
| uv | `uv run quarto render paper.qmd` |

So one general mechanism covers all of them: an optional **prefix**, and an
optional **setup command** run before it.

## Goals

- One mechanism for any environment tool, with no code specific to any tool
  in the render pipeline.
- The environment is chosen per project, because projects differ.
- It applies to every format: a flake can pin TeX Live or Typst as well as
  Quarto and R.
- A coauthor sets it up in a few clicks, with a preset when the project
  contains a file the app recognises.
- The project never chooses a command that runs on someone's machine.

## Non-goals

- Installing Nix, Docker, the devcontainer CLI, pixi or conda.
- Remote environments: Codespaces, remote Docker hosts, SSH.
- Rebuilding an environment when its config changes. The tool's own cache
  decides that, or the person rebuilds by hand.
- Tools that need a sentence in one shell string, such as `nix-shell --run`,
  as a preset. A person can still wrap them in a script and use that as a
  custom prefix.
- A sandbox. An environment is not a security boundary (see Security).
- Allowing dotfiles in projects. `.devcontainer/` still cannot travel with a
  project; see Dev containers below.
- Browser-only rendering (WASM). The environment applies to desktop renders
  only.

## Concepts

**Environment.** A per-project, per-machine setting with two parts:

- `prefix`: an argv list. Non-empty.
- `setup`: an optional argv list.

Both use the same limits as a tool record (`tool.rs`): at most 64 arguments,
each at most 4096 bytes. Neither takes placeholders, and any `{...}` is
refused when the environment is saved, as in `Record::validate`, so a
placeholder can be added later without changing the meaning of a saved
value. Both run with the project's render workspace as their working
directory, so `.` means the workspace.

**Project key.** An environment belongs to the same pair that names the
render workspace in `BindingStore::hosted_binding` (`bindings.rs`): the
deployment derived from the page's origin, and the project slug. Two servers
with a project of the same slug get separate environments, as they get
separate workspaces.

**Preset.** A named template for `prefix` and `setup`, defined in the web
app only. The helper never sees a preset name; it receives the argv lists.
A preset fills the form, and the person can edit the result before saving.

## Behaviour

### Rendering with an environment

When the job's project has an environment, `render::run` (`render/mod.rs`)
changes in three places:

1. **Discovery.** The up-front `discovery::resolve` check applies to
   `prefix[0]`, not to the record's program, because `quarto` may exist only
   inside the environment. If `setup` is present, `setup[0]` is checked too.
   A missing program fails before anything runs:
   `"nix was not found. Install it, or change this project's environment in Settings > Rendering."`
2. **Setup.** When `setup` is present, it runs first, through `supervise()`
   with the same cancel token, the same `kill_group`, and the job's deadline.
   It runs before every render. Measured costs make this acceptable: about
   0.7 s for `devcontainer up` on a running container and about 1 s to
   restart a stopped one. A non-zero exit fails the job with
   `"the environment setup command exited with status N"`, and its log is
   the job's log. The render command does not run.
3. **Command.** The expanded record runs as `prefix ++ record.command`. The
   program is `prefix[0]`, resolved as above.

The setup and render logs are concatenated in order, inside the existing
`MAX_LOG_BYTES` tail. The log starts with one line naming the environment:

```
Environment: nix develop path:. --command
```

so a person reading a failed log knows the render did not use their local
tools. That line is not translated: the log is tool output.

Everything after the command is unchanged: result reading, freshness,
SyncTeX, outcomes.

Exit status 127 inside an environment usually means the tool is not in it.
The helper does not try to detect this. The log shows the tool's own error,
such as `quarto: command not found`.

### Without an environment

Nothing changes. A project with no environment renders exactly as it does
today. There is no machine-wide environment: the machine-wide render
commands in Settings → Rendering already serve that purpose.

### Cancel and timeout

`supervise()` kills the whole process group on cancel or timeout. For Nix,
pixi, conda and uv, the tool runs as a child in that group, so it dies too.
For dev containers, the host-side `devcontainer exec` dies but the process
inside the container keeps running until it finishes. This is a known
limitation and is documented; it is not worked around.

### Machine-wide records and the environment together

An edited machine-wide record still applies inside the environment. If a
person changed `quarto.pdf` to `quarto render {input} --to pdf --quiet`,
that is what runs behind the prefix. The environment changes where a command
runs, not which command runs.

## Presets

The web app offers these presets. Each fills `prefix` and `setup`, and the
person can edit both before saving.

| Preset | `setup` | `prefix` | Suggested when the project root has |
|---|---|---|---|
| Nix flake | none | `nix --extra-experimental-features "nix-command flakes" develop path:. --command` | `flake.nix` |
| Dev container | `devcontainer up --workspace-folder . --config devcontainer.json` | `devcontainer exec --workspace-folder . --config devcontainer.json` | `devcontainer.json` |
| pixi | none | `pixi run` | `pixi.toml` |
| conda | none | `conda run --no-capture-output -n <name>` | `environment.yml` |
| uv | none | `uv run` | `pyproject.toml` and `uv.lock` |
| Custom | empty | empty | never suggested |

Notes:

- `"nix-command flakes"` is one argument. The preset lists argv, not a
  shell string, so no quoting is involved.
- The conda preset's `<name>` is a field the person fills in. When
  `environment.yml` has a top-level `name:`, the web app pre-fills it. The
  page reads this from the project's file, which it already has. The env must
  already exist on the machine: `conda run` does not create it.
- `pixi run` and `uv run` create or update the environment on first use, so
  the first render can take minutes. The render deadline (at most 3600 s)
  applies; the docs say so.

## Detection and suggestion

Detection only suggests. It never sets an environment.

When a project is open in the desktop app, has no environment, and its root
contains one of the files in the Presets table, the render panel shows one
dismissable line:

> This project has a flake.nix. Render inside it? [Set up]

**Set up** opens the environment form with that preset filled in. Saving
still goes through the shell's confirmation (below). Dismissing hides the
line for that project on that machine, stored in `localStorage` (wrapped in
try/catch; if storage fails the line shows again next time, which is
harmless). When several files match, the first row of the table wins.

Detection reads the project's file list in the page. It does not look at the
workspace on disk.

## Storage

The helper stores environments in
`<state_home>/librepaper/local/environments.json`, written with
`write_private_json` as `renderers.json` is (`render/records.rs`):

```json
{
  "3f9a1c0d7e2b4a65/paper-2026": {
    "prefix": ["nix", "--extra-experimental-features", "nix-command flakes",
               "develop", "path:.", "--command"],
    "setup": null
  }
}
```

The key is `<deployment>/<project>`, where `<deployment>` is the same
16-character digest of the normalised origin that `hosted_binding` uses for
the workspace path. The two should share one helper so they cannot drift. A new module,
`render/environments.rs`, mirrors `records.rs`: `init`, `get`, `set`,
`reset`, a process-wide `RwLock`, and a loader that drops entries that no
longer validate. `init` runs where `records::init` runs.

Entries are never removed automatically. A deleted project leaves a few
bytes behind, as it leaves its workspace. Resetting the environment removes
its entry.

The environment is not stored on the server, not synced to coauthors, and
not part of the project. That is deliberate: see Security.

## Wire

Two routes are added under `${base}librepaper/local/`, beside
`renderers/<key>`:

**`GET environments/<project>`**: authenticated like the other feature
routes. The deployment comes from the request's origin, as it does for jobs.

```json
{ "prefix": ["pixi", "run"], "setup": null }
```

or `{ "prefix": null, "setup": null }` when none is set. Never 404, so the
page needs no special case.

**`PUT environments/<project>`**: same authentication. The body is
`{ "prefix": [...], "setup": [...] | null }` or `{ "reset": true }`, decoded
with `deny_unknown_fields`, for the same reason `RendererRequest` is: the
shell shows this command to the person, and a dropped field could mean
approving one command and running another. A reset carries no command.
Validation errors return `400 {"error": "..."}`.

Additions only, so this is a minor bump under SPEC-wire.md 7.1. Fixtures
for both routes (`environments.get.response.json`,
`environments.put.request.set.json`, `environments.put.request.reset.json`,
`environments.put.response.json`) join the set in
`service/wire_tests.rs`.

`JobRequest` and `JobStatus` do not change. The helper looks the environment
up from the job's origin and project, which the request already carries.

`capabilities` does not change. It stays machine-wide; see Settings below
for how "not found" reads when an environment is set.

### Shell confirmation

Setting an environment makes the machine run a command, so it gets the same
protection as setting a render command. `prompt_for` (`desktop/src/main.rs`)
recognises `PUT /librepaper/local/environments/<project>` and shows:

> **Change how {project} renders**
>
> {origin} wants LibrePaper to render {project} inside:
>
> setup: devcontainer up --workspace-folder . --config devcontainer.json
> then: devcontainer exec --workspace-folder . --config devcontainer.json …
>
> Only allow this if you asked for it.

using `display_argument` for each argument, as `renderer_prompt` does. A
reset needs no confirmation. The shell parses the body with its own
`deny_unknown_fields` type, as it does for `RendererChange`, and refuses a
malformed body with 400 before forwarding.

Under SPEC-wire.md's frozen-routes rule, this route is frozen from the day
it ships: its path and body never change, and no migration maps a route onto
it. The test that passes confirmed routes through `wire::upgrade` for every
supported major gains this route.

The prompt is the shell's, in English, like the existing renderer prompt.

## Web

### Where the setting lives

Settings → Rendering gains a section, **Run environment**, above the list of
render commands. It appears only in the desktop app and only while a project
is open, and it names the project: "Environment for paper-2026". In the
browser without the app it is absent, like the rest of Settings → Rendering.

The section shows one of two states:

- **None:** "Renders use the tools installed on this computer." and a
  **Choose an environment** button.
- **Set:** the preset name when the saved value still matches a preset,
  else "Custom"; the setup and prefix as read-only command text; **Edit**
  and **Remove** buttons. Remove sends a reset.

The form has a preset picker, a setup field and a prefix field. The fields
use the same argv editor as `ToolCommand.svelte`, so quoting works the same
way. Saving sends `PUT environments/<project>`; the shell then asks. A
refusal leaves the form open with the shell's refusal text, as a refused
render command does.

### "Not found" next to render commands

When the open project has an environment, the found/not-found badge next to
each render command in `ToolCommand.svelte` is replaced by one line: "Runs
inside this project's environment." The badge describes the host, which is
not what will run, and a red "Not found" next to a command that renders
correctly would mislead.

### Suggestion line

The render panel line from Detection and suggestion. It belongs with the
other render-panel notices in `Reader.svelte` or the preview component that
shows render state; the implementer picks the existing notice slot rather
than adding a new layout.

### Client

`web/src/lib/desktop/client.js` gains `getEnvironment(project)`,
`setEnvironment(project, { prefix, setup })` and
`resetEnvironment(project)`, beside the existing renderer calls at
`client.js:521`.

### Messages

Every string above goes through Paraglide: keys in `web/messages/en.json`
and the same keys, translated with the glossaries, in fr, es, pt, de, ja and
zh. Preset names are messages too, except tool names (Nix, pixi, conda, uv),
which stay as written. "Dev container" is translated per glossary.
`web/tests/unit/untranslated.mjs` must pass.

## Tool notes

### Nix

- `flake.nix` and `flake.lock` are ordinary allowed files, so a flake
  travels with the project and through `librepaper pull` and `push`.
- **The workspace is not a git repository.** `nix develop .` on a
  directory that is not in git treats it as a path flake: Nix copies the
  whole workspace into the store, and the copy's hash changes whenever any
  file changes, including render outputs and `_freeze/`. Expect a fresh
  evaluation, and a new store path, on every render. Phase 0 measures the
  cost. The `path:` in the preset makes the choice of fetcher explicit; it
  does not avoid the copy.
- **Workaround, documented:** point the prefix at a flake outside the
  workspace, such as `nix develop /home/me/envs/paper --command` or a flake
  reference like `github:me/paper-env`. That evaluates once and is then
  cached. A setting kept on the person's machine is what makes this
  possible.
- If Phase 0 shows the copy is too slow for ordinary projects, the follow-up
  is a cached dev environment: run `nix print-dev-env` once, store the
  result keyed by the hash of `flake.nix` and `flake.lock`, and source it
  for later renders. That is tool-specific code and needs its own decision;
  it is not part of this spec.
- `nixConfig` in a flake asks for confirmation on a terminal. With stdin
  closed, Nix ignores it. Documented, not handled.
- Store growth: each render can leave a store path behind. Nix's garbage
  collector reclaims them. Documented.

### Dev containers

- `.devcontainer/devcontainer.json` and `.devcontainer.json` cannot be in a
  project, because `admit()` refuses dotfiles. The preset uses a root
  `devcontainer.json` with `--config`. VS Code and Codespaces do not read
  that name, so a repo that also uses them keeps two copies, or a
  `.devcontainer/devcontainer.json` that points at the root image. This is
  the trade-off from #96 option 1, accepted here rather than loosening a
  rule that has held since the first commit.
- `devcontainer up` finds an existing container through its
  `devcontainer.local_folder` label, so the workspace folder identifies the
  container.
- `initializeCommand` runs on the host at `up`. Documented under Security.
- Uploading a Quarto folder skips `.json` files unless
  `.librepaper-share.json` lists them. The docs tell people to list
  `devcontainer.json`.

### pixi, conda, uv

Ordinary prefixes. Their lock and manifest files are allowed project files.
conda needs the environment created beforehand; the docs give the command.

### Windows

- Nix is not available. The preset is shown, with a note that it needs WSL.
  Running through WSL is a custom prefix (`wsl nix develop …`) and untested.
- The devcontainer CLI installs as `devcontainer.cmd`. Rust spawns `.cmd`
  files through `cmd.exe` and escapes the arguments for it. The QA checklist
  includes a Windows dev container render, because argument escaping through
  batch files is where this breaks.
- pixi, conda and uv have native Windows builds; their presets work as they
  are.

### macOS

The QA checklist includes a dev container render on Apple Silicon with an
x86 image (emulation, slow) and a Nix flake render.

## Security

The trust model does not change, and this spec keeps it that way.

- **The project never chooses a command.** The environment is stored on
  the person's machine and set only through a route the shell confirms with
  the full command on screen. A coauthor who adds a `flake.nix` cannot make
  anyone run it; at most they make the app suggest it, and the person still
  sees and approves the exact command.
- **Running an environment runs project code.** A flake's `shellHook`, a
  dev container's `initializeCommand` and `postCreateCommand`, and pixi
  tasks are code that anyone who can edit the project can change, and they
  run on the host. This is the same trust **Run code** already requires:
  rendering a `.qmd` runs its R and Python chunks. The docs say so in one
  plain sentence.
- **A container is not a sandbox.** The dev container mounts the workspace
  and runs `initializeCommand` on the host. People may assume containers
  protect them. The docs say they do not.
- No new path is sent to the browser. `GET environments/<project>` returns
  only what the person saved.

## Docs

- A new section in `docs/notebooks/quarto.md`, "Render inside an
  environment", with one short recipe for each preset, the Nix and dev
  container caveats above, and the Security sentence.
- `docs/desktop-app.md` gets one paragraph pointing to it, since the setting
  applies to every format.
- Fix `docs/notebooks/quarto.md`'s description of **File → Project
  folder…**, removed in `ee82224fa`. This is overdue regardless.
- The macOS and Windows QA checklists gain the entries listed in Tool notes.

## Testing

Helper (`crates/librepaper-companion`):

- `environments.rs` unit tests mirroring `records.rs`: round trip, survives
  restart, reset removes the entry, invalid entries load as absent,
  placeholders and over-long values refused, keys from two deployments stay
  separate.
- `render/tests.rs`, Unix only, with fake programs written into a temp dir
  on PATH:
  - a prefix script that records its argv and execs the rest proves the
    command runs as `prefix ++ record.command`, in the workspace;
  - a missing `prefix[0]` fails before anything runs, with the message
    naming it, even when the record's own program is also missing;
  - a failing setup stops the render and returns its log and exit status;
  - setup and command logs appear in order after the `Environment:` line;
  - cancel during setup cancels the job;
  - a project with no environment runs exactly as before (existing tests
    keep passing unchanged).
- Wire fixtures for the two routes in `service/wire_tests.rs`.

Shell (`desktop/`):

- `prompt_for` returns a prompt for a set, none for a reset, and 400 for a
  reset that carries a command, an unknown field, or a missing prefix.
- The frozen-routes test includes the new route.

Web:

- Unit tests for preset matching (which file suggests which preset, first
  row wins) and for reading `name:` from `environment.yml`.
- A browser test in `web/tests/browser/` for the settings section against a
  fake helper: set, edit, remove, and the badge swap in the command list.
- `untranslated.mjs` for the new keys.

End to end, by hand, before merging to `dev`: render a Quarto project
inside a Nix flake and inside a dev container from the desktop app on Linux.

## Phases

**Phase 0: measure Nix, no code.** Use today's machine-wide render command
to run `nix develop path:. --command quarto render …` from the desktop app
on three projects: a small one, one with a 100 MB data folder, and one with
`_freeze/`. Record time per render and store growth. Also confirm the #96
dev container shim works from the app, not only from a shell. Write the
numbers into this spec. If a path flake is too slow for the small project,
stop and decide whether the cached dev environment belongs in v1.

**Phase 1: helper.** `environments.rs`, the render pipeline changes, the
two routes, wire fixtures, tests. Raise the wire minor.

**Phase 2: shell.** `prompt_for` and its tests, the frozen-routes test.

**Phase 3: web.** Client calls, the settings section, the badge swap, the
suggestion line, messages in all seven catalogues, tests.

**Phase 4: docs and QA.** The docs above, the checklist entries, and the
manual end-to-end renders on Linux, macOS and Windows.

Phases 1 and 2 can run in parallel. Phase 3 needs the routes' shapes, which
this spec fixes, so it can start in parallel against a fake helper.

## Open questions

1. **Per-project only, or also a machine default?** This spec says
   per-project only, because the machine-wide render commands already cover
   "always run Quarto through this". A Nix user with one flake for all their
   papers would have to set each project. Revisit if people ask.
2. **Should setup run before every render?** It does here, because it is
   cheap for the tools measured and running it once per session needs state
   about when it last ran and whether the container has since stopped. If a
   tool turns up whose setup is slow, a "run setup once per app start"
   option is a small addition.
3. **Should the environment travel with the project as a suggestion?** For
   example, a `librepaper.toml` in the project naming a preset, which the
   app would suggest but never apply. Detection by file name covers the
   common cases without a new file format, so this spec leaves it out.
