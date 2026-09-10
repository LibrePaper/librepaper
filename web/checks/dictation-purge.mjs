// "Remove downloaded model" against a fake Cache Storage, so
// web/src/lib/dictation/purge.js is driven under Node the same way as the
// rest of dictation.

import { removeCachedModel } from "../src/lib/dictation/purge.js";

let failures = 0;
function check(what, condition, detail = "") {
  if (condition) return;
  failures += 1;
  console.error(`dictation-purge: FAIL ${what}${detail ? ` -- ${detail}` : ""}`);
}

function fakeCaches(urls) {
  const requests = urls.map((url) => ({ url }));
  const deleted = [];
  const cache = {
    keys: async () => requests,
    delete: async (request) => {
      deleted.push(request.url);
      return true;
    },
  };
  return { open: async () => cache, deleted };
}

async function testDeletesOnlyMatchingUrls() {
  const caches = fakeCaches([
    "https://huggingface.co/onnx-community/whisper-small/resolve/abc123/onnx/encoder_model.onnx",
    "https://huggingface.co/onnx-community/whisper-small/resolve/abc123/tokenizer.json",
    "https://huggingface.co/onnx-community/whisper-base/resolve/def456/onnx/encoder_model.onnx",
    "https://huggingface.co/onnx-community/silero-vad/resolve/xyz789/model.onnx",
  ]);
  const entry = { repo: "onnx-community/whisper-small" };
  const removed = await removeCachedModel(entry, { caches });
  check("reports the count removed", removed === 2, removed);
  check("removes only the requested model's files", caches.deleted.every((url) => url.includes("/whisper-small/")), JSON.stringify(caches.deleted));
  check("leaves the other model and the VAD alone", caches.deleted.length === 2);
}

async function testMissingCachesApiReturnsZero() {
  const removed = await removeCachedModel({ repo: "onnx-community/whisper-small" }, {});
  check("no caches API -> 0", removed === 0, removed);
}

async function testMissingCacheReturnsZero() {
  const caches = { open: async () => { throw new Error("no such cache"); } };
  const removed = await removeCachedModel({ repo: "onnx-community/whisper-small" }, { caches });
  check("caches.open throwing -> 0", removed === 0, removed);
}

async function testNoEntryReturnsZero() {
  const caches = fakeCaches(["https://huggingface.co/onnx-community/whisper-small/resolve/abc/x"]);
  const removed = await removeCachedModel(null, { caches });
  check("no entry -> 0", removed === 0, removed);
}

for (const test of [testDeletesOnlyMatchingUrls, testMissingCachesApiReturnsZero, testMissingCacheReturnsZero, testNoEntryReturnsZero]) {
  await test();
}

if (failures) {
  console.error(`dictation-purge: ${failures} check(s) failed`);
  process.exit(1);
}
console.log("dictation-purge: cache removal targets exactly the requested model");
