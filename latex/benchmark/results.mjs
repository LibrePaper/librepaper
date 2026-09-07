import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { HERE } from "./corpus.mjs";

export const RESULTS = join(HERE, "results");
mkdirSync(RESULTS, { recursive: true });
export const writeJSON = (path, value) => writeFileSync(path, JSON.stringify(value, null, 2) + "\n");
export function inspectPDF(path) {
  if (!existsSync(path)) return { pdf: false, pages: 0, text: "" };
  try {
    const info = execFileSync("pdfinfo", [path], { encoding: "utf8", timeout: 10000 });
    const text = execFileSync("pdftotext", ["-layout", path, "-"], { encoding: "utf8", timeout: 10000 });
    return { pdf: true, pages: Number(info.match(/^Pages:\s+(\d+)/m)?.[1] || 0), text };
  } catch (error) {
    if (["ENOENT", "EPERM", "EACCES"].includes(error.code)) throw error;
    return { pdf: false, pages: 0, text: "", pdfError: error.message };
  }
}
export function warnings(log) {
  return [...new Set(log.split("\n").filter((line) =>
    /Missing character:|undefined references|undefined citations|Citation .*undefined|Empty bibliography|Please \(re\)run Biber|Please rerun LaTeX|Rerun to get cross-references/i.test(line)
  ).map((line) => line.trim()))];
}
export function textAgreement(left, right) {
  const words = (s) => {
    const counts = new Map();
    for (const word of s.normalize("NFKC").match(/[\p{L}\p{N}]+/gu) || []) counts.set(word, (counts.get(word) || 0) + 1);
    return counts;
  };
  const a = words(left), b = words(right);
  const total = (map) => [...map.values()].reduce((sum, n) => sum + n, 0);
  const shared = [...a].reduce((sum, [word, count]) => sum + Math.min(count, b.get(word) || 0), 0);
  return total(a) + total(b) ? 2 * shared / (total(a) + total(b)) : null;
}
export function referenceFor(id) {
  const path = join(RESULTS, "native", id, "result.json");
  return existsSync(path) ? JSON.parse(readFileSync(path, "utf8")) : null;
}
