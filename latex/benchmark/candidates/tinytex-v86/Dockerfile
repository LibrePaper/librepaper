FROM --platform=linux/386 debian:bookworm-slim@sha256:88200866dfff7ea7f5cbcb6ec7c8a701889efe6fe859fe64d6990e4b07ea4171
RUN apt-get update && apt-get install -y --no-install-recommends perl curl ca-certificates xz-utils fontconfig libstdc++6 && rm -rf /var/lib/apt/lists/*
COPY assets/install-tl.tar.gz assets/pkgs-custom.txt /build/
COPY install-tex.sh /build/install-tex.sh
RUN sh /build/install-tex.sh
ENV PATH="/opt/tinytex/bin/i386-linux:${PATH}"
RUN tlmgr install babel-english xstring totpages environ setspace zref hyperxmp ncctools cmap libertine newtx caption comment fancyhdr pbalance preprint fontaxes mweights tex-gyre ifmtarg trimspaces ifoddpage etex-pkg xpatch logreq && fc-cache -f
RUN tlmgr install everyshi oberdiek
RUN tlmgr install upquote
RUN mkdir -p /work /proc /dev /tmp && chmod 1777 /tmp && pdflatex --version && biber --version && xelatex --version
WORKDIR /work
