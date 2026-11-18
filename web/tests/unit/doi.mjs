import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { cslToBibtex, fetchDoi, normaliseDoi } from "../../src/lib/doi.js";

const fixture = JSON.parse(readFileSync(new URL("../fixtures/doi-bibtex.json", import.meta.url), "utf8"));

for (const { input, doi } of fixture.normalise) {
  assert.equal(normaliseDoi(input), doi, `normaliseDoi(${JSON.stringify(input)})`);
}
for (const { name, doi, csl, key, bibtex } of fixture.convert) {
  const result = cslToBibtex(csl, doi);
  assert.equal(result.key, key, `${name}: key`);
  assert.equal(result.bibtex, bibtex, `${name}: bibtex`);
}

// fetchDoi asks doi.org for CSL-JSON, one encoded segment at a time.
const calls = [];
const respond = (status, body) => async (url, init) => {
  calls.push({ url, init });
  return { status, ok: status >= 200 && status < 300, json: async () => body };
};
assert.deepEqual(await fetchDoi("10.1000/a b", respond(200, { title: "T" })), { title: "T" });
assert.equal(calls[0].url, "https://doi.org/10.1000/a%20b");
assert.equal(calls[0].init.headers.Accept, "application/vnd.citationstyles.csl+json");
await assert.rejects(fetchDoi("10.1000/x", respond(404, null)), { code: "not_found" });
await assert.rejects(fetchDoi("10.1000/x", respond(500, null)), { code: "unreachable" });
await assert.rejects(fetchDoi("10.1000/x", async () => { throw new TypeError("network"); }), { code: "unreachable" });
