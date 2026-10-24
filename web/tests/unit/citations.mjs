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
  source: "# Paper\n", targetPath: "main.qmd", texts: { "main.qmd": "# Paper\n", "references.bib": zoteroBib },
  item: { zotero_item: "NEW123", citation_key: "Smi2023", bibtex: "@article{Smi2023,\n  title = {New},\n  x-librepaper-zotero-item = {NEW123}\n}\n" },
});
assert.equal(planned.key, "Smi2023a");
assert.match(planned.bibtex, /@article\{Smi2023a,/);
assert.equal(planned.registration.insert, "---\nbibliography: references.bib\n---\n\n");
const repeated = planZoteroImport({ source: "bibliography: references.bib\n", targetPath: "main.qmd", texts: { "references.bib": planned.bibtex }, item: { zotero_item: "NEW123", citation_key: "Smi2023", bibtex: "ignored" } });
assert.equal(repeated.already, true);
assert.equal(repeated.key, "Smi2023a");
assert.equal(repeated.bibtex, planned.bibtex);
const nested = planZoteroImport({ source: "---\nbibliography: ../sources.bib\n---\n", targetPath: "chapters/paper.qmd", texts: { "chapters/paper.qmd": "" }, item: { zotero_item: "NESTED", citation_key: "Jon2024", bibtex: "@article{Jon2024,\n  x-librepaper-zotero-item = {NESTED}\n}\n" } });
assert.equal(nested.path, "sources.bib");
assert.equal(nested.create, true);
assert.equal(nested.registration, null);
const markdownTarget = planZoteroImport({ source: "# Notes\n", targetPath: "notes.md", texts: { "references.bib": "" }, item: { zotero_item: "MD1", citation_key: "Doe2025", bibtex: "@article{Doe2025,\n  x-librepaper-zotero-item = {MD1}\n}\n" } });
assert.deepEqual(markdownTarget.registration, { from: 0, to: 0, insert: "---\nbibliography: references.bib\n---\n\n" });
const subfolderTarget = planZoteroImport({ source: "# Chapter\n", targetPath: "chapters/one.md", texts: { "references.bib": "" }, item: { zotero_item: "MD2", citation_key: "Doe2025", bibtex: "@article{Doe2025,\n  x-librepaper-zotero-item = {MD2}\n}\n" } });
assert.equal(subfolderTarget.registration.insert, "---\nbibliography: ../references.bib\n---\n\n");
const biblatexSource = "\\documentclass{article}\n\\usepackage[style=authoryear]{biblatex}\n\\begin{document}\nHi\n\\end{document}\n";
assert.deepEqual(bibliographyRegistration(biblatexSource, "refs.bib", "latex"), { from: biblatexSource.indexOf("\\begin{document}"), to: biblatexSource.indexOf("\\begin{document}"), insert: "\\addbibresource{refs.bib}\n" });
assert.equal(bibliographyRegistration("\\usepackage{foo,biblatex}\n", "refs.bib", "latex").insert, "\\addbibresource{refs.bib}\n");
const bibtexSource = "\\begin{document}\nHi\n\\end{document}\n";
assert.deepEqual(bibliographyRegistration(bibtexSource, "refs.bib", "latex"), { from: bibtexSource.indexOf("\\end{document}"), to: bibtexSource.indexOf("\\end{document}"), insert: "\\bibliographystyle{plain}\n\\bibliography{refs}\n" });
const styledSource = "\\bibliographystyle{alpha}\n\\begin{document}\n\\end{document}\n";
assert.equal(bibliographyRegistration(styledSource, "refs.bib", "latex").insert, "\\bibliography{refs}\n");
assert.deepEqual(bibliographyRegistration("Hello\n", "refs.bib", "latex"), { from: 6, to: 6, insert: "\\bibliographystyle{plain}\n\\bibliography{refs}\n" });
assert.deepEqual(bibliographyRegistration("Hello", "refs.bib", "tex"), { from: 5, to: 5, insert: "\n\\bibliographystyle{plain}\n\\bibliography{refs}\n" });
assert.equal(bibliographyRegistration("\\usepackage{biblatex}\n\\addbibresource{refs.bib}\n", "refs.bib", "latex"), null);
const natbibSource = "\\usepackage[numbers]{natbib}\n\\begin{document}\n\\end{document}\n";
assert.equal(bibliographyRegistration(natbibSource, "refs.bib", "latex").insert, "\\bibliographystyle{plainnat}\n\\bibliography{refs}\n");
const commentedBiblatex = "% \\usepackage{biblatex}\n\\begin{document}\n\\end{document}\n";
assert.deepEqual(bibliographyRegistration(commentedBiblatex, "refs.bib", "latex"), { from: commentedBiblatex.indexOf("\\end{document}"), to: commentedBiblatex.indexOf("\\end{document}"), insert: "\\bibliographystyle{plain}\n\\bibliography{refs}\n" });
const commentedStyle = "% \\bibliographystyle{abbrv}\n\\begin{document}\n\\end{document}\n";
assert.equal(bibliographyRegistration(commentedStyle, "refs.bib", "latex").insert, "\\bibliographystyle{plain}\n\\bibliography{refs}\n");
assert.notEqual(bibliographyRegistration("\\begin{document}\n% \\bibliography{old}\n\\end{document}\n", "refs.bib", "latex"), null);
assert.equal(bibliographyRegistration("\\begin{document}\n\\bibliography{refs}\n\\end{document}\n", "refs.bib", "latex"), null);
const latexPlanned = planZoteroImport({
  source: "\\bibliography{refs}\n\\end{document}\n", targetPath: "main.tex", texts: { "main.tex": "", "refs.bib": "" }, format: "latex",
  item: { zotero_item: "L1", citation_key: "Doe2025", bibtex: "@article{Doe2025,\n  x-librepaper-zotero-item = {L1}\n}\n" },
});
assert.equal(latexPlanned.path, "refs.bib");
assert.equal(latexPlanned.registration, null);
assert.deepEqual(bibliographyRegistration("= Paper\n", "refs.bib", "typst"), { from: 8, to: 8, insert: '#bibliography("refs.bib")\n' });
assert.deepEqual(bibliographyRegistration("= Paper", "refs.bib", "typst"), { from: 7, to: 7, insert: '\n#bibliography("refs.bib")\n' });
assert.equal(bibliographyRegistration('#bibliography("refs.bib")\n', "refs.bib", "typst"), null);
{
  const crlf = "\\usepackage{biblatex}\r\n\\begin{document}\r\n";
  const at = bibliographyRegistration(crlf, "refs.bib", "latex");
  assert.equal(crlf.slice(0, at.from) + at.insert + crlf.slice(at.to), "\\usepackage{biblatex}\r\n\\addbibresource{refs.bib}\r\n\\begin{document}\r\n");
}
assert.notEqual(planZoteroImport({ source: "% \\bibliography{old}\n\\begin{document}\n\\end{document}\n", targetPath: "paper.tex", texts: {}, item: { zotero_item: "Z1", citation_key: "Doe2020", bibtex: "@article{Doe2020,\n title={T}}\n" }, format: "latex" }).registration, null);
