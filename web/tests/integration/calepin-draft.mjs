// Integration check that the browser's Typst compiler draws a Calepin notebook
// from stored results, against the actual built WASM. The fixture in
// fixtures/calepin is what `calepin compile` stored, minus the runtime.
// Requires web/wasm/typst.wasm (build it first, as for typst-pdf.mjs).
import assert from "node:assert/strict";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { FILES } from "../../src/lib/calepin-runtime.js";
import {
  MISSING_LABEL,
  overlay,
  recordOf,
  runtimeFiles,
  staleChunks,
} from "../../src/lib/calepin-results.js";
import { call, handOver } from "../../src/lib/renderer-wasm.js";

const fixture = new URL("../fixtures/calepin/", import.meta.url);
const storedRoot = fileURLToPath(new URL("stored/", fixture));
const { instance } = await WebAssembly.instantiate(readFileSync(new URL("../../wasm/typst.wasm", import.meta.url)), {});
const wasm = instance.exports;

const source = readFileSync(new URL("paper.typ", fixture), "utf8");
// `stored/X` in the fixture stands for `.calepin/X`, next to the bundled runtime.
const stored = [
  ...FILES,
  ...readdirSync(storedRoot, { recursive: true })
    .filter((name) => statSync(join(storedRoot, name)).isFile())
    .map((name) => ({
      path: ".calepin/" + relative(storedRoot, join(storedRoot, name)).split(sep).join("/"),
      text: readFileSync(join(storedRoot, name), "utf8"),
    })),
];

const tree = { main: "paper.typ", texts: { "paper.typ": source }, assets: {} };
const compile = (files, text = source, options) => {
  const merged = files === null ? tree : overlay({ ...tree, texts: { "paper.typ": text } }, files, options);
  handOver(wasm, { main: "paper.typ", texts: merged.texts, assets: merged.assets });
  return call(wasm, "compile", text, "paper");
};

// 1. Stored results draw the notebook.
const drawn = compile(stored);
assert.equal(drawn.ok, true, `stored results do not compile: ${JSON.stringify(drawn.diagnostics)}`);
assert.equal(drawn.kind, "pdf", "stored results should produce a PDF");
assert.ok(drawn.bytes.length > 0, "stored results produced no PDF bytes");

// 2. The stored code matches the fixture; editing a chunk makes one stale.
assert.equal(staleChunks(stored, source, { path: "paper.typ" }), 0, "the fixture's own chunks should not be stale");
const edited = source.replace("40 + 2", "40 + 3");
assert.notEqual(edited, source, "the fixture must contain 40 + 2 to edit");
assert.equal(staleChunks(stored, edited, { path: "paper.typ" }), 1, "one edited chunk should be one stale chunk");

// 3. An edited chunk still compiles with the stored results around it.
assert.equal(compile(stored, edited).ok, true, "an edited chunk should still compile against stored results");

// 4. An added chunk has no stored result, so the runtime stops.
const added = source + "\n```python\nprint(7)\n```\n";
const stopped = compile(stored, added);
assert.equal(stopped.ok, false, "an added chunk should stop the runtime");
assert.ok(
  stopped.diagnostics.some((item) => MISSING_LABEL.test(item.message ?? "")),
  `the added chunk should report a missing label: ${JSON.stringify(stopped.diagnostics)}`,
);

// 5. Turning stored results off lets the same added-chunk source compile.
assert.equal(compile(stored, added, { disabled: true }).ok, true, "disabled results should let the added chunk compile");

// 6. A notebook never run compiles with the bundled runtime alone.
const neverRun = compile(await runtimeFiles(), source, { disabled: true });
assert.equal(neverRun.ok, true, `a notebook never run should compile with the bundled runtime: ${JSON.stringify(neverRun.diagnostics)}`);

// 7. Without any overlay, the notebook imports a file the project does not hold.
const bare = compile(null);
assert.equal(bare.ok, false, "without the runtime the notebook should fail to compile");

// 8. The wire form round-trips into a record.
const record = recordOf(JSON.stringify({ version: 1, files: stored }), "paper.typ");
assert.ok(record, "the wire form should parse into a record");
assert.equal(record.engine, "calepin", "the record should name the calepin engine");
assert.equal(record.path, "paper.typ", "the record should keep the document path");
assert.equal(record.files.length, stored.length, "the record should keep every stored file");

// 9. A notebook in a subfolder is drawn from the same stored files.
const nested = "chapters/paper.typ";
const nestedOverlay = overlay({ main: nested, texts: { [nested]: source }, assets: {} }, stored);
handOver(wasm, { main: nested, texts: nestedOverlay.texts, assets: nestedOverlay.assets });
const nestedDrawn = call(wasm, "compile", source, "paper");
assert.equal(nestedDrawn.ok, true, `a notebook in a subfolder does not compile: ${JSON.stringify(nestedDrawn.diagnostics)}`);
assert.equal(staleChunks(stored, source, { path: nested }), 0, "a notebook in a subfolder should find its stored results");

console.log(
  "calepin-draft: stored results, stale counting, disabled and never-run notebooks, the wire record and a notebook in a subfolder passed",
);
