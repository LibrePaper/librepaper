# Komodoc. `make` builds the single static binary into dist/.
#
# Everything runs through the Go toolchain.

# Local settings, kept out of the repository: the GitHub OAuth app and who may
# publish. Copy .env.example to .env and fill it in. Values are read as Make
# assignments, so write them bare, with no surrounding quotes.
-include .env
export

BIN     := dist/komodoc
SOURCES := $(shell find src -type f) README.md

.DEFAULT_GOAL := help
.PHONY: help build test serve seed examples kill clean snapshot

help:  ## Display this help screen
	@printf "\033[1mAvailable commands:\033[0m\n\n"
	@grep -hE '^[a-z.A-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-22s\033[0m %s\n", $$1, $$2}' | sort

build: $(BIN)  ## Build dist/komodoc, with the worker and shell embedded

# Rebuilt whenever the tool or any worker/shell file changes.
$(BIN): $(SOURCES)
	@mkdir -p $(dir $@)
	@go build -o $@ ./src
	@echo "$@ ($$(($$(stat -c%s $@) / 1024 / 1024)) MiB) -- deploys on its own"

# The documentation page is the README, so it is copied in to be embedded. The
# tests read it too, so both depend on it rather than on the build.
$(BIN) test: src/shell/README.md
src/shell/README.md: README.md
	@cp $< $@

test:  ## Run gofmt, go vet and the test suite
	@gofmt -l src | grep . && { echo "gofmt needed"; exit 1; } || true
	@for file in src/shell/*.js src/worker/*.js; do node --check "$$file"; done
	@go vet ./...
	@go test ./...

# Release builds are described in .goreleaser.yaml and run on GitHub Actions
# when a v* tag is pushed. This does the same thing locally, without tagging.
snapshot:  ## Cross-compile every platform into dist/, as a release would
	@command -v goreleaser >/dev/null || { echo "goreleaser is not installed: go install github.com/goreleaser/goreleaser/v2@latest"; exit 1; }
	@goreleaser release --snapshot --clean

clean:  ## Remove build output
	@rm -rf dist

# The port is fixed because the GitHub OAuth app's callback URL names it.
PORT       ?= 8081
DATA       ?= komodoc-data
PUBLISHERS ?= any
COMMENTERS ?= anyone

serve: $(BIN)  ## Run the server and open it in Firefox (PORT=, DATA=, PUBLISHERS=, COMMENTERS=)
	@command -v firefox >/dev/null && (sleep 1; firefox http://localhost:$(PORT) >/dev/null 2>&1 &) || true
	@$(BIN) serve --port $(PORT) --data $(DATA) --publishers $(PUBLISHERS) --commenters $(COMMENTERS)

# One example per source format Komodoc accepts, each genuinely produced by the
# tool it is named after. style-guide.html is hand-written and regression-tables.md
# is rendered by Komodoc itself at publish time, so neither has a rule below.
EXAMPLES := examples/bootstrap.html examples/newton.html \
            examples/random-walks.html examples/simpsons-paradox.html \
            examples/style-guide.html examples/regression-tables.md

# Not in the help: a step of `deploy` and `deploy-sandbox`, not an entry point.
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
# self-contained and carries no seeded annotations -- see seed_examples.go.
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

# The two deployments. `deploy` is this machine; `deploy-sandbox` is the
# Cloudflare service. Both start from a freshly seeded set of examples, so
# either one is a known state rather than whatever was left over.
#
# There is only the one Cloudflare deployment for now, and it is a sandbox.
# It reads its own _SANDBOX credentials, so a second, less disposable service
# can be added later without either one inheriting the other's settings.
# Supply them however you like -- in .env, or through sops:
#
#     sops exec-env .keys.yaml 'make deploy-sandbox'
.PHONY: deploy deploy-sandbox

# No sign-in at all: publishing and commenting are both open, so this needs
# no GitHub OAuth app and no `komodoc login`.
deploy: seed  ## Seed the examples and serve them on this machine, no sign-in
	@$(MAKE) serve PUBLISHERS=anyone COMMENTERS=anyone

# The label is the first component of the endpoint host, so the URL is stated
# once and the two cannot drift apart.
# Only these accounts may install the reserved examples; they never expire.
EXAMPLE_PUBLISHERS ?= vincentarelbundock

SANDBOX_LABEL = $(firstword $(subst ., ,$(patsubst https://%,%,$(KOMODOC_ENDPOINT_SANDBOX))))

deploy-sandbox: $(BIN) $(EXAMPLES)  ## Deploy to Cloudflare and publish the examples there
	@test -n "$$KOMODOC_ENDPOINT_SANDBOX" || { echo "set KOMODOC_ENDPOINT_SANDBOX"; exit 1; }
	@test -n "$$KOMODOC_GITHUB_CLIENT_ID_SANDBOX" || { echo "set KOMODOC_GITHUB_CLIENT_ID_SANDBOX"; exit 1; }
	@test -n "$$KOMODOC_GITHUB_CLIENT_SECRET_SANDBOX" || { echo "set KOMODOC_GITHUB_CLIENT_SECRET_SANDBOX"; exit 1; }
	@# Through the environment, not the command line: an argument is visible
	@# in ps to every process on the machine, an environment variable is not.
	@KOMODOC_GITHUB_CLIENT_ID="$$KOMODOC_GITHUB_CLIENT_ID_SANDBOX" \
		KOMODOC_GITHUB_CLIENT_SECRET="$$KOMODOC_GITHUB_CLIENT_SECRET_SANDBOX" \
		$(BIN) deploy --label $(SANDBOX_LABEL) \
		--publishers $(PUBLISHERS) --commenters $(COMMENTERS) --examples $(EXAMPLE_PUBLISHERS) \
		--max-size 4 --quota 100 --expire-after 24h
	@$(BIN) login --client-id "$$KOMODOC_GITHUB_CLIENT_ID_SANDBOX" --endpoint "$$KOMODOC_ENDPOINT_SANDBOX"
	@$(BIN) seed --endpoint "$$KOMODOC_ENDPOINT_SANDBOX"

# A target cannot export into the shell that ran make, so `secrets` opens a
# subshell with the keys loaded. A one-off command can be wrapped as:
#
#     sops exec-env $(KEYS) 'make deploy-sandbox'
KEYS ?= .keys.yaml
.PHONY: secrets

secrets:  ## Open an interactive shell with the sops-encrypted keys in its environment
	@test -f $(KEYS) || { echo "no $(KEYS)"; exit 1; }
	@test -t 0 || { echo "make secrets opens an interactive subshell and needs a terminal" >&2; echo "use: sops exec-env $(KEYS) 'make deploy-sandbox'" >&2; exit 2; }
	@echo "$(KEYS) is loaded in this shell; exit to drop it"
	@sops exec-env $(KEYS) "$${SHELL:-/bin/sh}"
