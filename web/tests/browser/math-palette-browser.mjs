// The math palette in a real browser: the picker tiles, the template forms,
// the style choice and the stepper-driven matrix, with the insertion recorded
// by a fake host rather than an editor.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { mkdtempSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";
import { contentType } from "../helpers/loro.mjs";
import { KATEX_DEFINE, serveKatex } from "../helpers/katex.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-math-palette-browser-"));
const output = join(temporary, "build");
const imp = (path) => JSON.stringify(join(root, path));
const entry = join(temporary, "entry.js");
writeFileSync(entry, `
import { tick } from ${imp("node_modules/svelte/src/index-client.js")};
import { createClassComponent } from ${imp("node_modules/svelte/src/legacy/legacy-client.js")};
import MathPalette from ${imp("src/components/MathPalette.svelte")};
import { MATH_TEMPLATES } from ${imp("src/lib/symbols/templates.js")};
window.testErrors = [];
addEventListener("error", (e) => window.testErrors.push(e.message));
addEventListener("unhandledrejection", (e) => window.testErrors.push(String(e.reason)));
const insertions = [];
let palette;
// A LaTeX document with the caret outside math, so the style choice is shown.
const context = (format, selected) => ({ format, path: "paper.tex", text: "Before AFTER", files: [], selection: { from: 7, to: 7 + selected.length, text: selected } });
window.templateCount = MATH_TEMPLATES.length;
window.mountPalette = async (format = "latex", selected = "") => {
  palette?.$destroy();
  palette = createClassComponent({ component: MathPalette, target: document.getElementById("palette"), props: {
    format,
    getContext: () => context(format, selected),
    oninsert: (result, ctx) => { insertions.push({ result, format: ctx.format }); },
    onfocus: () => {}
  } });
  await tick();
};
window.insertions = () => insertions;
window.openTemplate = (id) => palette.openTemplate(id);
window.paletteReady = true;
`);

let server, page;
try {
  await build({ configFile: false, define: KATEX_DEFINE, root, plugins: [svelte()], logLevel: "error", build: { outDir: output, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "math-palette-check.js" } } });
  server = createServer((request, response) => {
    if (serveKatex(request, response)) return;
    const pathname = new URL(request.url, "http://localhost").pathname;
    const file = join(output, pathname);
    if (pathname !== "/" && existsSync(file)) {
      response.setHeader("Content-Type", contentType(file));
      response.end(readFileSync(file));
    } else {
      response.end('<!doctype html><html><head><meta charset="utf-8"></head><body><div id="palette"></div><script type="module" src="/math-palette-check.js"></script></body></html>');
    }
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  page = await browser("chromium", join(temporary, "profile"), 24000 + Math.floor(Math.random() * 10000));
  await page.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("Math palette test loaded", () => page.evaluate("window.paletteReady"), 10000);
  const evaluate = (code) => page.evaluate(code);
  const click = (selector) => evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`);
  const count = (selector) => evaluate(`document.querySelectorAll(${JSON.stringify(selector)}).length`);
  const fill = (index, value) => evaluate(`(() => {
    const input = document.querySelectorAll("[data-math-field]")[${index}];
    input.value = ${JSON.stringify(value)};
    input.dispatchEvent(new Event("input", { bubbles: true }));
  })()`);
  const submit = () => evaluate(`document.querySelector("[data-math-field]").form.requestSubmit()`);
  const lastInsertion = () => evaluate("(() => { const all = insertions(); return all[all.length - 1]; })()");

  await evaluate('mountPalette("latex")');
  await until("picker tiles", () => evaluate(`document.querySelectorAll("button[data-template]").length > 0`), 5000);
  assert.equal(await count("button[data-template]"), await evaluate("templateCount"), "one picker tile per template");

  // Fraction: two fields, display style by default.
  await click('button[data-template="fraction"]');
  await until("fraction form", () => evaluate(`document.querySelectorAll("[data-math-field]").length === 2`), 5000);
  await fill(0, "a+b");
  await fill(1, "2c");
  await submit();
  const display = await lastInsertion();
  assert.match(display.result.text, /\\frac\{a\+b\}\{2c\}/, "fraction inserts the typed numerator and denominator");
  assert.equal(display.format, "latex", "the insertion is handed the captured context");

  // Inline style gives dollars.
  await until("picker after insert", () => evaluate(`document.querySelectorAll("button[data-template]").length > 0`), 5000);
  await click('button[data-template="fraction"]');
  await until("fraction form again", () => evaluate(`document.querySelectorAll("[data-math-field]").length === 2`), 5000);
  await click('input[name="math-style"][value="inline"]');
  await fill(0, "a+b");
  await fill(1, "2c");
  await submit();
  const inline = await lastInsertion();
  assert.match(inline.result.text, /^\$\\frac\{a\+b\}\{2c\}\$$/, "inline style wraps the fraction in dollars");

  // Numbered display: ticking Numbered and typing a label adds the equation environment and the label.
  await until("picker before numbered", () => evaluate(`document.querySelectorAll("button[data-template]").length > 0`), 5000);
  await click('button[data-template="fraction"]');
  await until("fraction form for numbering", () => evaluate(`document.querySelectorAll("[data-math-field]").length === 2`), 5000);
  await click('input[name="math-numbered"]');
  await until("label field after numbered", () => evaluate(`document.querySelectorAll('input[name="math-label"]').length === 1`), 5000);
  await evaluate(`(() => {
    const input = document.querySelector('input[name="math-label"]');
    input.value = "eq:frac";
    input.dispatchEvent(new Event("input", { bubbles: true }));
  })()`);
  await fill(0, "a+b");
  await fill(1, "2c");
  await submit();
  const numbered = await lastInsertion();
  assert.match(numbered.result.text, /\\begin\{equation\}/, "a numbered display uses the equation environment");
  assert.match(numbered.result.text, /\\label\{eq:frac\}/, "the typed label reaches the inserted LaTeX");

  // Matrix: default 2 by 2, then one more column gives six fields.
  await until("picker before matrix", () => evaluate(`document.querySelectorAll("button[data-template]").length > 0`), 5000);
  await click('button[data-template="matrix"]');
  await until("matrix form", () => evaluate(`document.querySelectorAll("[data-math-field]").length === 4`), 5000);
  await click('[role="group"][aria-label="Columns"] [aria-label="More columns"]');
  assert.equal(await count("[data-math-field]"), 6, "three columns by two rows shows six cells");
  await evaluate(`document.querySelector('[role="group"][aria-label="Columns"] [aria-label="Fewer columns"]').click()`);
  assert.equal(await count("[data-math-field]"), 4, "removing a column drops the cells that no longer exist");
  await click('.math-actions button[type="button"]');
  await until("picker after cancel", () => evaluate(`document.querySelectorAll("button[data-template]").length > 0`), 5000);

  // The embedded symbol search, its results, and a typed query.
  const search = 'details.math-symbols input[type="search"]';
  const openSymbols = () => evaluate(`document.querySelector("details.math-symbols").open = true`);
  const typeQuery = (query) => evaluate(`(() => {
    const input = document.querySelector(${JSON.stringify(search)});
    input.value = ${JSON.stringify(query)};
    input.dispatchEvent(new Event("input", { bubbles: true }));
  })()`);
  const pressEscape = () => evaluate(`document.querySelector(${JSON.stringify(search)}).dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }))`);
  const field = (index) => `document.querySelectorAll("[data-math-field]")[${index}]`;
  const pickSymbol = async (query, id) => {
    await typeQuery(query);
    const tile = `details.math-symbols .symbol-tile[data-symbol="${id}"]`;
    await until(`${id} tile`, () => evaluate(`document.querySelectorAll(${JSON.stringify(tile)}).length > 0`), 5000);
    await click(tile);
  };
  const openFraction = async () => {
    await until("picker for fraction", () => evaluate(`document.querySelectorAll("button[data-template]").length > 0`), 5000);
    await click('button[data-template="fraction"]');
    await until("fraction form", () => evaluate(`document.querySelectorAll("[data-math-field]").length === 2`), 5000);
    await openSymbols();
  };

  // Escape in a populated search clears it and keeps the form; a second one leaves.
  await openFraction();
  await typeQuery("hat");
  await pressEscape();
  assert.equal(await count("[data-math-field]"), 2, "escape in a populated search keeps the form");
  assert.equal(await evaluate(`document.querySelector(${JSON.stringify(search)}).value`), "", "escape clears the search query");
  await pressEscape();
  await until("empty-query escape returns to picker", () => evaluate(`document.querySelectorAll("button[data-template]").length > 0`), 5000);

  // An accent picked with nothing selected puts the caret inside the braces.
  await openFraction();
  await evaluate(`${field(0)}.focus()`);
  await pickSymbol("hat", "hat");
  assert.equal(await evaluate(`${field(0)}.value`), "\\hat{}", "an accent with no selection leaves empty braces");
  assert.equal(await evaluate(`${field(0)}.selectionStart`), 5, "the caret sits inside the braces");

  // A picked symbol keeps its package.
  await fill(0, "\\nexists");
  await fill(1, "2");
  await submit();
  const packaged = await lastInsertion();
  const mentioned = packaged.result.notes.some((note) => note.includes("amssymb")) || JSON.stringify(packaged.result.additionalEdits).includes("amssymb");
  assert.ok(mentioned, "a symbol that needs amssymb brings the package along");

  // A symbol picked at the edge of a word does not run into it.
  await openFraction();
  await fill(0, "x");
  await evaluate(`(() => { const input = ${field(0)}; input.focus(); input.setSelectionRange(0, 0); })()`);
  await pickSymbol("alpha", "alpha");
  assert.equal(await evaluate(`${field(0)}.value`), "\\alpha x", "a control word is separated from the letter after it");
  await evaluate(`${field(0)}.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }))`);
  await until("picker after boundary check", () => evaluate(`document.querySelectorAll("button[data-template]").length > 0`), 5000);

  // Cancel with Escape returns to the picker and inserts nothing.
  await until("picker before escape", () => evaluate(`document.querySelectorAll("button[data-template]").length > 0`), 5000);
  const before = (await evaluate("insertions().length"));
  await click('button[data-template="fraction"]');
  await until("fraction form for escape", () => evaluate(`document.querySelectorAll("[data-math-field]").length === 2`), 5000);
  await evaluate(`document.querySelector("[data-math-field]").dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }))`);
  await until("escape returns to picker", () => evaluate(`document.querySelectorAll("button[data-template]").length > 0`), 5000);
  assert.equal(await evaluate("insertions().length"), before, "escape inserts nothing");

  // An html document has no math forms, and a command cannot open one.
  await evaluate('mountPalette("html")');
  assert.equal(await count("button[data-template]"), 0, "html shows no tiles");
  assert.ok((await evaluate(`document.querySelector(".math-palette").textContent`)).includes("Math templates need"), "html explains why");
  await evaluate('openTemplate("fraction")');
  assert.equal(await count("[data-math-field]"), 0, "openTemplate opens no form for html");
  await evaluate('mountPalette("latex")');
  await until("picker after remount", () => evaluate(`document.querySelectorAll("button[data-template]").length > 0`), 5000);

  // openTemplate jumps straight to a form, here the root template.
  await evaluate('openTemplate("root")');
  await until("root form", () => evaluate(`document.querySelectorAll("[data-math-field]").length >= 1 && document.querySelectorAll("button[data-template]").length === 0`), 5000);
  assert.equal(await count("button[data-template]"), 0, "openTemplate replaces the picker with the form");

  console.log("math-palette-browser: picker tiles, fraction and inline insertions, matrix steppers, search escape, accent and package picks, escape and openTemplate passed");
} catch (error) {
  console.error(await page?.evaluate("({ errors: window.testErrors })"));
  throw error;
} finally {
  await page?.close();
  server?.close();
  removeTemporary(temporary);
}
