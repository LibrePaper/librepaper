// The agent draws Mermaid diagrams in a real Chromium, with the Mermaid the
// build put under dist/assets: a fenced block becomes a figure holding its
// SVG, a caption comes from the block's fig-cap, a block Mermaid cannot draw
// stays as the listing it was, a repaint keeps the drawn SVG it already has,
// and a document without diagrams fetches nothing.
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
assertFreshDist("mermaid-browser");
const temporary = mkdtempSync(join(tmpdir(), "librepaper-mermaid-browser-"));

const types = { js: "text/javascript", css: "text/css", woff2: "font/woff2", html: "text/html" };
// The page is its own parent, as in math-browser: the frame script takes
// messages from `parent`, which at the top level is this window, so the check
// plays the sidebar. The query string names this page's real origin, which is
// not known until the server has picked its port.
const shell = (origin) => `<!doctype html><html><head><meta charset="utf-8"></head><body>
<script>window.published=[];addEventListener("message",e=>{if(e.data&&e.data.librepaper&&e.data.type==="ready")window.published.push(e.data.text)});</script>
<script src="/frame.js?reader=${encodeURIComponent(origin)}"></script>
</body></html>`;

// Every path the server is asked for, so the test can say what was fetched.
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

  const preview = (html, presentation) => page.evaluate(`postMessage(${JSON.stringify({librepaper:true,type:"preview",html,presentation})},"*")`);

  // No diagrams, nothing fetched.
  await preview('<!doctype html><html><head><title>Plain</title></head><body><p>No diagrams here.</p></body></html>');
  await until("plain text published", () => page.evaluate('window.published.at(-1) === "No diagrams here."'), 5000);
  assert.equal(requested.some((p) => p.includes("mermaid")), false, `no Mermaid fetched: ${requested}`);

  // Three fenced blocks: the first carries a caption, the second is a plain
  // Mermaid <pre>, the third is invalid and must stay as a listing.
  const diagramPage = '<!doctype html><html><head><title>Diagrams</title></head><body><p>Before</p><pre><code class="language-mermaid">%%| fig-cap: "Flow"\nflowchart LR\n  A[Start] --&gt; B[End]\n</code></pre><pre class="mermaid">graph TD\n  X--&gt;Y</pre><pre><code class="language-mermaid">flowchart LR\n  A --&gt;</code></pre></body></html>';
  await preview(diagramPage);
  await until("two diagrams drawn", () => page.evaluate('document.querySelectorAll("figure.lp-mermaid svg").length === 2'), 20000);
  assert.equal(await page.evaluate('document.querySelectorAll("figure.lp-mermaid")[0].querySelector("figcaption")?.textContent.trim()'), "Flow", "first figure captioned");
  assert.equal(await page.evaluate('document.querySelectorAll("code.language-mermaid").length'), 1, "the invalid block is still a listing");
  assert.equal(await page.evaluate('document.querySelector("code.language-mermaid")?.textContent.includes("flowchart LR")'), true, "the listing is the invalid source");
  await until("diagram text published", () => page.evaluate('window.published.at(-1).includes("Start")'), 5000);
  assert.ok(requested.some((p) => p.startsWith("/assets/mermaid-") && p.endsWith("/mermaid.min.js")), `Mermaid fetched: ${requested}`);

  // The same page again: the drawn SVG is the same node, not a new drawing.
  await page.evaluate('window.firstSvg = document.querySelector("figure.lp-mermaid svg")');
  const before = await page.evaluate("window.published.length");
  await preview(diagramPage);
  await until("second repaint published", () => page.evaluate(`window.published.length > ${before}`), 5000);
  assert.equal(await page.evaluate('document.querySelector("figure.lp-mermaid svg") === window.firstSvg'), true, "an unchanged diagram keeps its SVG");

  // Switching the paper to dark, as the reader's theme message does, redraws
  // every diagram in the dark palette: new SVG nodes, marked with the theme.
  await page.evaluate('window.darkBefore = document.querySelector("figure.lp-mermaid svg")');
  await page.evaluate(`postMessage(${JSON.stringify({ librepaper: true, type: "theme", mode: "dark", paper: "dark" })},"*")`);
  await until("diagrams redrawn in the dark theme", () => page.evaluate('document.querySelectorAll("figure.lp-mermaid").length === 2 && [...document.querySelectorAll("figure.lp-mermaid")].every((f) => f.getAttribute("data-lp-mermaid-theme") === "dark")'), 20000);
  assert.equal(await page.evaluate('document.querySelector("figure.lp-mermaid svg") === window.darkBefore'), false, "the dark diagram is a new SVG");
  assert.equal(await page.evaluate('document.documentElement.classList.contains("librepaper-dark")'), true, "the page is dark");

  console.log("mermaid-browser: Mermaid from this origin, captioned figures, invalid blocks kept, unchanged diagrams kept, redrawn on a paper change, no fetch without diagrams passed");
} finally {
  await page?.close?.();
  server?.close();
  removeTemporary(temporary);
}
