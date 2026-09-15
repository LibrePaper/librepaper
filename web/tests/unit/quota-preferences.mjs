import assert from "node:assert/strict";

import { loadStorageStatus, storageBytes } from "../../src/lib/quota-preferences.js";

const calls = [];
const originalFetch = globalThis.fetch;
try {
  globalThis.fetch = async (path, options) => {
    calls.push({ path, headers: options?.headers });
    return new Response(JSON.stringify({ ok: true }), { status: 200 });
  };

  await loadStorageStatus();
  assert.equal(calls.length, 1);
  assert.equal(calls[0].path, "/api/account/storage");
  assert.equal(calls[0].headers["X-LibrePaper-Client"], "shell");
} finally {
  globalThis.fetch = originalFetch;
}

assert.equal(storageBytes(null), "Unavailable");
assert.equal(storageBytes(0), "0 B");
assert.equal(storageBytes(1536), "1.5 KiB");

console.log("quota-preferences: storage status fetch helper and byte formatting are stable");
