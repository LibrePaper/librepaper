---
title: "Building from source"
---

## Building from source

The binary embeds the web build and pinned browser renderers.

| Built by |
| --- |
| `web/` (Svelte, Skeleton, CodeMirror 6, Loro): bun and vite |
| `crates/librepaper/` (server and CLI): cargo |

Renderer implementations live in `wasm-*` repositories. Cargo links pinned native libraries; the browser uses WebAssembly artifacts from the same release tags. The web build writes to `web/dist`, which the binary embeds.

```sh
make web                # pages from web/
tools/pins fetch        # pinned browser renderers
make build              # dist/librepaper with pages and renderers
make install            # to ~/.local/bin (override PREFIX= or BINDIR=)
make test               # rustfmt, clippy and test suite
tools/suite external    # Quarto/R/Python and local-service integrations
```

Build needs [bun](https://bun.sh) and Node.js. Browser renderers are fetched from exact tags and SHA256 digests in `assets.lock`. To update a renderer: `tools/pins update wasm wasm-markdown v0.2.0`, then review the lockfile diff.

The four browser modules (markdown, bibliography, citations, typst) are not embedded. `tools/pins fetch` fetches them to `web/wasm/` (ignored). `tools/deploy-assets publish` publishes to the asset mirror at `wasm/<sha256>/<module>` (SHA256 from `assets.lock`). The same lock pins LaTeX at `latex/<sha256>/`. The server passes these URLs to browsers on the mirror named by `--asset-mirror`. See [asset mirrors](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/asset-mirrors.md).

`make demo` runs the site, application, local companion and simulated activity. It runs `tools/deploy-assets check` on the LaTeX mirror at `MIRROR=` (default `../wasm-latex/mirror`), then `make serve` with Docker PostgreSQL from `tools/db dev` (or `LIBREPAPER_DATABASE_URL`). GitHub sign-in comes from `tools/deploy-keys.yaml` when `sops` can decrypt it; otherwise the demo runs without sign-in. Every new account gets five private editable examples (HTML, Markdown, Typst, LaTeX, Quarto). Use `PUBLISHERS=any COMMENTERS=anyone` for local development. Examples are created once per account, survive restarts, and stay deleted if removed. Existing accounts are unchanged; the catalogue is never reset or seeded. Starter sources ship in the binary.

## The interface

Pages use [Skeleton](https://skeleton.dev) on Tailwind 4. Skeleton provides buttons, cards, inputs, tables, dialogs, tooltips and toasts. `web/src/styles/theme.css` colours everything from four colours (palette written once). Three rules, enforced by `make test`:

1. Colours and sizes come from theme, not hex values or arbitrary Tailwind sizes.
2. A control is a component; there is one `IconButton`.
3. Layout comes from `Page`, `Stack` and `Row`, assembled not measured.

Agent highlights are drawn on the document origin (outside this stylesheet). Identifying colours in shared sessions travel over the wire (not a local decision).
