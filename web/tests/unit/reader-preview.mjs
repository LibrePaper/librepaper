import assert from "node:assert/strict";
import { availableDownloads } from "../../src/lib/reader/downloads.js";
import { createPreviewApi } from "../../src/lib/reader/preview-api.js";
import { clearLegacyRenderingCaches } from "../../src/lib/reader/boot.js";

// The browser preview API exposes source framing only. Rendered bytes never
// have a server endpoint that a reader can fetch as a fallback.
{
  const calls = [];
  const api = createPreviewApi({
    slug: "paper",
    key: "key",
    shellHeaders: { "x-shell": "shell" },
    keyHeaders: (key) => ({ "x-key": key }),
    request: async (...args) => { calls.push(args); return { ok: true }; },
  });
  await api.frame();
  assert.equal(calls.length, 1);
  assert.equal(calls[0][0], "/api/documents/paper/frame");
  assert.deepEqual(calls[0][1].headers, { "x-shell": "shell", "x-key": "key" });
  for (const name of ["latest", "rendering", "selectedResults", "resultBundle", "publishResults", "putRendering"]) {
    assert.equal(name in api, false, `${name} must not be a retained-result API`);
  }
}

assert.deepEqual(availableDownloads({ outputKind: "pdf" }), { pdf: false, html: false });
assert.deepEqual(availableDownloads({ outputKind: "pdf", deliveredKind: "pdf" }), { pdf: true, html: false });
assert.deepEqual(availableDownloads({ outputKind: "pdf", rendering: { sha: "old" } }), { pdf: false, html: false });
assert.deepEqual(availableDownloads({ outputKind: "html", displayedFormat: "html" }), { pdf: false, html: true });
assert.deepEqual(availableDownloads({ outputKind: "html", rendering: { sha: "old" } }), { pdf: false, html: false });

{
  const deleted = [];
  const indexedDB = {
    deleteDatabase(name) {
      deleted.push(name);
      const request = {};
      queueMicrotask(() => request.onsuccess?.());
      return request;
    },
  };
  assert.equal(await clearLegacyRenderingCaches({ indexedDB }), true);
  assert.deepEqual(deleted, ["librepaper-quarto-outbox"], "startup cleanup removes the legacy rendered-result outbox");
}

// The old shared cache held both immutable input assets and generated output.
// Startup cleanup must remove only document rendering/result routes, leaving
// source framing, input assets, and unrelated application bytes intact.
{
  const entries = new Map([
    ["/api/documents/paper/renderings/latest", new Response("{}")],
    ["/api/documents/paper/renderings/abc", new Response("%PDF-")],
    ["/api/documents/paper/quarto/bundles/render-1/artifact", new Response("<p>old</p>")],
    ["/api/documents/paper/assets/abc", new Response("input")],
    ["/api/documents/paper/frame", new Response("source")],
    ["/app/renderings/help", new Response("application")],
  ]);
  const store = {
    async keys() { return [...entries.keys()]; },
    async delete(request) { return entries.delete(typeof request === "string" ? request : request.url); },
  };
  const caches = {
    async keys() { return ["librepaper"]; },
    async open(name) { assert.equal(name, "librepaper"); return store; },
  };
  assert.equal(await clearLegacyRenderingCaches({ indexedDB: null, caches }), true);
  assert.deepEqual([...entries.keys()], [
    "/api/documents/paper/assets/abc", "/api/documents/paper/frame", "/app/renderings/help",
  ]);
}

console.log("reader-preview: source framing and transient download behavior passed");
