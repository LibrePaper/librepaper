import assert from "node:assert/strict";
import { _testing, configure, quartoRequest, runQuarto, startQuartoPreview, stopQuartoPreview, quartoPreviewStatus } from "../src/lib/latex/local.js";
import { parameterSha256 } from "../src/lib/quarto.js";

const digest = "a".repeat(64);

assert.equal(quartoRequest({
  job: { binding: "binding-1", id: "job-1" },
  entrypoint: "paper.qmd",
  format: "docx",
  inputDigest: digest,
  files: [{ path: "paper.qmd", sha256: digest, size: 1 }],
}).quarto.shared_tree_sha256, digest);
assert.deepEqual(quartoRequest({
  job: { binding: "binding-1" },
  entrypoint: "paper.qmd",
  parameters: { number: 1, text: "1", boolean: true, nothing: null },
}).quarto.parameters, { number: 1, text: "1", boolean: true, nothing: null });
assert.notEqual(await parameterSha256({ value: 1 }), await parameterSha256({ value: "1" }));
assert.equal(await parameterSha256({ value: 1 }), await parameterSha256({ value: 1.0 }));
const specialParameters = JSON.parse('{"__proto__":"literal","constructor":true}');
assert.deepEqual(quartoRequest({ job:{ binding:"binding-1" }, entrypoint:"paper.qmd", parameters:specialParameters }).quarto.parameters, specialParameters);
assert.throws(() => quartoRequest({ job:{ binding:"binding-1" }, entrypoint:"paper.qmd", parameters:{ large:"é".repeat(8193) } }), /too long/);
assert.equal(
  await parameterSha256({ A: 1, a: 1e-3, _: true, Z: "é😀", number: 2.5e4 }),
  "aeea5bcdd2f603f88bb8d6387ce656ebe5b6a3560947363a369b736071bb8b3d",
);
assert.throws(() => quartoRequest({ job: { binding: "binding-1" }, entrypoint: "paper.qmd", parameters: { bad: -0 } }));
assert.throws(() => quartoRequest({ job: { binding: "binding-1" }, entrypoint: "paper.qmd", parameters: { bad: Number.MAX_SAFE_INTEGER + 1 } }));
assert.throws(() => quartoRequest({ job: { binding: "binding-1" }, entrypoint: "./paper.qmd" }));
assert.throws(() => quartoRequest({ job: { binding: "binding-1" }, entrypoint: "paper.qmd", format: "HTML" }));
assert.throws(() => quartoRequest({ job: { binding: "binding-1" }, entrypoint: "paper.qmd", files: [{ path: "data/a", sha256: digest, size: 1 }, { path: "data/a", sha256: digest, size: 1 }] }));

function response(body, status = 200) {
  const bytes = body instanceof Uint8Array ? body : new TextEncoder().encode(JSON.stringify(body));
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => JSON.parse(new TextDecoder().decode(bytes)),
    arrayBuffer: async () => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength),
    clone() { return this; },
  };
}

async function sha(bytes) {
  return [...new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))]
    .map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

function setup(fetch) {
  const values = new Map([["librepaper-local-pairings", JSON.stringify({ "example.test|paper": { token: "token" } })]]);
  _testing.reset();
  configure({ origin: "example.test", project: "paper" });
  _testing.inject({ storage: { getItem: (key) => values.get(key) || null, setItem: (key, value) => values.set(key, value) }, fetch, wait: async () => {} });
}

const artifact = new TextEncoder().encode("<html><body>ok</body></html>");
const artifactSha = await sha(artifact);
const manifest = new TextEncoder().encode(JSON.stringify({
  schema: "librepaper-quarto-bundle/v1",
  render_id: "render-1",
  document_id: "paper",
  source: { revision: "rev-1", main: "paper.qmd", verification: "working-tree-verified" },
  context: { id: "ctx", fingerprint_version: 1, computation_sha256: digest, format: "html" },
  provenance: { kind: "managed-local-render" },
  artifact: { kind: "html", entrypoint: "index.html", sha256: artifactSha, size: artifact.byteLength, mime: "text/html" },
  cells: [], assets: [], coverage: { full_artifact: true, cell_outputs: "none" },
}));
const manifestSha = await sha(manifest);
const posts = [];
let firstPost = true;
setup(async (url, init = {}) => {
  if (init.method === "POST" && url.endsWith("/jobs")) {
    posts.push(await init.body.get("job").text());
    if (firstPost) { firstPost = false; throw new TypeError("connection lost after admission"); }
    return response({ id: "qjob-1", status: "queued" }, 202);
  }
  if (init.method === "GET" && url.endsWith("/jobs/qjob-1")) {
    return response({ id: "qjob-1", kind: "quarto", status: "done", stage: "finished", exit: 0, log_tail: "rendered", outputs: {
      "quarto-bundle.json": { size: manifest.byteLength, sha256: manifestSha },
      "artifact.html": { size: artifact.byteLength, sha256: artifactSha },
    } });
  }
  if (url.endsWith("/files/quarto-bundle.json")) return response(manifest);
  if (url.endsWith("/files/artifact.html")) return response(artifact);
  throw new Error(`unexpected local request: ${init.method || "GET"} ${url}`);
});
const first = await runQuarto({
  job: { id: "stable-job", binding: "binding-1" },
  tree: { main: "paper.qmd", texts: { "paper.qmd": "# Paper" }, assets: {} },
  options: { inputRevision: "rev-1", inputDigest: digest, executionMode:"isolated-snapshot", dataInputs:["data/local.csv"], renderScope:"project" },
});
assert.equal(first.ok, true);
assert.equal(first.kind, "html");
assert.equal(first.publish.blobs.length, 1);
assert.equal(posts.length, 2, "a lost POST response is retried once");
assert.equal(posts[0], posts[1], "retry reuses the exact logical multipart request");
const retriedJob = JSON.parse(posts[0]);
assert.equal(retriedJob.quarto.idempotency_key, "stable-job");
assert.equal(retriedJob.quarto.shared_tree_sha256, digest);
assert.equal(retriedJob.quarto.execution_mode, "isolated-snapshot");
assert.equal(retriedJob.quarto.shared_inventory_complete, true);
assert.deepEqual(retriedJob.quarto.data_inputs, ["data/local.csv"]);
assert.equal(retriedJob.quarto.render_scope, "project");

const missing = JSON.parse(new TextDecoder().decode(manifest));
missing.assets = [{ path: "figures/missing.png", sha256: digest, mime: "image/png", size: 1 }];
const missingBytes = new TextEncoder().encode(JSON.stringify(missing));
const missingSha = await sha(missingBytes);
// The manifest itself can be fetched, but its declared required asset cannot.
setup(async (url, init = {}) => {
  if (init.method === "POST" && url.endsWith("/jobs")) return response({ id: "qjob-3" }, 202);
  if (init.method === "GET" && url.endsWith("/jobs/qjob-3")) return response({ id: "qjob-3", kind: "quarto", status: "done", exit: 0, outputs: {
    "quarto-bundle.json": { size: missingBytes.byteLength, sha256: missingSha },
    "artifact.html": { size: artifact.byteLength, sha256: artifactSha },
  } });
  if (url.endsWith("/files/quarto-bundle.json")) return response(missingBytes);
  if (url.endsWith("/files/artifact.html")) return response(artifact);
  throw new Error(`unexpected local request: ${url}`);
});
const rejected = await runQuarto({ job: { id: "job-missing", binding: "binding-1" }, tree: { main: "paper.qmd", texts: { "paper.qmd": "# Paper" } }, options: { inputDigest: digest } });
assert.equal(rejected.ok, false);
assert.equal(rejected.publish, null);
assert.match(rejected.error, /missing required output/);

console.log("quarto-local: strict request validation, idempotent POST retry, and required closure passed");

const previewCalls = [];
setup(async (url, init) => {
  previewCalls.push({url, init});
  return response(init.method === "DELETE" ? {stopped:true} : {id:"preview-1",url:"http://127.0.0.1:4000/",state:"running"});
});
const preview = await startQuartoPreview({job:{binding:"binding-1"}, tree:{main:"paper.qmd",texts:{"paper.qmd":"# Preview"}},options:{}});
assert.equal(preview.id, "preview-1");
const previewRequest = JSON.parse(previewCalls[0].init.body);
assert.equal(previewRequest.manifest[0].sha256, await sha(new TextEncoder().encode("# Preview")));
assert.equal(previewCalls[0].init.headers.Authorization, "Bearer token");
assert.equal(previewRequest.token, undefined);
assert.equal((await quartoPreviewStatus(preview.id)).state, "running");
await stopQuartoPreview(preview.id);
assert.equal(previewCalls[2].init.method, "DELETE");
assert.ok(previewCalls[2].url.endsWith("/previews/preview-1"));
console.log("quarto-local: snapshot inventory and managed preview lifecycle requests passed");
