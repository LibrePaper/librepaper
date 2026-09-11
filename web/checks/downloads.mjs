// The file manager stays live while the document pane may show a checkpoint.
// Keep this small check close to Reader because that distinction is easy to
// lose when changing the history rendering code.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import { availableDownloads, inlineBlobUrls } from "../src/lib/reader/downloads.js";

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

// `downloadTree`/`downloadEntry` call this shared helper, so it has to be
// evaluated alongside them rather than stubbed: the point of the check is the
// order of the directory snapshot against the asset fetch, which happens
// inside it.
const gatherFigures = body("  async function gatherFigures(digests)", "  // Outline reads the same live text");
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
    authHeaders: () => ({}),
    basename: (path) => path.split("/").pop(),
    inside: (path, parent) => path === parent || path.startsWith(`${parent}/`),
    say: (message) => { throw new Error(message); },
    saveBlob: () => {},
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
  vm.runInContext(`${liveTreeNow}\n${treeNow}\n${gatherFigures}\n${executable}`, context);
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

// The File menu offers the download for the document's output kind, and only
// once there is something to hand over.
{
  const paged = (extra) => availableDownloads({ outputKind: "pdf", ...extra });
  assert.deepEqual(paged({}), { pdf: false, html: false }, "a paged document with nothing rendered offers nothing");
  assert.deepEqual(paged({ deliveredKind: "pdf" }), { pdf: true, html: false }, "a PDF the frame was handed can be saved");
  assert.deepEqual(paged({ deliveredKind: "" }), { pdf: false, html: false }, "an unrendered document has no PDF fallback");
  assert.deepEqual(paged({ deliveredKind: "html" }), { pdf: false, html: false }, "a paged document never offers HTML");

  const flow = (extra) => availableDownloads({ outputKind: "html", ...extra });
  assert.deepEqual(flow({}), { pdf: false, html: false }, "a page not yet painted offers nothing");
  assert.deepEqual(flow({ deliveredKind: "html" }), { pdf: false, html: true }, "a painted page can be saved");
  assert.deepEqual(flow({ displayedFormat: "html" }), { pdf: false, html: true }, "an authored HTML document is its own rendering");
  assert.deepEqual(flow({ deliveredKind: "" }), { pdf: false, html: false }, "a flow document has no generated-output fallback");
  assert.deepEqual(availableDownloads({ outputKind: "" }), { pdf: false, html: false });
}

// A painted page names its figures by object URL; the download carries the
// bytes instead, and leaves alone what it cannot fetch.
{
  const fetched = [];
  const fetcher = async (url) => {
    fetched.push(url);
    if (url.endsWith("gone")) return { ok: false };
    return {
      ok: true,
      headers: { get: () => "image/png" },
      arrayBuffer: async () => Uint8Array.of(1, 2, 3).buffer,
    };
  };
  const page = '<img src="blob:https://x/one#librepaper-asset=a"><img src="blob:https://x/one#librepaper-asset=a"><img src="blob:https://x/gone">';
  const inlined = await inlineBlobUrls(page, fetcher);
  assert.deepEqual(fetched, ["blob:https://x/one", "blob:https://x/gone"], "each object URL is fetched once, without its fragment");
  assert.equal(inlined, '<img src="data:image/png;base64,AQID"><img src="data:image/png;base64,AQID"><img src="blob:https://x/gone">');
  assert.equal(await inlineBlobUrls("<p>no figures</p>", fetcher), "<p>no figures</p>");
}

console.log("downloads: file exports follow transient output handed to the frame");
