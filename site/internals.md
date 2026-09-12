---
title: "Internals"
---

These are design notes on how LibrePaper is built.

## The look

The pages are [Skeleton](https://skeleton.dev) on Tailwind 4. Skeleton supplies the furniture -- buttons, cards, inputs, tables, dialogs, tooltips, toasts -- and `web/src/styles/theme.css` colours all of it from LibrePaper's own four colours, so the palette is written down once and nowhere else.

### Design rules

Three rules keep a growing application looking like one application, and `make test` enforces them:

1. A colour or a size comes from the theme. A hex value or an arbitrary Tailwind size in a component is a decision made twice.
2. A control is a component. There is one `IconButton`, so there cannot be a fourth kind of button that is almost like the other three.
3. Layout comes from `Page`, `Stack` and `Row`, so a new screen is assembled rather than measured.

Two things sit outside that system deliberately: the agent, which paints highlights inside a document on another origin where none of this stylesheet reaches it, and the colours identifying people in a shared editing session, which travel over the wire to other browsers.

## The editor

The source is edited in CodeMirror 6, bound to a Yjs document. Two people typing in the same sentence converge without either waiting for the other, each keeps their own undo history, and each sees the other's caret where it actually is, labelled with their name. The server relays those updates and applies them to the copy it keeps, which is the document.

A reader fetches publication metadata and the stored HTML display bundle. The annotation connection carries comments and publication notices without source state. Editors retain Yjs synchronization and local rendering; CodeMirror loads when the editor opens. The [publication protocol](https://github.com/LibrePaper/librepaper/blob/main/docs/protocol/publication.md) describes explicit activation, access checks, caching and storage limits.

## Building from source

The application embeds the web build and four prebuilt browser renderers.

| | What it is | Built by |
| --- | --- | --- |
| `web/` | the pages: Svelte, Skeleton, CodeMirror 6, Yjs | bun and vite |
| `crates/librepaper/` | the server and the command line | cargo |

Renderer implementations live in the `wasm-*` repositories. Cargo links their pinned native libraries, and the browser uses WASM artifacts from the same release tags. The web build writes into `web/dist`, which the binary embeds; nothing under that directory is edited by hand.

```sh
make web      # the pages, from web/
make wasm     # all four pinned browser renderers
make build    # dist/librepaper, with the pages and renderers embedded
make install  # build and install to ~/.local/bin (override PREFIX= or BINDIR=)
make test     # rustfmt, clippy and the test suite
make test-external          # Quarto/R/Python and local-service integrations
make test-release-workloads # supported limits and diagnostic workloads
```

`make build` needs [bun](https://bun.sh) and Node.js. The four browser renderers are fetched from the exact tags and SHA256 digests in `wasm-modules.lock`; `make wasm-check` verifies that those tags also match the native renderer dependencies without network access. To update one renderer, name both values explicitly, for example `make wasm-update REPO=wasm-markdown TAG=v0.2.0`, then review the resulting Cargo and lockfile diff.

### Deployment and initial setup

`make deploy` runs the normal application locally. Configure an OAuth app in `.env` (see `.env.example`), then sign in: every new account receives five private, editable examples, one each in HTML, Markdown, Typst, LaTeX, and Quarto. These are your own documents. In **Share**, create a Read, Comment, or Edit link and open it in a separate browser or private window to try that role. Use `PUBLISHERS=any COMMENTERS=anyone` for local development. Publishing still requires an authenticated account, while comments may remain anonymous.

Examples are created once per new account, survive restarts, and stay deleted if you remove them. Existing accounts are left unchanged. `make deploy` never resets or seeds the shared catalogue. The five starter sources ship inside the binary; signing in does not require Quarto or a checkout of this repository.
