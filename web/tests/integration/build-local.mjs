// The protocol 3 render contract, using the production client and fake HTTP.
import assert from "node:assert/strict";
import * as client from "../../src/lib/desktop/client.js";
import { formOf } from "../helpers/form-body.mjs";

const values = new Map();
const storage = { getItem: (key) => values.get(key) || null, setItem: (key, value) => values.set(key, String(value)), removeItem: (key) => values.delete(key) };
globalThis.window = { __LIBREPAPER_DESKTOP__: { base: "lp-local://localhost/", version: "test" } };
client._testing.reset();
client._testing.inject({ storage, wait: () => Promise.resolve() });
client.configure({ project: "paper", origin: "https://app.test" });

const hex = async (bytes) => [...new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))].map((byte) => byte.toString(16).padStart(2, "0")).join("");
function response(status, body, bytes = null) { return { ok: status >= 200 && status < 300, status, json: async () => body, text: async () => new TextDecoder().decode(bytes || new Uint8Array()), arrayBuffer: async () => (bytes || new Uint8Array()).buffer, clone() { return response(status, body, bytes); } }; }
const descriptor = async (bytes) => ({ size: bytes.byteLength, sha256: await hex(bytes) });
const tree = { main: "chapters/paper.tex", texts: { "chapters/paper.tex": "\\documentclass{article}" }, assets: { "figures/dot.png": new Uint8Array([0xff, 0x00, 0x80]) } };

// One finished job, with the three outputs a render can return.
const artifact = new TextEncoder().encode("%PDF-fake");
const synctex = new Uint8Array([0x1f, 0x8b, 0x08, 0x00]);
const log = new TextEncoder().encode("This is latexmk");
let submitted;
let form;
client._testing.inject({ fetch: async (url, init = {}) => {
  if (url.endsWith("/jobs") && init.method === "POST") { form = await formOf(init); submitted = JSON.parse(await form.get("job").text()); return response(202, { id: "job-1" }); }
  if (url.endsWith("/jobs/job-1")) return response(200, { id: "job-1", status: "done", exit: 0, outputs: { artifact: await descriptor(artifact), synctex: await descriptor(synctex), log: await descriptor(log) } });
  if (url.endsWith("/jobs/job-1/files/artifact")) return response(200, {}, artifact);
  if (url.endsWith("/jobs/job-1/files/synctex")) return response(200, {}, synctex);
  if (url.endsWith("/jobs/job-1/files/log")) return response(200, {}, log);
  throw new Error(`unexpected request ${url}`);
} });
const progress = [];
const sourceTreeDigest = "a".repeat(64);
const result = await client.runRender(
  { job: { snapshot: sourceTreeDigest, generation: 4.9, deadlineSeconds: 60 }, tree, format: "latex", output: "pdf", bindingId: "folder-1" },
  { onProgress: (entry) => progress.push(entry) },
);
assert.equal(result.ok, true);
assert.deepEqual([...result.artifact], [...artifact]);
assert.deepEqual([...result.synctex], [...synctex]);
assert.equal(result.log, "This is latexmk");
assert.deepEqual(result.provenance, { backend: "local", format: "latex", output: "pdf" });
assert.deepEqual(progress.map((entry) => entry.done), [0, 1]);

// The job names what to render and never how: no command, kind or builder.
assert.deepEqual(Object.keys(submitted).sort(), ["binding_id", "entrypoint", "format", "generation", "manifest", "options", "origin", "output", "project", "protocol", "snapshot"]);
assert.equal(submitted.protocol, 3);
assert.equal(submitted.project, "paper");
assert.equal(submitted.origin, "https://app.test");
assert.equal(submitted.snapshot, sourceTreeDigest);
assert.equal(submitted.generation, 4);
assert.equal(submitted.format, "latex");
assert.equal(submitted.output, "pdf");
assert.equal(submitted.entrypoint, "chapters/paper.tex");
assert.equal(submitted.binding_id, "folder-1");
assert.deepEqual(submitted.options, { deadline_seconds: 60 });
assert.deepEqual(submitted.manifest.map((file) => file.path).sort(), ["chapters/paper.tex", "figures/dot.png"]);
assert.ok(submitted.manifest.every((file) => /^[0-9a-f]{64}$/.test(file.sha256) && file.size > 0));
const sent = form.getAll("file");
assert.deepEqual(sent.map((file) => file.name).sort(), ["chapters/paper.tex", "figures/dot.png"]);
assert.deepEqual([...new Uint8Array(await sent.find((file) => file.name === "figures/dot.png").arrayBuffer())], [0xff, 0x00, 0x80]);

// With no folder granted the render goes to the hosted workspace, and the
// deadline defaults.
await client.runRender({ job: { snapshot: "s", generation: 1 }, tree, format: "latex", output: "pdf" });
assert.equal(submitted.binding_id, "hosted");
assert.deepEqual(submitted.options, { deadline_seconds: 300 });

// A failed job still returns its log, and says why it failed.
client._testing.inject({ fetch: async (url, init = {}) => {
  if (url.endsWith("/jobs") && init.method === "POST") return response(202, { id: "bad-1" });
  if (url.endsWith("/jobs/bad-1")) return response(200, { id: "bad-1", status: "failed", exit: 1, error: "typst failed", outputs: { log: await descriptor(log) } });
  if (url.endsWith("/jobs/bad-1/files/log")) return response(200, {}, log);
  throw new Error(`unexpected failed-job request ${url}`);
} });
const failed = await client.runRender({ job: {}, tree, format: "typst", output: "pdf" });
assert.equal(failed.ok, false);
assert.equal(failed.artifact, null);
assert.equal(failed.synctex, null);
assert.equal(failed.error, "typst failed");
assert.equal(failed.log, "This is latexmk");

// A success that returned no result file is not a success.
client._testing.inject({ fetch: async (url, init = {}) => {
  if (url.endsWith("/jobs") && init.method === "POST") return response(202, { id: "empty-1" });
  if (url.endsWith("/jobs/empty-1")) return response(200, { id: "empty-1", status: "done", exit: 0, outputs: {} });
  throw new Error(`unexpected empty-job request ${url}`);
} });
assert.equal((await client.runRender({ job: {}, tree, format: "typst", output: "pdf" })).ok, false);

// An output that does not match the digest the status announced is refused.
client._testing.inject({ fetch: async (url, init = {}) => {
  if (url.endsWith("/jobs") && init.method === "POST") return response(202, { id: "corrupt-1" });
  if (url.endsWith("/jobs/corrupt-1")) return response(200, { id: "corrupt-1", status: "done", exit: 0, outputs: { artifact: await descriptor(artifact) } });
  if (url.endsWith("/jobs/corrupt-1/files/artifact")) return response(200, {}, new TextEncoder().encode("BAD"));
  throw new Error(`unexpected corrupt-output request ${url}`);
} });
await assert.rejects(client.runRender({ job: {}, tree, format: "latex", output: "pdf" }), /wrong (digest|size)/);

// An aborted signal cancels the job, whether the abort lands between polls or
// during one.
let canceled = false;
client._testing.inject({ fetch: async (url, init = {}) => {
  if (url.endsWith("/jobs") && init.method === "POST") return response(202, { id: "job-2" });
  if (url.endsWith("/jobs/job-2")) return response(200, { id: "job-2", status: "running" });
  if (url.endsWith("/jobs/job-2/cancel")) { canceled = true; return response(200, {}); }
  throw new Error(`unexpected cancellation request ${url}`);
} });
const controller = new AbortController();
const pending = client.runRender({ job: {}, tree, format: "latex", output: "pdf" }, { signal: controller.signal });
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
const polling = client.runRender({ job: {}, tree, format: "latex", output: "pdf" }, { signal: duringPoll.signal });
while (!admitted) await new Promise((resolve) => setImmediate(resolve));
duringPoll.abort();
await assert.rejects(polling, (error) => error?.name === "Canceled");
assert.equal(canceled, true, "an admitted job is canceled when its status request is aborted");

// A submission whose response was lost is never retried.
let submissions = 0;
client._testing.inject({ fetch: async (url, init = {}) => {
  if (url.endsWith("/jobs") && init.method === "POST") { submissions += 1; throw Object.assign(new Error("response lost"), { name: "Unreachable" }); }
  throw new Error(`unexpected retry request ${url}`);
} });
await assert.rejects(client.runRender({ job: {}, tree, format: "latex", output: "pdf" }), /could not reach its local service/);
assert.equal(submissions, 1);

// A saved folder the desktop app no longer grants is forgotten, and the render
// runs again in the hosted workspace.
client.setBindingId("revoked-folder");
const bindings = [];
client._testing.inject({ fetch: async (url, init = {}) => {
  if (url.endsWith("/jobs") && init.method === "POST") {
    const job = JSON.parse(await (await formOf(init)).get("job").text());
    bindings.push(job.binding_id);
    return job.binding_id === "hosted" ? response(202, { id: "job-4" }) : response(404, { error: "unknown folder" });
  }
  if (url.endsWith("/jobs/job-4")) return response(200, { id: "job-4", status: "done", exit: 0, outputs: { artifact: await descriptor(artifact) } });
  if (url.endsWith("/jobs/job-4/files/artifact")) return response(200, {}, artifact);
  throw new Error(`unexpected revoked-folder request ${url}`);
} });
const rerun = await client.runRender({ job: {}, tree, format: "latex", output: "pdf", bindingId: client.bindingId() });
assert.equal(rerun.ok, true);
assert.deepEqual(bindings, ["revoked-folder", "hosted"]);
assert.equal(client.bindingId(), "hosted");

// A command is saved or reset by key, and a reset carries no command.
const sentRecords = [];
client._testing.inject({ fetch: async (url, init = {}) => {
  sentRecords.push([init.method, url.replace("lp-local://localhost/librepaper/local/", ""), JSON.parse(init.body)]);
  return response(200, { ok: true });
} });
await client.setRenderer("typst.pdf", { command: ["typst", "compile", "{input}", "{stem}.pdf"], result: "{stem}.pdf" });
await client.resetRenderer("quarto.html");
assert.deepEqual(sentRecords, [
  ["PUT", "renderers/typst.pdf", { command: ["typst", "compile", "{input}", "{stem}.pdf"], result: "{stem}.pdf" }],
  ["PUT", "renderers/quarto.html", { reset: true }],
]);

client._testing.reset();
console.log("build-local: protocol 3 render jobs, outputs, cancellation, folder fallback and renderer records passed");
