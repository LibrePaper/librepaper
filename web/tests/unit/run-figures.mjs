import assert from "node:assert/strict";
import test from "node:test";
import { pdfRunRecord, runFigureCount, runFiguresOf, wantedPaths, withRunFigures } from "../../src/lib/run-figures.js";

const b64 = (text) => Buffer.from(text).toString("base64");

test("wantedPaths lists references that are not project files, once each", () => {
  const references = [{ path: "a.png" }, { path: "fig/b.png" }, { path: "a.png" }, { path: "c.svg" }, {}, { path: "" }];
  const tree = { texts: { "c.svg": "<svg/>" }, assets: { "a.png": new Uint8Array() }, digests: {} };
  assert.deepEqual(wantedPaths(references, tree), ["fig/b.png"]);
  assert.deepEqual(wantedPaths(references, { texts: {}, assets: {}, digests: { "fig/b.png": "x" } }), ["a.png", "c.svg"]);
  assert.deepEqual(wantedPaths(null, tree), []);
});

test("runFiguresOf keeps only safe base64 figures", () => {
  const body = JSON.stringify({
    version: 1,
    files: [
      { path: "fig/a.png", base64: b64("x") },
      { path: "fig/a.png", base64: b64("y") },
      { path: "../b.png", base64: b64("x") },
      { path: ".librepaper/c.png", base64: b64("x") },
      { path: ".calepin/d.png", base64: b64("x") },
      { path: "e.txt", text: "x" },
    ],
  });
  assert.deepEqual(runFiguresOf(body), [{ path: "fig/a.png", base64: b64("x") }]);
  assert.deepEqual(runFiguresOf(new TextEncoder().encode(body)).length, 1);
  assert.deepEqual(runFiguresOf("nope"), []);
  assert.deepEqual(runFiguresOf(null), []);
  assert.deepEqual(runFiguresOf(JSON.stringify({ version: 2, files: [] })), []);
});

test("runFigureCount ignores the Calepin folder", () => {
  assert.equal(runFigureCount([{ path: ".calepin/x/results.json", text: "{}" }, { path: "a.png", base64: "AA==" }, { path: "b/c.png", base64: "AA==" }]), 2);
  assert.equal(runFigureCount(null), 0);
});

test("withRunFigures adds files only when there are some", () => {
  const record = { version: 1, path: "p.qmd", cells: [] };
  assert.equal(withRunFigures(record, []), record);
  assert.deepEqual(withRunFigures(record, [{ path: "a.png", base64: "AA==" }]).files, [{ path: "a.png", base64: "AA==" }]);
});

test("pdfRunRecord keeps the last HTML run's cells and carries this run's figures", () => {
  const files = [{ path: "a.png", base64: "AA==" }];
  const own = { path: "p.qmd", cells: [{ id: 1 }], inlines: [{ html: "x" }], files: [{ path: "old.png", base64: "AA==" }] };
  const shared = { path: "p.qmd", cells: [{ id: 2 }], inlines: [] };
  assert.deepEqual(pdfRunRecord({ path: "p.qmd", own, shared, files }), { version: 1, path: "p.qmd", cells: [{ id: 1 }], inlines: [{ html: "x" }], files });
  assert.deepEqual(pdfRunRecord({ path: "p.qmd", own: null, shared, files }).cells, [{ id: 2 }]);
  assert.deepEqual(pdfRunRecord({ path: "p.qmd", own: { ...own, path: "other.qmd" }, shared: null, files }), { version: 1, path: "p.qmd", cells: [], inlines: [], files });
  assert.deepEqual(pdfRunRecord({ path: "p.qmd", shared: { engine: "calepin", path: "p.qmd", files: [] } }), { version: 1, path: "p.qmd", cells: [], inlines: [] });
});
