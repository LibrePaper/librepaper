// Release read helpers shared by the mirror-serving dev tools.
//
// Building the mirror itself -- the compiler engines, TeX Live bundles, the
// SwiftLaTeX/BusyTeX/TeXlyre distribution comparisons this file used to
// fetch, and the SwiftLaTeX package-fetching endpoint -- moved to the
// wasm-latex repository (`make mirror`, `make push` there; layout in
// wasm-latex/docs/release.md). Nothing here fetches anything anymore; this
// file only finds and reads the immutable release directories
// (`<sha256>/release.json`) `tools/latex/tools/serve.mjs` expects.

import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
// This file is tools/latex/tools/mirror.mjs, so the repository is three directories up.
const REPO = dirname(dirname(dirname(HERE)));
const OUT = join(REPO, "tools", "latex", "mirror");

/// The release ids (directory names) present in a mirror directory.
export function releaseIds(out = OUT) {
  if (!existsSync(out)) return [];
  return readdirSync(out, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && /^[a-f0-9]{64}$/.test(entry.name))
    .map((entry) => entry.name)
    .sort();
}

/// One release's `release.json` (format 2, paths relative to its directory).
export function readRelease(id, out = OUT) {
  return JSON.parse(readFileSync(join(out, id, "release.json"), "utf8"));
}

if (process.argv[1] && process.argv[1].endsWith("mirror.mjs")) {
  console.log("mirror.mjs no longer builds a mirror: run `make mirror` (and `make push`) in wasm-latex.");
}
