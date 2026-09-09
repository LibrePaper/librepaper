import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { gzipSync } from "node:zlib";
import { call, handOver, load } from "../src/lib/renderer-wasm.js";
import { sourceSelectorFor } from "../src/lib/sync.js";
import { anchorSource } from "../src/lib/anchor.js";

function module(name) {
  const bytes = readFileSync(new URL(`../dist/wasm/${name}.wasm`, import.meta.url));
  console.log(`${name}: ${bytes.length} bytes, ${gzipSync(bytes).length} gzip bytes`);
  return new WebAssembly.Instance(new WebAssembly.Module(bytes), {}).exports;
}
const bibliography = module("bibliography"), citations = module("citations");
const bib = "@article{doe2020, author={Doe, Jane}, title={A useful study}, year={2020}, journal={Research}}";
const source = "---\nbibliography: refs.bib\nbibliography-style: apa\n---\n\n# Paper\n\nThe surrounding words explain the result [@doe2020] and why it matters.\n";
const tree = { main: "chapters/paper.md", texts: { "chapters/paper.md": source, "chapters/refs.bib": bib }, assets: {}, urls: {} };
const analysis = call(bibliography, "bibliography", JSON.stringify({ main: tree.main, format: "markdown", source, texts: tree.texts }));
assert.equal(analysis.ok, true);
const library = JSON.parse(analysis.text);
assert.equal(library.entries[0].key, "doe2020");
assert.equal(library.entries[0].path, "chapters/refs.bib");
assert.deepEqual(library.diagnostics, []);
assert.equal(call(bibliography, "bibliography", "[]").ok, false);
function render(body) {
  handOver(citations, { ...tree, texts: { ...tree.texts, [tree.main]: body } });
  const result = call(citations, "compile", body, "Paper");
  assert.equal(result.ok, true);
  return result;
}
const first = render(source);
assert.match(first.text, /Doe, 2020/, "APA citations include author and year");
assert.match(first.text, /A useful study/);
assert.doesNotMatch(first.text, /bibliography-style|\[@doe2020\]/);
assert.deepEqual(first.diagnostics, []);
const rendered = first.text.replace(/<head>[\s\S]*?<\/head>/, "").replace(/<[^>]*>/g, " ").replace(/\s+/g, " ").trim();
const exact = rendered.match(/The surrounding words.*?matters\./)[0];
const selector = sourceSelectorFor(rendered, { exact, position: rendered.indexOf(exact) }, tree, { formatOf: (path) => path.endsWith(".md") ? "markdown" : "" });
assert.ok(selector, "a sentence spanning a citation maps back to source");
assert.equal(selector.path, tree.main);
assert.ok(source.includes(selector.exact));
const changed = source.replace("apa", "ieee");
assert.notEqual(render(changed).text, first.text);
assert.ok(anchorSource({ ...tree, texts: { ...tree.texts, [tree.main]: changed } }, selector), "comment survives a style change");
tree.texts["chapters/refs.bib"] = bib.replace("A useful study", "A revised study");
assert.match(render(source).text, /A revised study/, "bibliography edits invalidate the prepared library");
tree.texts["chapters/refs.bib"] = bib;
assert.match(render(source).text, /A useful study/);
const unresolved = render(source.replace("@doe2020", "@absent"));
assert.ok(unresolved.diagnostics.some((item) => item.message.includes("absent")));
assert.match(unresolved.text, /absent/);
const large = Array.from({ length: 1000 }, (_, i) => `@article{k${i},author={Doe, Jane},title={Study ${i}},year={2020}}`).join("\n");
const started = performance.now();
const many = call(bibliography, "bibliography", JSON.stringify({ main: "paper.md", format: "markdown", source: "", texts: { "refs.bib": large } }));
assert.equal(JSON.parse(many.text).entries.length, 1000);
console.log(`1000 bibliography entries: ${(performance.now() - started).toFixed(1)}ms; WASM memory ${bibliography.memory.buffer.byteLength} bytes`);
const originalFetch = globalThis.fetch;
let fetches = 0;
globalThis.fetch = async () => {
  if (++fetches <= 2) throw new Error("temporary failure");
  return new Response(readFileSync(new URL("../dist/wasm/bibliography.wasm", import.meta.url)), { headers: { "Content-Type": "application/wasm" } });
};
try {
  await assert.rejects(load("https://example.test/retry.wasm"), /temporary failure/);
  assert.ok((await load("https://example.test/retry.wasm")).bibliography);
} finally { globalThis.fetch = originalFetch; }
console.log("bibliography WASM: parsing, rendering, diagnostics, source anchoring and fetch recovery passed");

// Assert complete citation text, including style punctuation and affixes.
const clusterBib = "@article{smith,author={Smith, Jane},title={First},year={2020}}\n@article{jones,author={Jones, Alex},title={Second},year={2019}}\n@article{brown,author={Brown, Bob},title={Third},year={2022}}";
tree.texts["chapters/refs.bib"] = clusterBib;
const citationText = (body) => render(body).text.match(/<span class="citation">([\s\S]*?)<\/span>/)?.[1].replace(/<[^>]*>/g, "");
assert.equal(citationText("[@smith; @jones; @brown]"), "(Brown, 2022; Jones, 2019; Smith, 2020)");
assert.equal(citationText("[see @smith, pp. 3–4; also @jones, p. 5; compare @brown, p. 9]"), "(compare Brown, 2022, p. 9; also Jones, 2019, p. 5; see Smith, 2020, pp. 3–4)");
assert.equal(citationText("[-@smith]"), "(2020)");
assert.equal(citationText("[-@smith, p. 5]"), "(2020, p. 5)");
assert.equal(citationText("@smith"), "Smith (2020)");
const unicodeMissing = "Before &amp; 😀 @unknown.\n\nAgain @unknown.";
const missingLocations = render(unicodeMissing).diagnostics.filter((item) => item.message.includes("unknown"));
assert.deepEqual(missingLocations.map((item) => [item.line, item.column]), [[1, unicodeMissing.indexOf("@unknown") + 1], [3, 7]]);
console.log("citation text: exact APA groups, sorted affixes, suppress-author locators and Unicode diagnostics passed");
