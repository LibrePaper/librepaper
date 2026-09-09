// Exercise the public renderer boundary, including structured cloning and
// worker failure recovery, without downloading a compiler.
import assert from "node:assert/strict";
const workers = [];
class Worker {
  constructor(url) { this.url = url; this.messages = []; workers.push(this); }
  postMessage(message) { this.messages.push(structuredClone(message)); }
  terminate() { this.dead = true; }
  reply(message, result, error) { this.onmessage({ data: { id: message.id, result, error } }); }
}
globalThis.Worker = Worker;
globalThis.location = { href: "https://example.org/docs/paper" };
globalThis.LIBREPAPER_MODULES = { typst: "/typst.wasm", markdown: "/markdown.wasm" };
const renderers = await import("../src/lib/renderers.js");
const text = new Proxy({ "main.typ": "Hello" }, {});
const tree = { main: "main.typ", texts: text, assets: { "figure.png": Uint8Array.of(1, 2) } };
const rendering = renderers.render(tree, "Title");
const worker = workers[0];
assert.ok(worker.url.pathname.endsWith("renderer-worker.js"));
const message = worker.messages[0];
assert.equal(message.url, "https://example.org/typst.wasm");
assert.equal(message.args.tree.texts["main.typ"], "Hello");
assert.deepEqual(message.args.tree.assets["figure.png"], Uint8Array.of(1, 2));
assert.equal(tree.assets["figure.png"].byteLength, 2);
const title = renderers.titleOf(tree);
worker.reply(worker.messages[1], "Heading");
assert.equal(await title, "Heading");
worker.reply(message, { pdf: Uint8Array.of(37, 80, 68, 70), diagnostics: [] });
const typstRendering = await rendering;
assert.equal(typstRendering.html, null);
assert.deepEqual(typstRendering.pdf, Uint8Array.of(37, 80, 68, 70));
const failed = renderers.render(tree, "Title");
worker.reply(worker.messages.at(-1), null, "Compiler unavailable");
await assert.rejects(failed, /Compiler unavailable/);
const pending = renderers.render(tree, "Title");
const pendingTitle = renderers.titleOf(tree);
worker.onerror({ message: "Worker crashed" });
await assert.rejects(pending, /Worker crashed/);
await assert.rejects(pendingTitle, /Worker crashed/);
assert.equal(worker.dead, true);
const retry = renderers.render(tree, "Title");
assert.equal(workers.length, 2);
workers[1].reply(workers[1].messages[0], { pdf: Uint8Array.of(1), diagnostics: [] });
const recovered = await retry;
assert.equal(recovered.html, null);
assert.deepEqual(recovered.pdf, Uint8Array.of(1));
const html = await renderers.render({ main: "main.html", texts: { "main.html": "<p>HTML</p>" } }, "Title");
assert.equal(html.html, "<p>HTML</p>");
assert.equal(workers.length, 2);
console.log("renderer-worker: cloning, reply routing, errors, recovery and HTML passed");

globalThis.LIBREPAPER_MODULES.bibliography = "/bibliography.wasm";
globalThis.LIBREPAPER_MODULES.citations = "/citations.wasm";
const { analyzeBibliography } = await import("../src/lib/bibliography-engine.js");
const analyzing = analyzeBibliography({ main: "paper.md", format: "markdown", source: "@doe", texts: { "refs.bib": "" } });
const bibliographyWorker = workers.at(-1);
assert.equal(bibliographyWorker.messages[0].operation, "bibliography");
assert.equal(bibliographyWorker.messages[0].url, "https://example.org/bibliography.wasm");
bibliographyWorker.reply(bibliographyWorker.messages[0], { entries: [{ key: "doe" }], diagnostics: [] });
assert.equal((await analyzing).entries[0].key, "doe");
const citing = renderers.render({ main: "paper.md", texts: { "paper.md": "See [@doe]." } }, "Paper");
const citationsWorker = workers.at(-1);
assert.equal(citationsWorker.messages[0].url, "https://example.org/citations.wasm");
citationsWorker.reply(citationsWorker.messages[0], { html: "<p>Doe</p>", diagnostics: [] });
assert.equal((await citing).html, "<p>Doe</p>");
console.log("bibliography worker analysis and citation routing passed");
