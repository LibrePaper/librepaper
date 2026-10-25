// The language a source declares, and whether Harper should check it.
import assert from "node:assert/strict";
import { declaredLanguage, isEnglish, isEnglishDocument } from "../../src/lib/harper/language.js";

// Babel: the last option is the main language.
{
  const text = "\\documentclass{article}\n\\usepackage[english,french]{babel}\n";
  assert.equal(declaredLanguage(text, "latex"), "french");
  assert.equal(isEnglishDocument(text, "latex"), false);
}
{
  const text = "\\usepackage[french,british]{babel}\n";
  assert.equal(declaredLanguage(text, "latex"), "british");
  assert.equal(isEnglishDocument(text, "latex"), true);
}

// Babel: main= wins over position.
{
  const text = "\\usepackage[main=english, german, french]{babel}\n";
  assert.equal(declaredLanguage(text, "latex"), "english");
  assert.equal(isEnglishDocument(text, "latex"), true);
}

// A commented-out babel line declares nothing.
{
  const text = "  % \\usepackage[french]{babel}\nHello.\n";
  assert.equal(declaredLanguage(text, "latex"), null);
  assert.equal(isEnglishDocument(text, "latex"), true);
}

// Polyglossia, with and without options.
{
  assert.equal(declaredLanguage("\\setmainlanguage{german}", "latex"), "german");
  assert.equal(declaredLanguage("\\setmainlanguage[variant=us]{english}", "latex"), "english");
  assert.equal(isEnglishDocument("\\setmainlanguage{spanish}", "latex"), false);
}

// Typst.
{
  const text = '#set text(font: "Libertinus Serif", lang: "fr")\nBonjour.';
  assert.equal(declaredLanguage(text, "typst"), "fr");
  assert.equal(isEnglishDocument(text, "typst"), false);
  assert.equal(isEnglishDocument('#set text(lang: "en-GB")', "typst"), true);
  assert.equal(declaredLanguage("#set text(size: 11pt)", "typst"), null);
}

// YAML front matter in Markdown and Quarto, quoted or not.
{
  const md = "---\ntitle: Paper\nlang: de\n---\nHallo.";
  assert.equal(declaredLanguage(md, "markdown"), "de");
  assert.equal(isEnglishDocument(md, "markdown"), false);
  const qmd = '---\nlang: "en-US"\n---\nHello.';
  assert.equal(declaredLanguage(qmd, "quarto"), "en-US");
  assert.equal(isEnglishDocument(qmd, "quarto"), true);
}

// A lang key outside the opening front matter is not a declaration.
{
  assert.equal(declaredLanguage("Intro\n\n---\nlang: fr\n---\n", "markdown"), null);
  assert.equal(declaredLanguage("---\ntitle: T\n---\nlang: fr\n", "markdown"), null);
}

// HTML.
{
  assert.equal(declaredLanguage('<!doctype html>\n<html lang="fr">\n<body></body>', "html"), "fr");
  assert.equal(isEnglishDocument('<html lang="en">', "html"), true);
  assert.equal(declaredLanguage("<html><body>Hi</body></html>", "html"), null);
}

// No declaration means English.
{
  assert.equal(declaredLanguage("Just words.", "latex"), null);
  assert.equal(isEnglishDocument("Just words.", "markdown"), true);
}

// English names.
{
  for (const name of [null, "", "en", "en-GB", "english", "American", "british", "usenglish", "ukenglish", "canadian", "australian", "newzealand"]) {
    assert.equal(isEnglish(name), true, String(name));
  }
  for (const name of ["fr", "german", "spanish", "de-CH"]) assert.equal(isEnglish(name), false, name);
}

// Only the start of a long source is scanned.
{
  const text = `${"x".repeat(25000)}\n\\usepackage[french]{babel}`;
  assert.equal(declaredLanguage(text, "latex"), null);
}
