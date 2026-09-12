import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const component = readFileSync(new URL("../../src/components/Try.svelte", import.meta.url), "utf8");
const landing = readFileSync(new URL("../../src/site/index.html", import.meta.url), "utf8");
const bar = readFileSync(new URL("../../src/site/SiteBar.svelte", import.meta.url), "utf8");

for (const forbidden of ["fetch(", "localStorage", "sessionStorage", "indexedDB", "/api/", "WebSocket", "Share.svelte"]) {
  assert.equal(component.includes(forbidden), false, `playground must not use ${forbidden}`);
}
assert.match(component, /sandbox=""/);
assert.match(component, /Nothing is uploaded, saved, or shareable/);
assert.equal((landing.match(/https:\/\/app\.librepaper\.org\/try/g) || []).length, 2);
assert.match(bar, /https:\/\/app\.librepaper\.org\/try/);

console.log("playground: temporary local-only contract and landing links passed");
