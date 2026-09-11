# LibrePaper. `make` builds the single static binary into dist/.
#
# The application crate provides the server and CLI and links the pinned
# renderer libraries directly. The web app in web/ is Svelte,
# bundled by vite and installed by bun. The binary embeds the web build (from
# web/dist) and the WASM renderers, and serves them.

# Local settings, kept out of the repository: the GitHub OAuth app and who may
# publish. Copy .env.example to .env and fill it in. Values are read as Make
# assignments, so write them bare, with no surrounding quotes.
-include .env
export

BIN     := dist/librepaper
PREFIX  ?= $(HOME)/.local
BINDIR  ?= $(PREFIX)/bin
DESTDIR ?=
# The markdown renderer, fetched for the browser: the editor previews with it,
# and it is embedded in the binary like every other shell file.
WASM    := web/dist/wasm/markdown.wasm
BIB     := web/dist/wasm/bibliography.wasm
CITES   := web/dist/wasm/citations.wasm
# Fetched, not built: see wasm-modules.lock and the bottom of this file.
TYPST   := web/dist/wasm/typst.wasm
# The pages. web/dist is entirely a build output, so it is an input to
# nothing: what the pages are built from lives in web/src and web/public.
SHELL_OUT := web/dist/index.html
WEB     := $(shell find web/src web/public -type f) $(wildcard web/pages/*.html web/package.json web/vite.config.js web/vite.agent.config.js)
# The renderers are generated, so they are not also inputs to themselves.
SOURCES := $(shell find crates -type f -not -path '*/target/*') $(shell find skills) $(shell find examples -type f) Cargo.toml README.md

.DEFAULT_GOAL := help
.PHONY: help build install test test-external test-release-workloads smoke serve seed examples kill clean snapshot wasm wasm-check wasm-update fmt web fuzz

help:  ## Display this help screen
	@printf "\033[1mAvailable commands:\033[0m\n\n"
	@grep -hE '^[a-z.A-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-22s\033[0m %s\n", $$1, $$2}' | sort

build: $(BIN)  ## Build dist/librepaper, with the shell and renderers embedded

install: $(BIN)  ## Build and install to ~/.local/bin (override PREFIX= or BINDIR=)
	@install -d "$(DESTDIR)$(BINDIR)"
	@install -m 755 "$(BIN)" "$(DESTDIR)$(BINDIR)/librepaper"
	@echo "Installed $(DESTDIR)$(BINDIR)/librepaper"

# Rebuilt whenever any source, page or renderer changes.
# Keep every required renderer in step with the shell and native compiler.
$(BIN): $(SOURCES) $(WASM) $(BIB) $(CITES) $(TYPST) $(SHELL_OUT) | wasm
	@mkdir -p $(dir $@)
	@cargo build --release -p librepaper
	@# Copied beside and renamed over: a server running from the old binary
	@# keeps its file open, which makes an overwrite fail with "text file
	@# busy" while a rename just leaves it holding the old inode.
	@cp target/release/librepaper $@.tmp && mv -f $@.tmp $@
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
test: wasm $(SHELL_OUT)  ## Run rustfmt, clippy and the test suite
	@cd web && bun run check
	@cargo fmt --check
	@node web/tools/pin-tools.test.mjs
	@node latex/tools/check-mirror.test.mjs
	@cargo clippy --workspace --all-targets -- -D warnings
# nextest runs each case in its own process, so one crate's failure does not
# abandon the crates after it and a hung case is named rather than waited on.
# It is not required: `cargo test` still runs everything, just more slowly and
# only up to the first crate that fails.
#
# LIBREPAPER_FSYNC=false is read by the storage layer. The suite's own crate
# relaxes durability under `cfg(test)` on its own; this is for the cases that
# spawn the real binary, which is built without it. Nothing a test writes
# outlives the run, so there is no crash for an fsync to survive.
	@command -v cargo-nextest >/dev/null \
		&& LIBREPAPER_FSYNC=false cargo nextest run --workspace \
		|| { echo "cargo-nextest not installed (cargo install cargo-nextest); using cargo test"; \
		     LIBREPAPER_FSYNC=false cargo test --workspace; }

# Explicit suites kept out of the default inventory because they require
# host tools or intentionally exercise release-sized resource ceilings.
test-external:  ## Run Quarto/R/Python and real local-service integration tests
	@cargo test -p librepaper quarto -- --ignored --nocapture
	@cargo test -p librepaper quarto_managed_preview_starts_serves_and_stops -- --ignored --nocapture

test-release-workloads:  ## Run supported-limit and diagnostic workloads
	@cargo test -p librepaper the_supported_maximum_source_survives_a_restart -- --ignored --nocapture
	@cargo test -p librepaper measure_source_history_profiles -- --ignored --nocapture
	@cargo test -p librepaper profile_unchanged_resident_metadata -- --ignored --nocapture
	@cargo test -p librepaper persistence_capacity_workload -- --ignored --nocapture
	@cargo test -p librepaper persistence_default_limits_refuse_without_discarding_dirty_rooms -- --ignored --nocapture
	@cargo test -p librepaper mcp_protocol_transfer_benchmark -- --ignored --nocapture

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
# inside yrs (the ignored test in crates/librepaper/src/tests/directories.rs
# reproduces it), so CI runs the other three until that is settled.
FUZZ_TARGETS ?= $(shell cd fuzz && cargo fuzz list)
# The global export would evaluate this for every recipe, even outside fuzz.
unexport FUZZ_TARGETS
fuzz: wasm $(SHELL_OUT)  ## Run every fuzz target for FUZZ_SECONDS (default 60) each
	@command -v cargo-fuzz >/dev/null || { echo "cargo-fuzz is not installed: cargo install cargo-fuzz"; exit 1; }
	@cd fuzz && for target in $(FUZZ_TARGETS); do \
		echo "fuzzing $$target for $(FUZZ_SECONDS)s"; \
		cargo fuzz run -s none $$target -- -max_total_time=$(FUZZ_SECONDS) || exit 1; \
	done

# Release builds are described in .github/workflows/release.yml and run when a
# v* tag is pushed. This does the same thing locally, without tagging.
snapshot: wasm $(SHELL_OUT)  ## Build the release binary locally, without tagging
	@cargo build --release -p librepaper
	@echo "target/release/librepaper"

clean:  ## Remove build output
	@rm -rf dist target/release/librepaper web/dist web/node_modules

# The port is fixed because the GitHub OAuth app's callback URL names it.
PORT       ?= 8081
DATA       ?= librepaper-data
# Ownership for the manual admin seed command, derived from .env's own
# LIBREPAPER_PUBLISHERS (exported above). Account onboarding creates private
# copies for each signed-in account automatically.
comma := ,
OWNER      ?= $(if $(filter any,$(LIBREPAPER_PUBLISHERS)),,$(if $(findstring $(comma),$(LIBREPAPER_PUBLISHERS)),,$(LIBREPAPER_PUBLISHERS)))
# Browsers fetch LaTeX directly from an HTTPS static mirror. With no override,
# the binary uses the project mirror. `LATEX_MIRROR=` selects an operator-hosted
# copy for local runs.
LATEX_MIRROR      ?=
LATEX_MIRROR_FLAG ?= $(if $(LATEX_MIRROR),--latex-mirror $(LATEX_MIRROR))

serve: $(BIN)  ## Run the server and open it in Firefox (PORT=, DATA=, LATEX_MIRROR=; everything else through .env)
	@command -v firefox >/dev/null && (sleep 1; firefox http://localhost:$(PORT) >/dev/null 2>&1 &) || true
	@$(BIN) admin serve --port $(PORT) --data-directory $(DATA) $(LATEX_MIRROR_FLAG)

# One tutorial project per source format LibrePaper accepts. Each project has
# a source file and the same relative icon asset; no example is generated.
EXAMPLES := $(shell find examples/tutorial-* -type f)

# Not in the help: a step of `deploy`, not an entry point.
examples: $(EXAMPLES)

# Not in the help: it is a step of `deploy`, not a thing to run on its own.
# A deployment is seeded once. Resetting a nonempty catalogue is a `librepaper
# admin seed --backup <verified-point>` the operator runs deliberately, so a second
# `make deploy` serves what is there rather than refusing to start.
seed: $(BIN) $(EXAMPLES)
	@if [ -e $(DATA)/catalog.db ]; then \
		echo "$(DATA) is already seeded; serving it as is (move it aside to reseed)"; \
	else \
		$(BIN) admin seed --data-directory $(DATA) $(if $(OWNER),--owner $(OWNER)); \
	fi

kill:  ## Stop a server started with make serve
	@# The bracket stops the pattern from matching this command line itself.
	@pkill -f '[d]ist/librepaper admin serve' && echo "stopped" || echo "nothing to stop"

.PHONY: deploy latex-check latex-smoke

# The mirror itself -- the compiler engines and the TeX Live bundles -- is built
# and pushed from the wasm-latex repository (`make mirror`, `make push`
# there; layout and manifest in wasm-latex/docs/mirror.md). MIRROR= below
# points at that build's output.
MIRROR ?= ../wasm-latex/mirror

latex-check:
	@node latex/tools/check-mirror.mjs $(MIRROR)

latex-smoke: $(BIN)  ## Compile and display the seeded LaTeX example in Chromium against MIRROR=
	@node latex/tools/check-mirror.test.mjs
	@node latex/tools/check-mirror.mjs $(MIRROR)
	@node web/tools/latex-e2e.mjs $(BIN) browser examples/tutorial-latex/librepaper.tex 120 $(MIRROR)

# Use the ordinary sign-in flow: each new account receives five private
# examples and owns its copies. Guest roles come from links created in Share.
deploy: latex-check $(BIN)  ## Serve locally; sign in for your five examples and share links to test roles
	@$(MAKE) serve

# The deployment keys -- the Cloudflare token, the endpoints, the GitHub app
# -- live sops-encrypted in deploy/keys.yaml. A target cannot export into the
# shell that ran make, so `secrets` opens a subshell with them decrypted in
# its environment; exit it to drop them. For one command instead of a shell:
#
#     sops exec-env deploy/keys.yaml 'wrangler deploy'
KEYS ?= deploy/keys.yaml
.PHONY: secrets

secrets:  ## Open an interactive shell with the sops-encrypted deployment keys in its environment
	@test -f $(KEYS) || { echo "no $(KEYS)"; exit 1; }
	@test -t 0 || { echo "make secrets opens an interactive subshell and needs a terminal" >&2; echo "use: sops exec-env $(KEYS) '<command>'" >&2; exit 2; }
	@echo "$(KEYS) is loaded in this shell; exit to drop it"
	@sops exec-env $(KEYS) "$${SHELL:-/bin/sh}"

# Pushing the LaTeX mirror to its hosting is wasm-latex's `make push` now
# (see the MIRROR= comment above latex-smoke); nothing here uploads it.

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
# Not built here any more. Each renderer is a repository of its own -- see
# wasm-modules.lock -- and releases a module built from the same tag this
# binary pins the crate at, so what the editor previews and what a save stores
# still come out of one version of one implementation.
#
# The fetch verifies every module against the digest in the lock and writes
# nothing that does not match, so a build either has the renderers it pinned or
# fails saying which one moved. Files already correct are left alone, which
# makes this cheap enough to run on every build.

wasm: wasm-check  ## Fetch and verify the pinned browser renderers
	@node web/tools/fetch-modules.mjs

wasm-check:  ## Check native and browser renderer tags without network access
	@node web/tools/check-renderer-pins.mjs

# The files are produced by the phony aggregate above. This rule lets Make
# resolve them as binary prerequisites on a clean checkout while preserving
# their mtimes so a changed renderer causes the embedding binary to rebuild.
$(WASM) $(BIB) $(CITES) $(TYPST): | wasm

# Update one explicitly named renderer tag in Cargo.toml and wasm-modules.lock.
# The command never looks up or selects a latest release implicitly.
wasm-update:  ## Update one renderer (REPO=wasm-markdown TAG=vX.Y.Z)
	@test -n "$(REPO)" -a -n "$(TAG)" || { echo 'usage: make wasm-update REPO=wasm-markdown TAG=vX.Y.Z' >&2; exit 2; }
	@node web/tools/update-module-pin.mjs --repo "$(REPO)" --tag "$(TAG)"
