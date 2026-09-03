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

EXAMPLES := examples/bootstrap.html examples/newton.html examples/random-walks.html \
            examples/style-guide.html

examples: $(EXAMPLES)  ## Render the example documents to standalone HTML

# Quarto and Calepin both inline their figures, so each output stands alone.
# Both tools resolve paths relative to the document, so both are run from
# inside examples/ rather than from the repository root.
examples/%.html: examples/%.qmd
	@cd examples && quarto render $(notdir $<) --quiet

examples/%.html: examples/%.typ
	@cd examples && calepin compile $(notdir $<) $(notdir $@) --format html

# Plain Typst, no preprocessing and no code to run. An explicit rule, so it
# wins over the Calepin pattern above.
examples/style-guide.html: examples/style-guide.typ
	@cd examples && typst compile $(notdir $<) $(notdir $@) --format html --features html 2>/dev/null

seed: $(BIN) $(EXAMPLES)  ## Wipe the data directory and fill it with the examples
	@$(BIN) seed --data $(DATA)

kill:  ## Stop a server started with make serve
	@# The bracket stops the pattern from matching this command line itself.
	@pkill -f '[d]ist/komodoc serve' && echo "stopped" || echo "nothing to stop"
