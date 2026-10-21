import { readFile } from "node:fs/promises";
import { join } from "node:path";

// Every pinned file: its name, the release repository it comes from, and the
// prefix it lives under, both in web/ locally and on the asset mirror.
export const PINNED_FILES = new Map([
  ["markdown.wasm", { repo: "wasm-markdown", prefix: "wasm" }],
  ["bibliography.wasm", { repo: "wasm-bibliography", prefix: "wasm" }],
  ["citations.wasm", { repo: "wasm-bibliography", prefix: "wasm" }],
  ["typst.wasm", { repo: "wasm-typst", prefix: "wasm" }],
  ...["jetbrains-mono", "jetbrains-mono-italic", "fira-code", "source-code-pro", "source-code-pro-italic",
    "ibm-plex-mono", "ibm-plex-mono-italic", "ibm-plex-mono-semibold", "cascadia-code", "cascadia-code-italic",
    "opendyslexic-mono"].map((name) => [`${name}.woff2`, { repo: "editor-fonts", prefix: "fonts" }]),
]);
const SHA256 = /^[a-f0-9]{64}$/;

export function parseAssetsLockText(text) {
  const rows = text.split(/\r?\n/).map((raw) => {
    const line = raw.replace(/#.*$/, "").trim();
    if (!line) return null;
    const fields = line.split(/\s+/);
    if (fields.length !== 4 || !SHA256.test(fields[3])) throw new Error("assets.lock contains a malformed entry");
    return { module: fields[0], repo: fields[1], tag: fields[2], sha256: fields[3] };
  }).filter(Boolean);
  const latex = rows.filter(({ module }) => module === "latex");
  const pinned = rows.filter(({ module }) => module !== "latex").map((row) => ({ ...row, prefix: PINNED_FILES.get(row.module)?.prefix }));
  if (latex.length !== 1 || latex[0].repo !== "wasm-latex" || pinned.length !== PINNED_FILES.size ||
      new Set(pinned.map(({ module }) => module)).size !== PINNED_FILES.size ||
      pinned.some(({ module, repo }) => PINNED_FILES.get(module)?.repo !== repo)) {
    throw new Error("assets.lock must contain exactly the required pinned files (wasm modules and editor fonts) once each plus one latex row");
  }
  return { text, rows, pinned, latex: latex[0] };
}

export async function parseAssetsLock(root) {
  return parseAssetsLockText(await readFile(join(root, "assets.lock"), "utf8"));
}
