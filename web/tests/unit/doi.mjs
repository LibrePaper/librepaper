import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { cslText, cslToBibtex, cslYear, fetchDoi, normaliseDoi, usableCsl } from "../../src/lib/doi.js";

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
assert.deepEqual(await fetchDoi("10.1000/a b", respond(200, { type: "misc", title: "T" })), { type: "misc", title: "T" });
assert.equal(calls[0].url, "https://doi.org/10.1000/a%20b");
assert.equal(calls[0].init.headers.Accept, "application/vnd.citationstyles.csl+json");
await assert.rejects(fetchDoi("10.1000/x", respond(200, {})), { code: "not_found" });
for (const csl of fixture.invalid ?? []) assert.equal(usableCsl(csl), false, `usableCsl(${JSON.stringify(csl)})`);
assert.equal(cslText("Fitting <b>lme4</b> &amp; {co}"), "Fitting lme4 & co");
assert.equal(cslYear({ issued: { "date-parts": [[-44]] } }), "");
await assert.rejects(fetchDoi("10.1000/x", respond(404, null)), { code: "not_found" });
await assert.rejects(fetchDoi("10.1000/x", respond(500, null)), { code: "unreachable" });
await assert.rejects(fetchDoi("10.1000/x", async () => { throw new TypeError("network"); }), { code: "unreachable" });

// Crossref names, undated records and double-encoded entities.
assert.match(cslToBibtex({ type: "journal-article", title: "T", "container-title": "J" }, "10.1/x").bibtex, /^@article\{/);
assert.match(cslToBibtex({ type: "book-chapter", title: "T" }, "10.1/x").bibtex, /^@incollection\{/);
assert.equal(cslYear({ issued: { "date-parts": [[null]] } }), "");
assert.equal(cslYear({ issued: { "date-parts": [["2013"]] } }), "2013");
assert.equal(cslYear({ issued: { "date-parts": [[2013.5]] } }), "");
assert.match(cslToBibtex({ type: "misc", title: "A &amp;lt; B" }, "10.1/x").bibtex, /title = \{A \\&lt; B\}/);

// TeX specials are escaped in values (not doi or url); the key uses the unescaped text.
{
  const { key, bibtex } = cslToBibtex({ type: "misc", title: "R&D_x 50% #1 $5 \\o", author: [{ literal: "A&B_C" }], DOI: "10.1/a_b", URL: "https://x.org/a_b?c=1&d=2" }, "10.1/x");
  assert.match(bibtex, /author = \{\{A\\&B\\_C\}\}/);
  assert.match(bibtex, /title = \{R\\&D\\_x 50\\% \\#1 \\\$5 o\}/);
  assert.match(bibtex, /doi = \{10\.1\/a_b\}/);
  assert.match(bibtex, /url = \{https:\/\/x\.org\/a_b\?c=1&d=2\}/);
  assert.equal(key, "AbcND");
}
// Title words: empty fragments do not use up the three slots.
assert.equal(cslToBibtex({ title: "日本語 Alpha Beta Gamma Delta" }, "10.1/x").key, "AlpBetGamND");
// Query strings and fragments are cut before percent-decoding.
assert.equal(normaliseDoi("https://doi.org/10.1038/nature12373?utm=1#top"), "10.1038/nature12373");
assert.equal(normaliseDoi("https://doi.org/10.1000/a%3Fb%23c?x=1"), "10.1000/a?b#c");
