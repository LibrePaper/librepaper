import { readFileSync } from "node:fs";

// KaTeX as the build serves it. The path is named for the version the
// lockfile pins, so a new KaTeX is a new path, and the copy tools/vendor-katex.mjs
// makes and the two Vite configs that name it cannot disagree.
export const KATEX_VERSION = JSON.parse(
  readFileSync(new URL("../node_modules/katex/package.json", import.meta.url), "utf8"),
).version;

export const KATEX_PATH = `/assets/katex-${KATEX_VERSION}/`;
