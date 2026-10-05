import { readFile } from "node:fs/promises";
import { join } from "node:path";

export const PINNED_MODULES = new Map([
  ["markdown.wasm", "wasm-markdown"], ["bibliography.wasm", "wasm-bibliography"],
  ["citations.wasm", "wasm-bibliography"], ["typst.wasm", "wasm-typst"],
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
  const modules = rows.filter(({ module }) => module !== "latex");
  if (latex.length !== 1 || latex[0].repo !== "wasm-latex" || modules.length !== PINNED_MODULES.size ||
      new Set(modules.map(({ module }) => module)).size !== PINNED_MODULES.size ||
      modules.some(({ module, repo }) => PINNED_MODULES.get(module) !== repo)) {
    throw new Error("assets.lock must contain exactly the four required wasm modules plus one latex row");
  }
  return { text, rows, modules, latex: latex[0] };
}

export async function parseAssetsLock(root) {
  return parseAssetsLockText(await readFile(join(root, "assets.lock"), "utf8"));
}
