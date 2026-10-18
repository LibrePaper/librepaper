// The symbols palette mounted over a LaTeX context: tabs, search, the Recent
// row, and the spelling a format gives each tile.
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
import { SYMBOLS } from ${imp("src/lib/symbols/index.js")};
window.testErrors=[];addEventListener("error",e=>window.testErrors.push(e.message));addEventListener("unhandledrejection",e=>window.testErrors.push(String(e.reason)));
window.ids={
  alpha: SYMBOLS.find((symbol) => symbol.latex === "\\\\alpha").id,
  implies: SYMBOLS.find((symbol) => symbol.latex === "\\\\Rightarrow").id,
};
let component, currentContext, inserted = [];
// The caret sits in plain prose, outside any math span.
const contextFor = (format) => ({ format, path: "paper." + ({latex:"tex",typst:"typ"}[format]), text: "Before AFTER", selection: { from: 7, to: 7, text: "" }, files: [] });
window.setupPalette = async (format) => {
  component?.$destroy();
  inserted = [];
  window.inserted = inserted;
  currentContext = contextFor(format);
  component = createClassComponent({ component: SymbolPalette, target: document.getElementById("palette"), props: {
    getContext: () => currentContext,
    oninsert: (result) => { inserted.push(result); },
    onfocus: () => {},
  }});
  await tick();
};
// The palette reads the format through getContext, so a new format is a new
// getContext prop, which re-renders the tiles.
window.switchFormat = async (format) => {
  currentContext = contextFor(format);
  component.$set({ getContext: () => currentContext });
  await tick();
};
window.typeQuery = async (text) => {
  const input = document.querySelector(".symbol-palette input[type=search]");
  input.value = text;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  await tick();
};
window.clickGridTile = (id) => document.querySelector(".symbol-grid [data-symbol=" + JSON.stringify(id) + "]")?.click();
window.insertedCount = () => inserted.length;
window.insertedText = () => inserted.at(-1)?.text ?? null;
window.gridTileCount = () => document.querySelectorAll(".symbol-grid [data-symbol]").length;
window.gridTileTitle = (id) => document.querySelector(".symbol-grid [data-symbol=" + JSON.stringify(id) + "]")?.title ?? null;
window.resultTile = (id) => Boolean(document.querySelector(".symbol-search .results [data-symbol=" + JSON.stringify(id) + "]"));
window.recentHas = (id) => Boolean(document.querySelector('[role="group"][aria-label="Recent symbols"] [data-symbol=' + JSON.stringify(id) + "]"));
window.hasGridTile = (id) => Boolean(document.querySelector(".symbol-grid [data-symbol=" + JSON.stringify(id) + "]"));
window.setPaletteWidth = (px) => { document.getElementById("palette").style.width = px + "px"; };
window.categorySelectShown = () => Boolean(document.querySelector('select[aria-label="Symbol category"]'));
window.tabStripShown = () => Boolean(document.querySelector('[aria-label="Symbol categories"]'));
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
  const alpha = await evaluate("window.ids.alpha");
  const implies = await evaluate("window.ids.implies");

  // The Greek tab is the first tab, so its tiles show straight away.
  await evaluate('setupPalette("latex")');
  await until("Greek tiles", () => evaluate("gridTileCount() > 0"), 5000);
  await until("alpha tile", () => evaluate(`hasGridTile(${JSON.stringify(alpha)})`), 5000);

  // A plain click inserts the spelling for a caret outside math.
  await evaluate(`clickGridTile(${JSON.stringify(alpha)})`);
  assert.equal(await evaluate("insertedCount()"), 1, "clicking a tile records one insertion");
  assert.equal(await evaluate("insertedText()"), "$\\alpha$", "alpha inserts as inline math outside math");

  // A typed query hides the tabs and the Recent row, and finds the arrow by an alias.
  await evaluate('typeQuery("implies")');
  await until("implies result", () => evaluate(`resultTile(${JSON.stringify(implies)})`), 5000);
  assert.equal(await evaluate("tabStripShown()"), false, "the tab strip is hidden while a query is typed");

  // Clearing the query returns to the tab that was open, and Recent now lists alpha.
  await evaluate('typeQuery("")');
  await until("Greek tiles again", () => evaluate("gridTileCount() > 0"), 5000);
  assert.equal(await evaluate("tabStripShown()"), true, "the tab strip returns once the query is cleared");
  await until("Recent row shows alpha", () => evaluate(`recentHas(${JSON.stringify(alpha)})`), 5000);

  // The title follows the format of the open document.
  await evaluate('switchFormat("typst")');
  await until("typst title", () => evaluate(`gridTileTitle(${JSON.stringify(alpha)}) === "alpha"`), 5000);
  assert.equal(await evaluate(`gridTileTitle(${JSON.stringify(alpha)})`), "alpha", "the typst spelling of alpha is alpha");

  // Below 480px the tab strip folds into a select. The panel measures its own
  // width, so the check waits for the ResizeObserver to report the change.
  await evaluate("setPaletteWidth(320)");
  await until("category select at narrow width", () => evaluate("categorySelectShown()"), 5000);
  assert.equal(await evaluate("tabStripShown()"), false, "the tab strip folds away below 480px");

  console.log("symbols-browser: palette tabs, search, Recent row, insertion and format-aware titles passed");
} catch (error) {
  console.error(await page?.evaluate("({errors:window.testErrors,tabs:document.querySelectorAll('.symbol-palette [role=tab]').length,tiles:document.querySelectorAll('[data-symbol]').length})"));
  throw error;
} finally {
  await page?.close();
  server?.close();
  removeTemporary(temporary);
}
