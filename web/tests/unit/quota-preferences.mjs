import assert from "node:assert/strict";

import { deleteSharedResults, loadStorageStatus, storageBytes, trimHistory } from "../../src/lib/quota-preferences.js";

const calls = [];
const originalFetch = globalThis.fetch;
try {
  globalThis.fetch = async (path, options) => {
    calls.push({ path, headers: options?.headers, method: options?.method, body: options?.body });
    return new Response(JSON.stringify({ ok: true }), { status: 200 });
  };

  await loadStorageStatus();
  assert.equal(calls.length, 1);
  assert.equal(calls[0].path, "/api/account/storage");
  assert.equal(calls[0].headers["X-LibrePaper-Client"], "shell");

  calls.length = 0;
  const trimResult = await trimHistory("test-slug");
  assert.equal(calls.length, 1);
  assert.equal(calls[0].path, "/api/documents/test-slug/history/trim");
  assert.equal(calls[0].method, "POST");
  assert.equal(calls[0].headers["X-LibrePaper-Client"], "shell");
  assert.equal(calls[0].headers["content-type"], "application/json");
  assert.equal(calls[0].body, "{}");
  assert.deepEqual(trimResult, { ok: true });
  calls.length = 0;
  const deleteResult = await deleteSharedResults("test-slug");
  assert.equal(calls.length, 1);
  assert.equal(calls[0].path, "/api/documents/test-slug/results");
  assert.equal(calls[0].method, "DELETE");
  assert.equal(calls[0].headers["X-LibrePaper-Client"], "shell");
  assert.deepEqual(deleteResult, { ok: true });

  for (const status of [404, 500]) {
    globalThis.fetch = async () => new Response(JSON.stringify({ error: "No shared results." }), { status });
    await assert.rejects(deleteSharedResults("test-slug"), { message: "No shared results." });
  }
  globalThis.fetch = async () => new Response("", { status: 500 });
  await assert.rejects(deleteSharedResults("test-slug"), { message: "500" });
} finally {
  globalThis.fetch = originalFetch;
}

assert.equal(storageBytes(null), "Unavailable");
assert.equal(storageBytes(0), "0 B");
assert.equal(storageBytes(1536), "1.5 KiB");

console.log("quota-preferences: storage status fetch helper and byte formatting are stable");
