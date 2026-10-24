import assert from "node:assert/strict";
import { citedKeys, typstLabels, unknownCitations } from "../../src/lib/citation-keys.js";

const keys = (text, format, options) => citedKeys(text, format, options).map((cite) => cite.key);

// LaTeX: offsets of each key in a multi-key cite.
assert.deepEqual(citedKeys("See \\citep{a, b} now.", "latex"), [
  { key: "a", from: 11, to: 12 },
  { key: "b", from: 14, to: 15 },
]);
assert.deepEqual(keys("\\nocite{*}", "tex"), []);
assert.deepEqual(keys("\\Textcite[p.~3]{smith}", "latex"), ["smith"]);
assert.deepEqual(keys("% \\cite{hidden}\n\\cite{shown}", "latex"), ["shown"]);
assert.deepEqual(keys("\\cite{x} % \\cite{y}", "latex"), ["x"]);

// Markdown and Quarto.
assert.deepEqual(keys("See @smith2020 and [@a; @b].", "markdown"), ["smith2020", "a", "b"]);
assert.deepEqual(keys("mail a@b.com", "markdown"), []);
assert.deepEqual(keys("See @fig-plot and @smith", "quarto"), ["smith"]);
assert.deepEqual(keys("Use `@x` here.\n```\n@y\n```\n@z", "markdown"), ["z"]);
assert.deepEqual(keys("---\ntitle: x\nauthor: @jane\n---\n\nSee @smith", "quarto"), ["smith"]);
assert.deepEqual(keys("see @smith.", "markdown"), ["smith"]);
assert.deepEqual(citedKeys("see @smith.", "markdown"), [{ key: "smith", from: 5, to: 10 }]);
assert.deepEqual(keys("@smith", "html"), []);

// Typst, with labels from the project.
const labels = typstLabels({ "a.typ": "= Intro <sec-intro>\n<fig:x>", "b.bib": "<nope>" });
assert.deepEqual([...labels].sort(), ["fig:x", "sec-intro"]);
assert.deepEqual(
  keys("See @smith. #cite(<jones>) and @sec-intro // @ignored\n`@raw` @fine", "typst", { labels }),
  ["smith", "jones", "fine"],
);
// A label used as a citation argument is not a declaration.
const missingSource = "#cite(<missing>)\n@smith";
const missingLabels = typstLabels({ "main.typ": missingSource });
assert.equal(missingLabels.has("missing"), false);
assert.deepEqual(
  unknownCitations({ text: missingSource, format: "typst", entries: [{ key: "smith" }], labels: missingLabels }).map((cite) => cite.key),
  ["missing"],
);
// A heading label still suppresses the matching @key.
assert.deepEqual(keys("= Intro <intro>\n@intro", "typst", { labels: typstLabels({ "a.typ": "= Intro <intro>\n@intro" }) }), []);
// A multiline raw block is masked, so @example does not count.
const fenced = "```\n@example\n```\nSee @smith";
assert.deepEqual(citedKeys(fenced, "typst"), [{ key: "smith", from: fenced.indexOf("@smith") + 1, to: fenced.indexOf("@smith") + 6 }]);

// Unknown citations need a parsed bibliography.
assert.deepEqual(unknownCitations({ text: "@smith @jones", format: "markdown", entries: null }), []);
assert.deepEqual(unknownCitations({ text: "@smith @jones", format: "markdown", entries: undefined }), []);
assert.deepEqual(
  unknownCitations({ text: "@smith @jones", format: "markdown", entries: [{ key: "smith" }] }).map((cite) => cite.key),
  ["jones"],
);
assert.deepEqual(
  unknownCitations({ text: "\\citep{Smith}", format: "latex", entries: [{ key: "smith" }] }).map((cite) => cite.key),
  ["Smith"],
);
assert.deepEqual(citedKeys("= Intro <intro>\n#cite(<intro>) and @intro", "typst", { labels: new Set(["intro"]) }).map((cite) => cite.key), ["intro"]);
