import assert from "node:assert/strict";
import test from "node:test";
import { loadRunes } from "../helpers/runes.mjs";

const { fetchResults, putResults, createSharedResults } = await loadRunes(new URL("../../src/lib/reader/shared-results.svelte.js", import.meta.url));

const record = { version: 1, path: "main.qmd", cells: [{ fingerprint: "a".repeat(64), label: null, html: "<p>1</p>" }] };

async function withFetch(handler, body) {
  const original = globalThis.fetch;
  const calls = [];
  globalThis.fetch = async (path, options) => { calls.push({ path, options }); return handler(path, options); };
  try { await body(calls); } finally { globalThis.fetch = original; }
}

test("a document without results reads as null", async () => {
  await withFetch(async () => new Response(JSON.stringify({ error: "none" }), { status: 404 }), async (calls) => {
    assert.equal(await fetchResults("paper", {}), null);
    assert.equal(calls[0].path, "/api/documents/paper/results");
  });
});

test("identical results are not uploaded twice", async () => {
  await withFetch(async () => new Response(JSON.stringify({ bytes: 120, updatedAt: "2026-11-12T10:00:00Z" }), { status: 200 }), async (calls) => {
    const shared = createSharedResults("paper", () => ({}));
    assert.deepEqual(await shared.upload(record), { kind: "shared", bytes: 120 });
    await shared.upload(record);
    assert.equal(calls.filter((call) => call.options?.method === "PUT").length, 1);
    await shared.upload({ ...record, path: "other.qmd" });
    assert.equal(calls.length, 2);
  });
});

test("a full storage answers failed with the storage reason", async () => {
  await withFetch(async () => new Response(JSON.stringify({ error: "full" }), { status: 413 }), async () => {
    const put = await putResults("paper", record, {});
    assert.deepEqual(put, { ok: false, status: 413, error: "full" });
    const shared = createSharedResults("paper", () => ({}));
    assert.deepEqual(await shared.upload(record), { kind: "failed", reason: "storage" });
  });
});
