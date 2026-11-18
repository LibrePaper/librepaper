// A selection made in the preview says which source lines it came from. The
// frame reads the data-sourcepos attributes at either end of the selection and,
// when the preview's `sources` name the file, posts a `hint` beside the quote.
// The server uses it to tell apart passages that read the same, such as a
// sentence repeated in two places of one document. A hint is only ever a
// narrowing aid, so it is left out whenever the two ends disagree, the source
// is bundled rather than a file of the project, the preview names no sources,
// or the selection would span more than 200 lines.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { existsSync, mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";
import { assertFreshDist } from "../helpers/fresh-dist.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const dist = join(root, "dist");
assert.ok(existsSync(join(dist, "frame.js")), "run `bun run build` first");
assertFreshDist("preview-source-hint-browser");
const temporary = mkdtempSync(join(tmpdir(), "librepaper-source-hint-browser-"));

const types = { js: "text/javascript", css: "text/css", woff2: "font/woff2", html: "text/html" };
// The page is its own parent, as in mermaid-browser: the frame script takes
// messages from `parent`, which at the top level is this window, so the check
// plays the sidebar and records every selection the frame posts.
const shell = (origin) => `<!doctype html><html><head><meta charset="utf-8"></head><body>
<script>window.published=[];window.selections=[];addEventListener("message",e=>{if(e.data&&e.data.librepaper&&e.data.type==="ready")window.published.push(e.data.text);if(e.data&&e.data.librepaper&&e.data.type==="selection")window.selections.push(e.data)});</script>
<script src="/frame.js?reader=${encodeURIComponent(origin)}"></script>
</body></html>`;

// Pages used by the cases. Each tagged or untagged position is a distinct
// attribute value, so a case can wait for its own page to be in place.
const repeated = '<!doctype html><html><head><title>Repeated</title></head><body><section><p data-sourcepos="1:12:1-1:12:40">The same sentence appears here.</p><p data-sourcepos="1:30:1-1:31:20">The same sentence appears here.</p></section></body></html>';
const untagged = '<!doctype html><html><head><title>Untagged</title></head><body><h1 data-sourcepos="1:1-1:7">Title</h1><p data-sourcepos="3:1-4:10">First line and <em data-sourcepos="3:16-3:22">second</em> words.</p></body></html>';
const bundled = '<!doctype html><html><head><title>Bundled</title></head><body><p data-sourcepos="0:1:1-0:1:40">Bundled text here.</p></body></html>';
const mismatched = '<!doctype html><html><head><title>Mismatched</title></head><body><p data-sourcepos="0:1:1-0:1:20">Mixed</p><p data-sourcepos="1:5:1-1:5:20">ends here</p></body></html>';

// The sources a document names. Tag 0 is the bundled source, which maps to
// null: its text is not a file the project keeps.
const sources = [null, "chapters/intro.tex"];

// Selects from one text node to another, as a mouse selection would, then
// dispatches the events the frame listens for and returns the selection it
// posted. Each end is a CSS query for the element whose first child holds the
// text, and an offset into that text.
const selectText = (start, end) => `(async () => {
  const [startQuery, startOffset] = ${JSON.stringify(start)};
  const [endQuery, endOffset] = ${JSON.stringify(end)};
  const doc = document;
  const range = doc.createRange();
  range.setStart(doc.querySelector(startQuery).firstChild, startOffset);
  range.setEnd(doc.querySelector(endQuery).firstChild, endOffset);
  const selection = doc.getSelection();
  selection.removeAllRanges();
  selection.addRange(range);
  doc.dispatchEvent(new Event("selectionchange"));
  window.selections.length = 0;
  doc.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
  await new Promise((r) => setTimeout(r, 300));
  return window.selections.at(-1);
})()`;

// Whitespace between blocks is not fixed by the frame's text model, so a
// selection across two elements is compared with its whitespace removed.
const squash = (text) => text.replace(/\s+/g, "");

const requested = [];

let server, page;
try {
  server = createServer((request, response) => {
    const path = new URL(request.url, "http://127.0.0.1").pathname;
    requested.push(path);
    if (path === "/") {
      response.setHeader("content-type", types.html);
      response.end(shell(`http://${request.headers.host}`));
      return;
    }
    const file = join(dist, path);
    if (!existsSync(file)) {
      response.statusCode = 404;
      response.end("not found");
      return;
    }
    response.setHeader("content-type", types[path.split(".").pop()] || "application/octet-stream");
    response.end(readFileSync(file));
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  page = await browser("chromium", join(temporary, "profile"), 22000 + Math.floor(Math.random() * 10000));
  await page.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("agent loaded", () => page.evaluate("window.published.length > 0"), 10000);

  // Posts a preview. `withSources` is false for the case that has none, so the
  // key is absent rather than null.
  const preview = (html, withSources = true) => page.evaluate(`postMessage(${JSON.stringify({
    librepaper: true,
    type: "preview",
    html,
    ...(withSources ? { sources } : {}),
  })},"*")`);
  const showing = (marker) => until(
    `the page shows ${marker}`,
    () => page.evaluate(`Boolean(document.querySelector(${JSON.stringify(marker)}))`),
    5000,
  );

  // 1. Tagged positions: the second paragraph's words come from line 30 to 31
  // of the file the tag names, not the first paragraph's line 12.
  await preview(repeated);
  await showing('[data-sourcepos="1:30:1-1:31:20"]');
  const repeatedSelection = await page.evaluate(selectText(
    ['p[data-sourcepos="1:30:1-1:31:20"]', 4],
    ['p[data-sourcepos="1:30:1-1:31:20"]', 17],
  ));
  assert.equal(repeatedSelection.selector.exact, "same sentence", "tagged: the quote is the selected words");
  assert.deepEqual(
    repeatedSelection.selector.hint,
    { path: "chapters/intro.tex", from_line: 30, to_line: 31 },
    "tagged: the hint names the file and the lines of the second paragraph",
  );

  // 2. Untagged positions: the source is the one file, tag 0.
  await preview(untagged);
  await showing('[data-sourcepos="3:16-3:22"]');
  const inEmphasis = await page.evaluate(selectText(
    ['em[data-sourcepos="3:16-3:22"]', 0],
    ['em[data-sourcepos="3:16-3:22"]', 6],
  ));
  assert.equal(inEmphasis.selector.exact, "second", "untagged: the quote is the emphasised word");
  assert.deepEqual(
    inEmphasis.selector.hint,
    { path: "paper.md", from_line: 3, to_line: 3 },
    "untagged: a selection inside the em names its own line",
  );

  // Across the heading and the paragraph: the hint spans from line 1 to line 4.
  const acrossBlocks = await page.evaluate(selectText(
    ['h1[data-sourcepos="1:1-1:7"]', 0],
    ['p[data-sourcepos="3:1-4:10"]', 10],
  ));
  assert.equal(squash(acrossBlocks.selector.exact), "TitleFirstline", "untagged: the quote spans both blocks");
  assert.deepEqual(
    acrossBlocks.selector.hint,
    { path: "paper.md", from_line: 1, to_line: 4 },
    "untagged: the hint covers both ends' lines",
  );

  // 3. A bundled source: tag 0 maps to null in sources, so the selection has
  // no hint, but it is still a selection with its quote.
  await preview(bundled);
  await showing('[data-sourcepos="0:1:1-0:1:40"]');
  const bundledSelection = await page.evaluate(selectText(
    ['p[data-sourcepos="0:1:1-0:1:40"]', 8],
    ['p[data-sourcepos="0:1:1-0:1:40"]', 12],
  ));
  assert.equal(bundledSelection.selector.exact, "text", "bundled: the quote is still taken");
  assert.equal("hint" in bundledSelection.selector, false, "bundled: no hint key at all");

  // 4. No sources in the preview: the same untagged page gives no hint, and
  // the quote is still there.
  await preview(untagged, false);
  await showing('[data-sourcepos="3:16-3:22"]');
  const withoutSources = await page.evaluate(selectText(
    ['em[data-sourcepos="3:16-3:22"]', 0],
    ['em[data-sourcepos="3:16-3:22"]', 6],
  ));
  assert.equal(withoutSources.selector.exact, "second", "no sources: the quote is still taken");
  assert.equal("hint" in withoutSources.selector, false, "no sources: no hint");

  // 5. Mismatched tags: one end is the bundled tag 0 and the other tag 1, both
  // mapped in sources. The two ends do not name one source, so no hint.
  await preview(mismatched);
  await showing('[data-sourcepos="1:5:1-1:5:20"]');
  const mixed = await page.evaluate(selectText(
    ['p[data-sourcepos="0:1:1-0:1:20"]', 0],
    ['p[data-sourcepos="1:5:1-1:5:20"]', 4],
  ));
  assert.equal(squash(mixed.selector.exact), "Mixedends", "mismatched: the quote spans both paragraphs");
  assert.equal("hint" in mixed.selector, false, "mismatched tags: no hint");

  console.log("preview-source-hint-browser: tagged and untagged selections carry the lines they came from, bundled and unsourced selections carry none, mismatched ends carry none");
} finally {
  await page?.close?.();
  server?.close();
  removeTemporary(temporary);
}
