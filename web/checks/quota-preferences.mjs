import assert from "node:assert/strict";
import {
  draftPreferences, displayTimestamp, validateTimezone, storageBytes, storageSnapshot,
  loadQuotaPreferences, previewQuotaPreferences, applyQuotaPreferences,
} from "../src/lib/quota-preferences.js";

const saved = {
  version: 1, retentionPolicyVersion: 1, customRetention: { tiers: [] },
  milestonePreferences: { named: true, comment: true },
  documentOverrides: { project: { retentionProfile: "balanced" } },
};
const input = {
  budgetKind: "bytes", budgetValue: 1.5, profile: "balanced", timezone: "America/Toronto",
  milestones: { named: false }, warnings: "75, 90",
};
const draft = draftPreferences(saved, input);
assert.equal(draft.historyBudget.value, 1.5 * 1024 * 1024);
assert.equal(draft.milestonePreferences.named, false);
assert.equal(draft.milestonePreferences.comment, true);
assert.deepEqual(draft.documentOverrides, saved.documentOverrides);
assert.equal(draft.customRetention, null);
assert.deepEqual(draft.warningThresholds, [75, 90]);
assert.equal(saved.milestonePreferences.named, true, "draft edits cannot mutate persisted preferences");
for (const value of [-1, Infinity, NaN]) assert.throws(() => draftPreferences(saved, { ...input, budgetValue: value }));
for (const value of [-1, 100.1, 101, 25.5]) assert.throws(() => draftPreferences(saved, { ...input, budgetKind: "percent", budgetValue: value }));
assert.equal(draftPreferences(saved, { ...input, budgetKind: "percent", budgetValue: 0 }).historyBudget.value, 0);
for (const warnings of ["", "90, 75", "75, 75", "0", "101", "abc", "1,2,3,4,5"]) {
  assert.throws(() => draftPreferences(saved, { ...input, warnings }));
}
assert.equal(validateTimezone("America/Toronto"), true);
assert.equal(validateTimezone("UTC"), true);
assert.equal(validateTimezone("wrong/timezone"), false);
assert.equal(displayTimestamp("invalid"), "Unavailable");
assert.equal(displayTimestamp("2026-03-08T07:00:00Z", "wrong/timezone"), displayTimestamp("2026-03-08T07:00:00Z", "UTC"));
assert.equal(storageBytes(null), "Unavailable");
assert.equal(storageBytes(0), "0 B");
assert.equal(storageBytes(1536), "1.5 KiB");
const unavailable = storageSnapshot({
  preferences: { version: 1, retentionProfile: "balanced" },
  constraints: { hardQuotaBytes: 100, minimumBucketSeconds: 300 },
  usage: { chargedBytes: 50, physicalAccounting: false },
  effective: { incompatible: true, retention: { tiers: [{ maxAgeSeconds: 3600, bucketSeconds: 300 }] } },
  profiles: ["balanced", "custom"],
});
assert.equal(unavailable.canManage, false, "unknown policies cannot enable a destructive preference form");
assert.equal(unavailable.usage.authoritative, false, "logical totals cannot become a physical quota meter");
assert.equal(unavailable.usage.hardQuota, 100);
assert.deepEqual(unavailable.profiles, [{ id: "balanced", label: "Balanced" }]);
assert.equal(unavailable.effective.tiers[0].ageLabel, "Less than 1 hour");

const calls = [];
const originalFetch = globalThis.fetch;
try {
  globalThis.fetch = async (path, options) => {
    calls.push({ path, ...options });
    return new Response(JSON.stringify({ revision: 3 }), { status: 200 });
  };
  await loadQuotaPreferences();
  await previewQuotaPreferences(3, draft);
  await applyQuotaPreferences(3, "preview-generation", draft, true);
  assert.equal(calls.length, 3);
  for (const call of calls) assert.equal(call.headers["X-LibrePaper-Client"], "shell");
  assert.equal(calls[1].method, "POST");
  assert.deepEqual(JSON.parse(calls[2].body), {
    revision: 3, generation: "preview-generation", preferences: draft, confirmed: true,
  });
  globalThis.fetch = async () => new Response(JSON.stringify({ error: "Storage preview is stale" }), { status: 409 });
  await assert.rejects(applyQuotaPreferences(3, "old", draft, true), /stale/);
} finally { globalThis.fetch = originalFetch; }

console.log("quota-preferences: safe drafts, display-only timezone, authenticated preview/apply and stale rejection");
