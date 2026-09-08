// The file manager stays live while the document pane may show a checkpoint.
// Keep this small check close to Reader because that distinction is easy to
// lose when changing the history rendering code.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const reader = readFileSync(new URL("../src/components/Reader.svelte", import.meta.url), "utf8");
const body = (start, end) => {
  const from = reader.indexOf(start);
  assert.notEqual(from, -1, `Reader function not found: ${start}`);
  const to = reader.indexOf(end, from);
  assert.notEqual(to, -1, `Reader function boundary not found: ${end}`);
  return reader.slice(from, to);
};

const liveTreeNow = body("  function liveTreeNow()", "  function treeNow()");
const treeNow = body("  function treeNow()", "  // Painting the preview");

const checkpoint = {
  main: "old.md",
  texts: { "old.md": "checkpoint" },
  files: { "old.md": { kind: "text" } },
};
const live = {
  main: "project/current.md",
  texts: { "project/current.md": "live", "project/new.md": "new live file" },
  digests: { "project/fig.png": "asset-sha" },
};
const context = vm.createContext({
  session: { tree: () => live, text: { toString: () => "fallback" } },
  sourceFormat: "markdown",
  viewing: checkpoint,
  checkpointTree: (point) => ({ main: point.main, texts: point.texts, digests: {} }),
});

vm.runInContext(`${liveTreeNow}\n${treeNow}`, context);
assert.deepEqual(vm.runInContext("liveTreeNow()", context), live);
assert.deepEqual(vm.runInContext("treeNow()", context), {
  main: checkpoint.main,
  texts: checkpoint.texts,
  digests: {},
});

const downloadTree = body("  async function downloadTree()", "  // A text dropped");
const downloadEntry = body("  async function downloadEntry(entry)", "  // Dropping a file");

function deferred() {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
}

async function runDownload(source, entry) {
  const assets = deferred();
  const context = vm.createContext({
    Blob,
    Uint8Array,
    Promise,
    setTimeout: () => 0,
    folders: ["project", "project/old-folder"],
    session: { tree: () => live },
    figures: { gather: () => assets.promise },
    SHELL_HEADERS: {},
    KEY: "key",
    SLUG: "paper",
    keyHeaders: () => ({}),
    basename: (path) => path.split("/").pop(),
    inside: (path, parent) => path === parent || path.startsWith(`${parent}/`),
    say: (message) => { throw new Error(message); },
    URL: { createObjectURL: () => "blob:test", revokeObjectURL: () => {} },
    document: { createElement: () => ({ click() {} }) },
    captured: null,
    entry,
    viewing: checkpoint,
    checkpointTree: (point) => ({ main: point.main, texts: point.texts, digests: {} }),
  });
  const executable = source.replaceAll(
    'const { zip } = await import("../lib/zip.js");',
    'const zip = (files) => { captured = files; return new Blob(); };',
  );
  assert(!executable.includes('await import("../lib/zip.js")'));
  vm.runInContext(`${liveTreeNow}\n${treeNow}\n${executable}`, context);
  const pending = vm.runInContext(`${entry ? "downloadEntry(entry)" : "downloadTree()"}`, context, {
    filename: "Reader.svelte",
  });
  // The directory snapshot must precede the asynchronous asset fetch.
  context.folders = ["project", "project/new-folder"];
  assets.resolve({ assets: { "project/fig.png": Uint8Array.of(7) } });
  await pending;
  return context.captured;
}

const wholeProject = await runDownload(downloadTree, null);
assert.equal(wholeProject["project/current.md"], "live");
assert.equal(wholeProject["project/new.md"], "new live file");
assert.deepEqual([...wholeProject["project/fig.png"]], [7]);
assert(wholeProject["project/"] instanceof Uint8Array);
assert(wholeProject["project/old-folder/"] instanceof Uint8Array);
assert(!wholeProject["project/new-folder/"]);

const selectedProject = await runDownload(downloadEntry, { kind: "folder", path: "project" });
assert.equal(selectedProject["project/current.md"], "live");
assert.equal(selectedProject["project/new.md"], "new live file");
assert.deepEqual([...selectedProject["project/fig.png"]], [7]);
assert(selectedProject["project/"] instanceof Uint8Array);
assert(selectedProject["project/old-folder/"] instanceof Uint8Array);

console.log("downloads: file exports stay live while history rendering stays historical");
