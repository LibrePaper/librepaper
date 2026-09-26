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
WASM_BR := $(WASM).br $(BIB).br $(CITES).br $(TYPST).br
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
# web/src/site is deliberately not in this list. The marketing page and the
# docs chrome are built by vite.site.config.js into site/_site, which the
# binary does not embed, and no page under web/pages reaches them. Counting
# them here made a one-line copy change rebuild the application bundle, which
# rewrites every file in web/dist, which build.rs watches -- so editing the
# landing page recompiled the whole crate. The site builds on its own: see the
# site target at the bottom of this file.
WEB     := $(shell find web/src web/public -type f -not -path 'web/src/site/*') $(wildcard web/pages/*.html web/package.json web/vite.config.js web/vite.agent.config.js)
# The renderers are generated, so they are not also inputs to themselves.
SOURCES := $(shell find crates -type f -not -path '*/target/*') $(shell find skills) $(shell find docs/examples -type f) Cargo.toml

.DEFAULT_GOAL := help
.PHONY: help build install test check-all browser test-external test-release-workloads smoke serve seed examples kill clean snapshot wasm wasm-check wasm-update loro-codemirror loro-update fmt web fuzz

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
$(BIN): $(SOURCES) $(WASM) $(BIB) $(CITES) $(TYPST) $(WASM_BR) $(SHELL_OUT) | wasm
	@mkdir -p $(dir $@)
	@cargo build --release -p librepaper
	@# Copied beside and renamed over: a server running from the old binary
	@# keeps its file open, which makes an overwrite fail with "text file
	@# busy" while a rename just leaves it holding the old inode.
	@cp target/release/librepaper $@.tmp && mv -f $@.tmp $@
	@echo "$@ ($$(($$(stat -c%s $@) / 1024 / 1024)) MiB) -- deploys on its own"

# The suite reads the built shell -- a test that asserts a page names its own
# bundle needs that bundle to exist -- so the pages are built first.
test: wasm $(SHELL_OUT)  ## Run rustfmt, clippy and the test suite
	@cd web && bun run check
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
	@echo "  make browser                              the components in a real chromium"
	@echo "  LIBREPAPER_TEST_POSTGRES_URL=... \\"
	@echo "    cargo test -p librepaper --lib -- --ignored --test-threads=1"
	@echo "  make check-all                            test + browser in one go"

# Explicit suites kept out of the default inventory because they require
# host tools or intentionally exercise release-sized resource ceilings.
test-external:  ## Run Quarto/R/Python and real local-service integration tests
	@cargo test -p librepaper quarto -- --ignored --nocapture --test-threads=1 \
		--skip quarto_project_scope_collects_pages_and_nested_web_resources \
		--skip quarto_book_scope_keeps_chapter_navigation

test-release-workloads:  ## Run supported-limit and diagnostic workloads
	@cargo test -p librepaper the_supported_maximum_source_survives_a_restart -- --ignored --nocapture
	@cargo test -p librepaper measure_source_history_profiles -- --ignored --nocapture
	@cargo test -p librepaper profile_unchanged_resident_metadata -- --ignored --nocapture
	@cargo test -p librepaper persistence_capacity_workload -- --ignored --nocapture
	@cargo test -p librepaper persistence_default_limits_refuse_without_discarding_dirty_rooms -- --ignored --nocapture
	@cargo test -p librepaper mcp_protocol_transfer_benchmark -- --ignored --nocapture

# One command that means what a green `test` looks like it means. The Postgres
# cases stay out even here: they want a database to point at and they TRUNCATE
# it, which is not something a default target should assume it may do.
check-all: test browser  ## Run the test suite AND the browser components

# The components, in a real browser. Not part of `test`: each one builds a
# bundle with vite and drives a headless chromium, which is minutes rather than
# seconds. But nothing else runs them, and a substrate change once left three of
# them referring to a library that had been uninstalled -- broken for as long as
# nobody looked, because every other check passed.
#
# The per-test bound is generous rather than tuned: the LaTeX one drives a real
# TeX Live through a browser and takes minutes by itself, and a tighter bound
# reported that passing test as broken.
# $(LCM) because each test builds its own bundle from web/src, and the editor
# imports the CodeMirror binding, which is fetched rather than vendored. The CI
# job runs this target on its own, so the dependency has to be here: without it
# every test that builds Editor.svelte failed with an unresolved import, which
# is how removing the vendored source from the repository broke this job.
browser: $(LCM)  ## Run the component tests in headless chromium (needs chromium)
	@command -v chromium >/dev/null || command -v google-chrome >/dev/null || \
		{ echo "no chromium to drive; skipping the browser tests"; exit 0; }
	@failed=""; \
	for test in web/tests/browser/*.mjs; do \
		printf '%s: ' "$$(basename $$test)"; \
		if (cd web && timeout 900 node "../$$test" >/tmp/browser-test.log 2>&1); then \
			echo ok; \
		else \
			echo FAILED; failed="$$failed $$(basename $$test)"; \
			sed 's/^/    /' /tmp/browser-test.log | tail -8; \
		fi; \
	done; \
	[ -z "$$failed" ] || { echo "failed:$$failed"; exit 1; }

# The rendered reader, in a real browser. Not part of `test`: it needs the
# built binary and a chromium, and it starts a server of its own on a
# temporary directory. It touches no deployment and no data but its own.
smoke: $(BIN)  ## Drive the reader in headless chromium (needs chromium)
	@command -v chromium >/dev/null || command -v google-chrome >/dev/null || \
		{ echo "no chromium to drive; skipping the browser smoke test"; exit 0; }
	@bun web/tools/browser-smoke.mjs $(BIN)

fmt:  ## Format every crate
	@cargo fmt

# The fuzz targets in tools/fuzz/: libFuzzer over the functions that read what
# somebody else wrote -- the path rules, the shared document and its repair, a
# v1 update against the admission ceilings, a catalog source archive off object
# storage, the anchoring that places a reader's selection in a source file
# the author uploaded, and what a page in the browser may say to the loopback
# build service. Each runs for FUZZ_SECONDS, then stops; a finding is left in
# tools/fuzz/artifacts/. Needs a nightly toolchain and cargo-fuzz
# (`cargo install cargo-fuzz`). No sanitizer:
# the code under test is safe Rust, the oracle is the assertions, and the
# sanitizer doubles the build time to find nothing the panics do not.
FUZZ_SECONDS ?= 60
# Every target unless told otherwise. `update` used to be held back for a
# panic inside yrs, reproduced by an ignored test that went with the
# substrate; there is no yrs now and nothing is excluded.
FUZZ_TARGETS ?= $(shell cargo fuzz list --fuzz-dir tools/fuzz)
# The global export would evaluate this for every recipe, even outside fuzz.
unexport FUZZ_TARGETS
fuzz: wasm $(SHELL_OUT)  ## Run every fuzz target for FUZZ_SECONDS (default 60) each
	@command -v cargo-fuzz >/dev/null || { echo "cargo-fuzz is not installed: cargo install cargo-fuzz"; exit 1; }
	@for target in $(FUZZ_TARGETS); do \
		echo "fuzzing $$target for $(FUZZ_SECONDS)s"; \
		cargo fuzz run --fuzz-dir tools/fuzz -s none $$target -- -max_total_time=$(FUZZ_SECONDS) || exit 1; \
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
# The static site is a second server on a second port: it is a directory of
# files with no application behind it, and the application is what the Sign in
# button on it points at.
SITE_PORT  ?= 8082
# The pairing code `deploy` starts its companion with. Fixed, so connecting an
# agent in development does not mean reading a fresh code off a terminal on
# every restart. A real install rotates it per run; this is a convenience for
# a machine that is already serving its own documents over plain loopback.
LOCAL_CODE ?= 123456
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

# SIMULATE_ACTIVITY=<days> writes each example as though it had been typed
# over that many days, so the history panel has a calendar in it. For admin serve,
# set simulate_activity_days in the advanced config file instead.

OPEN ?= 1

serve: $(BIN)  ## Run the server and open it in Firefox (PORT=, DATA=, LATEX_MIRROR=; everything else through .env)
	@test "$(OPEN)" = 1 && command -v firefox >/dev/null && (sleep 1; firefox http://localhost:$(PORT) >/dev/null 2>&1 &) || true
	@$(BIN) admin serve --port $(PORT) --data-directory $(DATA) $(LATEX_MIRROR_FLAG)

# One tutorial project per source format LibrePaper accepts. Each project has
# a source file and the same relative icon asset; no example is generated.
EXAMPLES := $(shell find docs/examples/tutorial-* -type f)

# Not in the help: a step of `deploy`, not an entry point.
examples: $(EXAMPLES)

# Not in the help: it is a step of `deploy`, not a thing to run on its own.
# A deployment is seeded once. Resetting a nonempty catalogue is a `librepaper
# admin seed --backup <verified-point>` the operator runs deliberately, so a second
# `make deploy` serves what is there rather than refusing to start.
# SIMULATE_ACTIVITY=<days> writes each example as though it had been typed
# over that many days -- drafted, cut and rewritten, in sittings -- so a
# demonstration deployment has a calendar of versions in the history panel
# rather than the one cell a document published once has. The operations are
# real and every version opens; only the times are invented.
seed: $(BIN) $(EXAMPLES)
	@$(BIN) admin seed --data-directory $(DATA) $(if $(OWNER),--owner $(OWNER)) \
		$(if $(SIMULATE_ACTIVITY),--simulate-activity $(SIMULATE_ACTIVITY))

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
	@node tools/latex/tools/check-mirror.mjs $(MIRROR)

latex-smoke: $(BIN)  ## Compile and display the seeded LaTeX example in Chromium against MIRROR=
	@node tools/latex/tools/check-mirror.test.mjs
	@node tools/latex/tools/check-mirror.mjs $(MIRROR)
	@node web/tools/latex-e2e.mjs $(BIN) browser docs/examples/tutorial-latex/librepaper.tex 120 $(MIRROR)

# The local deployment keeps PostgreSQL on the same machine in a persistent
# Docker volume. An explicitly configured database URL always wins, so hosted
# and native PostgreSQL installations do not involve Docker.
DEV_POSTGRES_CONTAINER ?= librepaper-postgres
DEV_POSTGRES_PORT      ?= 55432
DEV_POSTGRES_VOLUME    ?= librepaper-postgres-data
DEV_POSTGRES_PASSWORD  ?= librepaper-local
DEV_POSTGRES_URL       := postgresql://postgres:$(DEV_POSTGRES_PASSWORD)@127.0.0.1:$(DEV_POSTGRES_PORT)/librepaper

.PHONY: postgres-dev
postgres-dev:  ## Start the persistent PostgreSQL used by an unconfigured local deploy
	@command -v docker >/dev/null || { echo "make deploy needs Docker or LIBREPAPER_DATABASE_URL"; exit 1; }
	@docker inspect $(DEV_POSTGRES_CONTAINER) >/dev/null 2>&1 || docker run -d \
		--name $(DEV_POSTGRES_CONTAINER) --restart unless-stopped \
		-p 127.0.0.1:$(DEV_POSTGRES_PORT):5432 \
		-e POSTGRES_PASSWORD=$(DEV_POSTGRES_PASSWORD) -e POSTGRES_DB=librepaper \
		-v $(DEV_POSTGRES_VOLUME):/var/lib/postgresql/data postgres:17-alpine >/dev/null
	@docker start $(DEV_POSTGRES_CONTAINER) >/dev/null
	@for attempt in $$(seq 1 30); do \
		docker exec $(DEV_POSTGRES_CONTAINER) pg_isready -U postgres -d librepaper >/dev/null 2>&1 && exit 0; \
		sleep 1; \
	done; echo "PostgreSQL did not become ready"; exit 1

# Starting over. The local deployment is seeded state and nothing else -- the
# accounts, the five example copies per account and the CRDT history behind
# them -- so the way out of a document the current code cannot read is to
# throw it away rather than to migrate it. That is what an unreleased
# substrate change leaves behind: the documents in a deploy from before the
# swap are encoded for the engine that was replaced, and the reader says so
# ("Invalid magic bytes") instead of opening them.
#
# Only the Docker deployment this Makefile owns. A configured
# LIBREPAPER_DATABASE_URL points somewhere this target has no business
# dropping, so it refuses rather than guessing.
.PHONY: wipe
wipe:  ## Delete the local deployment -- database and data directory -- and start over
	@if [ -n "$(LIBREPAPER_DATABASE_URL)" ]; then \
		echo "LIBREPAPER_DATABASE_URL is set: wipe that database yourself."; exit 1; \
	fi
	@$(MAKE) --no-print-directory kill >/dev/null
	@$(MAKE) --no-print-directory postgres-dev
	@# FORCE, because a server that outlived `kill` still holds a connection
	@# and DROP DATABASE waits for it otherwise.
	@docker exec $(DEV_POSTGRES_CONTAINER) psql -U postgres -c \
		'DROP DATABASE IF EXISTS librepaper WITH (FORCE)' >/dev/null
	@docker exec $(DEV_POSTGRES_CONTAINER) psql -U postgres -c 'CREATE DATABASE librepaper' >/dev/null
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
# next `make deploy` fail on --strictPort.
deploy: SIMULATE_ACTIVITY ?= 21
deploy: latex-check $(BIN)  ## Serve the site, the application and a local companion (LOCAL_CODE= pairing code) and open the site in Firefox
	@LIBREPAPER_APP_ORIGIN=http://localhost:$(PORT) $(MAKE) --no-print-directory site
	@test -n "$(LIBREPAPER_DATABASE_URL)" || $(MAKE) --no-print-directory postgres-dev
	@set -e; \
	(cd web && bun run serve:site -- --port $(SITE_PORT) --strictPort >/dev/null 2>&1) & \
	site_pid=$$!; companion_pid=; \
	if $(BIN) local status >/dev/null 2>&1; then \
		echo "local $$($(BIN) local status | sed -n 2p)  (already running; left alone)"; \
	else \
		$(BIN) local start --code $(LOCAL_CODE) >/dev/null 2>&1 & \
		companion_pid=$$!; \
	fi; \
	trap "kill $$site_pid $$companion_pid 2>/dev/null || true" EXIT INT TERM; \
	command -v firefox >/dev/null && (sleep 2; firefox http://localhost:$(SITE_PORT) >/dev/null 2>&1 &) || true; \
	echo "site  http://localhost:$(SITE_PORT)"; \
	echo "app   http://localhost:$(PORT)"; \
	test -z "$$companion_pid" || echo "local pairing code: $(LOCAL_CODE)  (agent panel, Connection tab)"; \
	LIBREPAPER_DATABASE_URL="$${LIBREPAPER_DATABASE_URL:-$(DEV_POSTGRES_URL)}" \
	LIBREPAPER_SITE_ORIGIN=http://localhost:$(SITE_PORT) \
		$(MAKE) serve OPEN=0 LIBREPAPER_PUBLISHERS=any SIMULATE_ACTIVITY=$(SIMULATE_ACTIVITY)

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
# Svelte, Skeleton, CodeMirror and Loro, bundled into the pages the binary
# embeds. The output goes to web/dist, so nothing under that directory is
# edited by hand. The build refuses to run if a page has drifted from the
# design system -- see web/tests/unit/vocabulary.js.

web: $(SHELL_OUT)  ## Build the pages from web/

$(SHELL_OUT): $(WEB) $(LCM)
	@command -v bun >/dev/null || { echo "bun is not installed: https://bun.sh"; exit 1; }
	@cd web && bun install --silent && bun run build


# --- the CodeMirror binding ------------------------------------------------
#
# A fork of loro-codemirror, fetched rather than vendored. Upstream cannot keep
# an editor in step with a document holding more than one container, which is
# every update here; docs/loro-codemirror.md says what is changed and why.
#
# It was vendored into web/vendor until 2026-09-16, which meant the same source
# existed twice -- here and in the fork -- and they drifted, the fork sitting
# three fixes behind for a while with nothing to notice it. Now the fork is the
# only copy and this fetches it by pinned commit, verifying every file against
# loro-codemirror.lock and writing nothing that does not match.

loro-codemirror:  ## Fetch and verify the pinned CodeMirror binding
	@node web/tools/fetch-loro-codemirror.mjs

# As with the renderers: the files are produced by the phony target above, and
# this rule lets Make resolve them as prerequisites on a clean checkout while
# leaving an already-correct file's mtime alone.
$(LCM): | loro-codemirror

# Move the pin to a commit you name. Never resolves a branch or a "latest".
loro-update:  ## Move the binding pin (COMMIT=<full 40-char sha>)
	@test -n "$(COMMIT)" || { echo "usage: make loro-update COMMIT=<full 40-char sha>"; exit 1; }
	@node web/tools/update-loro-codemirror.mjs $(COMMIT)

# --- the docs site ----------------------------------------------------------
#
# A separate static artifact, not the shell above: site/**/*.md rendered by
# the same markdown engine the application embeds (web/tools/build-site.mjs),
# wrapped in the sidebar from site/nav.js, beside the landing page authored in
# web/src/site/. Built by vite.site.config.js into site/_site, which nothing
# else reads -- it is deployed on its own by .github/workflows/site.yml.
.PHONY: site site-serve

site: wasm  ## Build the static docs site into site/_site
	@command -v bun >/dev/null || { echo "bun is not installed: https://bun.sh"; exit 1; }
	@cd web && bun install --silent && bun run build:site

site-serve: site  ## Serve the built docs site on SITE_PORT (Ctrl-C to stop)
	@cd web && bun run serve:site -- --port $(SITE_PORT) --strictPort

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
$(WASM) $(BIB) $(CITES) $(TYPST) $(WASM_BR): | wasm

# Update one explicitly named renderer tag in Cargo.toml and wasm-modules.lock.
# The command never looks up or selects a latest release implicitly.
wasm-update:  ## Update one renderer (REPO=wasm-markdown TAG=vX.Y.Z)
	@test -n "$(REPO)" -a -n "$(TAG)" || { echo 'usage: make wasm-update REPO=wasm-markdown TAG=vX.Y.Z' >&2; exit 2; }
	@node web/tools/update-module-pin.mjs --repo "$(REPO)" --tag "$(TAG)"

# The compile-time-checked SQL cache. `.sqlx/` is what lets an ordinary build
# verify every query without a database; it goes stale the moment a query or a
# migration changes, so regenerating it is part of changing either.
#
# Both targets use their own database, separate from the one `make deploy`
# keeps, because preparing applies every migration to it.
SQLX_POSTGRES_DB  ?= librepaper_sqlx
SQLX_DATABASE_URL := postgresql://postgres:$(DEV_POSTGRES_PASSWORD)@127.0.0.1:$(DEV_POSTGRES_PORT)/$(SQLX_POSTGRES_DB)

.PHONY: sqlx-database
sqlx-database: postgres-dev  ## Recreate the schema-only database the SQL checks run against
	@docker exec $(DEV_POSTGRES_CONTAINER) psql -U postgres -c 'DROP DATABASE IF EXISTS $(SQLX_POSTGRES_DB)' >/dev/null
	@docker exec $(DEV_POSTGRES_CONTAINER) psql -U postgres -c 'CREATE DATABASE $(SQLX_POSTGRES_DB)' >/dev/null
	@DATABASE_URL='$(SQLX_DATABASE_URL)' sqlx migrate run --source crates/librepaper/migrations/postgres

.PHONY: sqlx-prepare
sqlx-prepare: sqlx-database  ## Regenerate .sqlx/ after changing any query or migration
	@SQLX_OFFLINE=false DATABASE_URL='$(SQLX_DATABASE_URL)' cargo sqlx prepare --workspace -- --all-targets

.PHONY: sqlx-check
sqlx-check: sqlx-database  ## Fail if .sqlx/ no longer matches the queries in the tree
	@SQLX_OFFLINE=false DATABASE_URL='$(SQLX_DATABASE_URL)' cargo sqlx prepare --check --workspace -- --all-targets
