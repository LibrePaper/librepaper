import assert from "node:assert/strict";
import { BibliographyCache, bibliographyCacheKey, bibliographyCompletion, bibliographyNeedsAnalysis, bibliographyRegistration, citationContext, entryLabel, insertCitation, planZoteroImport, rankEntries, zoteroCitationKey } from "../../src/lib/bibliography.js";

const entries = [
  { key: "smith2020", authors: ["Jane Smith"], year: 2020, title: "A Study of Rivers", container: "Nature" },
  { key: "jones2019", authors: ["Alex Jones", "Pat Lee"], year: 2019, title: "Writing Better Papers", container: "JOSS" },
  { key: "river+data", authors: "Lee, Pat", year: 2022, title: "River Data", container: "Data" },
];
assert.deepEqual(rankEntries(entries, "smi").map((e) => e.key), ["smith2020"]);
assert.deepEqual(rankEntries(entries, "Jones").map((e) => e.key), ["jones2019"]);
assert.deepEqual(rankEntries(entries, "river data").map((e) => e.key), ["river+data"]);
assert.equal(entryLabel(entries[1]), "Jones et al. — 2019 — Writing Better Papers — JOSS");

assert.deepEqual(citationContext("See @smith", 10, "markdown"), { kind: "markdown", from: 5, to: 10, query: "smith", trigger: "@" });
assert.deepEqual(citationContext("See @river data", 15, "markdown"), { kind: "markdown", from: 5, to: 15, query: "river data", trigger: "@" });
assert.deepEqual(rankEntries(entries, "river data").map((entry) => entry.key), ["river+data"]);
assert.equal(citationContext("mail a@smith.example", 18, "markdown"), null);
assert.equal(citationContext(String.fromCharCode(96) + "See @smith" + String.fromCharCode(96), 11, "markdown"), null);
assert.deepEqual(citationContext("See [-@smith]", 11, "markdown"), { kind: "markdown", from: 7, to: 11, query: "smit", trigger: "@" });
assert.deepEqual(citationContext("See \\citep{smith, jo}", 20, "latex"), { kind: "latex", from: 18, to: 20, query: "jo", trigger: "\\citep{" });
assert.equal(citationContext("@smith", 6, "html"), null);
assert.equal(insertCitation("See @smi", 8, "smith2020", "markdown"), "See @smith2020");
assert.equal(insertCitation("See \\citep{smith, jo}", 20, "jones2019", "latex"), "See \\citep{smith, jones2019}");

const fake = (text, pos) => ({ pos, explicit: true, state: { doc: { toString: () => text } } });
const completion = bibliographyCompletion({ entries, format: "markdown" })(fake("See @smi", 8));
assert.equal(completion.options[0].label, "smith2020");
assert.equal(completion.options[0].displayLabel.includes("Rivers"), true);
assert.equal(completion.filter, false);
assert.equal(bibliographyCompletion({ entries: [], format: "markdown" })(fake("@", 1)), null);
assert.equal(bibliographyCompletion({ entries, format: "markdown" })(fake("See @smith2020 ", 16)), null);
let imported = null;
const remoteCompletion = await bibliographyCompletion({ entries: [], format: "markdown", remote: async () => [{ key: "Smi2023", authors: ["Smith, Ada"], year: 2023, title: "Local paper", zotero_item: "ABC123" }], onRemote: (entry) => { imported = entry; } })(fake("See @smi", 8));
assert.equal(remoteCompletion.options[0].detail, "Zotero — import");
remoteCompletion.options[0].apply({}, {}, 5, 8);
assert.equal(imported.zotero_item, "ABC123");

const base = { main: "paper.md", format: "markdown", source: "bibliography: refs.bib\n[@smith2020]", texts: { "refs.bib": "a", "other.bib": "b" } };
assert.equal(bibliographyCacheKey({ ...base, source: "bibliography: refs.bib\nchanged" }), bibliographyCacheKey(base));
assert.equal(bibliographyNeedsAnalysis({ source: "bibliography: missing.bib", format: "markdown", texts: {} }), true);
const cache = new BibliographyCache(4);
let calls = 0;
const analyze = async () => { calls++; return { entries }; };
await cache.get(base, analyze);
await cache.get({ ...base, source: "bibliography: refs.bib\nnew prose" }, analyze);
assert.equal(calls, 1);
for (let i = 0; i < 5; i += 1) {
  await cache.get({ ...base, texts: { ...base.texts, "refs.bib": String(i) } }, analyze);
}
assert.equal(cache.values.size, 4);
console.log("citations: matching, insertion, and bounded cache fixtures passed");

const commentedResource = { main: "paper.typ", format: "typst", texts: { "a.bib": "a", "b.bib": "b" }, source: '\n#bibliography("a.bib")\n' };
assert.notEqual(bibliographyCacheKey(commentedResource), bibliographyCacheKey({ ...commentedResource, source: '/*\n#bibliography("a.bib")\n*/' }), "comment state changes invalidate resource selection");

for (const code of ["``code @Smith", "````\n```\n@Smith"]) assert.equal(citationContext(code, code.length, "markdown"), null);

const zoteroBib = "@article{Smi2023,\n  title = {Old},\n  x-librepaper-zotero-item = {OLD123}\n}\n";
assert.equal(zoteroCitationKey(zoteroBib, "OLD123", "Smi2023"), "Smi2023", "an imported Zotero item reuses its key");
assert.equal(zoteroCitationKey(zoteroBib, "NEW123", "Smi2023"), "Smi2023a", "a new collision is suffixed");
assert.deepEqual(bibliographyRegistration("---\ntitle: Paper\n---\nBody"), { from: 17, to: 17, insert: "bibliography: references.bib\n" });
const planned = planZoteroImport({
  source: "# Paper\n", mainPath: "main.qmd", texts: { "main.qmd": "# Paper\n", "references.bib": zoteroBib },
  item: { zotero_item: "NEW123", citation_key: "Smi2023", bibtex: "@article{Smi2023,\n  title = {New},\n  x-librepaper-zotero-item = {NEW123}\n}\n" },
});
assert.equal(planned.key, "Smi2023a");
assert.match(planned.bibtex, /@article\{Smi2023a,/);
assert.equal(planned.registration.insert, "---\nbibliography: references.bib\n---\n\n");
const repeated = planZoteroImport({ source: "bibliography: references.bib\n", mainPath: "main.qmd", texts: { "references.bib": planned.bibtex }, item: { zotero_item: "NEW123", citation_key: "Smi2023", bibtex: "ignored" } });
assert.equal(repeated.already, true);
assert.equal(repeated.key, "Smi2023a");
assert.equal(repeated.bibtex, planned.bibtex);
const nested = planZoteroImport({ source: "---\nbibliography: ../sources.bib\n---\n", mainPath: "chapters/paper.qmd", texts: { "chapters/paper.qmd": "" }, item: { zotero_item: "NESTED", citation_key: "Jon2024", bibtex: "@article{Jon2024,\n  x-librepaper-zotero-item = {NESTED}\n}\n" } });
assert.equal(nested.path, "sources.bib");
assert.equal(nested.create, true);
assert.equal(nested.registration, null);
