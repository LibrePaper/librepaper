---
title: "Building from source"
---

## Building from source

The binary embeds the web build. Browser renderers are fetched separately from the configured asset mirror.

- `web/` (Svelte, Skeleton, CodeMirror 6, Loro): bun and vite
- `crates/librepaper/` (server and CLI): cargo

Renderer implementations live in `wasm-*` repositories. The browser uses pinned WebAssembly artifacts; the production server binary does not link native document renderers. Some renderer crates remain dev-dependencies for fixtures and tests. The web build writes to `web/dist`, which the binary embeds.

```sh
make web                # pages from web/
tools/pins fetch        # pinned browser renderers
make build              # dist/librepaper with embedded pages
make install            # to ~/.local/bin (override PREFIX= or BINDIR=)
make test               # rustfmt, clippy and test suite
tools/suite external    # Quarto/R/Python and local-service integrations
```

Build needs [bun](https://bun.sh) and Node.js. Rust builds for `x86_64-unknown-linux-gnu` also require `mold`, configured in `.cargo/config.toml`. Browser renderers are fetched from exact tags and SHA256 digests in `assets.lock`. To update a renderer: `tools/pins update wasm wasm-markdown v0.2.0`, then review the lockfile diff.

The four browser modules (markdown, bibliography, citations, typst) are not embedded. `tools/pins fetch` fetches them to `web/wasm/` (ignored). `tools/deploy-assets publish` publishes to the asset mirror at `wasm/<sha256>/<module>` (SHA256 from `assets.lock`). The same lock pins LaTeX at `latex/<sha256>/`. The server passes these URLs to browsers on the mirror named by `--asset-mirror`. See [asset mirrors](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/asset-mirrors.md).

`make demo` runs the site, application, local companion and simulated activity. It runs `tools/deploy-assets check` on the LaTeX mirror at `MIRROR=` (default `../wasm-latex/mirror`), then Docker PostgreSQL from `tools/db dev` or `LIBREPAPER_DATABASE_URL`. GitHub sign-in comes from `tools/deploy-keys.yaml` when `sops` can decrypt it; otherwise none. Use `LIBREPAPER_PUBLISHERS=any LIBREPAPER_COMMENTERS=anyone` for local development.

## The interface

Pages use [Skeleton](https://skeleton.dev) on Tailwind 4. Skeleton provides buttons, cards, inputs, tables, dialogs, tooltips and toasts. `web/src/styles/theme.css` colours everything from four colours (palette written once). Three rules, enforced by `make test`:

1. Colours and sizes come from theme, not hex values or arbitrary Tailwind sizes.
2. A control is a component; there is one `IconButton`.
3. Layout comes from `Page`, `Stack` and `Row`, assembled not measured.

Agent highlights are drawn on the document origin (outside this stylesheet). Identifying colours in shared sessions travel over the wire (not a local decision).
