import assert from "node:assert/strict";
import { publishQuartoBundle } from "../src/lib/quarto-publication.js";

const payload = { manifest:{ render_id:"one", context:{ id:"html" }, artifact:{ sha256:"same" } }, blobs:[{ sha256:"same", data:"original" }], expected_generation:3, select:true };
const before = JSON.stringify(payload);
const calls = [];
const receipt = await publishQuartoBundle({
  quartoPublish: async (body) => { calls.push(body); return { ok:false, status:409 }; },
  quartoBundle: async () => ({ ok:true, json:async () => structuredClone(payload.manifest) }),
}, payload, { artifact:{ sha256:"same" } });
assert.equal(receipt.selected, false);
assert.equal(calls[0].blobs.length, 0);
assert.equal(calls[0].expected_generation, 3);
assert.equal(JSON.stringify(payload), before, "deduplication preserves the complete outbox/local preview payload");
await assert.rejects(publishQuartoBundle({
  quartoPublish: async () => ({ ok:false, status:409, json:async () => ({ error:"render ID conflict" }) }),
  quartoBundle: async () => ({ ok:true, json:async () => ({ ...payload.manifest, context:{ id:"pdf" } }) }),
}, payload), /render ID conflict/);
for (const status of [400, 404]) {
const fallback = [];
await publishQuartoBundle({ quartoPublish: async (body) => {
  fallback.push(body);
  return fallback.length === 1 ? { ok:false, status } : { ok:true, json:async () => ({ selected:true }) };
} }, payload, { artifact:{ sha256:"same" } });
assert.deepEqual(fallback.map((body) => body.blobs.length), [0, 1]);
assert.equal(fallback[1], payload);
}
console.log("quarto publication: selection conflict acknowledgement, immutable outbox, and missing-blob retry passed");
