// The compiler queue's ownership boundaries, with a deliberately tiny worker.
// This exercises real latex.js code without downloading a distribution.
import assert from "node:assert/strict";

globalThis.self = { location: { href: "http://localhost/" } };
globalThis.localStorage = {
  getItem() { return null; },
  setItem() {},
};
globalThis.fetch = async () =>
  new Response(JSON.stringify({ distributions: { a: { files: {} }, b: { files: {} } } }), {
    status: 200,
    headers: { "content-type": "application/json" },
  });

const workers = [];
class FakeWorker {
  static delayReady = false;
  static pendingReady = [];
  constructor() {
    this.messages = [];
    this.dead = false;
    workers.push(this);
  }
  postMessage(message) {
    if (this.dead) throw new Error("posted to a terminated worker");
    this.messages.push(message);
    if (message.cmd === "choose") {
      const ready = () => this.onmessage?.({ data: { ready: true } });
      if (FakeWorker.delayReady) FakeWorker.pendingReady.push(ready);
      else queueMicrotask(ready);
    }
  }
  terminate() { this.dead = true; }
  compileResult(log = "ok") {
    this.onmessage?.({ data: { compiled: { pdf: null, synctex: null, log, diagnostics: [] } } });
  }
}
globalThis.Worker = FakeWorker;

const latex = await import("../src/lib/latex.js?latex-worker-check");

// Replacing a worker while it is still waiting for its initialization ready
// message must settle the old choose promise as well as the old compile job.
FakeWorker.delayReady = true;
const initializing = latex.choose("a");
await new Promise((resolve) => setImmediate(resolve));
const initializingWorker = workers.at(-1);
assert.equal(initializingWorker.messages.at(-1).cmd, "choose");
FakeWorker.delayReady = false;
const chosenB = latex.choose("b");
FakeWorker.pendingReady.splice(0).forEach((ready) => ready());
await chosenB;
await assert.rejects(initializing, /distribution changed while loading/);

const first = latex.compile({ main: "main.tex", texts: { "main.tex": "a" } });
await new Promise((resolve) => setImmediate(resolve));
const old = workers.at(-1);
assert.equal(old.messages.at(-1).cmd, "compile");

// Terminating a running distribution settles its promise and leaves the queue
// usable by the replacement distribution.
await latex.choose("a");
await assert.rejects(first, /distribution changed/);
await latex.choose("b");
const second = latex.compile({ main: "main.tex", texts: { "main.tex": "b" } });
await new Promise((resolve) => setImmediate(resolve));
const replacement = workers.at(-1);
replacement.compileResult("replacement");
assert.equal((await second).log, "replacement");

// A runtime worker error rejects the active job and automatically reloads the
// selected distribution, so the next compile cannot hang on a dead worker.
replacement.onerror({ message: "worker exploded while idle" });
await new Promise((resolve) => setImmediate(resolve));
const recoveredFromIdle = workers.at(-1);
assert.notEqual(recoveredFromIdle, replacement);
const failed = latex.compile({ main: "main.tex", texts: { "main.tex": "error" } });
await new Promise((resolve) => setImmediate(resolve));
const broken = workers.at(-1);
broken.onerror({ message: "worker exploded" });
await assert.rejects(failed, /worker exploded/);
await new Promise((resolve) => setImmediate(resolve));
const recovered = workers.at(-1);
assert.notEqual(recovered, broken);
const final = latex.compile({ main: "main.tex", texts: { "main.tex": "valid" } });
await new Promise((resolve) => setImmediate(resolve));
recovered.compileResult("recovered");
assert.equal((await final).log, "recovered");

const interrupted = latex.compile({ main: "main.tex", texts: { "main.tex": "active" } });
await new Promise((resolve) => setImmediate(resolve));
const pending = latex.compile({ main: "main.tex", texts: { "main.tex": "queued" } });
const interruptedCheck = assert.rejects(interrupted, /mirror changed/);
const pendingCheck = assert.rejects(pending, /no LaTeX distribution/);
latex.at("https://replacement.example/");
await Promise.all([interruptedCheck, pendingCheck]);

console.log("latex worker: queue cancellation and worker recovery passed");
