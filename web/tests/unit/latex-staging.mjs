// Staging a project tree into a nested engine: retain sweeps and changed-only
// writes when the engine supports it, flush-and-write otherwise.
import assert from "node:assert/strict";
import { test } from "node:test";
import { sameBytes, stageEngine } from "../../src/lib/latex/staging.js";

// A fake engine that records every call. `retainImpl` plays the controller's
// part of `retain`: it deletes what is not kept and returns the dirty set.
function fakeEngine(kind = "pdftex", { retainImpl } = {}) {
  const engine = {
    kind,
    staged: null,
    calls: [],
    files: new Map(),
    async flushCache() {
      engine.calls.push(["flushCache"]);
      engine.files.clear();
    },
    async mkdir(path) {
      engine.calls.push(["mkdir", path]);
    },
    async writeFile(path, value) {
      engine.calls.push(["writeFile", path]);
      engine.files.set(path, value);
    },
    async retain(keep) {
      engine.calls.push(["retain", keep]);
      if (retainImpl) return retainImpl(engine, keep);
      for (const path of [...engine.files.keys()]) {
        if (!keep.includes(path)) engine.files.delete(path);
      }
      return new Set();
    },
  };
  return engine;
}

const written = (engine) => engine.calls.filter(([cmd]) => cmd === "writeFile").map(([, path]) => path);
const names = (engine, cmd) => engine.calls.filter(([c]) => c === cmd).length;

test("first stage with canRetain flushes, writes the whole tree and records it", async () => {
  const engine = fakeEngine();
  const tree = { texts: { "main.tex": "\\documentclass{article}", "sec/a.tex": "A" }, assets: { "fig.png": Uint8Array.of(1, 2) } };
  await stageEngine(engine, tree, { canRetain: true });
  assert.equal(names(engine, "flushCache"), 1);
  assert.equal(names(engine, "retain"), 0);
  assert.deepEqual(written(engine).sort(), ["fig.png", "main.tex", "sec/a.tex"]);
  assert.ok(engine.calls.some(([cmd, path]) => cmd === "mkdir" && path === "sec"));
  assert.ok(engine.staged instanceof Map);
  assert.equal(engine.staged.size, 3);
  assert.equal(engine.staged.get("main.tex"), "\\documentclass{article}");
});

test("second stage with one text changed retains all tree paths and writes only that text", async () => {
  const engine = fakeEngine();
  const first = { texts: { "main.tex": "one", "b.tex": "two" }, assets: {} };
  await stageEngine(engine, first, { canRetain: true });
  engine.calls.length = 0;
  const second = { texts: { "main.tex": "one", "b.tex": "changed" }, assets: {} };
  await stageEngine(engine, second, { canRetain: true });
  assert.equal(names(engine, "flushCache"), 0);
  assert.equal(names(engine, "retain"), 1);
  const keep = engine.calls.find(([cmd]) => cmd === "retain")[1];
  assert.deepEqual([...keep].sort(), ["b.tex", "main.tex"]);
  assert.deepEqual(written(engine), ["b.tex"]);
  assert.equal(engine.staged.get("b.tex"), "changed");
});

test("a removed file is not written and is absent from keep", async () => {
  const engine = fakeEngine();
  await stageEngine(engine, { texts: { "main.tex": "m", "old.tex": "o" }, assets: {} }, { canRetain: true });
  engine.calls.length = 0;
  await stageEngine(engine, { texts: { "main.tex": "m" }, assets: {} }, { canRetain: true });
  const keep = engine.calls.find(([cmd]) => cmd === "retain")[1];
  assert.ok(!keep.includes("old.tex"));
  assert.deepEqual(written(engine), []);
  assert.ok(!engine.staged.has("old.tex"));
});

test("a dirty path reported by retain is rewritten even though unchanged", async () => {
  const engine = fakeEngine("xetex", {
    retainImpl: () => new Set(["main.tex"]),
  });
  await stageEngine(engine, { texts: { "main.tex": "same" }, assets: {} }, { canRetain: true });
  engine.calls.length = 0;
  await stageEngine(engine, { texts: { "main.tex": "same" }, assets: {} }, { canRetain: true });
  assert.deepEqual(written(engine), ["main.tex"]);
  assert.equal(names(engine, "flushCache"), 0);
});

test("retain throwing falls back to flushCache and a full write", async () => {
  let fail = false;
  const engine = fakeEngine("pdftex", {
    retainImpl: () => {
      if (fail) throw new Error("boom");
      return new Set();
    },
  });
  const tree = { texts: { "main.tex": "m", "b.tex": "b" }, assets: {} };
  await stageEngine(engine, tree, { canRetain: true });
  engine.calls.length = 0;
  fail = true;
  await stageEngine(engine, tree, { canRetain: true });
  assert.equal(names(engine, "retain"), 1);
  assert.equal(names(engine, "flushCache"), 1);
  assert.deepEqual(written(engine).sort(), ["b.tex", "main.tex"]);
  assert.ok(engine.staged instanceof Map);
});

test("canRetain false always flushes and writes the whole tree, never records staged", async () => {
  const engine = fakeEngine();
  const tree = { texts: { "main.tex": "m" }, assets: {} };
  await stageEngine(engine, tree, { canRetain: false });
  engine.calls.length = 0;
  await stageEngine(engine, tree, { canRetain: false });
  assert.equal(names(engine, "retain"), 0);
  assert.equal(names(engine, "flushCache"), 1);
  assert.deepEqual(written(engine), ["main.tex"]);
  assert.equal(engine.staged, null);
});

test("an engine kind without retain support is never swept", async () => {
  const engine = fakeEngine("bibtex");
  const tree = { texts: { "refs.bib": "@article{x}" }, assets: {} };
  await stageEngine(engine, tree, { canRetain: true });
  assert.equal(names(engine, "retain"), 0);
  assert.equal(names(engine, "flushCache"), 1);
  assert.equal(engine.staged, null);
});

test("generated files are kept by retain but never recorded as staged", async () => {
  const engine = fakeEngine();
  await stageEngine(engine, { texts: { "main.tex": "m" }, assets: {} }, { canRetain: true, generated: { "main.aux": Uint8Array.of(1) } });
  engine.calls.length = 0;
  await stageEngine(engine, { texts: { "main.tex": "m" }, assets: {} }, { canRetain: true, generated: { "main.aux": Uint8Array.of(1) } });
  const keep = engine.calls.find(([cmd]) => cmd === "retain")[1];
  assert.ok(keep.includes("main.aux"));
  assert.ok(!engine.staged.has("main.aux"));
});

test("an asset with the same bytes in a new Uint8Array is not rewritten; different bytes are", async () => {
  const engine = fakeEngine();
  await stageEngine(engine, { texts: {}, assets: { "fig.png": Uint8Array.of(1, 2, 3) } }, { canRetain: true });
  engine.calls.length = 0;
  await stageEngine(engine, { texts: {}, assets: { "fig.png": Uint8Array.of(1, 2, 3) } }, { canRetain: true });
  assert.deepEqual(written(engine), []);
  engine.calls.length = 0;
  await stageEngine(engine, { texts: {}, assets: { "fig.png": Uint8Array.of(1, 2, 4) } }, { canRetain: true });
  assert.deepEqual(written(engine), ["fig.png"]);
});

test("sameBytes compares lengths, contents, and rejects strings", () => {
  assert.equal(sameBytes(Uint8Array.of(1, 2), Uint8Array.of(1, 2)), true);
  assert.equal(sameBytes(Uint8Array.of(1, 2), Uint8Array.of(1)), false);
  assert.equal(sameBytes(Uint8Array.of(1, 2), Uint8Array.of(1, 3)), false);
  assert.equal(sameBytes(Uint8Array.of(65), "A"), false);
});
