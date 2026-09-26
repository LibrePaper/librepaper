#!/bin/sh
# SPEC-loro.md §6 Phase 3: "The Rust crate and the npm wasm build are pinned to
# one version from one place, and CI fails if they diverge."
#
# Loro ships the Rust crate and the wasm/npm build from one repository at one
# version. `loro-crdt` carries a patch number the crate does not, so agreement
# is on major.minor -- a divergence there means the two sides are speaking
# different wire formats, which is the whole failure this guards.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

rust=$(sed -n 's/^loro = "\([0-9.]*\)".*/\1/p' "$root/crates/librepaper/Cargo.toml")
npm=$(sed -n 's/.*"loro-crdt": "\([0-9.]*\)".*/\1/p' "$root/web/package.json")

[ -n "$rust" ] || { echo "check-loro-pin: no loro version in crates/librepaper/Cargo.toml" >&2; exit 1; }
[ -n "$npm" ]  || { echo "check-loro-pin: no loro-crdt version in web/package.json" >&2; exit 1; }

minor() { echo "$1" | cut -d. -f1,2; }

if [ "$(minor "$rust")" != "$(minor "$npm")" ]; then
	echo "check-loro-pin: loro $rust (rust) and loro-crdt $npm (npm) disagree" >&2
	echo "  the wire format is shared; these must move together" >&2
	exit 1
fi
echo "check-loro-pin: loro $rust / loro-crdt $npm agree on $(minor "$rust")"
