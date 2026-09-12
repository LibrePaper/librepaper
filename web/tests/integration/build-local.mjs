// v2 companion build contract using the production client and fake HTTP.
import assert from "node:assert/strict";
import * as client from "../../src/lib/companion/client.js";

const values = new Map();
const storage = { getItem: (key) => values.get(key) || null, setItem: (key, value) => values.set(key, String(value)), removeItem: (key) => values.delete(key) };
const scope = "https://app.test|paper";
values.set("librepaper-local-pairings", JSON.stringify({ [scope]: { token: "token", instance: "one" } }));
client._testing.reset();
client._testing.inject({ storage, wait: () => Promise.resolve() });
client.configure({ project: "paper", origin: "https://app.test" });
let submitted;
const artifact = new TextEncoder().encode("PDF");
const artifactDigest = [...new Uint8Array(await crypto.subtle.digest("SHA-256", artifact))].map((byte) => byte.toString(16).padStart(2, "0")).join("");
function response(status, body, bytes = null) { return { ok: status >= 200 && status < 300, status, json: async () => body, text: async () => new TextDecoder().decode(bytes || new Uint8Array()), arrayBuffer: async () => (bytes || new Uint8Array()).buffer, clone() { return response(status, body, bytes); } }; }
client._testing.inject({ fetch: async (url, init = {}) => {
  if (url.endsWith("/health")) return response(200, { service: "librepaper-local", protocol: [2], version: "2", instance: "one" });
  if (url.endsWith("/capabilities")) return response(200, { builders: [{ id: "latexmk", available: true, operations: [{ kind: "build", workspace_modes: ["snapshot"] }] }] });
  if (url.endsWith("/jobs") && init.method === "POST") { submitted = JSON.parse(await init.body.get("job").text()); return response(202, { id: "job-1" }); }
  if (url.endsWith("/jobs/job-1")) return response(200, { id: "job-1", status: "done", exit: 0, outputs: { pdf: { size: artifact.byteLength, sha256: artifactDigest } }, provenance: { builder: "latexmk" } });
  if (url.endsWith("/jobs/job-1/files/pdf")) return response(200, {}, artifact);
  if (url.endsWith("/jobs/job-1/files/log")) return response(200, {}, new Uint8Array());
  throw new Error(`unexpected request ${url}`);
} });
await client.probe({ force: true });
const result = await client.runBuild({ job: { snapshot: "source", generation: 4 }, tree: { main: "paper.tex", texts: { "paper.tex": "\\documentclass{article}" }, assets: {} }, builder: "latexmk", engine: "xelatex", output: "pdf", options: { shell_escape: false } });
assert.equal(result.ok, true);
assert.equal(submitted.protocol, 2);
assert.equal(submitted.builder, "latexmk");
assert.deepEqual(submitted.workspace, { mode: "snapshot" });
assert.equal(submitted.entrypoint, "paper.tex");
assert.equal(submitted.options.engine, "xelatex");
assert.deepEqual([...result.artifact], [...artifact]);

client._testing.inject({ fetch: async (url, init = {}) => {
  if (url.endsWith("/jobs") && init.method === "POST") return response(202, { id: "bad-1" });
  if (url.endsWith("/jobs/bad-1")) return response(200, { id: "bad-1", status: "done", exit: 0, outputs: { pdf: { size: artifact.byteLength, sha256: artifactDigest } } });
  if (url.endsWith("/jobs/bad-1/files/pdf")) return response(200, {}, new TextEncoder().encode("BAD"));
  throw new Error(`unexpected corrupt-output request ${url}`);
} });
await assert.rejects(
  client.runBuild({ job: { snapshot: "source", generation: 5 }, tree: { main: "paper.tex", texts: { "paper.tex": "x" }, assets: {} }, builder: "latexmk" }),
  /wrong digest/,
);

const html = new TextEncoder().encode("<h1>ok</h1>");
const digest = [...new Uint8Array(await crypto.subtle.digest("SHA-256", html))].map((byte) => byte.toString(16).padStart(2, "0")).join("");
const bundle = { artifact: { entrypoint: "index.html", kind: "html", sha256: digest, size: html.byteLength, mime: "text/html" }, assets: [] };
client._testing.inject({ fetch: async (url, init = {}) => {
  if (url.endsWith("/jobs") && init.method === "POST") {
    const request = JSON.parse(await init.body.get("job").text());
    assert.equal(request.kind, "build"); assert.equal(request.protocol, 2); assert.equal(request.builder, "quarto");
    assert.equal(request.workspace.mode, "snapshot"); assert.equal(request.entrypoint, "index.qmd");
    assert.equal(request.quarto, undefined);
    return response(202, { id: "q-1" });
  }
  if (url.endsWith("/jobs/q-1")) return response(200, { id: "q-1", status: "done", exit: 0, outputs: { "manifest.json": { size: JSON.stringify(bundle).length }, "artifact.html": { size: html.byteLength } } });
  if (url.endsWith("/jobs/q-1/files/manifest.json")) return response(200, {}, new TextEncoder().encode(JSON.stringify(bundle)));
  if (url.endsWith("/jobs/q-1/files/artifact.html")) return response(200, {}, html);
  throw new Error(`unexpected Quarto request ${url}`);
} });
const quarto = await client.runBuild({ job: { snapshot: "source", generation: 6, binding: "binding" }, tree: { main: "index.qmd", texts: { "index.qmd": "# hello" }, assets: {} }, builder: "quarto", output: "html" });
assert.equal(quarto.ok, true);
assert.equal(quarto.kind, "html");
assert.deepEqual([...quarto.artifact], [...html]);

let previewRequest;
client._testing.inject({ fetch: async (url, init = {}) => {
  if (url.endsWith("/previews")) { previewRequest = JSON.parse(init.body); return response(201, { id: "preview-1" }); }
  throw new Error(`unexpected preview request ${url}`);
} });
await client.startLocalPreview({ engine: "quarto", job: { binding: "binding" }, tree: { main: "index.qmd", texts: { "index.qmd": "# hello" }, assets: {} }, options: { format: "html" } });
assert.equal(previewRequest.protocol, 2);
assert.equal(previewRequest.kind, "preview");
assert.equal(previewRequest.builder, "quarto");
assert.deepEqual(previewRequest.workspace, { mode: "bound", binding_id: "binding" });
assert.equal(previewRequest.entrypoint, "index.qmd");
assert.equal("quarto" in previewRequest, false);
assert.equal("profile" in previewRequest.options, false);
await client.startLocalPreview({ engine: "calepin", job: { binding: "binding" }, tree: { main: "index.typ", texts: { "index.typ": "= hello" }, assets: {} }, options: { entrypoint: "index.typ", format: "pdf" } });
assert.equal(previewRequest.builder, "calepin");
assert.deepEqual(previewRequest.workspace, { mode: "bound", binding_id: "binding" });
assert.deepEqual(previewRequest.options, {});

let canceled = false;
client._testing.inject({ fetch: async (url, init = {}) => {
  if (url.endsWith("/jobs") && init.method === "POST") return response(202, { id: "job-2" });
  if (url.endsWith("/jobs/job-2")) return response(200, { id: "job-2", status: "running" });
  if (url.endsWith("/jobs/job-2/cancel")) { canceled = true; return response(200, {}); }
  throw new Error(`unexpected cancellation request ${url}`);
} });
const controller = new AbortController();
const pending = client.runBuild({ job: { snapshot: "source", generation: 5 }, tree: { main: "paper.tex", texts: { "paper.tex": "x" }, assets: {} }, builder: "latexmk" }, { signal: controller.signal });
controller.abort();
await assert.rejects(pending, (error) => error?.name === "Canceled");
assert.equal(canceled, true);

let admitted = false;
canceled = false;
client._testing.inject({ fetch: async (url, init = {}) => {
  if (url.endsWith("/jobs") && init.method === "POST") return response(202, { id: "job-3" });
  if (url.endsWith("/jobs/job-3") && init.method === "GET") {
    admitted = true;
    return new Promise((_resolve, reject) => init.signal.addEventListener("abort", () => reject(new DOMException("canceled", "AbortError")), { once: true }));
  }
  if (url.endsWith("/jobs/job-3/cancel")) { canceled = true; return response(200, {}); }
  throw new Error(`unexpected in-flight cancellation request ${url}`);
} });
const duringPoll = new AbortController();
const polling = client.runBuild({ job: { snapshot: "source", generation: 6 }, tree: { main: "paper.tex", texts: { "paper.tex": "x" }, assets: {} }, builder: "latexmk" }, { signal: duringPoll.signal });
while (!admitted) await new Promise((resolve) => setImmediate(resolve));
duringPoll.abort();
await assert.rejects(polling, (error) => error?.name === "Canceled");
assert.equal(canceled, true, "an admitted job is canceled when its status request is aborted");

let submissions = 0;
client._testing.inject({ fetch: async (url, init = {}) => {
  if (url.endsWith("/jobs") && init.method === "POST") { submissions += 1; throw Object.assign(new Error("response lost"), { name: "Unreachable" }); }
  throw new Error(`unexpected retry request ${url}`);
} });
await assert.rejects(client.runBuild({ job: { snapshot: "source", generation: 7 }, tree: { main: "paper.tex", texts: { "paper.tex": "x" }, assets: {} }, builder: "latexmk" }), /response lost/);
assert.equal(submissions, 1, "v2 requests without an idempotency key are never retried");
client._testing.reset();
