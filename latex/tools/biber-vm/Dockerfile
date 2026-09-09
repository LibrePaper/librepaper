# Minimal 32-bit Debian guest containing only Biber (no TeX, no Perl, no
# fonts). Biber's TeX Live binary distribution is a self-contained,
# statically-linked-enough executable: it depends only on libc/libpthread,
# both already present in the base image (confirmed with `ldd` inside an
# i386 container during development; see sources.json). Docker's i386
# platform on an amd64 host needs no CPU emulation/binfmt handler - the CPU
# executes 32-bit x86 instructions natively, only the userland ABI differs.
FROM --platform=linux/386 debian:bookworm-slim@sha256:88200866dfff7ea7f5cbcb6ec7c8a701889efe6fe859fe64d6990e4b07ea4171

# Pinned Biber 2.21 i386-linux binary from the frozen TeX Live 2025 tlnet-final
# repository (see sources.json for the exact URL/digest).
# This VM artifact preserves the Biber 2.21 pairing used by an earlier browser
# release. The current bundled release pairs BCF 3.11 with Biber 2.22.
# Extracted from the upstream .tar.xz on the build host
# (assets/biber.i386-linux.tar.xz -> assets/biber) because bookworm-slim ships
# no xz-utils and pulling one in just to unpack once is unnecessary guest bloat.
COPY assets/biber /usr/local/bin/biber

# Biber's TeX Live binary is a PAR::Packer (`pp`) self-extracting executable:
# it bundles its own private Perl runtime and CPAN modules and unpacks them
# into $TMPDIR/par-<hash>/ on first run (that is how it needs no system Perl
# at all, per sources.json's ldd evidence). Verifying it with `biber
# --version` during the build therefore leaves a ~97 MB extraction cache
# under /tmp that would otherwise get baked permanently into this layer -- it
# must be removed in the SAME RUN so it never lands in a committed layer.
# (The guest still extracts this cache again lazily on its own first `biber`
# invocation at VM runtime; that is unavoidable and happens once per boot.)
RUN set -eu; \
    chmod 0755 /usr/local/bin/biber; \
    rm -rf /usr/share/doc /usr/share/man /usr/share/info /usr/share/locale; \
    rm -rf /var/lib/apt/lists/* /var/cache/apt/*; \
    mkdir -p /work; \
    /usr/local/bin/biber --version; \
    rm -rf /tmp/*

# glibc >= 2.35 (bookworm ships 2.36) provides C.UTF-8 without a locales
# package or locale-gen; Biber needs a UTF-8 locale for Unicode sorting.
ENV LANG=C.UTF-8 LC_ALL=C.UTF-8
WORKDIR /work
