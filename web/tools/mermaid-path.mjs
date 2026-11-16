import { readFileSync } from "node:fs";

// Mermaid as the build serves it. The path is named for the version the
// lockfile pins, so a new Mermaid is a new path, and the copy tools/vendor-mermaid.mjs
// makes and the two Vite configs that name it cannot disagree.
export const MERMAID_VERSION = JSON.parse(
  readFileSync(new URL("../node_modules/mermaid/package.json", import.meta.url), "utf8"),
).version;

export const MERMAID_PATH = `/assets/mermaid-${MERMAID_VERSION}/`;
