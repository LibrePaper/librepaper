// Clipboards captured from real apps, in tests/fixtures/paste/captured. Each
// <name>.html is converted with the test parser and compared with its goldens
// (<name>.expected.tex, .typ, .md, .org). The invariants at the end hold
// whatever the golden says, so a bad golden cannot pass just by matching a bad
// output.
//
// To write or refresh goldens: UPDATE_PASTE_GOLDENS=1 node --test tests/unit/paste-captured.mjs
// (run from web/), then review the diff before committing.
import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { htmlToSource } from "../../src/lib/paste-html.js";
import { parseHtmlTree } from "./helpers/html-tree.mjs";

const dir = fileURLToPath(new URL("../fixtures/paste/captured/", import.meta.url));
const FORMATS = { latex: "tex", typst: "typ", markdown: "md", org: "org" };
// Enough words to catch dropped paragraphs, few enough to keep the failure readable.
const WORDS_CHECKED = 200;
const updating = process.env.UPDATE_PASTE_GOLDENS === "1";

const names = existsSync(dir)
  ? readdirSync(dir).filter((file) => file.endsWith(".html")).map((file) => file.slice(0, -".html".length)).sort()
  : [];

// Goldens are written with one trailing newline, so only one is ever noise.
const trimOneNewline = (text) => (text.endsWith("\n") ? text.slice(0, -1) : text);
const read = (name, ext) => readFileSync(join(dir, `${name}.${ext}`), "utf8");
// The paste the editor would make: the converted source, or the plain text
// when the converter has nothing to say.
const convert = (name, format) => htmlToSource(parseHtmlTree(read(name, "html")), format) ?? read(name, "txt");

// Without captures the suite stays green, and says why.
if (names.length === 0) {
  test("no captured clipboards yet", (t) => {
    t.diagnostic("no captured clipboards yet");
  });
}

for (const name of names) {
  for (const [format, ext] of Object.entries(FORMATS)) {
    test(`${name} converts to ${format} as its golden`, () => {
      const out = convert(name, format);
      const goldenPath = join(dir, `${name}.expected.${ext}`);
      if (updating) {
        writeFileSync(goldenPath, `${trimOneNewline(out)}\n`);
        return;
      }
      assert.ok(existsSync(goldenPath), `missing golden ${name}.expected.${ext}: run UPDATE_PASTE_GOLDENS=1 node --test tests/unit/paste-captured.mjs from web/, then review the file`);
      assert.equal(trimOneNewline(out), trimOneNewline(read(`${name}.expected`, ext)), `${name} (${format}) differs from its golden`);
    });
  }
}

// The characters inside \textbf{...} groups. Escaped braces are text, not group edges.
function boldChars(latex) {
  let total = 0;
  let at = latex.indexOf("\\textbf{");
  while (at !== -1) {
    const start = at + "\\textbf".length + 1;
    let depth = 1;
    let i = start;
    for (; i < latex.length; i++) {
      const ch = latex[i];
      if (ch === "\\") { i++; continue; }
      if (ch === "{") depth++;
      else if (ch === "}" && --depth === 0) break;
    }
    total += i - start;
    at = latex.indexOf("\\textbf{", i + 1);
  }
  return total;
}

for (const name of names) {
  test(`${name}: invariants hold whatever the golden says`, () => {
    const html = read(name, "html");
    const text = read(name, "txt");
    const tree = () => parseHtmlTree(html);

    // (f) The converter itself produced the output, not the plain-text fallback.
    for (const format of Object.keys(FORMATS)) {
      assert.notEqual(htmlToSource(tree(), format), null, `${name}: the converter returned null for ${format}`);
    }

    // (a) A wrapper must not make everything bold.
    const latex = htmlToSource(tree(), "latex");
    assert.ok(!latex.startsWith("\\textbf{"), `${name}: LaTeX output starts with \\textbf{`);
    assert.ok(boldChars(latex) < latex.length / 2, `${name}: at least half of the LaTeX output is inside \\textbf{}`);

    // (b) No text lost: each long word of the clipboard text survives into the Markdown.
    const markdown = htmlToSource(tree(), "markdown").replaceAll("\\", "");
    const words = [...text.matchAll(/\p{L}{4,}/gu)].map((match) => match[0]).slice(0, WORDS_CHECKED);
    for (const word of words) {
      assert.ok(markdown.includes(word), `${name}: the word "${word}" from the clipboard text is missing from the Markdown`);
    }

    // (c) and (d) No markup or style residue from the source document.
    for (const format of Object.keys(FORMATS)) {
      const out = htmlToSource(tree(), format);
      assert.doesNotMatch(out, /<span|<p |style=/, `${name} (${format}): HTML tag residue in the output`);
      assert.doesNotMatch(out, /mso-|docs-internal-guid|font-family/, `${name} (${format}): style residue in the output`);
    }

    // (e) Characters that LaTeX would read as syntax are escaped.
    if (text.includes("$")) {
      assert.ok(latex.includes("\\$"), `${name}: the text has $ but the LaTeX output has no \\$`);
      assert.doesNotMatch(latex, /(?<!\\)\$/, `${name}: an unescaped $ in the LaTeX output`);
    }
    if (text.includes("~")) {
      assert.doesNotMatch(latex, /(?<!\\)~/, `${name}: a bare ~ in the LaTeX output`);
    }
  });
}
