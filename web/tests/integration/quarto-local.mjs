import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import * as localBridge from "../../src/lib/latex/local.js";
import { createLocalPreview } from "../../src/lib/reader/local-preview.js";
import { _testing, configure, quartoRequest, runQuarto, startQuartoPreview, stopQuartoPreview, quartoPreviewStatus, quartoPreviewPage, syncWorkspace, startLocalPreview, localPreviewPage, localPreviewStatus, stopLocalPreview, calepinAvailable } from "../../src/lib/latex/local.js";
import { parameterSha256 } from "../../src/lib/engines/quarto.js";

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

function response(body, status = 200, headers = {}) {
  const bytes = body instanceof Uint8Array ? body : new TextEncoder().encode(JSON.stringify(body));
  const lower = new Map(Object.entries(headers).map(([key, value]) => [key.toLowerCase(), value]));
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: { get: (name) => lower.get(String(name).toLowerCase()) ?? null },
    json: async () => JSON.parse(new TextDecoder().decode(bytes)),
    text: async () => new TextDecoder().decode(bytes),
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
  options: { inputRevision: "rev-1", inputDigest: digest, executionMode:"isolated-snapshot", dataInputs:["data/local.csv"] },
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
assert.equal(retriedJob.quarto.render_scope, "document");
await assert.rejects(
  runQuarto({
    job: { id: "project-job", binding: "binding-1" },
    tree: { main: "paper.qmd", texts: { "paper.qmd": "# Paper" } },
    options: { renderScope: "project" },
  }),
  /website and book project renders are not supported/,
);

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
await stopQuartoPreview(preview.id);
const pdfPreview = await startQuartoPreview({job:{binding:"binding-1"}, tree:{main:"paper.qmd",texts:{"paper.qmd":"---\nformat: pdf\n---\n# Preview"}},options:{format:"pdf"}});
assert.equal(pdfPreview.id, "preview-1");
const pdfPreviewRequest = JSON.parse(previewCalls[2].init.body);
assert.equal(pdfPreviewRequest.output || pdfPreviewRequest.quarto?.format, "pdf");
assert.equal((await quartoPreviewStatus(preview.id)).state, "running");
await stopQuartoPreview(pdfPreview.id);
assert.equal(previewCalls[4].init.method, "DELETE");
assert.ok(previewCalls[4].url.endsWith("/previews/preview-1"));
console.log("quarto-local: snapshot inventory and managed preview lifecycle requests passed");

let syncRequest = null;
setup(async (url, init = {}) => {
  assert.equal(init.method, "PUT");
  assert.ok(url.endsWith("/librepaper/local/v1/workspace"));
  assert.equal(init.headers.Authorization, "Bearer token");
  syncRequest = init;
  return response({ synced: 2 });
});
const tree = { main: "paper.qmd", texts: { "paper.qmd": "# Paper" }, assets: { "figures/plot.png": new Uint8Array([1, 2, 3]) } };
const syncResult = await syncWorkspace({ tree });
assert.deepEqual(syncResult, { synced: 2 });
const manifestPart = syncRequest.body.get("manifest");
const manifestEntries = JSON.parse(await manifestPart.text());
assert.deepEqual(
  manifestEntries.map((entry) => entry.path).sort(),
  ["figures/plot.png", "paper.qmd"],
);
for (const entry of manifestEntries) {
  const bytes = entry.path === "paper.qmd" ? new TextEncoder().encode("# Paper") : new Uint8Array([1, 2, 3]);
  assert.equal(entry.sha256, await sha(bytes));
  assert.equal(entry.size, bytes.byteLength);
}
const filesInForm = syncRequest.body.getAll("file");
assert.equal(filesInForm.length, 2);
const fileNames = filesInForm.map((part) => part.name).sort();
assert.deepEqual(fileNames, ["figures/plot.png", "paper.qmd"]);
console.log("quarto-local: syncWorkspace uploads a manifest and file parts to the workspace endpoint");

// The live preview's own page: a fresh render comes back with an etag, a
// repeat of the same etag is a 304 the caller reads as "nothing new", and a
// preview with no render yet is a 404 the caller can tell apart from any
// other failure.
{
  const pageCalls = [];
  setup(async (url, init = {}) => {
    pageCalls.push({ url, init });
    if (url.endsWith("/previews/preview-1/page")) {
      if (init.headers?.["If-None-Match"] === '"etag-1"') {
        return response(new Uint8Array(), 304, { "X-LibrePaper-Rendering": "true" });
      }
      return response(new TextEncoder().encode("<html>page</html>"), 200, { ETag: '"etag-1"', "X-LibrePaper-Rendering": "false" });
    }
    if (url.endsWith("/previews/preview-missing/page")) {
      return response({ error: "not rendered yet" }, 404, { "X-LibrePaper-Rendering": "true" });
    }
    throw new Error(`unexpected local request: ${init.method || "GET"} ${url}`);
  });
  const first = await quartoPreviewPage("preview-1", {});
  assert.equal(first.html, "<html>page</html>");
  assert.equal(first.etag, '"etag-1"');
  assert.equal(first.rendering, false);
  assert.equal(pageCalls[0].init.headers["If-None-Match"], undefined);
  const unchanged = await quartoPreviewPage("preview-1", { etag: '"etag-1"' });
  assert.equal(unchanged.html, undefined);
  assert.equal(unchanged.rendering, true);
  assert.equal(pageCalls[1].init.headers["If-None-Match"], '"etag-1"');
  await assert.rejects(quartoPreviewPage("preview-missing", {}), (error) => error.name === "NotRendered" && error.rendering === true);
}
console.log("quarto-local: quartoPreviewPage resolves fresh HTML, a 304 miss, and a distinguishable 404, each carrying the render-state header");

// startLocalPreview({ engine: "calepin", ... }) validates the typed options
// block and posts the generalized envelope; a .qmd entrypoint or a bad
// format is rejected before any request is sent.
{
  const calepinCalls = [];
  setup(async (url, init) => {
    calepinCalls.push({ url, init });
    return response({ id: "preview-calepin", url: "http://127.0.0.1:4000/", state: "running" });
  });
  const calepinTree = { texts: { "doc.typ": "= Hello" } };
  const calepinPreview = await startLocalPreview({
    engine: "calepin",
    job: { binding: "binding-1" },
    tree: calepinTree,
    options: { entrypoint: "doc.typ", format: "pdf" },
  });
  assert.equal(calepinPreview.id, "preview-calepin");
  const calepinRequest = JSON.parse(calepinCalls[0].init.body);
  assert.equal(calepinRequest.engine, "calepin");
  assert.equal(calepinRequest.calepin.binding_id, "binding-1");
  assert.equal(calepinRequest.calepin.main, "doc.typ");
  assert.equal(calepinRequest.calepin.format, "pdf");
  assert.equal(calepinRequest.manifest[0].sha256, await sha(new TextEncoder().encode("= Hello")));

  await assert.rejects(
    startLocalPreview({ engine: "calepin", job: { binding: "binding-1" }, tree: calepinTree, options: { entrypoint: "doc.qmd", format: "html" } }),
    /\.typ file/,
  );
  await assert.rejects(
    startLocalPreview({ engine: "calepin", job: { binding: "binding-1" }, tree: calepinTree, options: { entrypoint: "doc.typ", format: "docx" } }),
    /invalid Calepin output format/,
  );

  // startQuartoPreview keeps producing exactly the Quarto envelope, with no
  // `engine`/`calepin` fields leaking in.
  const quartoOnly = await startQuartoPreview({ job: { binding: "binding-1" }, tree: { main: "paper.qmd", texts: { "paper.qmd": "# Preview" } }, options: {} });
  assert.equal(quartoOnly.id, "preview-calepin");
  const quartoOnlyRequest = JSON.parse(calepinCalls[calepinCalls.length - 1].init.body);
  assert.equal(quartoOnlyRequest.engine, undefined);
  assert.equal(quartoOnlyRequest.kind, "quarto");
}
console.log("quarto-local: startLocalPreview validates and posts the calepin engine envelope, and startQuartoPreview is unchanged");

// localPreviewPage recognizes a PDF artifact by its content type and hands
// back raw bytes rather than text.
{
  const pdfBytes = new TextEncoder().encode("%PDF-1.7 fake");
  setup(async (url, init = {}) => {
    if (url.endsWith("/previews/preview-pdf/page")) {
      return response(pdfBytes, 200, { ETag: '"etag-pdf"', "X-LibrePaper-Rendering": "false", "X-LibrePaper-Kind": "pdf", "Content-Type": "application/pdf" });
    }
    throw new Error(`unexpected local request: ${init.method || "GET"} ${url}`);
  });
  const page = await localPreviewPage("preview-pdf", {});
  assert.equal(page.kind, "pdf");
  assert.ok(page.bytes instanceof Uint8Array);
  assert.deepEqual([...page.bytes], [...pdfBytes]);
  assert.equal(page.etag, '"etag-pdf"');
  assert.equal(page.rendering, false);
}
console.log("quarto-local: localPreviewPage returns pdf bytes as a Uint8Array with kind \"pdf\"");

// localPreviewStatus/stopLocalPreview are aliases of the existing calls, and
// calepinAvailable reads the last probed capabilities without fetching.
{
  setup(async (url, init) => {
    if (init.method === "DELETE") return response({ stopped: true });
    return response({ id: "preview-1", url: "http://127.0.0.1:4000/", state: "running" });
  });
  assert.equal((await localPreviewStatus("preview-1")).state, "running");
  await stopLocalPreview("preview-1");
  assert.equal(calepinAvailable(), false);
  _testing.inject({});
}
console.log("quarto-local: localPreviewStatus/stopLocalPreview alias the existing calls, and calepinAvailable defaults false");

// Exercise the Reader's actual controller configuration with the wire client:
// an old external binding must not redirect a synchronized live preview.
{
  const reader = readFileSync(new URL("../../src/components/Reader.svelte", import.meta.url), "utf8");
  for (const engine of ["quarto", "calepin"]) {
    const calls = [];
    setup(async (url, init) => {
      if (init.method === "PUT" && url.endsWith("/workspace")) {
        calls.push("sync");
        return response({ synced: 1 });
      }
      if (init.method === "POST" && url.endsWith("/previews")) {
        const request = init.body?.get ? JSON.parse(await init.body.get("job").text()) : JSON.parse(init.body);
        calls.push(request[engine].binding_id);
        return request[engine].binding_id === "hosted"
          ? response({ id: "live" }, 201)
          : response({ error: "Preview binding is not authorized" }, 400);
      }
      if (init.method === "DELETE") return response({ stopped: true });
      throw new Error(`unexpected request: ${url}`);
    });
    localBridge.setBindingId("revoked-project-binding");
    const name = `${engine}PreviewController`;
    const start = reader.indexOf(`  const ${name} = createLocalPreview({`);
    assert.ok(start >= 0);
    const end = reader.indexOf("\n  });", start) + "\n  });".length;
    const errors = [];
    const context = vm.createContext({
      localQuarto: localBridge, quartoBindingId: localBridge.bindingId(),
      buildPreferences: { output: "html", profile: null, parameters: {} },
      readerDisposed: false, navigationGeneration: 0,
      treeNow: () => ({ main: engine === "quarto" ? "main.qmd" : "main.typ", texts: { [engine === "quarto" ? "main.qmd" : "main.typ"]: "Hello" } }),
      quartoRenderContext: () => ({ format: "html", profiles: [], parameters: {} }),
      createLocalPreview: options => createLocalPreview({ ...options,
        onError: message => errors.push(message), setTimer: () => ({}), clearTimer: () => {},
      }),
    });
    vm.runInContext(`${reader.slice(start, end)}\nglobalThis.controller = ${name};`, context);
    await context.controller.start();
    assert.deepEqual(calls, ["sync", "hosted"], `${engine} previews the workspace just synchronized`);
    assert.equal(context.controller.running, true, errors.join("; "));
    assert.equal(localBridge.bindingId(), "revoked-project-binding", "explicit render-job setting is preserved");
    await context.controller.stop();
  }
}
console.log("quarto-local: Reader previews use the synchronized workspace even with a revoked saved binding");
