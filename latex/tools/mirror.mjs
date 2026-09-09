// Manifest read/write helpers shared by the mirror-serving dev tools.
//
// Building the mirror itself -- the compiler engines, TeX Live bundles, the
// SwiftLaTeX/BusyTeX/TeXlyre distribution comparisons this file used to
// fetch, and the SwiftLaTeX package-fetching endpoint -- moved to the
// wasm-latex repository (`make mirror`, `make push` there; layout and
// manifest in wasm-latex/docs/mirror.md). Nothing here fetches anything
// anymore; this file only reads and writes `manifest.json` in the shape
// `latex/tools/serve.mjs` expects.

import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

const HERE = dirname(new URL(import.meta.url).pathname);
// This file is latex/tools/mirror.mjs, so the repository is two directories up.
const REPO = dirname(dirname(HERE));
const OUT = join(REPO, "latex", "mirror");

/// The manifest as it stands, or an empty one. It is the mirror's index and
/// the only file the browser fetches by a name without a digest in it.
export function readManifest(out = OUT) {
  const path = join(out, "manifest.json");
  if (!existsSync(path)) return { format: 1, version: 1, releases: {} };
  return JSON.parse(readFileSync(path, "utf8"));
}

export function writeManifest(manifest, out = OUT) {
  mkdirSync(out, { recursive: true });
  writeFileSync(join(out, "manifest.json"), JSON.stringify(manifest, null, 2) + "\n");
}

if (process.argv[1] && process.argv[1].endsWith("mirror.mjs")) {
  console.log("mirror.mjs no longer builds a mirror: run `make mirror` (and `make push`) in wasm-latex.");
}
