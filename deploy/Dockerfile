# syntax=docker/dockerfile:1.7
# The image is the released binary and nothing else. LibrePaper is one static
# musl executable with its pages and server compiled in. Browser renderer assets
# are fetched separately from sources pinned by the embedded lock. There is no
# toolchain to carry and nothing to assemble at start-up. By default
# the build fetches a release archive and checks it against the release
# checksums. `--build-arg SOURCE=local` copies a static musl binary named
# `librepaper` (or LIBREPAPER_BINARY) from this directory instead. For
# `SOURCE=checkout`, use the repository root as the build context and run
# `make web` first so `web/dist` is ready to embed; BuildKit skips unused stages.
#
#   docker build --build-arg LIBREPAPER_VERSION=<compatible-tag> -t librepaper .
#   docker build --build-arg SOURCE=local -t librepaper .
#   make web  # from the repository root
#   docker build -f deploy/Dockerfile --build-arg SOURCE=checkout -t librepaper .  # root context
ARG SOURCE=release
ARG RUST_BUILDER_IMAGE=rust:1-alpine

# The kit builds from deploy/; checkout and demo builds use the repository
# root. Separate context stages let each build copy the right source tree.
FROM scratch AS backup-assets-release
COPY backup/ /
FROM scratch AS backup-assets-local
COPY backup/ /
FROM scratch AS backup-assets-checkout
COPY deploy/backup/ /
FROM backup-assets-${SOURCE} AS backup-assets

FROM alpine:3.21 AS base

# ca-certificates for the OAuth providers and the object store; wget is also
# what the healthcheck uses.
RUN apk add --no-cache ca-certificates wget

# The deployment's own directory: immutable objects under the filesystem
# profile, and the secrets that keep sessions and share links valid. That and
# the place backups are written are the two paths in this image worth a volume.
RUN adduser -S -D -H -u 10001 librepaper \
	&& install -d -o librepaper -g nogroup /var/lib/librepaper /var/backups/librepaper

FROM base AS release

# The version to install. There is no default on purpose: an image that
# silently tracked "latest" would change under a deployment that rebuilt it.
ARG LIBREPAPER_VERSION
# Supplied by buildkit; map it to the Cargo Dist target archive name.
ARG TARGETARCH

# The release's .sha256 file ends with a blank line, which BusyBox sha256sum
# counts as a failed entry, so blank lines are dropped before the check.
RUN set -eu; \
	[ -n "${LIBREPAPER_VERSION:-}" ] || { echo "build: set --build-arg LIBREPAPER_VERSION" >&2; exit 1; }; \
	case "$TARGETARCH" in \
		amd64) target="x86_64-unknown-linux-musl" ;; \
		arm64) target="aarch64-unknown-linux-musl" ;; \
		*) echo "build: no release archive for $TARGETARCH" >&2; exit 1 ;; \
	esac; \
	archive="librepaper-${target}.tar.xz"; \
	base="https://github.com/LibrePaper/librepaper/releases/download/${LIBREPAPER_VERSION}"; \
	cd /tmp; \
	wget -q "$base/$archive"; \
	wget -q "$base/$archive.sha256"; \
	sed '/^$/d' "$archive.sha256" | sha256sum -c -; \
	tar -xJf "$archive" -C /usr/local/bin --strip-components=1 "librepaper-$target/librepaper"; \
	rm -f "$archive" "$archive.sha256"

FROM base AS local
ARG LIBREPAPER_BINARY=librepaper
COPY ${LIBREPAPER_BINARY} /usr/local/bin/librepaper
RUN chmod 0755 /usr/local/bin/librepaper

FROM ${RUST_BUILDER_IMAGE} AS checkout-build
ARG TARGETARCH
WORKDIR /src
RUN apk add --no-cache build-base cmake musl-dev perl
COPY Cargo.toml Cargo.lock README.md assets.lock ./
COPY .cargo ./.cargo
COPY crates ./crates
COPY docs/examples ./docs/examples
COPY docs/dev/protocol ./docs/dev/protocol
COPY web/dist ./web/dist
RUN --mount=type=cache,target=/usr/local/cargo/registry \
	--mount=type=cache,target=/usr/local/cargo/git \
	--mount=type=cache,target=/src/target \
	set -eu; \
	case "$TARGETARCH" in \
		amd64) target="x86_64-unknown-linux-musl" ;; \
		arm64) target="aarch64-unknown-linux-musl" ;; \
		*) echo "build: checkout source does not support $TARGETARCH" >&2; exit 1 ;; \
	esac; \
	rustup target add "$target"; \
	cargo build --locked --release --target "$target" -p librepaper --bin librepaper; \
	mkdir -p /out; \
	cp "/src/target/$target/release/librepaper" /out/librepaper

FROM base AS checkout
COPY --from=checkout-build /out/librepaper /usr/local/bin/librepaper
RUN chmod 0755 /usr/local/bin/librepaper

FROM ${SOURCE} AS app-runtime
RUN librepaper --version

# The backup exporter uses pg_dump from the matching major PostgreSQL release.
RUN apk add --no-cache postgresql17-client

# Declared here, not only in compose: a fresh named volume is initialised from
# the image directory it covers and inherits its ownership, while a volume
# Docker invents on its own belongs to root -- and the server does not run as
# root.
VOLUME /var/lib/librepaper
VOLUME /var/backups/librepaper
USER librepaper

# Both origins arrive on this one port; the server routes them by Host. 9091 is
# the metrics listener.
EXPOSE 8080 9091
# Bounded readiness checks do not depend on the public origin in the config.
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
	CMD wget -qO- http://127.0.0.1:8080/ready >/dev/null || exit 1

ENTRYPOINT ["librepaper"]
CMD ["admin", "serve", "--config", "/etc/librepaper/librepaper.toml"]

# A separate, unprivileged scheduler shares the app's runtime and volumes.
FROM creativeprojects/resticprofile:0.33.1 AS resticprofile-binary

FROM app-runtime AS backup
USER root
RUN set -eu; \
	apk add --no-cache openssh-client python3 restic supercronic; \
	install -d -o 10001 -g 65534 -m 0700 /run/librepaper-backup; \
	install -d -o 10001 -g 65534 -m 0755 /var/backups/librepaper/metrics
COPY --from=resticprofile-binary /usr/bin/resticprofile /usr/bin/resticprofile
COPY --from=backup-assets /profiles.toml /etc/resticprofile/profiles.toml
COPY --from=backup-assets /status.py /usr/local/bin/librepaper-backup-status
COPY --from=backup-assets /entrypoint.sh /usr/local/bin/librepaper-backup-entrypoint
RUN chmod 0644 /etc/resticprofile/profiles.toml \
	&& chmod 0755 /usr/local/bin/librepaper-backup-status /usr/local/bin/librepaper-backup-entrypoint
USER 10001:65534
HEALTHCHECK NONE
ENTRYPOINT ["/usr/local/bin/librepaper-backup-entrypoint"]
CMD []

# Keep plain `docker build` selecting the application image.
FROM app-runtime AS app
