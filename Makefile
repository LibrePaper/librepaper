# LibrePaper. `make` builds the single static binary into dist/.
#
# The application crate provides the server and CLI and links the pinned
# renderer libraries directly. The web app in web/ is Svelte,
# bundled by vite and installed by bun. The binary embeds the web build (from
# web/dist) and serves it. The WASM renderers are not embedded: they are
# published to the asset mirror (tools/deploy-assets publish) and browsers load them there.

# Local settings, kept out of the repository: the GitHub OAuth app and who may
# publish. Copy .env.example to .env and fill it in. Values are read as Make
# assignments, so write them bare, with no surrounding quotes.
-include .env
export

BIN     := dist/librepaper
PREFIX  ?= $(HOME)/.local
BINDIR  ?= $(PREFIX)/bin
DESTDIR ?=
# The browser renderers, fetched into web/wasm (not embedded in the binary):
# tests read them, and tools/deploy-assets publish sends them to the asset mirror.
WASM    := web/wasm/markdown.wasm
BIB     := web/wasm/bibliography.wasm
CITES   := web/wasm/citations.wasm
# Fetched, not built: see assets.lock and the pinned inputs section below.
TYPST   := web/wasm/typst.wasm
# The pages. web/dist is entirely a build output, so it is an input to
# nothing: what the pages are built from lives in web/src and web/public.
SHELL_OUT := web/dist/index.html
# The CodeMirror binding, fetched from the fork rather than vendored: see
# loro-codemirror.lock and docs/loro-codemirror.md. Every fetched file is
# named, not just the entry point: moving the pin to a commit that changes
# only sync.ts must still re-bundle the pages, and listing one file meant it
# did not -- the fetch corrected the file and vite was never asked again.
LCM_DIR := web/vendor/loro-codemirror
LCM     := $(LCM_DIR)/index.ts $(LCM_DIR)/sync.ts $(LCM_DIR)/undo.ts \
           $(LCM_DIR)/awareness.ts $(LCM_DIR)/ephemeral.ts $(LCM_DIR)/utils.ts
# Everything tools/pins fetch writes; see the pinned inputs section below.
PINNED  := $(WASM) $(BIB) $(CITES) $(TYPST) $(LCM)
# web/src/site is deliberately not in this list. The marketing page and the
# docs chrome are built by vite.site.config.js into docs/_site, which the
# binary does not embed, and no page under web/pages reaches them. Counting
# them here made a one-line copy change rebuild the application bundle, which
# rewrites every file in web/dist, which build.rs watches -- so editing the
# landing page recompiled the whole crate. The site builds on its own: see the
# site target at the bottom of this file.
WEB     := $(shell find web/src web/public -type f -not -path 'web/src/site/*') $(wildcard web/pages/*.html web/package.json web/vite.config.js web/vite.frame.config.js)
SOURCES := $(shell find crates -type f -not -path '*/target/*') $(shell find skills) $(shell find docs/examples -type f) Cargo.toml

.PHONY: help build install test check fmt serve demo demo-run wipe kill clean snapshot web pins site site-serve

help:  ## Display this help screen
	@printf "\033[1mAvailable commands:\033[0m\n\n"
	@grep -hE '^[a-z.A-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-22s\033[0m %s\n", $$1, $$2}' | sort

build: $(BIN)  ## Build dist/librepaper, with the shell embedded

install: $(BIN)  ## Build and install to ~/.local/bin (override PREFIX= or BINDIR=)
	@install -d "$(DESTDIR)$(BINDIR)"
	@install -m 755 "$(BIN)" "$(DESTDIR)$(BINDIR)/librepaper"
	@echo "Installed $(DESTDIR)$(BINDIR)/librepaper"

# Rebuilt whenever any source or page changes.
$(BIN): $(SOURCES) $(SHELL_OUT)
	@mkdir -p $(dir $@)
	@cargo build --release -p librepaper
	@# Copied beside and renamed over: a server running from the old binary
	@# keeps its file open, which makes an overwrite fail with "text file
	@# busy" while a rename just leaves it holding the old inode.
	@cp target/release/librepaper $@.tmp && mv -f $@.tmp $@
	@echo "$@ ($$(($$(stat -c%s $@) / 1024 / 1024)) MiB) -- deploys on its own"

# The suite reads the built shell -- a test that asserts a page names its own
# bundle needs that bundle to exist -- so the pages are built first.
test: pins $(SHELL_OUT)  ## Run rustfmt, clippy and the test suite
	@cd web && bun run check
	@cd web && bun run check:names
	@cargo fmt --check
	@node --test 'web/tools/*.test.mjs' 'tools/**/*.test.mjs'
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
# Said out loud because a silent green `test` reads as "everything passes", and
# it is not: the browser components and the Postgres-gated cases are both out
# of this target on purpose, and both have hidden real faults while every
# other check was green.
	@echo
	@echo "test: passed -- NOT everything. Still to run:"
	@echo "  tools/suite browser                       the components in a real chromium"
	@echo "  LIBREPAPER_TEST_POSTGRES_URL=... \\"
	@echo "    cargo test -p librepaper --lib -- --ignored --test-threads=1"
	@echo "  make check                                test + browser + reader smoke in one go"

# One command that means what a green `test` looks like it means. The Postgres
# cases stay out even here: they want a database to point at and they TRUNCATE
# it, which is not something a default target should assume it may do.
check: test  ## Run the test suite, the browser components and the reader smoke test
	@tools/suite browser
	@tools/suite smoke

fmt:  ## Format every crate
	@cargo fmt

# Release builds are described in .github/workflows/release.yml and run when a
# v* tag is pushed. This does the same thing locally, without tagging.
snapshot: pins $(SHELL_OUT)
	@cargo build --release -p librepaper
	@echo "target/release/librepaper"

clean:  ## Remove build output
	@rm -rf dist target/release/librepaper web/dist web/node_modules

# The port is fixed because the GitHub OAuth app's callback URL names it.
PORT       ?= 8081
# The static site is a second server on a second port: it is a directory of
# files with no application behind it, and the application is what the Sign in
# button on it points at.
SITE_PORT  ?= 8082
DATA       ?= librepaper-data
# Browsers fetch the wasm renderers and LaTeX directly from an HTTPS static
# asset mirror. With no override the binary uses the project mirror, which
# serves the releases assets.lock pins. `ASSET_MIRROR=` selects an
# operator-hosted copy for local runs.
ASSET_MIRROR      ?=
ASSET_MIRROR_FLAG ?= $(if $(ASSET_MIRROR),--asset-mirror $(ASSET_MIRROR))

# SIMULATE_ACTIVITY=<days> writes every starter document a new account is
# given as though it had been typed over that many days, so the history panel
# has a calendar in it rather than the one cell a document published once has.
# The operations are real; only the clock is invented. See seed::activity.
# 0 -- or nothing at all -- asks for no simulation, which is what `serve`
# does on its own; `demo` supplies a default below.
SIMULATE_ACTIVITY_FLAG ?= $(if $(filter-out 0,$(SIMULATE_ACTIVITY)),--simulate-activity $(SIMULATE_ACTIVITY))

OPEN ?= 1

# Without LIBREPAPER_DATABASE_URL, the server uses the Docker PostgreSQL that
# tools/db dev keeps, started here if it is not running.
serve: $(BIN)  ## Run the server and open it in Firefox (PORT=, DATA=, ASSET_MIRROR=, LIBREPAPER_DATABASE_URL=)
	@test -n "$(LIBREPAPER_DATABASE_URL)" || tools/db dev >/dev/null
	@test "$(OPEN)" = 1 && command -v firefox >/dev/null && (sleep 1; firefox http://localhost:$(PORT) >/dev/null 2>&1 &) || true
	@LIBREPAPER_DATABASE_URL="$${LIBREPAPER_DATABASE_URL:-$$(tools/db url)}" \
		$(BIN) admin serve --port $(PORT) --data-directory $(DATA) $(ASSET_MIRROR_FLAG) $(SIMULATE_ACTIVITY_FLAG)

kill:  ## Stop a server started with make serve
	@# The bracket stops the pattern from matching this command line itself.
	@pkill -f '[d]ist/librepaper admin serve' && echo "stopped" || echo "nothing to stop"

# Starting over. The local deployment is seeded state and nothing else -- the
# accounts, the five example copies per account and the CRDT history behind
# them -- so the way out of a document the current code cannot read is to
# throw it away rather than to migrate it. That is what an unreleased
# substrate change leaves behind: the documents in a deploy from before the
# swap are encoded for the engine that was replaced, and the reader says so
# ("Invalid magic bytes") instead of opening them.
#
# Only the Docker deployment tools/db owns. A configured
# LIBREPAPER_DATABASE_URL points somewhere this target has no business
# dropping, so it refuses rather than guessing.
wipe:  ## Delete the local deployment -- database and data directory -- and start over
	@if [ -n "$(LIBREPAPER_DATABASE_URL)" ]; then \
		echo "LIBREPAPER_DATABASE_URL is set: wipe that database yourself."; exit 1; \
	fi
	@$(MAKE) --no-print-directory kill >/dev/null
	@tools/db dev >/dev/null
	@# FORCE, because a server that outlived `kill` still holds a connection
	@# and DROP DATABASE waits for it otherwise.
	@docker exec $${DEV_POSTGRES_CONTAINER:-librepaper-postgres} psql -U postgres -c \
		'DROP DATABASE IF EXISTS librepaper WITH (FORCE)' >/dev/null
	@docker exec $${DEV_POSTGRES_CONTAINER:-librepaper-postgres} psql -U postgres -c 'CREATE DATABASE librepaper' >/dev/null
	@# The blobs and the session key. The schema comes back from the
	@# migrations on the next start, and the directory with it.
	@rm -rf "$(DATA)"
	@echo "Wiped: database librepaper, $(DATA)/ -- sign in again for a fresh set of examples"

# Use the ordinary sign-in flow: each new account receives five private
# examples and owns its copies. Guest roles come from links created in Share.
#
# A local deploy is a demonstration, so its examples are written with a past:
# three weeks of drafting, so the history panel and the activity calendar have
# something in them the first time they are opened. The operations are real
# and every version opens; only the clock is invented (seed::activity).
# SIMULATE_ACTIVITY=0 asks for the honest history of a document published
# once, and any other number overrides the three weeks. It applies to the
# examples an account is given at first sign-in, so changing it means `wipe`
# and signing in again.
# The whole product, locally: the marketing site on one port and the
# application on the other, with the site built to point its Sign in button at
# the application this target just started rather than at the published
# deployment. Firefox opens the site, because that is where a stranger
# arrives; `serve` is told not to open the application on top of it.
#
# The site server is a background process, so the recipe traps its own exit
# and takes it down: a preview server left holding SITE_PORT would make the
# next `make demo` fail on --strictPort.
#
# Publishing needs sign-in, so `demo` runs under the sops-encrypted deployment
# keys when it can decrypt them: their GitHub app is registered for
# http://localhost:$(PORT). Without sops or the key, the demo starts with no
# sign-in, and reading and commenting still work.
DEMO_KEYS ?= tools/deploy-keys.yaml
demo:  ## Serve the site, the app, a local companion and simulated activity (SIMULATE_ACTIVITY=21)
	@if [ -z "$$LIBREPAPER_GITHUB_CLIENT_ID" ] && command -v sops >/dev/null 2>&1 \
		&& sops --decrypt --extract '["LIBREPAPER_GITHUB_CLIENT_ID"]' $(DEMO_KEYS) >/dev/null 2>&1; then \
		echo "demo: GitHub sign-in from $(DEMO_KEYS)"; \
		exec sops exec-env $(DEMO_KEYS) '$(MAKE) --no-print-directory demo-run'; \
	else \
		echo "demo: $$([ -n \"$$LIBREPAPER_GITHUB_CLIENT_ID\" ] && echo 'GitHub sign-in from environment' || echo 'no sign-in (sops cannot decrypt $(DEMO_KEYS)); publishing is off')"; \
		exec $(MAKE) --no-print-directory demo-run; \
	fi

demo-run: SIMULATE_ACTIVITY ?= 21
demo-run: $(BIN)
	@tools/deploy-assets check
	@LIBREPAPER_APP_ORIGIN=http://localhost:$(PORT) $(MAKE) --no-print-directory site
	@set -e; \
	(cd web && bun run serve:site -- --port $(SITE_PORT) --strictPort >/dev/null 2>&1) & \
	site_pid=$$!; companion_pid=; \
	if $(BIN) local status >/dev/null 2>&1; then \
		echo "local $$($(BIN) local status | sed -n 2p)  (already running; left alone)"; \
	else \
		$(BIN) local start --foreground >/dev/null 2>&1 & \
		companion_pid=$$!; \
	fi; \
	trap "kill $$site_pid $$companion_pid 2>/dev/null || true" EXIT INT TERM; \
	command -v firefox >/dev/null && (sleep 2; firefox http://localhost:$(SITE_PORT) >/dev/null 2>&1 &) || true; \
	echo "site  http://localhost:$(SITE_PORT)"; \
	echo "app   http://localhost:$(PORT)"; \
	LIBREPAPER_SITE_ORIGIN=http://localhost:$(SITE_PORT) \
		$(MAKE) serve OPEN=0 LIBREPAPER_PUBLISHERS=any SIMULATE_ACTIVITY=$(SIMULATE_ACTIVITY)

# --- the web app -----------------------------------------------------------
#
# Svelte, Skeleton, CodeMirror and Loro, bundled into the pages the binary
# embeds. The output goes to web/dist, so nothing under that directory is
# edited by hand. The build refuses to run if a page has drifted from the
# design system -- see web/tests/unit/vocabulary.js.

web: $(SHELL_OUT)

$(SHELL_OUT): $(WEB) $(LCM)
	@command -v bun >/dev/null || { echo "bun is not installed: https://bun.sh"; exit 1; }
	@cd web && bun install --silent && bun run build


# --- the pinned inputs -----------------------------------------------------
#
# The browser wasm renderers (assets.lock) and the loro-codemirror binding
# (loro-codemirror.lock) are fetched, not built or vendored, and verified
# against their locks; tools/pins owns fetching and moving them. Files already
# correct are left alone, so running it on every build is cheap. `pins` is the
# internal step. The files themselves depend on it order-only: a clean checkout
# resolves them as prerequisites, an already-fetched one does not re-run it.

pins:
	@tools/pins fetch

$(PINNED): | pins

# --- the docs site ----------------------------------------------------------
#
# A separate static artifact, not the shell above: docs/**/*.md rendered by
# the same markdown engine the application embeds (web/tools/build-site.mjs),
# wrapped in the sidebar from docs/nav.js, beside the landing page authored in
# web/src/site/. Built by vite.site.config.js into docs/_site, which nothing
# else reads -- it is deployed on its own by .github/workflows/site.yml.
site: pins
	@command -v bun >/dev/null || { echo "bun is not installed: https://bun.sh"; exit 1; }
	@cd web && bun install --silent && bun run build:site

site-serve: site
	@cd web && bun run serve:site -- --port $(SITE_PORT) --strictPort
