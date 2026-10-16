// The driver against a faked Worker: bundle-capable engines load a stored
// working set after the bundle index and export a changed one after a compile.
import assert from "node:assert/strict";

const stores = new Map();
globalThis.caches = {
  async open(name) {
    const map = stores.get(name) ?? new Map();
    stores.set(name, map);
    return {
      async match(key) { return map.get(key)?.clone(); },
      async put(key, response) { map.set(key, response); },
      async delete(key) { return map.delete(key); },
      async keys() { return [...map.keys()]; },
    };
  },
  async keys() { return [...stores.keys()]; },
  async delete(name) { return stores.delete(name); },
};

const workers = [];
class FakeWorker {
  constructor() {
    this.sent = [];
    this.changed = false;
    workers.push(this);
  }
  postMessage(message, transfer) {
    this.sent.push({ message, transfer });
    if (message.librepaperEngineHost) return void queueMicrotask(() => this.onmessage({ data: { result: "ok" } }));
    const reply = (extra) => queueMicrotask(() => this.onmessage({ data: { cmd: message.cmd, ...extra } }));
    switch (message.cmd) {
      case "loadbundleindex": return reply({ result: "ok" });
      case "loadworkingset": return reply({ result: "ok", files: 2 });
      // Every compile command answers tagged `compile`, as the controllers do.
      case "compilelatex":
        return reply({ cmd: "compile", result: "ok", status: 0, log: "done", phaseTimings: { total: 5 } });
      case "workingset":
        return reply(this.changed
          ? { result: "ok", changed: true, resolutions: [[26, "a.sty", "a.sty"]], files: Uint8Array.of(9, 8).buffer, stats: { memoHits: 1 } }
          : { result: "ok", changed: false, stats: { memoHits: 0 } });
      default: return reply({ result: "ok" });
    }
  }
  terminate() {}
}
globalThis.Worker = FakeWorker;
const { createEngine } = await import("../../src/lib/latex/driver.js");
const { writeWorkingSet, readWorkingSet } = await import("../../src/lib/latex/resources.js");

const release = { digest: "c".repeat(64), id: "test" };
const make = (kind) => createEngine({ kind, url: "https://example.org/e.js", texliveUrl: "https://example.org/t/", release });
const cmds = (worker) => worker.sent.map(({ message }) => message.cmd).filter(Boolean);

// No stored working set: only the index is loaded; the compile exports nothing stored.
const cold = make("pdftex");
await cold.init();
assert.ok(cold.timings.assetsMs >= 0 && cold.timings.startMs >= 0 && cold.timings.formatMs >= 0);
assert.equal((await cold.loadBundleIndex(Uint8Array.of(1, 2))).result, "ok");
assert.equal(cmds(workers[0]).includes("loadworkingset"), false);
assert.equal(cold.timings.workingSetFiles, 0);
assert.ok(cold.timings.indexMs >= 0 && cold.timings.workingSetMs >= 0);
const unchanged = await cold.run("compilelatex");
assert.equal(unchanged.ok, true);
assert.deepEqual(unchanged.phaseTimings, { total: 5 });
assert.deepEqual(unchanged.bundleStats, { memoHits: 0 });
assert.equal(await readWorkingSet(release, "pdftex"), null, "an unchanged working set is not written");

// A changed working set is stored without the caller waiting on it.
workers[0].changed = true;
const changed = await cold.run("compilelatex");
assert.deepEqual(changed.bundleStats, { memoHits: 1 });
let stored = null;
for (let tries = 0; !stored && tries < 100; tries++) {
  await new Promise((resolve) => setTimeout(resolve, 20));
  stored = await readWorkingSet(release, "pdftex");
}
assert.deepEqual(stored.resolutions, [[26, "a.sty", "a.sty"]]);
assert.deepEqual([...stored.files], [9, 8]);

// A warm start sends the stored working set, transferring its bytes.
const warm = make("pdftex");
await warm.init();
await warm.loadBundleIndex(Uint8Array.of(1, 2));
const sent = workers[1].sent.find(({ message }) => message.cmd === "loadworkingset");
assert.deepEqual(sent.message.resolutions, [[26, "a.sty", "a.sty"]]);
assert.deepEqual([...new Uint8Array(sent.message.files)], [9, 8]);
assert.equal(sent.transfer[0], sent.message.files);
assert.equal(warm.timings.workingSetFiles, 2);
assert.deepEqual(cmds(workers[1]).filter((cmd) => cmd.startsWith("load")), ["loadbundleindex", "loadworkingset"]);

// An engine without bundle mode neither loads nor exports a working set.
await writeWorkingSet(release, "luatex", { resolutions: [], files: new Uint8Array(0) });
const lua = make("luatex");
await lua.init();
assert.equal(await lua.loadBundleIndex(Uint8Array.of(1)), null);
const luaRun = await lua.run("compilelatex");
assert.equal(luaRun.bundleStats, null);
assert.equal(cmds(workers[2]).includes("workingset"), false);
assert.equal(cmds(workers[2]).includes("loadworkingset"), false);

console.log("latex-driver: working sets load after the index and export after a compile");
