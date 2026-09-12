// Format routing for protocol-v2 one-shot builders. These checks prove that
// explicit native Typst and Pandoc selections reach the companion directly
// and that a local failure is returned instead of starting a browser render.
import assert from "node:assert/strict";
import * as client from "../../src/lib/companion/client.js";
import * as renderers from "../../src/lib/renderers.js";

globalThis.location = { origin: "https://app.test" };

const values = new Map();
const storage = {
  getItem: (key) => values.get(key) || null,
  setItem: (key, value) => values.set(key, String(value)),
  removeItem: (key) => values.delete(key),
};
values.set("librepaper-local-pairings", JSON.stringify({
  "https://app.test|paper": { token: "token", instance: "one" },
}));

function response(status, body, bytes = null) {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
    text: async () => new TextDecoder().decode(bytes || new Uint8Array()),
    arrayBuffer: async () => (bytes || new Uint8Array()).buffer,
    clone() { return response(status, body, bytes); },
  };
}
async function digest(bytes) {
  return [...new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))]
    .map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

client._testing.reset();
client._testing.inject({ storage, wait: () => Promise.resolve() });
client.configure({ project: "paper", origin: "https://app.test", active: true });

let submitted = null;
let artifact = new TextEncoder().encode("%PDF-typst");
client._testing.inject({ fetch: async (url, init = {}) => {
  if (url.endsWith("/health")) return response(200, { service: "librepaper-local", protocol: [2], version: "2", instance: "one" });
  if (url.endsWith("/capabilities")) return response(200, { builders: [] });
  if (url.endsWith("/jobs") && init.method === "POST") {
    submitted = JSON.parse(await init.body.get("job").text());
    return response(202, { id: "build-1" });
  }
  if (url.endsWith("/jobs/build-1")) return response(200, { status: "done", exit: 0, outputs: { [submitted.output]: { size: artifact.byteLength, sha256: await digest(artifact) } }, provenance: { builder: submitted.builder } });
  if (url.endsWith(`/jobs/build-1/files/${submitted.output}`)) return response(200, {}, artifact);
  throw new Error(`unexpected request ${url}`);
} });
await client.probe({ force: true });

const typst = await renderers.render(
  { main: "main.typ", texts: { "main.typ": "= Test" }, assets: {} },
  "Test",
  { project: "paper", buildPreferences: { selection: "tool", backend: "local", tool: "typst", output: "pdf" } },
);
assert.equal(typst.ok, true);
assert.equal(submitted.builder, "typst");
assert.deepEqual(submitted.workspace, { mode: "snapshot" });
assert.deepEqual([...typst.pdf], [...artifact]);

artifact = new TextEncoder().encode("<!doctype html><h1>Test</h1>");
const markdown = await renderers.render(
  { main: "main.md", texts: { "main.md": "# Test" }, assets: {} },
  "Test",
  { project: "paper", buildPreferences: { selection: "tool", backend: "local", tool: "pandoc", output: "html" } },
);
assert.equal(markdown.ok, true);
assert.equal(submitted.builder, "pandoc");
assert.deepEqual(submitted.workspace, { mode: "snapshot" });
assert.match(markdown.html, /<h1>Test<\/h1>/);

artifact = new Uint8Array([0x50, 0x4b, 0x03, 0x04]);
const docx = await renderers.render(
  { main: "main.md", texts: { "main.md": "# Test" }, assets: {} },
  "Test",
  { project: "paper", buildPreferences: { selection: "tool", backend: "local", tool: "pandoc", output: "docx" } },
);
assert.equal(docx.ok, true);
assert.equal(docx.artifactKind, "docx");
assert.deepEqual(docx.artifact, artifact);
assert.equal(docx.html, null);
assert.equal(docx.pdf, null);

client._testing.inject({ fetch: async (url, init = {}) => {
  if (url.endsWith("/jobs") && init.method === "POST") return response(202, { id: "failed-1" });
  if (url.endsWith("/jobs/failed-1")) return response(200, { status: "failed", exit: 1, error: "pandoc failed", outputs: {} });
  throw new Error(`unexpected failed-build request ${url}`);
} });
const failed = await renderers.render(
  { main: "main.md", texts: { "main.md": "# Broken" }, assets: {} },
  "Broken",
  { project: "paper", buildPreferences: { selection: "tool", backend: "local", tool: "pandoc", output: "html" } },
);
assert.equal(failed.ok, false);
assert.equal(failed.failure.message, "pandoc failed");
assert.equal(failed.html, null);

client._testing.reset();
