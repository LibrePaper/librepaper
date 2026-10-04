import assert from "node:assert/strict";
import { EditorState } from "@codemirror/state";
import { ensureSyntaxTree, syntaxTree } from "@codemirror/language";
import { highlightTree, tagHighlighter, tags } from "@lezer/highlight";
import { codeLanguage, sourceLanguage } from "../../src/lib/source-language.js";

const languageName = (info) => codeLanguage(info)?.name || null;

assert.equal(languageName("r"), "R");
assert.equal(languageName("python"), "Python");
assert.equal(languageName("javascript"), "JavaScript");
assert.equal(languageName("{r}"), "R");
assert.equal(languageName('{python label="fig-x"}'), "Python");
assert.equal(languageName("{r, echo=FALSE}"), "R");
assert.equal(languageName("{PYTHON label=plot}"), "Python");
assert.equal(codeLanguage("python-unknown"), null, "language matching must not accept fuzzy prefixes");
assert.equal(codeLanguage("{unknown, echo=FALSE}"), null);
assert.equal(languageName("{r"), "R", "Markdown may pass a truncated Quarto fence header to the resolver");
assert.equal(codeLanguage(""), null);

// Load the embedded parsers before asking Markdown's mixed parser to build
// trees for fenced code. sourceLanguage itself remains a synchronous editor
// extension; the fence descriptions retain CodeMirror's lazy loading.
await Promise.all([codeLanguage("r").load(), codeLanguage("python").load()]);

function highlighted(path, source, fallback = "") {
  const state = EditorState.create({ doc: source, extensions: sourceLanguage(path, fallback) });
  const tree = ensureSyntaxTree(state, state.doc.length, 1000);
  assert.ok(tree, `${path} did not produce a complete syntax tree`);
  const result = [];
  const highlighter = tagHighlighter([
    { tag: tags.comment, class: "comment" },
    { tag: tags.string, class: "string" },
    { tag: tags.keyword, class: "keyword" },
    { tag: tags.tagName, class: "tag" },
    { tag: tags.heading, class: "heading" },
    { tag: tags.propertyName, class: "property" },
  ]);
  highlightTree(tree, highlighter, (from, to, classes) => {
    result.push({ from, to, classes });
  });
  return { source, result };
}

function hasToken({ source, result }, text, kind, message = `${JSON.stringify(text)} was not highlighted as ${kind}`) {
  const from = source.indexOf(text);
  assert.notEqual(from, -1, `test token ${JSON.stringify(text)} is absent`);
  const to = from + text.length;
  assert.ok(result.some((token) => token.from <= from && token.to >= to && token.classes === kind), message);
}

const latex = highlighted("paper.tex", "% comment\n\\documentclass{article}");
hasToken(latex, "comment", "comment");
hasToken(latex, "\\documentclass", "tag");
hasToken(highlighted("theme.sty", "% package note\n\\ProvidesPackage{theme}"), "package note", "comment");
hasToken(highlighted("class.cls", "\\newcommand{\\name}{text}"), "\\newcommand", "tag");
const typst = highlighted("paper.typ", "/* block note */\n#let title = $x$");
hasToken(typst, "block note", "comment");
hasToken(typst, "#let", "keyword");

const html = highlighted("page.html", "<!-- note --><title>hello</title>");
hasToken(html, "<!-- note -->", "comment");
hasToken(html, "title", "tag");

const markdown = highlighted("notes.md", "# Heading\n\n```python\n# comment\nprint(\"hello\")\n```\n");
hasToken(markdown, "Heading", "heading");
hasToken(markdown, "# comment", "comment", "Python fence comment was not parsed as Python");
hasToken(markdown, "\"hello\"", "string", "Python fence string was not parsed as Python");

for (const [info, file] of [["r", "plain-r.md"], ["{r}", "engine-r.qmd"], ["{r, echo=FALSE}", "options-r.qmd"]]) {
  const embeddedR = highlighted(file, `\`\`\`${info}\n#| label: fig-x\n# comment\nprint("plot")\n\`\`\`\n`);
  hasToken(embeddedR, "# comment", "comment", `${info} fence comment was not parsed as R`);
  hasToken(embeddedR, "\"plot\"", "string", `${info} fence string was not parsed as R`);
}

const quarto = highlighted("paper.qmd", "---\ntitle: \"Demo\"\n---\n\n```{python label=fig-x}\n# comment\nprint(\"plot\")\n```\n");
hasToken(quarto, "title", "property", "YAML front matter was not parsed");
hasToken(quarto, "\"Demo\"", "string", "YAML front matter string was not highlighted");
hasToken(quarto, "# comment", "comment", "Quarto chunk options prevented embedded Python highlighting");
hasToken(quarto, "\"plot\"", "string", "Quarto embedded Python string was not highlighted");

assert.equal(syntaxTree(EditorState.create({ doc: "plain", extensions: sourceLanguage("data.py") })).length, 0,
  "unsupported named files should remain plain text");
assert.ok(syntaxTree(EditorState.create({ doc: "# heading", extensions: sourceLanguage("", "markdown") })).length > 0,
  "an unnamed document should use its declared format");
assert.equal(syntaxTree(EditorState.create({ doc: "# heading", extensions: sourceLanguage("data.py", "markdown") })).length, 0,
  "a named unsupported file should take precedence over the fallback format");

console.log("source language: filename modes, exact fence aliases, embedded syntax and plain-text fallback passed");
