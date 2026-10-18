// The symbols picker mounted over a LaTeX context: category nav, search, the
// details row, favorites, keyboard movement and the spelling a format gives
// each tile.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { mkdtempSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";
import { contentType, loroAlias } from "../helpers/loro.mjs";
import { KATEX_DEFINE, serveKatex } from "../helpers/katex.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-symbols-browser-"));
const output = join(temporary, "build");
const imp = (path) => JSON.stringify(join(root, path));
const entry = join(temporary, "entry.js");
writeFileSync(entry, `
import { tick } from ${imp("node_modules/svelte/src/index-client.js")};
import { createClassComponent } from ${imp("node_modules/svelte/src/legacy/legacy-client.js")};
import SymbolPalette from ${imp("src/components/SymbolPalette.svelte")};
import { PICKER_SYMBOLS } from ${imp("src/lib/symbols/catalog.js")};
window.testErrors=[];addEventListener("error",e=>window.testErrors.push(e.message));addEventListener("unhandledrejection",e=>window.testErrors.push(String(e.reason)));
const find = (latex) => PICKER_SYMBOLS.find((symbol) => symbol.latex === latex).id;
window.ids={
  alpha: find("\\\\alpha"),
  beta: find("\\\\beta"),
  arrow: PICKER_SYMBOLS.find((symbol) => symbol.category === "arrows").id,
  template: PICKER_SYMBOLS.find((symbol) => symbol.kind === "template").id,
};
let component, currentContext, inserted = [];
// The caret sits in plain prose, outside any math span.
const contextFor = (format) => ({ format, path: "paper." + ({latex:"tex",typst:"typ",html:"html"}[format]), text: "Before AFTER", selection: { from: 7, to: 7, text: "" }, files: [] });
window.setupPalette = async (format) => {
  component?.$destroy();
  inserted = [];
  window.inserted = inserted;
  currentContext = contextFor(format);
  component = createClassComponent({ component: SymbolPalette, target: document.getElementById("palette"), props: {
    format,
    getContext: () => currentContext,
    oninsert: (result) => { inserted.push(result); },
  }});
  await tick();
};
// The format is a prop and the context a getter, so a new format sets both,
// which re-renders the tiles.
window.switchFormat = async (format) => {
  currentContext = contextFor(format);
  component.$set({ format, getContext: () => currentContext });
  await tick();
};
const search = () => document.querySelector(".symbol-palette input[type=search]");
window.typeQuery = async (text) => {
  const input = search();
  input.value = text;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  await tick();
};
const navButton = (label) => [...document.querySelectorAll('nav[aria-label="Symbol categories"] button')].find((button) => button.querySelector(".category-label").textContent.trim() === label);
window.navLabels = () => [...document.querySelectorAll('nav[aria-label="Symbol categories"] .category-label')].map((span) => span.textContent.trim());
window.navTruncated = () => [...document.querySelectorAll('nav[aria-label="Symbol categories"] .category-label')].some((span) => span.scrollWidth > span.clientWidth + 1);
window.openCategory = (label) => navButton(label)?.click();
window.currentCategory = () => navButton(window.navLabels().find((label) => navButton(label).getAttribute("aria-current") === "true"))?.querySelector(".category-label").textContent.trim() ?? null;
const tile = (id) => document.querySelector(".grid-scroll [data-symbol=" + JSON.stringify(id) + "]");
window.clickGridTile = (id) => tile(id)?.click();
window.hasGridTile = (id) => Boolean(tile(id));
window.firstTileId = () => document.querySelector(".grid-scroll [data-symbol]")?.dataset.symbol ?? null;
window.gridTileCount = () => document.querySelectorAll(".grid-scroll [data-symbol]").length;
window.gridTileTitle = (id) => tile(id)?.title ?? null;
window.insertedCount = () => inserted.length;
window.insertedText = () => inserted.at(-1)?.text ?? null;
window.noteText = () => document.querySelector(".symbol-palette [role=status]")?.textContent.trim() ?? null;
window.clickStar = () => document.querySelector('.details button[aria-label="Favorite"]')?.click();
window.starPressed = () => document.querySelector('.details button[aria-label="Favorite"]')?.getAttribute("aria-pressed") ?? null;
window.favoriteIds = () => JSON.parse(localStorage.getItem("librepaper-symbols-favorites") || "[]");
window.tabStops = () => document.querySelectorAll('.grid-scroll [data-symbol][tabindex="0"]').length;
window.focusFirstTile = () => document.querySelector(".grid-scroll [data-symbol]").focus();
window.pressKey = (key, options = {}) => (document.activeElement ?? document.body).dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...options }));
window.focusedTileIndex = () => [...document.querySelectorAll(".grid-scroll [data-symbol]")].indexOf(document.activeElement);
window.searchFocused = () => document.activeElement === search();
window.insertReady = true;
`);
let server, page;
try {
  await build({configFile:false, define: KATEX_DEFINE,root,plugins:[svelte()],logLevel:"error",resolve:{alias:loroAlias},build:{outDir:output,emptyOutDir:true,lib:{entry,formats:["es"],fileName:()=>"symbols-check.js"}}});
  server = createServer((request, response) => {
    if (serveKatex(request, response)) return;
    const pathname = new URL(request.url, "http://localhost").pathname;
    const file = join(output, pathname);
    if (pathname !== "/" && existsSync(file)) {
      response.setHeader("Content-Type", contentType(file));
      response.end(readFileSync(file));
    } else response.end('<!doctype html><html><head><meta charset="utf-8"></head><body><div id="palette" style="height:600px;width:560px"></div><script type="module" src="/symbols-check.js"></script></body></html>');
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  page = await browser("chromium", join(temporary, "profile"), 24000 + Math.floor(Math.random() * 10000));
  await page.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("Symbols test loaded", () => page.evaluate("window.insertReady"), 10000);
  const evaluate = (code) => page.evaluate(code);
  const ids = await evaluate("window.ids");
  const has = (id) => evaluate(`hasGridTile(${JSON.stringify(id)})`);

  // The nav lists all twelve categories and none is cut off.
  await evaluate('setupPalette("latex")');
  const labels = await evaluate("navLabels()");
  assert.deepEqual(labels, ["Recent", "Favorites", "Greek", "Operators", "Relations", "Arrows", "Logic and sets", "Accents", "Delimiters", "Templates", "Text", "Emoji"]);
  assert.equal(await evaluate("navTruncated()"), false, "no category label is truncated");

  // With nothing remembered the picker opens on Greek.
  assert.equal(await evaluate("currentCategory()"), "Greek", "an empty Recent opens on Greek");
  await until("alpha tile", () => has(ids.alpha), 5000);
  assert.equal(await evaluate("tabStops()"), 1, "exactly one tile is a tab stop");

  // Another category shows its own tiles.
  await evaluate('openCategory("Arrows")');
  await until("arrow tile", () => has(ids.arrow), 5000);
  assert.equal(await has(ids.alpha), false, "Greek tiles are gone from Arrows");
  await evaluate('openCategory("Greek")');
  await until("alpha tile again", () => has(ids.alpha), 5000);

  // Two clicks in a row record two insertions and the picker stays mounted.
  await evaluate(`clickGridTile(${JSON.stringify(ids.alpha)})`);
  assert.equal(await evaluate("insertedText()"), "$\\alpha$", "alpha inserts as inline math outside math");
  await evaluate(`clickGridTile(${JSON.stringify(ids.beta)})`);
  assert.equal(await evaluate("insertedCount()"), 2, "two clicks record two insertions");
  assert.equal(await has(ids.beta), true, "the picker is still mounted after insertions");

  // The details row follows the last insertion, and its star writes a favorite.
  assert.equal(await evaluate("starPressed()"), "false", "the star starts off");
  await evaluate("clickStar()");
  await until("favorite stored", async () => (await evaluate("favoriteIds()")).includes(ids.beta), 5000);
  assert.equal(await evaluate("starPressed()"), "true", "the star shows the favorite");
  await evaluate('openCategory("Favorites")');
  await until("favorite listed", () => has(ids.beta), 5000);

  // Recent lists what was inserted, newest first.
  await evaluate('openCategory("Recent")');
  await until("recent listed", () => has(ids.alpha), 5000);
  assert.equal(await evaluate("firstTileId()"), ids.beta, "the newest insertion leads Recent");

  // A typed query ignores the category and ranks the command first.
  await evaluate(`typeQuery(${JSON.stringify("\\alpha")})`);
  await until("alpha result", () => evaluate(`firstTileId() === ${JSON.stringify(ids.alpha)}`), 5000);
  await evaluate('typeQuery("")');
  await until("category returns", () => evaluate("currentCategory() === 'Recent'"), 5000);

  // The arrow keys move focus between tiles, and Ctrl+F reaches the search.
  await evaluate('openCategory("Greek")');
  await until("greek tiles", () => has(ids.alpha), 5000);
  await evaluate("focusFirstTile()");
  await evaluate('pressKey("ArrowRight")');
  assert.equal(await evaluate("focusedTileIndex()"), 1, "ArrowRight moves to the second tile");
  await evaluate('pressKey("f", { ctrlKey: true })');
  assert.equal(await evaluate("searchFocused()"), true, "Ctrl+F focuses the search");

  // A pair inserts a snippet in LaTeX.
  await evaluate('openCategory("Delimiters")');
  await until("pair tile", () => has("pair-parentheses"), 5000);
  await evaluate('clickGridTile("pair-parentheses")');
  assert.ok((await evaluate("insertedText()")).includes("\\left("), "the parentheses pair inserts \\left(");

  // The title follows the format of the open document.
  await evaluate('openCategory("Greek")');
  await until("greek again", () => has(ids.alpha), 5000);
  await evaluate('switchFormat("typst")');
  await until("typst title", () => evaluate(`gridTileTitle(${JSON.stringify(ids.alpha)}) === "Alpha: alpha"`), 5000);

  // A template has no HTML form, so it inserts nothing and says so.
  await evaluate('setupPalette("html")');
  await evaluate('openCategory("Templates")');
  await until("template tile", () => has(ids.template), 5000);
  await evaluate(`clickGridTile(${JSON.stringify(ids.template)})`);
  assert.equal(await evaluate("insertedCount()"), 0, "a template does not insert in HTML");
  await until("html note", () => evaluate("noteText() === 'Not available in HTML.'"), 5000);

  console.log("symbols-browser: picker categories, search, insertion, favorites, keyboard and format-aware titles passed");
} catch (error) {
  console.error(await page?.evaluate("({errors:window.testErrors,tiles:document.querySelectorAll('[data-symbol]').length})"));
  throw error;
} finally {
  await page?.close();
  server?.close();
  removeTemporary(temporary);
}
