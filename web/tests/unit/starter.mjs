// What a name becomes in each format, and which templates a search finds.
// fillTemplate escapes for the one kind of place a format lets a placeholder
// stand; matchTemplates is the picker's filter.
import assert from "node:assert/strict";
import { fillTemplate, matchTemplates } from "../../src/lib/starter.js";

const title = 'A & B "C" \\ $x_1$ #1 *bold*';
const fill = (formatId, text, values = { title, author: "Ada" }) => fillTemplate([{ path: "main", text }], formatId, values)[0].text;

assert.equal(
  fill("latex", "\\title{{{title}}}"),
  "\\title{A \\& B \"C\" \\textbackslash{} \\$x\\_1\\$ \\#1 *bold*}",
  "latex escapes TeX syntax",
);
assert.equal(fill("typst", '#let title = "{{title}}"'), '#let title = "A & B \\"C\\" \\\\ $x_1$ #1 *bold*"');
assert.equal(fill("quarto", 'title: "{{title}}"'), 'title: "A & B \\"C\\" \\\\ $x_1$ #1 *bold*"');
assert.equal(
  fill("markdown", "# {{title}}"),
  "# A \\& B \\\"C\\\" \\\\ \\$x\\_1\\$ \\#1 \\*bold\\*",
  "markdown escapes every ASCII punctuation character",
);
assert.equal(fill("html", '<h1 title="{{title}}">{{title}}</h1>'), '<h1 title="A &amp; B &quot;C&quot; \\ $x_1$ #1 *bold*">A &amp; B &quot;C&quot; \\ $x_1$ #1 *bold*</h1>');

// A name with a dollar sign is not a replacement pattern, and both
// placeholders are filled wherever they occur.
assert.equal(fill("html", "{{title}}/{{title}}/{{author}}", { title: "$& $1", author: "Ada" }), "$&amp; $1/$&amp; $1/Ada");

// Nothing typed is a name that still renders.
assert.equal(fill("typst", "{{title}} by {{author}}", { title: "  ", author: "" }), "Untitled by Your Name");

// Paths and the number of files are untouched.
const files = fillTemplate([{ path: "main.typ", text: "{{title}}" }, { path: "chapters/one.typ", text: "x" }], "typst", { title: "T", author: "A" });
assert.deepEqual(files, [{ path: "main.typ", text: "T" }, { path: "chapters/one.typ", text: "x" }]);

const templates = [
  { id: "blank", name: "Blank", description: "An empty page.", keywords: ["empty"], formats: ["typst", "markdown", "latex"] },
  { id: "letter", name: "Letter", description: "A formal letter.", keywords: ["cover letter", "reference"], formats: ["typst", "latex"] },
  { id: "cv", name: "CV", description: "A curriculum vitae.", keywords: ["resume"], formats: ["html", "typst"] },
  { id: "mine", name: "Lab report", description: "", keywords: [], formats: ["markdown"], custom: true },
];
const ids = (list) => list.map((each) => each.id);

assert.deepEqual(ids(matchTemplates(templates, "", null)), ["blank", "letter", "cv", "mine"], "no query and no format is everything");
assert.deepEqual(ids(matchTemplates(templates, "", "")), ["blank", "letter", "cv", "mine"], "an empty format is any");
assert.deepEqual(ids(matchTemplates(templates, "", "html")), ["cv"], "the format filters");
assert.deepEqual(ids(matchTemplates(templates, "", "typst")), ["blank", "letter", "cv"]);
assert.deepEqual(ids(matchTemplates(templates, "RESUME", null)), ["cv"], "keywords match, in any case");
assert.deepEqual(ids(matchTemplates(templates, "lab", null)), ["mine"], "a custom template matches by name");
assert.deepEqual(ids(matchTemplates(templates, "formal letter", null)), ["letter"], "every word has to match, in any order");
assert.deepEqual(ids(matchTemplates(templates, "letter empty", null)), [], "a word that matches nowhere excludes the template");
assert.deepEqual(ids(matchTemplates(templates, "cover  letter", "latex")), ["letter"], "query and format together");
assert.deepEqual(ids(matchTemplates(templates, "letter", "html")), []);
