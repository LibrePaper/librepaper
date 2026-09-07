#!/bin/sh
set -eu
# TinyTeX's infraonly profile + pinned pkgs-custom.txt, with the binary platform
# selected explicitly for v86. All TeX packages come from one frozen repository.
repo=https://texlive.info/historic/systems/texlive/2025/tlnet-final
cd /build
tar -xzf install-tl.tar.gz
cat > tinytex.profile <<'EOF'
selected_scheme scheme-infraonly
TEXDIR /opt/tinytex
TEXMFCONFIG /opt/tinytex/texmf-config
TEXMFVAR /opt/tinytex/texmf-var
TEXMFSYSCONFIG /opt/tinytex/texmf-config
TEXMFSYSVAR /opt/tinytex/texmf-var
TEXMFLOCAL /opt/tinytex/texmf-local
binary_i386-linux 1
instopt_portable 1
tlpdbopt_install_docfiles 0
tlpdbopt_install_srcfiles 0
tlpdbopt_autobackup 0
EOF
perl install-tl-*/install-tl --no-gui --profile tinytex.profile --repository "$repo" --no-interaction
PATH=/opt/tinytex/bin/i386-linux:$PATH
export PATH
tlmgr option repository "$repo"
tlmgr conf texmf max_print_line 10000
# The additional packages exercise ACM and real Biber, beyond TinyTeX-1's core.
xargs tlmgr install < pkgs-custom.txt
tlmgr install biblatex biber csquotes acmart cm-super microtype pgf siunitx libertinus-fonts
fmtutil-sys --all
updmap-sys
cp pkgs-custom.txt /opt/tinytex/tinytex-pkgs-custom.txt
rm -rf /build
