# The image is the released binary and nothing else. LibrePaper is one static
# musl executable with the pages, the renderers and the server compiled in, so
# there is no toolchain to carry and nothing to assemble at start-up: the build
# fetches a release archive, checks it against the release checksums, and stops
# there. Building from a fork means building the archive first -- `make build`
# -- and swapping the fetch below for a COPY.
#
#   docker build --build-arg LIBREPAPER_VERSION=v0.0.3 -t librepaper .
FROM alpine:3.21

# The version to install. There is no default on purpose: an image that
# silently tracked "latest" would change under a deployment that rebuilt it.
ARG LIBREPAPER_VERSION
# Supplied by buildkit; the release archives are named for the same pair.
ARG TARGETARCH

# ca-certificates for the OAuth providers and the object store; wget is also
# what the healthcheck uses.
RUN apk add --no-cache ca-certificates wget

RUN set -eu; \
	[ -n "${LIBREPAPER_VERSION:-}" ] || { echo "build: set --build-arg LIBREPAPER_VERSION" >&2; exit 1; }; \
	case "$TARGETARCH" in \
		amd64|arm64) arch="$TARGETARCH" ;; \
		*) echo "build: no release archive for $TARGETARCH" >&2; exit 1 ;; \
	esac; \
	archive="librepaper_linux_${arch}.tar.gz"; \
	base="https://github.com/LibrePaper/librepaper/releases/download/${LIBREPAPER_VERSION}"; \
	cd /tmp; \
	wget -q "$base/$archive"; \
	wget -q "$base/checksums.txt"; \
	# checksums.txt covers every archive in the release; check only the one
	# this build downloaded, so an unrelated missing file is not a failure.
	grep " ${archive}\$" checksums.txt > wanted.txt; \
	sha256sum -c wanted.txt; \
	tar -xzf "$archive" -C /usr/local/bin librepaper; \
	rm -f "$archive" checksums.txt wanted.txt; \
	librepaper --version

# The deployment's own directory: immutable objects under the filesystem
# profile, and the secrets that keep sessions and share links valid. That and
# the place backups are written are the two paths in this image worth a volume.
RUN adduser -S -D -H -u 10001 librepaper \
	&& install -d -o librepaper -g nogroup /var/lib/librepaper /var/backups/librepaper
# Declared here, not only in compose: a fresh named volume is initialised from
# the image directory it covers and inherits its ownership, while a volume
# Docker invents on its own belongs to root -- and the server does not run as
# root.
VOLUME /var/lib/librepaper
VOLUME /var/backups/librepaper
USER librepaper

# Both origins arrive on this one port; the server routes them by Host.
EXPOSE 8080
ENV LIBREPAPER_DATA=/var/lib/librepaper

# Loopback is always served whatever --origin says, so the check does not have
# to know the deployment's public name.
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
	CMD wget -qO- http://127.0.0.1:8080/health >/dev/null || exit 1

ENTRYPOINT ["librepaper"]
CMD ["admin", "serve", "--port", "8080"]
