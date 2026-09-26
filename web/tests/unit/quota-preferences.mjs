import assert from "node:assert/strict";

import { loadStorageStatus, storageBytes, trimHistory } from "../../src/lib/quota-preferences.js";

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
} finally {
  globalThis.fetch = originalFetch;
}

assert.equal(storageBytes(null), "Unavailable");
assert.equal(storageBytes(0), "0 B");
assert.equal(storageBytes(1536), "1.5 KiB");

console.log("quota-preferences: storage status fetch helper and byte formatting are stable");
