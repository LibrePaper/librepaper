// Integration check against the actual built WASM, using Poppler as an
// independent PDF parser. Run after `make wasm`; requires pdfinfo/pdftotext.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { call, handOver } from "../../src/lib/renderer-wasm.js";

const corpus = new URL("../fixtures/typst-corpus/", import.meta.url);
const { instance } = await WebAssembly.instantiate(readFileSync(new URL("../../dist/wasm/typst.wasm", import.meta.url)), {});
const wasm = instance.exports;
const directory = mkdtempSync(join(tmpdir(), "librepaper-typst-corpus-"));
const inputs = Object.fromEntries(["paper.typ", "lib.typ", "long.typ", "refs.bib", "broken.typ"].map((name) => [name, readFileSync(new URL(name, corpus), "utf8")]));
const assets = { "asset.svg": new Uint8Array(readFileSync(new URL("asset.svg", corpus))) };
const inspect = (result, name) => {
  assert.equal(result.kind, "pdf");
  assert.equal(result.ok, true, JSON.stringify(result.diagnostics));
  const file = join(directory, `${name}.pdf`);
  writeFileSync(file, result.bytes);
  const environment = { ...process.env, LC_ALL: "C" };
  const info = execFileSync("pdfinfo", [file], { encoding: "utf8", env: environment });
  const text = execFileSync("pdftotext", ["-layout", file, "-"], { encoding: "utf8", env: environment });
  return { text, pages: Number(info.match(/^Pages:\s+(\d+)/m)?.[1]), info };
};
const compile = (main, source = inputs[main]) => {
  handOver(wasm, { main, texts: { ...inputs, [main]: source }, assets });
  return call(wasm, "compile", source, "PDF fixture");
};

try {
  const paper = inspect(compile("paper.typ"), "paper");
  for (const phrase of ["A paged Typst corpus", "Typst PDF fixture", "Input", "Estimate", "An embedded SVG asset"])
    assert.ok(paper.text.includes(phrase), `PDF text is missing ${phrase}`);
  assert.match(paper.info, /Page size:\s+595[.\d]* x 841[.\d]* pts/);
  const long = inspect(compile("long.typ"), "long");
  assert.ok(long.pages > 1, "long fixture must have multiple pages");
  assert.ok(long.text.includes("Section 23"));
  const first = compile("paper.typ");
  const preserved = first.bytes.slice();
  const broken = compile("broken.typ");
  assert.equal(broken.ok, false);
  assert.equal(broken.bytes.length, 0);
  assert.ok(broken.diagnostics.some((item) => item.severity === "error"));
  assert.deepEqual(first.bytes, preserved, "later compilation must not overwrite an earlier PDF");

  const times = [];
  const memories = [];
  for (let index = 0; index < (process.argv.includes("--soak") ? 100 : 8); index++) {
    const marker = `Revision ${index} is visible.`;
    const started = performance.now();
    const result = compile("paper.typ", `${inputs["paper.typ"]}\n\n${marker}\n`);
    times.push(Math.round(performance.now() - started));
    memories.push(wasm.memory.buffer.byteLength);
    assert.ok(inspect(result, `revision-${index}`).text.includes(marker));
  }
  console.log("typst-pdf: actual WASM PDF text, page geometry, imports/assets, error recovery and owned bytes passed");
  console.log(JSON.stringify({ warm_compile_ms: times, wasm_memory_bytes: memories, long_pages: long.pages }));
} finally {
  if (process.argv.includes("--keep")) console.log(`PDF artifacts: ${directory}`);
  else rmSync(directory, { recursive: true, force: true });
}
