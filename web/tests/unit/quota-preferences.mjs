import assert from "node:assert/strict";
import { draftPreferences, displayTimestamp, validateTimezone, storageBytes, storageSnapshot,
  loadQuotaPreferences, previewQuotaPreferences, applyQuotaPreferences } from "../../src/lib/quota-preferences.js";
import { newRequestKey } from "../../src/lib/request-key.js";
const saved = { version: 2, retentionProfile: "default", retentionPolicyVersion: 2 };
const input = { profile: "default", timezone: "America/Toronto", warnings: "75, 90", maxCount: "", maxDays: "" };
const draft = draftPreferences(saved, input);
assert.equal(draft.version, 2);
assert.equal(draft.customRetention, null);
assert.deepEqual(draft.warningThresholds, [75, 90]);
assert.deepEqual(saved, { version: 2, retentionProfile: "default", retentionPolicyVersion: 2 });
assert.deepEqual(draftPreferences(saved, {...input, profile:"custom", maxCount:0, maxDays:1.5}).customRetention, {maxRoutineCount:0,maxAgeMs:129600000});
assert.deepEqual(draftPreferences(saved, {...input, profile:"custom"}).customRetention, {maxRoutineCount:null,maxAgeMs:null});
for (const maxCount of [-1, 4097, 1.5, Infinity, NaN]) assert.throws(() => draftPreferences(saved, {...input,profile:"custom",maxCount}));
for (const maxDays of [-1, Infinity, NaN, Number.MAX_SAFE_INTEGER]) assert.throws(() => draftPreferences(saved, {...input,profile:"custom",maxDays}));
for (const warnings of ["", "90, 75", "75, 75", "0", "101", "abc", "1,2,3,4,5"]) assert.throws(() => draftPreferences(saved, {...input,warnings}));
assert.equal(validateTimezone("America/Toronto"),true);
assert.equal(validateTimezone("wrong/timezone"),false);
assert.equal(displayTimestamp("invalid"),"Unavailable");
assert.equal(displayTimestamp(1772953200000,"UTC"),displayTimestamp("2026-03-08T07:00:00Z","UTC"));
assert.equal(storageBytes(null),"Unavailable"); assert.equal(storageBytes(0),"0 B"); assert.equal(storageBytes(1536),"1.5 KiB");
const unavailable=storageSnapshot({preferences:{version:2},effective:{incompatible:true},profiles:["default","manual","custom"]});
assert.equal(unavailable.canManage,false); assert.equal(unavailable.profiles.length,3);
const warned=storageSnapshot({preferences:{warningThresholds:[75,90]},constraints:{hardQuotaBytes:100},usage:{chargedBytes:95}});
assert.match(warned.usage.message,/90%/);
const keys=new Set(Array.from({length:100},()=>newRequestKey(123456)));
assert.equal(keys.size,100);
for(const key of keys) assert.match(key,/^v2\.123456\.[a-f0-9]{32}$/);
assert.throws(()=>newRequestKey(-1));
const calls=[],originalFetch=globalThis.fetch;
try {
 globalThis.fetch=async(path,options)=>{calls.push({path,...options});return new Response(JSON.stringify({revision:3}),{status:200});};
 await loadQuotaPreferences();await previewQuotaPreferences(3,draft);await applyQuotaPreferences(3,draft);
 for(const call of calls) assert.equal(call.headers["X-LibrePaper-Client"],"shell");
 assert.deepEqual(JSON.parse(calls[2].body),{revision:3,preferences:draft});
 globalThis.fetch=async()=>new Response(JSON.stringify({error:"preference revision is stale"}),{status:409});
 await assert.rejects(applyQuotaPreferences(3,draft),/stale/);
} finally {globalThis.fetch=originalFetch;}
console.log("quota-preferences: bounded v2 policies, milliseconds, advisory preview, revision conflicts, random request keys");
