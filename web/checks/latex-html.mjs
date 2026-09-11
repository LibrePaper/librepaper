import assert from "node:assert/strict";
import { createHtmlCompiler } from "../src/lib/latex/html.js";

const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
const manifest = {
  format: 1, default_release: "r1",
  releases: Object.fromEntries(["r1", "r2"].map((id) => [id, {
    id, base: `engines/${id}/`, engines: { latexml: { worker: "latexml.worker.js" } },
    bundles: { index: "bundles/bundles.json", sha256: "a".repeat(64) },
  }])),
};
function harness(options = {}) {
  const instances = [], reads = [], verified = [];
  const compiler = createHtmlCompiler({
    deadline: options.deadline ?? 1000,
    request: async (url) => { reads.push(String(url)); return { ok: true, json: async () => options.manifest ?? manifest }; },
    verified: async (release, url, check) => {
      verified.push({ release, url, check });
      return { arrayBuffer: async () => new TextEncoder().encode("{}").buffer };
    },
    makeEngine: (config) => {
      const engine = {
        config, files: {}, runs: [], dead: false,
        async init() { await options.init?.(engine); },
        async loadBundleIndex(bytes) { engine.index = bytes; return { result: "ok" }; },
        async flushCache() { engine.files = {}; },
        async mkdir() {},
        async writeFile(path, content) { engine.files[path] = content; },
        setMainFile(path) { engine.main = path; },
        async run(cmd) {
          const files = { ...engine.files };
          engine.runs.push({ cmd, files });
          return options.run ? options.run(engine, files) : { ok: true, html: `<p>${files[engine.main]}</p>`, diagnostics: [], log: "" };
        },
        terminate() { engine.dead = true; },
      };
      instances.push(engine);
      return engine;
    },
  });
  return { ...compiler, instances, reads, verified };
}
const tree = (source, assets = {}) => ({ main: "paper/main.tex", texts: { "paper/main.tex": source }, assets });
const base = "https://example.org/latex/";

const warm = harness();
assert.equal((await warm.compile(tree("first", { "fig.png": Uint8Array.of(1) }), { base })).html, "<p>first</p>");
assert.equal((await warm.compile(tree("second"), { base })).html, "<p>second</p>");
assert.equal(warm.instances.length, 1, "warm edits reuse the engine");
assert.equal(warm.reads.length, 1);
assert.equal(warm.verified.length, 1);
assert.equal(warm.verified[0].check.sha256, "a".repeat(64));
assert.equal(warm.instances[0].config.texliveUrl, "https://example.org/latex/bundles/");
assert.equal(warm.instances[0].runs[1].files["fig.png"], undefined, "deleted files do not leak into a later edit");
await warm.compile(tree("new release"), { base, settings: { release: "r2" } });
assert.equal(warm.instances.length, 2);
assert.equal(warm.instances[0].dead, true);
warm.cancel();

const legacyManifest = structuredClone(manifest);
legacyManifest.releases.legacy = { engines: { pdftex: {} } };
const legacy = harness({ manifest: legacyManifest });
const pinnedSettings = Object.freeze({ release: "legacy", engine: "pdflatex" });
assert.equal((await legacy.compile(tree("existing document"), { base, settings: pinnedSettings })).ok, true);
assert.equal(legacy.instances[0].config.url, "https://example.org/latex/engines/r1/latexml.worker.js");
assert.equal(pinnedSettings.release, "legacy", "HTML preview preserves the PDF release pin");
legacy.cancel();
const unsupported = harness({ manifest: { format: 1, default_release: "legacy", releases: { legacy: legacyManifest.releases.legacy } } });
await assert.rejects(unsupported.compile(tree("no renderer"), { base, settings: pinnedSettings }), /does not include the HTML preview renderer/);
assert.equal(unsupported.instances.length, 0);
unsupported.cancel();

const started = deferred(), finish = deferred();
const queue = harness({ run: async (engine, files) => {
  if (engine.runs.length === 1) { started.resolve(); await finish.promise; }
  return { ok: true, html: files[engine.main], diagnostics: [] };
} });
const first = queue.compile(tree("first"), { base });
await started.promise;
const skipped = queue.compile(tree("skipped"), { base });
const skipCheck = assert.rejects(skipped, { name: "Superseded" });
const newestTree = tree("latest", { "fig.png": Uint8Array.of(7, 8) });
const latest = queue.compile(newestTree, { base });
newestTree.texts["paper/main.tex"] = "mutated";
newestTree.assets["fig.png"][0] = 0;
finish.resolve();
await first;
await skipCheck;
assert.equal((await latest).html, "latest");
assert.equal(queue.instances[0].runs.length, 2);
assert.deepEqual(queue.instances[0].runs[1].files["fig.png"], Uint8Array.of(7, 8));
queue.cancel();

const initializing = deferred(), initialized = deferred();
let starts = 0;
const cancelled = harness({ init: async () => {
  if (++starts === 1) { initializing.resolve(); await initialized.promise; }
} });
const old = cancelled.compile(tree("old"), { base });
const oldCheck = assert.rejects(old, { name: "Superseded" });
await initializing.promise;
cancelled.cancel();
assert.equal((await cancelled.compile(tree("new"), { base })).html, "<p>new</p>", "cancelling an initializing worker does not block the next preview");
await oldCheck;
initialized.resolve();
await Promise.resolve();
assert.equal(cancelled.instances[0].runs.length, 0);
cancelled.cancel();

const timed = harness({ deadline: 5, run: () => new Promise(() => {}) });
await assert.rejects(timed.compile(tree("loop"), { base }), /timed out/);
assert.equal(timed.instances[0].dead, true);
timed.cancel();

const invalid = harness();
await assert.rejects(invalid.compile({ main: "../escape.tex", texts: { "../escape.tex": "bad" } }, { base }), /Invalid project file path/);
assert.deepEqual(invalid.instances[0].files, {});
assert.equal(invalid.instances[0].dead, true);
invalid.cancel();

const failed = harness({ run: () => ({ ok: false, html: "partial", log: "Undefined macro", diagnostics: [] }) });
const failure = await failed.compile(tree("bad"), { base });
assert.equal(failure.html, null);
assert.equal(failure.diagnostics[0].message, "Undefined macro");
failed.cancel();

let trapRuns = 0;
const trapped = harness({ run: () => ++trapRuns === 1
  ? { ok: false, status: -254, log: "unreachable" }
  : { ok: true, status: 0, html: "recovered" } });
assert.equal((await trapped.compile(tree("trap"), { base })).ok, false);
assert.equal(trapped.instances[0].dead, true, "a runtime trap retires the damaged engine");
assert.equal((await trapped.compile(tree("fixed"), { base })).html, "recovered");
assert.equal(trapped.instances.length, 2);
trapped.cancel();
console.log("latex-html: warm resources, snapshots, queued edits, cancellation, deadlines, paths and failures passed");
