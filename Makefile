# The README links its screenshots, so they are copied in beside it and served
# at the same relative path the page on GitHub uses.
IMAGES := $(patsubst docs/images/%,web/dist/docs/images/%,$(wildcard docs/images/*.png))
$(BIN) test: $(IMAGES)
web/dist/docs/images/%.png: docs/images/%.png
	@mkdir -p $(dir $@)
	@cp $< $@
# Komodoc. `make` builds the single static binary into dist/.
#
# Three crates under crates/: engine (markdown and typst rendering, CLI and WASM),
# komodoc (server and CLI), text (utilities). The web app in web/ is Svelte,
# bundled by vite and installed by bun. The binary embeds the web build (from
# web/dist) and the WASM renderers, and serves them.

# Local settings, kept out of the repository: the GitHub OAuth app and who may
# publish. Copy .env.example to .env and fill it in. Values are read as Make
# assignments, so write them bare, with no surrounding quotes.
-include .env
export

BIN     := dist/komodoc
# The markdown renderer, built for the browser: the editor previews with it,
# and it is embedded in the binary like every other shell file.
WASM    := web/dist/wasm/markdown.wasm
# Optional, and built separately by `make typst`: see the bottom of this file.
TYPST   := web/dist/wasm/typst.wasm
MODULE  := target/wasm32-unknown-unknown/wasm/komodoc_engine.wasm
# The pages. web/dist is entirely a build output, so it is an input to
# nothing: what the pages are built from lives in web/src and web/public.
SHELL_OUT := web/dist/index.html
WEB     := $(shell find web/src web/public -type f) $(wildcard web/pages/*.html web/package.json web/vite.config.js web/vite.agent.config.js)
# The renderers are generated, so they are not also inputs to themselves.
SOURCES := $(shell find crates -type f -not -path '*/target/*') Cargo.toml README.md

.DEFAULT_GOAL := help
.PHONY: help build test smoke serve seed examples kill clean snapshot wasm typst fmt web

help:  ## Display this help screen
	@printf "\033[1mAvailable commands:\033[0m\n\n"
	@grep -hE '^[a-z.A-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-22s\033[0m %s\n", $$1, $$2}' | sort

build: $(BIN)  ## Build dist/komodoc, with the shell and renderers embedded

# Rebuilt whenever any source, page or renderer changes.
$(BIN): $(SOURCES) $(WASM) $(SHELL_OUT)
	@mkdir -p $(dir $@)
	@cargo build --release -p komodoc
	@cp target/release/komodoc $@
	@echo "$@ ($$(($$(stat -c%s $@) / 1024 / 1024)) MiB) -- deploys on its own"

# The documentation page is the README, so it is copied in to be embedded. The
# tests read it too, so both depend on it rather than on the build.
$(BIN) test: web/dist/README.md

# The README links its screenshots, so they are copied in beside it and served
# at the relative path the page on GitHub uses.
IMAGES := $(patsubst docs/images/%,web/dist/docs/images/%,$(wildcard docs/images/*.png))
$(BIN) test: $(IMAGES)
web/dist/docs/images/%.png: docs/images/%.png
	@mkdir -p $(dir $@)
	@cp $< $@
web/dist/README.md: README.md
	@mkdir -p $(dir $@)
	@cp $< $@

# The suite reads the built shell -- a test that asserts a page names its own
# bundle needs that bundle to exist -- so the pages are built first.
test: $(WASM) $(SHELL_OUT)  ## Run rustfmt, clippy and the test suite
	@cd web && bun run check
	@cargo fmt --check
	@cargo clippy --workspace --all-targets -- -D warnings
	@cargo test --workspace

# The rendered reader, in a real browser. Not part of `test`: it needs the
# built binary and a chromium, and it starts a server of its own on a
# temporary directory. It touches no deployment and no data but its own.
smoke: $(BIN)  ## Drive the reader in headless chromium (needs chromium)
	@command -v chromium >/dev/null || command -v google-chrome >/dev/null || \
		{ echo "no chromium to drive; skipping the browser smoke test"; exit 0; }
	@bun web/tools/browser-smoke.mjs $(BIN)

fmt:  ## Format every crate
	@cargo fmt

# Release builds are described in .github/workflows/release.yml and run when a
# v* tag is pushed. This does the same thing locally, without tagging.
snapshot: $(WASM) $(TYPST) $(SHELL_OUT)  ## Build the release binary locally, without tagging
	@cargo build --release -p komodoc
	@echo "target/release/komodoc"

clean:  ## Remove build output
	@rm -rf dist target/release/komodoc web/dist web/node_modules

# The port is fixed because the GitHub OAuth app's callback URL names it.
PORT       ?= 8081
DATA       ?= komodoc-data
PUBLISHERS ?= any
COMMENTERS ?= anyone

serve: $(BIN)  ## Run the server and open it in Firefox (PORT=, DATA=, PUBLISHERS=, COMMENTERS=)
	@command -v firefox >/dev/null && (sleep 1; firefox http://localhost:$(PORT) >/dev/null 2>&1 &) || true
	@$(BIN) serve --port $(PORT) --data $(DATA) --publishers $(PUBLISHERS) --commenters $(COMMENTERS)

# One example per source format Komodoc accepts, each genuinely produced by the
# tool it is named after. style-guide.html is hand-written, and the .md and the
# .typ are rendered by Komodoc itself at publish time, so none of the three has
# a rule below.
EXAMPLES := examples/bootstrap.html examples/newton.html \
            examples/random-walks.html examples/simpsons-paradox.html \
            examples/style-guide.html examples/regression-tables.md \
            examples/intervals.typ

# Not in the help: a step of `deploy`, not an entry point.
examples: $(EXAMPLES)

# Quarto and Calepin both inline their figures, so each output stands alone.
# Both tools resolve paths relative to the document, so both are run from
# inside examples/ rather than from the repository root.
examples/%.html: examples/%.qmd
	@cd examples && quarto render $(notdir $<) --quiet

examples/%.html: examples/%.typ
	@cd examples && calepin compile $(notdir $<) $(notdir $@) --format html

# marimo and Jupyter come from PyPI rather than from the system, so they are run
# through uv against examples/pyproject.toml. uv fetches its own interpreter, so
# neither Python nor either tool has to be installed to build the examples.
#
# marimo's exporter runs the notebook itself. Export without the notebook code:
# Komodoc serves the rendered result as a static document, not as a live marimo
# session. Its HTML still loads the marimo frontend from a CDN and keeps the
# prose in a JSON island the page hydrates, so this one document is not
# self-contained and carries no seeded annotations -- see seed_examples.rs.
examples/%.html: examples/%.py
	@cd examples && uv run --quiet marimo export html $(notdir $<) -o $(notdir $@) -f --no-include-code

# --execute runs the notebook, --embed-images inlines the figures, and blanking
# the two CDN URLs nbconvert would otherwise link keeps the page self-contained.
examples/%.html: examples/%.ipynb
	@cd examples && uv run --quiet jupyter nbconvert --to html --execute --embed-images \
		--HTMLExporter.mathjax_url='' --HTMLExporter.require_js_url='' \
		--log-level WARN $(notdir $<) --output $(notdir $@)

# Not in the help: it is a step of `deploy`, not a thing to run on its own.
seed: $(BIN) $(EXAMPLES)
	@$(BIN) seed --data $(DATA)

kill:  ## Stop a server started with make serve
	@# The bracket stops the pattern from matching this command line itself.
	@pkill -f '[d]ist/komodoc serve' && echo "stopped" || echo "nothing to stop"

.PHONY: deploy

# No sign-in at all: publishing and commenting are both open, so this needs
# no GitHub OAuth app and no `komodoc login`.
deploy: seed  ## Seed the examples and serve them on this machine, no sign-in
	@$(MAKE) serve PUBLISHERS=anyone COMMENTERS=anyone

# --- the web app -----------------------------------------------------------
#
# Svelte, Skeleton, CodeMirror and Yjs, bundled into the pages the binary
# embeds. The output goes to web/dist, so nothing under that directory is
# edited by hand. The build refuses to run if a page has drifted from the
# design system -- see web/checks/vocabulary.js.

web: $(SHELL_OUT)  ## Build the pages from web/

$(SHELL_OUT): $(WEB) web/dist/README.md
	@command -v bun >/dev/null || { echo "bun is not installed: https://bun.sh"; exit 1; }
	@cd web && bun install --silent && bun run build

# --- the browser renderers -------------------------------------------------
#
# One crate, built twice, each with one renderer: markdown is a few hundred
# kilobytes and travels with the shell, while typst is the compiler and the
# fonts it sets documents in -- thirty megabytes, fetched only by someone who
# opens a typst document to edit.

wasm: $(WASM)  ## Build the markdown renderer for the browser

$(WASM): $(shell find crates/engine/src -type f) crates/engine/Cargo.toml crates/engine/document.css
	@cargo build --profile wasm --target wasm32-unknown-unknown -p komodoc-engine \
		--no-default-features --features markdown
	@mkdir -p $(dir $@)
	@cp $(MODULE) $@
	@echo "$@ ($$(($$(stat -c%s $@) / 1024)) KiB)"

typst: $(TYPST)  ## Build the typst renderer for the browser (slow: ~30 MB)

$(TYPST): $(shell find crates/engine/src -type f) crates/engine/Cargo.toml crates/engine/document.css
	@cargo build --profile wasm --target wasm32-unknown-unknown -p komodoc-engine \
		--no-default-features --features typst
	@mkdir -p $(dir $@)
	@cp $(MODULE) $@
	@echo "$@ ($$(($$(stat -c%s $@) / 1024 / 1024)) MiB)"
