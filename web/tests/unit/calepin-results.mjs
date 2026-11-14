import assert from "node:assert/strict";
import test from "node:test";
import {
  DISABLED_ACTIVE,
  chunksOf,
  isCalepinRecord,
  overlay,
  rawLangs,
  recordOf,
  runtimeFiles,
  staleChunks,
  usesCalepin,
} from "../../src/lib/calepin-results.js";

const RESULTS = ".calepin/paper/results.json";
const b64 = (bytes) => Buffer.from(bytes).toString("base64");

test("usesCalepin finds an import with or without the leading slash", () => {
  assert.equal(usesCalepin({ "paper.typ": '#import "/.calepin/calepin.typ" as calepin' }), true);
  assert.equal(usesCalepin({ "paper.typ": '#import ".calepin/calepin.typ" as calepin' }), true);
});

test("usesCalepin ignores non-Typst files, plain Typst and missing input", () => {
  assert.equal(usesCalepin({ "notes.md": '#import "/.calepin/calepin.typ"' }), false);
  assert.equal(usesCalepin({ "paper.typ": '#import "@preview/cetz:0.2.0"' }), false);
  assert.equal(usesCalepin(null), false);
  assert.equal(usesCalepin(undefined), false);
});

test("isCalepinRecord checks the engine and the files array", () => {
  assert.equal(isCalepinRecord({ engine: "calepin", files: [] }), true);
  assert.equal(isCalepinRecord({ engine: "calepin", files: "no" }), false);
  assert.equal(isCalepinRecord({ engine: "quarto", files: [] }), false);
  assert.equal(isCalepinRecord(null), false);
});

test("recordOf reads good bytes and a good string", () => {
  const body = {
    version: 1,
    files: [
      { path: RESULTS, text: "{}" },
      { path: ".calepin/paper/fig.png", base64: "AAAA" },
    ],
  };
  const expected = { version: 1, path: "paper.typ", engine: "calepin", files: body.files };
  assert.deepEqual(recordOf(new TextEncoder().encode(JSON.stringify(body)), "paper.typ"), expected);
  assert.deepEqual(recordOf(JSON.stringify(body), "paper.typ"), expected);
});

test("recordOf drops properties it does not keep", () => {
  const body = {
    version: 1,
    files: [{ path: RESULTS, text: "{}", extra: "x" }],
  };
  assert.deepEqual(recordOf(JSON.stringify(body), "paper.typ").files, [{ path: RESULTS, text: "{}" }]);
});

test("recordOf returns null for bad JSON, a wrong version and missing input", () => {
  assert.equal(recordOf("{", "paper.typ"), null);
  assert.equal(recordOf(null, "paper.typ"), null);
  assert.equal(recordOf(undefined, "paper.typ"), null);
  assert.equal(recordOf(JSON.stringify({ version: 2, files: [{ path: RESULTS, text: "{}" }] }), "paper.typ"), null);
});

test("recordOf rejects paths outside .calepin/ and other unsafe paths", () => {
  const withResults = (extra) => JSON.stringify({ version: 1, files: [{ path: RESULTS, text: "{}" }, extra] });
  assert.equal(recordOf(withResults({ path: "../x", text: "a" }), "paper.typ"), null);
  assert.equal(recordOf(withResults({ path: "main.typ", text: "a" }), "paper.typ"), null);
  assert.equal(recordOf(withResults({ path: ".calepin/a\\b", text: "a" }), "paper.typ"), null);
  assert.equal(recordOf(withResults({ path: ".calepin/a/./b", text: "a" }), "paper.typ"), null);
});

test("recordOf requires exactly one of text and base64 per entry", () => {
  const both = JSON.stringify({ version: 1, files: [{ path: RESULTS, text: "{}", base64: "AA==" }] });
  const neither = JSON.stringify({ version: 1, files: [{ path: RESULTS }] });
  assert.equal(recordOf(both, "paper.typ"), null);
  assert.equal(recordOf(neither, "paper.typ"), null);
});

test("recordOf requires a results.json entry", () => {
  const body = JSON.stringify({ version: 1, files: [{ path: ".calepin/paper/fig.png", base64: "AAAA" }] });
  assert.equal(recordOf(body, "paper.typ"), null);
});

test("overlay places text and base64 entries and keeps the originals", () => {
  const tree = { texts: { "main.typ": "= Hi" }, assets: { "a.png": new Uint8Array([1]) } };
  const before = JSON.stringify({ texts: tree.texts, assets: { "a.png": [1] } });
  const result = overlay(tree, [
    { path: ".calepin/calepin.typ", text: "X" },
    { path: ".calepin/paper/fig.png", base64: b64([0, 255]) },
  ]);
  assert.equal(result.texts[".calepin/calepin.typ"], "X");
  assert.deepEqual(result.assets[".calepin/paper/fig.png"], new Uint8Array([0, 255]));
  assert.equal(result.texts["main.typ"], "= Hi");
  assert.deepEqual(result.assets["a.png"], new Uint8Array([1]));
  assert.notEqual(result.texts, tree.texts);
  assert.equal(JSON.stringify({ texts: tree.texts, assets: { "a.png": [1] } }), before);
  assert.equal(tree.texts[".calepin/calepin.typ"], undefined);
  assert.equal(tree.assets[".calepin/paper/fig.png"], undefined);
});

test("overlay never replaces a project file at the same path", () => {
  const tree = { texts: { "main.typ": "= Hi" }, assets: {} };
  const result = overlay(tree, [{ path: "main.typ", text: "overlay" }]);
  assert.equal(result.texts["main.typ"], "= Hi");
});

test("overlay with disabled replaces active.typ", () => {
  const tree = { texts: { ".calepin/active.typ": "#let config = (enabled: true)\n" }, assets: {} };
  const files = [{ path: ".calepin/active.typ", text: "old" }];
  const result = overlay(tree, files, { disabled: true });
  assert.equal(result.texts[".calepin/active.typ"], DISABLED_ACTIVE);
  assert.equal(tree.texts[".calepin/active.typ"], "#let config = (enabled: true)\n");
  assert.equal(overlay({ texts: {}, assets: {} }, [], { disabled: true }).texts[".calepin/active.typ"], DISABLED_ACTIVE);
});

test("chunksOf returns the python blocks in order", () => {
  const source = [
    "Intro",
    "```python",
    "print(40 + 2)",
    "```",
    "",
    "```r",
    "x <- 1",
    "```",
    "```python",
    "print(1 + 1)",
    "```",
  ].join("\n");
  assert.deepEqual(chunksOf(source, ["python", "r"]), [
    { lang: "python", source: "print(40 + 2)" },
    { lang: "r", source: "x <- 1" },
    { lang: "python", source: "print(1 + 1)" },
  ]);
});

test("chunksOf skips a language that is not run", () => {
  const source = "```js\nlet a = 1\n```\n```python\nprint(1)\n```";
  assert.deepEqual(chunksOf(source, ["python"]), [{ lang: "python", source: "print(1)" }]);
});

test("chunksOf skips a four-backtick example whole, python fence inside included", () => {
  const source = "````typ\n```python\nprint(9)\n```\n````\n";
  assert.deepEqual(chunksOf(source, ["python"]), []);
});

test("chunksOf removes the common indentation", () => {
  const source = "  ```python\n  a = 1\n    b = 2\n  ```";
  assert.deepEqual(chunksOf(source, ["python"]), [{ lang: "python", source: "a = 1\n  b = 2" }]);
});

test("chunksOf ends the scan at an unclosed fence", () => {
  const source = "```python\nprint(1)\n```\n```python\nprint(2)\n";
  assert.deepEqual(chunksOf(source, ["python"]), [{ lang: "python", source: "print(1)" }]);
});

test("rawLangs reads the raw-langs list from runtime-config.typ", () => {
  const files = [
    {
      path: ".calepin/paper/runtime-config.typ",
      text: '#let config = (\n  "raw-langs": ("d2", "dot", "mermaid"),\n)\n',
    },
  ];
  assert.deepEqual(rawLangs(files, { path: "paper.typ" }), ["d2", "dot", "mermaid"]);
});

test("rawLangs falls back to the default list", () => {
  assert.deepEqual(rawLangs([], { path: "paper.typ" }), ["python", "r", "mermaid", "dot", "tikz", "d2"]);
});

test("rawLangs finds the config of a document in a subfolder", () => {
  const files = [
    {
      path: ".calepin/chapters/intro/runtime-config.typ",
      text: '"raw-langs": ("julia",),',
    },
  ];
  assert.deepEqual(rawLangs(files, { path: "chapters/intro.typ" }), ["julia"]);
});

const PAPER = [
  {
    path: RESULTS,
    text: JSON.stringify({
      schema: 3,
      chunks: {
        "chunk-1": { label: "chunk-1", engine: "python", source: "print(40 + 2)\n" },
        "inline-1": { label: "inline-1", engine: "python", source: "print(1 + 1)" },
      },
    }),
  },
  { path: ".calepin/paper/runtime-config.typ", text: '"raw-langs": ("python",),' },
];

const block = (code) => "```python\n" + code + "\n```";

test("staleChunks counts nothing when every chunk has a stored result", () => {
  const source = [block("print(40 + 2)"), block("print(1 + 1)")].join("\n");
  assert.equal(staleChunks(PAPER, source, { path: "paper.typ" }), 0);
});

test("staleChunks counts one edited chunk", () => {
  const source = [block("print(40 + 3)"), block("print(1 + 1)")].join("\n");
  assert.equal(staleChunks(PAPER, source, { path: "paper.typ" }), 1);
});

test("staleChunks counts one added chunk", () => {
  const source = [block("print(40 + 2)"), block("print(1 + 1)"), block("print(7)")].join("\n");
  assert.equal(staleChunks(PAPER, source, { path: "paper.typ" }), 1);
});

test("staleChunks counts duplicated code once per stored copy", () => {
  const files = [
    {
      path: RESULTS,
      text: JSON.stringify({
        chunks: {
          a: { source: "print(1)" },
          b: { source: "print(1)\n" },
        },
      }),
    },
  ];
  const source = [block("print(1)"), block("print(1)"), block("print(1)")].join("\n");
  assert.equal(staleChunks(files, source, { path: "paper.typ" }), 1);
});

test("staleChunks treats every chunk as stale when results.json is unreadable", () => {
  const files = [{ path: RESULTS, text: "{not json" }];
  const source = [block("print(40 + 2)"), block("print(1 + 1)")].join("\n");
  assert.equal(staleChunks(files, source, { path: "paper.typ" }), 2);
  assert.equal(staleChunks([], source, { path: "paper.typ" }), 2);
});

test("runtimeFiles ends with the disabled active.typ and includes the runtime", async () => {
  const files = await runtimeFiles();
  assert.ok(files.some((file) => file.path === ".calepin/calepin.typ"));
  assert.equal(files.filter((file) => file.path === ".calepin/active.typ").length, 1);
  assert.deepEqual(files.at(-1), { path: ".calepin/active.typ", text: DISABLED_ACTIVE });
});
