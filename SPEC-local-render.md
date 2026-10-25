# Local renders

Status: building as Kata 0n7d, 2026-10-25.

## Goal

The desktop app renders a document with tools installed on the user's computer: TeX Live, Typst, Quarto, or anything else that turns files into a PDF or HTML page. Nothing goes over the network on LibrePaper's side. The tools may still fetch on their own (Typst `@preview` packages, MiKTeX on-the-fly installs, Quarto extensions), and the settings note says so.

Every local render uses one mechanism: write the project's files to a folder, run one command there, read one result file. The sidecar knows nothing about any particular tool.

## Decisions

- **Full commands.** The user edits the whole argv, not extra flags. There is no injected argument the user cannot see.
- **One pipeline.** Quarto, Typst, LaTeX and Calepin go through the same code. Calepin stops being an integration and becomes a command a user may type.
- **No Quarto cell capture.** The Lua capture filter and the result bundle it feeds are deleted.
- **No render policies, profile or parameters.** Default, refresh and frozen become whatever the command says (`--use-freezer`, `--profile`, `-P`). The profile and parameter UI is deleted.
- **No live watch processes.** `quarto preview` and `calepin watch` are deleted. A change re-runs the one-shot command and cancels the run in progress.
- **`--embed-resources` is a default, not a rule.** It is in the default Quarto HTML command. A user who removes it gets broken CDN resources in the preview frame.
- **Run code gates every local render.** A format set to "This computer" renders locally only while the Run code switch is on.
- **The browser-only LaTeX rule is lifted.** `renderers.js` no longer excludes `latex` from local renders.

## The tool record

A tool is one record per format and output:

```toml
# <state_home>/librepaper/local/integrations.json holds the same fields as JSON
[latex.pdf]
command = ["latexmk", "-pdf", "-cd", "-synctex=1", "-interaction=nonstopmode", "-file-line-error", "{input}"]
result  = "{stem}.pdf"

[typst.pdf]
command = ["typst", "compile", "--root", ".", "--font-path", ".", "{input}", "{stem}.pdf"]
result  = "{stem}.pdf"

[quarto.html]
command = ["quarto", "render", "{input}", "--to", "html", "--embed-resources", "--standalone"]
result  = "{stem}.html"

[quarto.pdf]
command = ["quarto", "render", "{input}", "--to", "pdf"]
result  = "{stem}.pdf"

[quarto.docx]
command = ["quarto", "render", "{input}", "--to", "docx"]
result  = "{stem}.docx"

# markdown.html, markdown.pdf and markdown.docx default to the same three
# Quarto commands.
```

- `{input}` is the entry file's path relative to the workspace root, for example `chapters/main.tex`.
- `{stem}` is the same path without its extension, for example `chapters/main`.
- These two are the only placeholders. Any other `{...}` is refused when the command is saved.
- The command runs with the workspace root as its working directory. `-cd` makes latexmk write next to the source, so `{stem}.pdf` holds for an entry file in a subfolder.
- `argv[0]` is a program name looked up on the login-shell PATH the app already caches, or an absolute path.
- The built-in defaults live in one Rust table. A record the user never edited is not written to the file, so a later release can change a default.
- The existing validation stays: at most 64 arguments, each non-empty, at most 4096 bytes, no NUL.

## The pipeline

1. **Admit.** The page sends a job. It never sends a command. The sidecar picks the tool record from `(format, output)`.
2. **Write.** The files go into the project's persistent workspace. That is the granted project folder when `binding_id` names one, through `sync_granted_folder` with its `.local` copies, and otherwise the hosted workspace (`binding_id` `"hosted"`), through `sync_hosted_workspace`. The folder is kept between renders so `.aux`, `.bbl`, Typst and Quarto caches survive and latexmk stays incremental. The service-owned temporary copy per build is gone.
3. **Run.** The command goes through `run_confined_logged`: process group, cancellation, deadline, captured log. A new job for the same workspace cancels the one running there, so two commands never share a folder.
4. **Read.** The sidecar reads `result`. For a PDF it also reads `{stem}.synctex.gz`, or `{stem}.synctex`, when present. HTML is returned as the one file; the default commands embed their resources.
5. **Answer.** A missing result file with exit status 0 is a failure whose message names the expected path.

Results, the PDF and the build files land in the granted folder too. That is what a local TeX user expects.

## Wire contract

The transport stays as it is: multipart `POST jobs` (a `job` JSON part plus one `file` part per project file, named by its path), `GET jobs/{id}` until `done`, `failed` or `canceled`, `GET jobs/{id}/files/{name}` checked against the size and SHA-256 in `status.outputs`, `POST jobs/{id}/cancel`.

The `job` part:

```json
{
  "protocol": 3,
  "project": "<slug>",
  "origin": "https://librepaper.org",
  "snapshot": "<tree digest>",
  "generation": 1761400000000,
  "format": "latex",
  "output": "pdf",
  "entrypoint": "chapters/main.tex",
  "binding_id": "hosted",
  "manifest": [{ "path": "chapters/main.tex", "sha256": "<hex>", "size": 1234 }],
  "options": { "deadline_seconds": 300 }
}
```

- `format` is one of `latex`, `typst`, `markdown`, `quarto`. `output` is one of `pdf`, `html`, `docx`. A pair with no tool record is refused.
- Protocol 2 requests are refused with a message asking to update the page. There is no compatibility path.

The finished status carries `exit`, `error` and these `outputs`: `artifact` (the result file), `synctex` (when found), `log`. Nothing else: no `diagnostics`, no `provenance` beyond `{ backend: "local", format, output }`, no cells.

Tool records:

- `GET capabilities` reports `renderers`, keyed `"<format>.<output>"`: `{ command, result, edited, found, version }`. `version` is the first line of `argv[0] --version`, or null.
- `GET state` reports the same map under `settings.renderers`, in place of `settings.integrations`.
- `PUT renderers/<format>.<output>` with `{ command, result }` saves after the native dialog shows the full command. `{ "reset": true }` returns the record to the built-in default and needs no dialog.
- `PUT integrations/{name}` is deleted.

`PUT workspace` stays: the editor writes into a granted folder outside renders too.

## Diagnostics and SyncTeX

- The sidecar returns the log and nothing else about errors. `builders/diagnostics.rs` is deleted.
- The page turns the log into diagnostics by format. LaTeX uses `parse` in `web/src/lib/latex/log.js`. Typst gets the regex from `diagnostics.rs` ported to JS. Quarto shows the log only.
- `parse` in `web/src/lib/synctex.js` already takes bytes, gunzips them and matches absolute paths by their project-relative ending. The local SyncTeX file goes to it unchanged.

## Discovery

- For each format the sidecar reports whether `argv[0]` of the current command resolves, and the first line of `argv[0] --version`.
- This replaces the per-tool discovery in `discovery.rs` and `preview/calepin.rs`. The cache rule stays: rescan on request, on a changed PATH, or when a cached executable changes on disk.

## Settings

Settings > Rendering keeps one section per format. Each section gains:

- **Renders in: Browser / This computer.** "This computer" appears only in the desktop app. Quarto has no browser engine, so its section shows only the command.
- **One command row per output** when "This computer" is chosen, built from `ToolCommand.svelte`. It edits the whole command as one line, shows whether the program was found and its version, and has Reset to default. Saving goes through the existing native confirmation dialog that shows the full command.
- A one-line note: the tool may download packages on its own.

Deleted from Settings: the Quarto profile and parameters block, the Calepin section, the Quarto and Calepin path-and-args rows.

## Run code

- When a format is set to "This computer" and Run code is off, LaTeX, Typst and Markdown render in the browser. Quarto shows "Turn on Run code to render on this computer."
- The stored "This computer" choice is no longer reset to the browser when a document opens. The Run code switch alone decides, and it still starts off for every document.
- Run code is offered for every editable document whose format is set to "This computer", in the desktop app only.

## Deleted

Rust, in `crates/librepaper-companion/src/local/`:
- `quarto/` except the workspace sync, which moves to a new `render/` module. The resource closure in `quarto/collect.rs` goes.
- `quarto_capture.rs`, `preview/`, the preview routes in `service/`.
- `builders/mod.rs` typed plans and `builders/diagnostics.rs`. `builders/runner.rs` becomes the pipeline's run step.
- `engine_adapter.rs` dispatch by builder.
- Quarto job options, render policies, render scope and builder-specific `BuildInputs` in `protocol.rs`.
- The Quarto bundle and cell result types in `crates/librepaper-document` (`results/`, `quarto/`). The server still uses `results::document_metadata`, `results::sha256`, `ExecutionEngine` and `DraftFormat`; those stay.
- The `/api/documents/{slug}/quarto/checkpoint` route and `quarto_checkpoint.rs` in `librepaper-server`. Nothing calls it.

Web:
- The Quarto request and preview functions in `web/src/lib/desktop/client.js`, `QUARTO_JOB_KINDS`, `quarto-options.js`.
- Live-preview handling in `web/src/lib/reader/preview-render.js` (`livePreviewOwnsPane`) and `Reader.svelte`.
- The `format !== "latex"` exclusion at `web/src/lib/renderers.js:174` and the two Quarto and builder branches below it, which become one call.

Docs: the pages on Quarto profiles, parameters and Calepin.

## Checks before building (done 2026-10-25)

1. Nothing user-visible reads Quarto cell results. The export path in `web/src/lib/reader/exports.js` builds through local Quarto with profile and parameters; it moves to the new job with neither.
2. Windows resolves `argv[0]` with `PATHEXT`. TeX Live ships `latexmk.exe` wrappers, MiKTeX ships `latexmk.exe`, Quarto and Typst ship `.exe` files.

## Work split

Two parallel workers, each in its own worktree:

- **Rust.** The `render/` module, the tool table and defaults, the reshaped `integrations.json`, the job route, discovery, and every Rust deletion above.
- **Web.** The single job call in `renderers.js`, Settings > Rendering, the Run code rule, diagnostics and SyncTeX, and every web deletion above.

The wire shape in "The pipeline" is the contract between them. Docs follow once both are merged.

## Done

- `make test` is green.
- In the real desktop app under Xvfb, a LaTeX document with a bibliography, a Typst document and a Quarto document each render with the default command, then with an edited one.
- A second edit during a long LaTeX run cancels the first.
- With Run code off, LaTeX falls back to the browser and Quarto shows its message.
