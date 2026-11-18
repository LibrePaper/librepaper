import assert from "node:assert/strict";
import { BibliographyCache, bibliographyCacheKey, declaredBibliography, bibliographyCompletion, bibliographyNeedsAnalysis, bibliographyRegistration, citationContext, entryLabel, insertCitation, planDoiImport, planZoteroImport, rankEntries, selectedBibliographyFiles, zoteroCitationKey } from "../../src/lib/bibliography.js";
import { Text } from "@codemirror/state";

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
assert.equal(citationContext("See @river data", 15, "markdown"), null, "a citation query is one word");
assert.deepEqual(citationContext("See @river data", 10, "markdown"), { kind: "markdown", from: 5, to: 10, query: "river", trigger: "@" });
assert.deepEqual(rankEntries(entries, "river data").map((entry) => entry.key), ["river+data"]);
assert.equal(citationContext("mail a@smith.example", 18, "markdown"), null);
assert.equal(citationContext(String.fromCharCode(96) + "See @smith" + String.fromCharCode(96), 11, "markdown"), null);
assert.deepEqual(citationContext("See [-@smith]", 11, "markdown"), { kind: "markdown", from: 7, to: 11, query: "smit", trigger: "@" });
assert.deepEqual(citationContext("See \\citep{smith, jo}", 20, "latex"), { kind: "latex", from: 18, to: 20, query: "jo", trigger: "\\citep{" });
assert.equal(citationContext("@smith", 6, "html"), null);
assert.equal(insertCitation("See @smi", 8, "smith2020", "markdown"), "See @smith2020");
assert.equal(insertCitation("See \\citep{smith, jo}", 20, "jones2019", "latex"), "See \\citep{smith, jones2019}");

const fake = (text, pos) => ({ pos, explicit: true, state: { doc: Text.of(text.split("\n")) } });
const completion = bibliographyCompletion({ entries, format: "markdown" })(fake("See @smi", 8));
assert.equal(completion.options[0].label, "smith2020");
assert.equal(completion.options[0].displayLabel.includes("Rivers"), true);
assert.equal(completion.filter, false);
assert.equal(bibliographyCompletion({ entries: [], format: "markdown" })(fake("@", 1)), null);
assert.equal(bibliographyCompletion({ entries, format: "markdown" })(fake("See @smith2020 ", 15)), null);
assert.equal(citationContext("see @smith2020 for details", 26, "markdown"), null, "prose after a citation is not a query");
assert.deepEqual(citationContext("see @smi", 8, "markdown"), { kind: "markdown", from: 5, to: 8, query: "smi", trigger: "@" });
assert.deepEqual(citationContext("[@smith2020; @jo", 16, "markdown"), { kind: "markdown", from: 14, to: 16, query: "jo", trigger: "@" });

const base = { main: "paper.md", format: "markdown", source: "---\nbibliography: refs.bib\n---\n[@smith2020]", texts: { "refs.bib": "a", "other.bib": "b" } };
assert.equal(bibliographyCacheKey({ ...base, source: "---\nbibliography: refs.bib\n---\nchanged" }), bibliographyCacheKey(base));
assert.equal(bibliographyNeedsAnalysis({ source: "---\nbibliography: missing.bib\n---\n", format: "markdown", texts: {} }), true);
const cache = new BibliographyCache(4);
let calls = 0;
const analyze = async () => { calls++; return { entries }; };
await cache.get(base, analyze);
await cache.get({ ...base, source: "---\nbibliography: refs.bib\n---\nnew prose" }, analyze);
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
const repeated = planZoteroImport({ source: "---\nbibliography: references.bib\n---\n", targetPath: "main.qmd", texts: { "references.bib": planned.bibtex }, item: { zotero_item: "NEW123", citation_key: "Smi2023", bibtex: "ignored" } });
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

// A DOI import appends a new entry, reuses an entry with the same DOI, and suffixes a key collision.
const doiEntry = { doi: "10.1038/nature12373", key: "Kuc2013", bibtex: "@article{Kuc2013,\n  title = {T},\n  doi = {10.1038/nature12373}\n}\n" };
const doiAdded = planDoiImport({ source: "---\nbibliography: refs.bib\n---\n", targetPath: "main.qmd", texts: { "refs.bib": "@book{Old2020,\n  title = {Old}\n}\n" }, entry: doiEntry });
assert.equal(doiAdded.key, "Kuc2013");
assert.equal(doiAdded.already, false);
assert.equal(doiAdded.path, "refs.bib");
assert.equal(doiAdded.addition, "\n" + doiEntry.bibtex);
assert.equal(doiAdded.registration, null);
const doiAgain = planDoiImport({ source: "---\nbibliography: refs.bib\n---\n", targetPath: "main.qmd", texts: { "refs.bib": "@article{Mine,\n  doi = {10.1038/NATURE12373}\n}\n" }, entry: doiEntry });
assert.equal(doiAgain.already, true);
assert.equal(doiAgain.key, "Mine");
assert.equal(doiAgain.addition, "");
const doiClash = planDoiImport({ source: "---\nbibliography: refs.bib\n---\n", targetPath: "main.qmd", texts: { "refs.bib": "@book{kuc2013,\n  doi = {10.1/other}\n}\n" }, entry: doiEntry });
assert.equal(doiClash.key, "Kuc2013a");
assert.match(doiClash.addition, /^\n@article\{Kuc2013a,/);
const doiRegistered = planDoiImport({ source: "# Notes\n", targetPath: "notes.md", texts: {}, entry: doiEntry });
assert.equal(doiRegistered.create, true);
assert.equal(doiRegistered.path, "references.bib");
assert.equal(doiRegistered.registration.insert, "---\nbibliography: references.bib\n---\n\n");

const doiLinked = planDoiImport({ source: "---\nbibliography: refs.bib\n---\n", targetPath: "main.qmd", texts: { "refs.bib": "@article{Linked,\n  doi = {https://doi.org/10.1038/Nature12373}\n}\n" }, entry: doiEntry });
assert.equal(doiLinked.already, true);
assert.equal(doiLinked.key, "Linked");

// A CSL-JSON or YAML bibliography is never appended to.
for (const refs of ["refs.json", "refs.yaml"]) {
  const source = `---\nbibliography: ${refs}\n---\n`, texts = { [refs]: "[]" };
  assert.throws(() => planZoteroImport({ source, targetPath: "main.qmd", texts, item: { zotero_item: "Z1", citation_key: "Doe2020", bibtex: "@article{Doe2020,\n title={T}}\n" } }), { code: "bibliography_not_bibtex" });
  assert.throws(() => planDoiImport({ source, targetPath: "main.qmd", texts, entry: doiEntry }), { code: "bibliography_not_bibtex" });
}

// A DOI is reused only from a .bib the document loads; every project .bib counts for collisions.
{
  const texts = { "refs.bib": "@book{Old2020,\n  title = {Old}\n}\n", "other.bib": "@article{Elsewhere,\n  doi = {10.1038/nature12373}\n}\n@book{Kuc2013,\n  title = {X}\n}\n" };
  const skipped = planDoiImport({ source: "---\nbibliography: refs.bib\n---\n", targetPath: "main.qmd", texts, entry: doiEntry });
  assert.equal(skipped.already, false);
  assert.equal(skipped.key, "Kuc2013a");
  const both = planDoiImport({ source: "---\nbibliography: [refs.bib, other.bib]\n---\n", targetPath: "main.qmd", texts, entry: doiEntry });
  assert.equal(both.already, true);
  assert.equal(both.key, "Elsewhere");
  const clash = planDoiImport({ source: "---\nbibliography: refs.bib\n---\n", targetPath: "main.qmd", texts: { "refs.bib": "", "other.bib": "@book{Kuc2013,\n  doi = {10.1/z}\n}\n" }, entry: doiEntry });
  assert.equal(clash.key, "Kuc2013a");
}

// x-doi and a commented doi are not duplicates.
for (const bib of ["@article{A,\n  x-doi = {10.1038/nature12373}\n}\n", "@article{A,\n  title = {T}\n}\n  % doi = {10.1038/nature12373}\n"]) {
  const plan = planDoiImport({ source: "---\nbibliography: refs.bib\n---\n", targetPath: "main.qmd", texts: { "refs.bib": bib }, entry: doiEntry });
  assert.equal(plan.already, false);
}

// The bibliography scanner keeps every item of a block list and reads Typst sources of any type (#115).
{
  const md = (body) => selectedBibliographyFiles(`---\n${body}\n---\n`, "markdown");
  assert.deepEqual(md("bibliography:\n  - refs.json"), ["refs.json"]);
  assert.deepEqual(md("bibliography:\n  - refs.json\n  - refs.bib"), ["refs.bib", "refs.json"]);
  assert.deepEqual(md("bibliography:\n  - refs.json\n  - refs.bib\ntitle: T"), ["refs.bib", "refs.json"]);
  assert.deepEqual(md("bibliography:\n  - \"a.bib\"\n  - 'b.json'\ncsl: x.csl"), ["a.bib", "b.json"]);
  assert.deepEqual(md("bibliography: refs.json"), ["refs.json"]);
  assert.deepEqual(md("bibliography: [a.bib, b.json]"), ["a.bib", "b.json"]);
  assert.deepEqual(md("bibliography: refs.bib"), ["refs.bib"]);
  const typ = (call) => selectedBibliographyFiles(call, "typst");
  assert.deepEqual(typ('#bibliography("refs.yaml")'), ["refs.yaml"]);
  assert.deepEqual(typ('#bibliography("refs.bib")'), ["refs.bib"]);
  assert.deepEqual(typ('#bibliography("refs.yaml", style: "apa")'), ["refs.yaml"]);
  assert.deepEqual(typ('#bibliography(("a.bib", "b.yml"), title: "Works", style: "my.csl")'), ["a.bib", "b.yml"]);
  const item = { zotero_item: "Z1", citation_key: "Doe2020", bibtex: "@article{Doe2020,\n title={T}}\n" };
  const listed = planZoteroImport({ source: "---\nbibliography:\n  - refs.json\n  - refs.bib\n---\n", targetPath: "main.qmd", texts: { "refs.bib": "" }, item });
  assert.equal(listed.path, "refs.bib");
  assert.equal(listed.registration, null);
  assert.throws(() => planZoteroImport({ source: "---\nbibliography:\n  - refs.json\n---\n", targetPath: "main.qmd", texts: {}, item }), { code: "bibliography_not_bibtex" });
  assert.throws(() => planDoiImport({ source: '#bibliography("refs.yaml")\n', targetPath: "main.typ", texts: {}, entry: doiEntry, format: "typst" }), { code: "bibliography_not_bibtex" });
}

// Only the leading front matter declares a bibliography: a "Bibliography:" line
// and bullet list in the body are prose, not files.
assert.deepEqual(selectedBibliographyFiles("---\ntitle: Notes\n---\n\nBibliography:\n\n- Smith 2020, Rivers\n- Jones 2021\n", "markdown"), []);
assert.deepEqual(selectedBibliographyFiles("No front matter\nbibliography: refs.bib\n", "markdown"), []);
assert.deepEqual(selectedBibliographyFiles("---\nbibliography: refs.bib\n...\nBody\n", "quarto"), ["refs.bib"]);

// One resolver finds the declared bibliography: chapter, then entry file, then _quarto.yml (Quarto only).
{
  const fm = (files) => `---\nbibliography: ${files}\n---\n\nBody\n`;
  const own = declaredBibliography({ format: "quarto", chapterPath: "ch.qmd", chapterText: fm("mine.bib"), entryPath: "index.qmd", entryText: fm("entry.bib"), texts: { "_quarto.yml": "bibliography: q.bib\n" } });
  assert.deepEqual(own, { files: ["mine.bib"], owner: "chapter", ownerPath: "ch.qmd" });
  const viaEntry = declaredBibliography({ format: "quarto", chapterPath: "ch/one.qmd", chapterText: "# One\n", entryPath: "book/index.qmd", entryText: fm("refs.bib"), texts: {} });
  assert.deepEqual(viaEntry, { files: ["book/refs.bib"], owner: "entry", ownerPath: "book/index.qmd" });
  const viaQuarto = declaredBibliography({ format: "quarto", chapterPath: "ch.qmd", chapterText: "# One\n", entryPath: "index.qmd", entryText: "# Home\n", texts: { "_quarto.yml": "project:\n  type: book\nbibliography: refs.bib\n" } });
  assert.deepEqual(viaQuarto, { files: ["refs.bib"], owner: "quarto", ownerPath: "_quarto.yml" });
  const markdownIgnores = declaredBibliography({ format: "markdown", chapterPath: "ch.md", chapterText: "# One\n", entryPath: "index.md", entryText: "# Home\n", texts: { "_quarto.yml": "bibliography: refs.bib\n" } });
  assert.deepEqual(markdownIgnores, { files: [], owner: null, ownerPath: null });
  assert.equal(declaredBibliography({ format: "quarto", chapterPath: "ch.qmd", chapterText: "# One\n", texts: {} }).owner, null);
  const nested = declaredBibliography({ format: "quarto", chapterPath: "sub/deep/ch.qmd", chapterText: "# One\n", texts: { "_quarto.yml": "bibliography: root.bib\n", "sub/_quarto.yml": "bibliography: sub.bib\n" } });
  assert.deepEqual(nested, { files: ["sub/sub.bib"], owner: "quarto", ownerPath: "sub/_quarto.yml" });
}

// A DOI import into a chapter without its own bibliography follows the entry file or _quarto.yml.
{
  const fm = "---\nbibliography: refs.bib\n---\n";
  const texts = { "refs.bib": "@book{Old2020,\n  title = {Old}\n}\n", "other.bib": "" };
  const viaEntry = planDoiImport({ source: fm, targetPath: "index.qmd", texts, entry: doiEntry, format: "quarto", chapterPath: "ch.qmd", chapterText: "# One\n", entryPath: "index.qmd", entryText: fm });
  assert.equal(viaEntry.path, "refs.bib");
  assert.equal(viaEntry.registration, null);
  const viaQuarto = planDoiImport({ source: "# Home\n", targetPath: "index.qmd", texts: { ...texts, "_quarto.yml": "bibliography: refs.bib\n" }, entry: doiEntry, format: "quarto", chapterPath: "ch.qmd", chapterText: "# One\n", entryPath: "index.qmd", entryText: "# Home\n" });
  assert.equal(viaQuarto.path, "refs.bib");
  assert.equal(viaQuarto.registration, null);
  const nothing = planDoiImport({ source: "# Home\n", targetPath: "index.qmd", texts: {}, entry: doiEntry, format: "quarto", chapterPath: "ch.qmd", chapterText: "# One\n", entryPath: "index.qmd", entryText: "# Home\n" });
  assert.equal(nothing.path, "references.bib");
  assert.notEqual(nothing.registration, null);
}
