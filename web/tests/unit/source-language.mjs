import assert from "node:assert/strict";
import { EditorState } from "@codemirror/state";
import { LanguageDescription, ensureSyntaxTree, foldable, syntaxTree } from "@codemirror/language";
import { languages } from "@codemirror/language-data";
import { highlightTree, tagHighlighter, tags } from "@lezer/highlight";
import { codeLanguage, sourceFormat, sourceLanguage } from "../../src/lib/source-language.js";

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
hasToken(highlighted("fig.pgf", "% pgf note\n\\draw"), "pgf note", "comment");
hasToken(highlighted("size10.clo", "\\newcommand{\\name}{text}"), "\\newcommand", "tag");
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

assert.equal(sourceFormat("data.py"), "", "named Python files are still outside the format table");
assert.equal(syntaxTree(EditorState.create({ doc: "plain", extensions: sourceLanguage("README") })).length, 0,
  "a name with no known language should remain plain text");
assert.equal(syntaxTree(EditorState.create({ doc: "plain", extensions: sourceLanguage("data.xyz") })).length, 0,
  "an unknown extension should remain plain text");

// Language-data modes load on first use. Loading one here lets the
// synchronous sourceLanguage() return the loaded support directly.
for (const [filename, load] of [
  ["data.py", () => LanguageDescription.matchFilename(languages, "data.py")],
  ["model.r", () => LanguageDescription.matchFilename(languages, "model.r")],
  ["style.csl", () => LanguageDescription.matchLanguageName(languages, "xml", false)],
]) {
  const description = load();
  assert.ok(description, `${filename} has no language-data description`);
  await description.load();
}
hasToken(highlighted("data.py", "# comment\nprint(\"hi\")"), "# comment", "comment", "Python comment was not highlighted");
hasToken(highlighted("model.r", "# comment\nx <- 1"), "# comment", "comment", "R comment was not highlighted");
hasToken(highlighted("style.csl", "<!-- x -->\n<style/>"), "<!-- x -->", "comment", "CSL XML comment was not highlighted");
assert.ok(syntaxTree(EditorState.create({ doc: "# heading", extensions: sourceLanguage("", "markdown") })).length > 0,
  "an unnamed document should use its declared format");
assert.equal(syntaxTree(EditorState.create({ doc: "# heading", extensions: sourceLanguage("data.xyz", "markdown") })).length, 0,
  "a named unsupported file should take precedence over the fallback format");

assert.equal(languageName("json"), "JSON", ".json code fence should resolve to JSON language");
const gfm = "| a | b |\n| - | - |\n| 1 | 2 |\n\n~~gone~~\n";
const gfmState = EditorState.create({ doc: gfm, extensions: sourceLanguage("notes.md") });
const gfmNodes = new Set();
ensureSyntaxTree(gfmState, gfmState.doc.length, 1000)?.iterate({ enter: (node) => { gfmNodes.add(node.name); } });
assert.ok(gfmNodes.has("Table"), "GFM table was not parsed in a .md file");
assert.ok(gfmNodes.has("Strikethrough"), "GFM strikethrough was not parsed in a .md file");

const json = highlighted("config.json", '{\n  "key": "value"\n}');
assert.ok(syntaxTree(EditorState.create({ doc: json.source, extensions: sourceLanguage("config.json") })).length > 0,
  ".json file should produce a complete syntax tree");
hasToken(json, "key", "property", "JSON property name was not highlighted");
hasToken(json, '"value"', "string", "JSON string value was not highlighted");

assert.equal(sourceFormat("a/b.sty"), "latex");
assert.equal(sourceFormat("x.cls"), "latex");
assert.equal(sourceFormat("main.tex"), "latex");
assert.equal(sourceFormat("notes.txt", "markdown"), "markdown");
assert.equal(sourceFormat("", "tex"), "latex");
assert.equal(sourceFormat("data.json"), "json");
assert.equal(sourceFormat("README"), "");

// BibTeX: entries, nested braces, quoted values and % comments.
const bibSource = "@article{key2020,\n  title = {A {Nested} Title},\n  year = 2020,\n  author = \"Doe, J.\"\n}\n% note\n";
const bib = highlighted("refs.bib", bibSource);
hasToken(bib, "article", "keyword", "BibTeX entry type was not highlighted as a keyword");
hasToken(bib, "title", "property", "BibTeX field name was not highlighted");
// Braces carry their own tag, which splits the braced span, so check its text runs.
hasToken(bib, "A ", "string", "BibTeX braced value was not highlighted as a string");
hasToken(bib, "Nested", "string", "BibTeX nested braced group was not highlighted as a string");
hasToken(bib, "Title", "string", "BibTeX braced value after a nested group was not highlighted");
hasToken(bib, "Doe, J.", "string", "BibTeX quoted value was not highlighted as a string");
hasToken(bib, "note", "comment", "BibTeX % comment was not highlighted");
const bibState = EditorState.create({ doc: bibSource, extensions: sourceLanguage("refs.bib") });
ensureSyntaxTree(bibState, bibState.doc.length, 1000);
const bibFirstLine = bibState.doc.line(1);
assert.ok(foldable(bibState, bibFirstLine.from, bibFirstLine.to), "a multi-line BibTeX entry should fold");

console.log("source language: filename modes, exact fence aliases, embedded syntax and plain-text fallback passed");
