import assert from "node:assert/strict";
import test from "node:test";
import { referencedFigures } from "../../src/lib/figure-references.js";
import { fillFigures, referenceFigures, placeholderBytes, placeholderUrl, resultsFigureUrl } from "../../src/lib/missing-figures.js";

const starts = (bytes, ...head) => head.every((byte, i) => bytes[i] === byte);
const ascii = (bytes) => new TextDecoder().decode(bytes);
const base64 = (text) => Buffer.from(text).toString("base64");

test("placeholder bytes match the extension's format", () => {
  assert.ok(starts(placeholderBytes("a.png"), 0x89, 0x50, 0x4e, 0x47));
  assert.ok(starts(placeholderBytes("a.JPG"), 0xff, 0xd8, 0xff));
  assert.ok(starts(placeholderBytes("a.jpeg"), 0xff, 0xd8, 0xff));
  assert.ok(starts(placeholderBytes("a.gif"), 0x47, 0x49, 0x46, 0x38));
  const webp = placeholderBytes("a.webp");
  assert.equal(ascii(webp.slice(0, 4)), "RIFF");
  assert.equal(ascii(webp.slice(8, 12)), "WEBP");
  assert.ok(starts(placeholderBytes("a.pdf"), 0x25, 0x50, 0x44, 0x46));
  assert.match(ascii(placeholderBytes("a.pdf")), /%%EOF\n$/);
  assert.match(ascii(placeholderBytes("dir/a.svg")), /^<svg [^>]*>[\s\S]*a\.svg[\s\S]*<\/svg>$/);
  assert.ok(starts(placeholderBytes("noext"), 0x89, 0x50, 0x4e, 0x47));
});

test("the PDF placeholder's cross-reference offsets point at their objects", () => {
  const text = ascii(placeholderBytes("a.pdf"));
  const table = text.slice(text.indexOf("xref"));
  const offsets = [...table.matchAll(/(\d{10}) 00000 n/g)].map((match) => Number(match[1]));
  offsets.forEach((offset, i) => assert.ok(text.startsWith(`${i + 1} 0 obj`, offset)));
  assert.equal(text.slice(Number(/startxref\n(\d+)/.exec(text)[1])).startsWith("xref"), true);
});

test("the placeholder URL is an SVG data URL naming the file, escaped", () => {
  const url = placeholderUrl("figs/a&b.png");
  assert.ok(url.startsWith("data:image/svg+xml"));
  const svg = decodeURIComponent(url.slice(url.indexOf(",") + 1));
  assert.match(svg, /a&amp;b\.png/);
});

test("results figures are read from base64 entries outside .calepin", () => {
  const files = [{ path: "fig.png", base64: base64("png") }, { path: ".calepin/x.json", text: "{}" }, { path: "t.txt", text: "x" }];
  assert.equal(resultsFigureUrl(files, "fig.png"), `data:image/png;base64,${base64("png")}`);
  assert.equal(resultsFigureUrl(files, "./fig.png"), `data:image/png;base64,${base64("png")}`);
  assert.equal(resultsFigureUrl(files, "t.txt"), null);
  assert.equal(resultsFigureUrl(null, "fig.png"), null);
});

test("fillFigures: project wins over the run, the run over a placeholder", () => {
  const projectBytes = new Uint8Array([1, 2, 3]);
  const source = {
    main: "main.typ",
    texts: { "main.typ": '#image("a.png")\n#image("b.png")\n#image("c.png")\n' },
    assets: { "a.png": projectBytes },
    digests: { "a.png": "sha" },
    urls: {},
  };
  const resultsFiles = [{ path: "a.png", base64: base64("run-a") }, { path: "b.png", base64: base64("run-b") }];
  const { missing } = fillFigures(source, { references: referencedFigures(source), resultsFiles });
  assert.equal(source.assets["a.png"], projectBytes);
  assert.equal(ascii(source.assets["b.png"]), "run-b");
  assert.ok(starts(source.assets["c.png"], 0x89, 0x50));
  assert.deepEqual(missing, [{ path: "c.png", name: "c.png", file: "main.typ", line: 3 }]);
  assert.ok(source.urls["c.png"].startsWith("data:image/svg+xml"));
});

test("fillFigures: an unfetchable project blob gets a placeholder and is not reported missing", () => {
  const source = { main: "m.typ", texts: { "m.typ": '#image("a.png")' }, assets: {}, digests: { "a.png": "sha" }, urls: {} };
  const { missing } = fillFigures(source, { references: referencedFigures(source) });
  assert.deepEqual(missing, []);
  assert.ok(starts(source.assets["a.png"], 0x89, 0x50));
});

test("fillFigures: flow formats also answer to the name as written", () => {
  const bytes = new Uint8Array([9]);
  const source = {
    main: "doc/main.qmd",
    texts: { "doc/main.qmd": "![a](fig.png)\n![b](gone.png)\n" },
    assets: { "doc/fig.png": bytes },
    digests: { "doc/fig.png": "sha" },
    urls: { "doc/fig.png": "data:image/png;base64,AA==" },
  };
  const { missing } = fillFigures(source, { references: referencedFigures(source) });
  assert.equal(source.assets["fig.png"], bytes);
  assert.equal(source.urls["fig.png"], "data:image/png;base64,AA==");
  assert.deepEqual(missing.map(({ path }) => path), ["doc/gone.png"]);
  assert.ok(source.urls["gone.png"].startsWith("data:image/svg+xml"));
  assert.ok(source.assets["gone.png"]);
});

test("fillFigures: a figure filled earlier does not hide a later miss", () => {
  const source = { main: "m.typ", texts: { "m.typ": '#image("a.png")' }, urls: {} };
  const references = referencedFigures(source);
  const withRun = fillFigures(source, { references, resultsFiles: [{ path: "a.png", base64: base64("x") }] });
  assert.deepEqual(withRun.missing, []);
  const without = fillFigures(source, { references });
  assert.equal(without.missing.length, 1);
});

test("fillFigures: LaTeX gives a bare missing name an extension to find", () => {
  const source = { main: "m.tex", texts: { "m.tex": "\\includegraphics{gone}" }, urls: {} };
  const { missing } = fillFigures(source, { references: referencedFigures(source) });
  assert.deepEqual(missing.map(({ path }) => path), ["gone"]);
  assert.ok(source.assets["gone.png"]);
});

test("referenceFigures: run figures become references, SVG placeholders stay inline", async () => {
  const source = {
    main: "doc/main.qmd",
    texts: { "doc/main.qmd": "![a](a.png)\n![b](gone.png)\n" },
    urls: {},
  };
  fillFigures(source, { references: referencedFigures(source), resultsFiles: [{ path: "doc/a.png", base64: base64("run-a") }] });
  const references = await referenceFigures(source);
  assert.equal(references.length, 1);
  assert.equal(references[0].type, "image/png");
  assert.equal(ascii(references[0].bytes), "run-a");
  assert.equal(source.urls["doc/a.png"], `about:blank#librepaper-asset=${references[0].sha}`);
  assert.equal(source.urls["a.png"], source.urls["doc/a.png"]);
  assert.ok(source.urls["doc/gone.png"].startsWith("data:image/svg+xml"));
});
