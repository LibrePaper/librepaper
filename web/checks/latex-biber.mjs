import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
const enc=new TextEncoder();
const original = { fetch: globalThis.fetch, Worker: globalThis.Worker };
let made=0,terminated=0,payload;
class Worker {
  constructor() { made++; }
  postMessage(data) {
    payload = data;
    queueMicrotask(() => this.onmessage?.({ data: { ok: true, bbl: enc.encode("BBL"), blg: "" } }));
  }
  terminate() { terminated++; }
}
const contents={
  "biber.worker.js": enc.encode("worker"),
  "biber.js": enc.encode("glue"),
  "biber.wasm": new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0]),
  "biber.data": enc.encode("runtime"),
  "biber.build.json": enc.encode('{"schemaVersion":1,"version":"2.22"}'),
};
const release = {
  id: "r",
  digest: "a".repeat(64),
  engines: { biber: { worker: "biber.worker.js", files: Object.keys(contents) } },
  bibliography: { biber: { version: "2.22" } },
  files: Object.fromEntries(Object.entries(contents).map(([name, data]) => [name, {
    url: `engines/r/${name}`,
    size: data.length,
    sha256: createHash("sha256").update(data).digest("hex"),
  }])),
};
const request = {
  stem: "main",
  main: "chapters/main.tex",
  bcf: enc.encode("bcf"),
  files: { "chapters/refs.bib": enc.encode("bib") },
  identity: "i",
};
try{
  globalThis.Worker = Worker;
  globalThis.fetch = async url => new Response(contents[String(url).split("/").at(-1)]);
  const backend = await import("../src/lib/latex/biber.js?success");
  const result = await backend.runBiber(request, { base: "https://mirror.test/", release });
  assert.equal(result.ok, true);
  assert.equal(result.tool.version, "2.22");
  assert.equal(result.tool.backend, "browser");
  assert.equal(payload.stem, "main");
  assert.equal(payload.main, "chapters/main.tex");
  assert.deepEqual(payload.files, request.files);
  assert.ok(payload.wasm instanceof WebAssembly.Module);
  assert.equal(made, 1);
  assert.equal(terminated, 1);

  const incomplete = { ...release, engines: { biber: { worker: "biber.worker.js", files: Object.keys(contents).filter(name => name !== "biber.build.json") } } };
  await assert.rejects(backend.runBiber(request, { base: "https://mirror.test/", release: { ...incomplete, digest: "b".repeat(64) } }), /biber\.build\.json/);

  globalThis.fetch = async () => new Response("corrupt");
  const corrupt = await import("../src/lib/latex/biber.js?corruption");
  await assert.rejects(corrupt.runBiber(request, { base: "https://mirror.test/", release }), /sha256/);
  assert.equal(made, 1, "corrupt runtime must not create a worker");

  globalThis.fetch = async url => new Response(contents[String(url).split("/").at(-1)]);
  const abort = new AbortController();
  abort.abort();
  await assert.rejects(backend.runBiber(request, { base: "https://mirror.test/", release, signal: abort.signal }), e => e.name === "AbortError");
  assert.equal(made, 1, "canceled job must not create a worker");

  let entered;
  const waiting = new Promise(resolve => { entered = resolve; });
  Worker.prototype.postMessage = function() { entered(); };
  const controller = new AbortController();
  const running = backend.runBiber(request, { base: "https://mirror.test/", release, signal: controller.signal });
  await waiting;
  controller.abort();
  await assert.rejects(running, e => e.name === "AbortError");
  assert.equal(terminated, 2, "cancellation must terminate the running worker");
  console.log("Biber backend: verified manifest inventory, nested paths, provenance, corruption rejection and cancellation passed");
} finally {
  Object.assign(globalThis, original);
}
