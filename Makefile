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
.PHONY: help build test smoke serve seed examples kill clean snapshot wasm typst fmt web fuzz

help:  ## Display this help screen
	@printf "\033[1mAvailable commands:\033[0m\n\n"
	@grep -hE '^[a-z.A-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-22s\033[0m %s\n", $$1, $$2}' | sort

build: $(BIN)  ## Build dist/komodoc, with the shell and renderers embedded

# Rebuilt whenever any source, page or renderer changes.
# Once the optional Typst module has been opted into, keep it in step with the
# shell and native compiler. Otherwise a new binary can embed an old HTML ABI.
$(BIN): $(SOURCES) $(WASM) $(wildcard $(TYPST)) $(SHELL_OUT)
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
test: $(WASM) $(wildcard $(TYPST)) $(SHELL_OUT)  ## Run rustfmt, clippy and the test suite
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

# The fuzz targets in fuzz/: libFuzzer over the functions that read what a
# peer sends -- the word diff and merge, the path rules, the shared document
# and its repair, and a v1 update against the admission ceilings. Each runs
# for FUZZ_SECONDS, then stops; a finding is left in fuzz/artifacts/. Needs a
# nightly toolchain and cargo-fuzz (`cargo install cargo-fuzz`). No sanitizer:
# the code under test is safe Rust, the oracle is the assertions, and the
# sanitizer doubles the build time to find nothing the panics do not.
FUZZ_SECONDS ?= 60
# Every target unless told otherwise. `update` is known to fail on a panic
# inside yrs (the ignored test in crates/komodoc/src/tests/directories.rs
# reproduces it), so CI runs the other three until that is settled.
FUZZ_TARGETS ?= $(shell cd fuzz && cargo fuzz list)
fuzz: $(WASM) $(SHELL_OUT)  ## Run every fuzz target for FUZZ_SECONDS (default 60) each
	@command -v cargo-fuzz >/dev/null || { echo "cargo-fuzz is not installed: cargo install cargo-fuzz"; exit 1; }
	@cd fuzz && for target in $(FUZZ_TARGETS); do \
		echo "fuzzing $$target for $(FUZZ_SECONDS)s"; \
		cargo fuzz run -s none $$target -- -max_total_time=$(FUZZ_SECONDS) || exit 1; \
	done

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
# Who owns the seeded examples. Empty means nobody, and a document nobody owns
# is everybody's: each visitor holds the owner's controls on it. That is fine
# where nobody signs in and wrong where somebody does, since the point of a
# sign-in is that everybody else meets the examples as a reader or a
# commenter. Defaults to the one account `--publishers` names, when it names
# exactly one; `any`, `anyone` and a list name nobody in particular.
comma := ,
OWNER      ?= $(if $(filter any anyone,$(PUBLISHERS)),,$(if $(findstring $(comma),$(PUBLISHERS)),,$(PUBLISHERS)))
# Whether .env has supplied a way to sign people in. `deploy` reads this to
# decide between a server that asks for a sign-in and one that never does.
SIGN_IN    := $(strip $(KOMODOC_GITHUB_CLIENT_ID)$(KOMODOC_GOOGLE_CLIENT_ID))
# Where LaTeX distributions come from: a mirror directory or an https bucket.
# Empty means no `--latex`, so `.tex` documents are stored and shown but not
# compiled. `deploy` below passes the bare flag, which is the project's own
# mirror; `LATEX=` names another for either target.
LATEX      ?=
LATEX_FLAG ?= $(if $(LATEX),--latex $(LATEX))

serve: $(BIN)  ## Run the server and open it in Firefox (PORT=, DATA=, PUBLISHERS=, COMMENTERS=, LATEX=)
	@command -v firefox >/dev/null && (sleep 1; firefox http://localhost:$(PORT) >/dev/null 2>&1 &) || true
	@$(BIN) serve --port $(PORT) --data $(DATA) --publishers $(PUBLISHERS) --commenters $(COMMENTERS) $(LATEX_FLAG)

# One example per source format Komodoc accepts. Only the HTML one is built:
# Quarto renders it from the .qmd beside it. The .md and the .typ are rendered
# by Komodoc itself at publish time, and the .tex is compiled by the browser
# that opens it, so none of those three has a rule below.
EXAMPLES := examples/bootstrap.html examples/regression-tables.md \
            examples/intervals.typ examples/standard-errors.tex

# Not in the help: a step of `deploy`, not an entry point.
examples: $(EXAMPLES)

# Quarto inlines its figures, so the output stands alone. It resolves paths
# relative to the document, so it is run from inside examples/ rather than
# from the repository root.
examples/%.html: examples/%.qmd
	@cd examples && quarto render $(notdir $<) --quiet

# Not in the help: it is a step of `deploy`, not a thing to run on its own.
seed: $(BIN) $(EXAMPLES)
	@$(BIN) seed --data $(DATA) $(if $(OWNER),--owner $(OWNER))

kill:  ## Stop a server started with make serve
	@# The bracket stops the pattern from matching this command line itself.
	@pkill -f '[d]ist/komodoc serve' && echo "stopped" || echo "nothing to stop"

.PHONY: deploy latex-check latex-mirror latex-smoke

latex-mirror:  ## Build the pinned WasmTex release and its TeX package set (requires network)
	@node latex/tools/wasmtex.mjs
	@node latex/tools/wasmtex.mjs --scheme
	@node latex/tools/check-mirror.mjs latex/mirror

latex-check:
	@node latex/tools/check-mirror.mjs $(LATEX)

latex-smoke: $(BIN)  ## Compile and display the seeded LaTeX example in Chromium against MIRROR=
	@node latex/tools/check-mirror.test.mjs
	@node latex/tools/check-mirror.mjs $(MIRROR)
	@node web/tools/latex-e2e.mjs $(BIN) browser examples/standard-errors.tex 120 $(MIRROR)

# The whole app on this machine. With a GitHub app in .env (see .env.example)
# it asks people to sign in exactly as a deployment does, and the examples
# belong to OWNER: sign in as them to hold every right, and open a link they
# minted in a private window to see what a reader or a commenter gets.
# Without one, publishing and commenting are both open, so a trial needs no
# OAuth app and no `komodoc login`. LaTeX is on either way, from the
# project's mirror unless LATEX= names another, so the seeded .tex example
# compiles.
deploy: latex-check $(BIN) $(EXAMPLES)  ## Seed the examples and serve them here; sign-in when .env has an OAuth app, LaTeX on (LATEX=)
ifneq ($(SIGN_IN),)
	@$(MAKE) seed serve LATEX_FLAG="--latex $(LATEX)"
else
	@$(MAKE) seed serve OWNER= PUBLISHERS=anyone COMMENTERS=anyone LATEX_FLAG="--latex $(LATEX)"
endif

# The deployment keys -- the Cloudflare token, the endpoints, the GitHub app
# -- live sops-encrypted in deploy/keys.yaml. A target cannot export into the
# shell that ran make, so `secrets` opens a subshell with them decrypted in
# its environment; exit it to drop them. For one command instead of a shell:
#
#     sops exec-env deploy/keys.yaml 'wrangler deploy'
KEYS ?= deploy/keys.yaml
.PHONY: secrets latex-push

secrets:  ## Open an interactive shell with the sops-encrypted deployment keys in its environment
	@test -f $(KEYS) || { echo "no $(KEYS)"; exit 1; }
	@test -t 0 || { echo "make secrets opens an interactive subshell and needs a terminal" >&2; echo "use: sops exec-env $(KEYS) '<command>'" >&2; exit 2; }
	@echo "$(KEYS) is loaded in this shell; exit to drop it"
	@sops exec-env $(KEYS) "$${SHELL:-/bin/sh}"

# The LaTeX mirror, on Cloudflare. It is served as a worker made of static
# files, deploy/latex/wrangler.toml, so the only credential it needs is the
# CLOUDFLARE_API_TOKEN that `make secrets` provides. What goes up is the
# WasmTex release, its TeX Live snapshot files and the Biber VM image -- see
# latex/tools/README.md -- and not the legacy distributions an older mirror
# directory may still hold. A file named by its digest is cached forever; the
# manifest is not cached at all. Build the mirror first:
#
#     node latex/tools/wasmtex.mjs                      # the engine release
#     node latex/tools/wasmtex-record.mjs               # the package set the corpus asks for
#     node latex/tools/wasmtex.mjs --texlive-root icudt68l.dat
#     node latex/tools/biber-vm/build.mjs               # the Biber VM (needs Docker)
#
# Deploys upload only files whose content changed, so updating is cheap.
MIRROR ?= latex/mirror

latex-push: latex-smoke  ## Verify and push the LaTeX mirror to Cloudflare (run inside make secrets)
	@node latex/tools/check-mirror.mjs $(MIRROR)
	@test -n "$$CLOUDFLARE_API_TOKEN" || { echo "CLOUDFLARE_API_TOKEN is not set; run this inside make secrets"; exit 1; }
	@# The three distributions the card does not show, and the download cache
	@# mirror.mjs keeps beside them, which holds the release archives whole.
	@printf '.cache/\nbusytex/\ntexlyre-busytex/\nswiftlatex-xetex/\nswiftlatex-pdftex/\npackages/\n' > $(MIRROR)/.assetsignore
	@printf '/*\n  Cache-Control: public, max-age=31536000, immutable\n/manifest.json\n  Cache-Control: no-store\n' > $(MIRROR)/_headers
	@cd deploy/latex && bunx wrangler deploy --assets "$(abspath $(MIRROR))"
	@echo "serve with: komodoc serve --latex https://komodoc-latex.<account>.workers.dev/"

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

$(WASM): $(shell find crates/engine/src crates/text/src -type f) crates/engine/Cargo.toml crates/text/Cargo.toml crates/engine/document.css
	@cargo build --profile wasm --target wasm32-unknown-unknown -p komodoc-engine \
		--no-default-features --features markdown
	@mkdir -p $(dir $@)
	@cp $(MODULE) $@
	@echo "$@ ($$(($$(stat -c%s $@) / 1024)) KiB)"

typst: $(TYPST)  ## Build the typst renderer for the browser (slow: ~30 MB)

$(TYPST): $(shell find crates/engine/src crates/text/src -type f) crates/engine/Cargo.toml crates/text/Cargo.toml crates/engine/document.css
	@cargo build --profile wasm --target wasm32-unknown-unknown -p komodoc-engine \
		--no-default-features --features typst
	@mkdir -p $(dir $@)
	@cp $(MODULE) $@
	@echo "$@ ($$(($$(stat -c%s $@) / 1024 / 1024)) MiB)"
