// The agent typesets math in a real Chromium, with the KaTeX the build put
// under dist/assets: the spans a markdown document arrives with become
// rendered formulas, the text published for anchoring is the typeset text,
// and a document without math fetches nothing.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until } from "../../tools/browser-driver.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const dist = join(root, "dist");
assert.ok(existsSync(join(dist, "agent.js")), "run `bun run build` first");
const temporary = mkdtempSync(join(tmpdir(), "librepaper-math-browser-"));

const types = { js: "text/javascript", css: "text/css", woff2: "font/woff2", html: "text/html" };
// The page is its own parent: the agent takes messages from `parent`, which
// at the top level is the window itself, so the check plays the sidebar.
const shell = `<!doctype html><html><head><meta charset="utf-8"></head><body>
<script>window.published=[];addEventListener("message",e=>{if(e.data&&e.data.librepaper&&e.data.type==="ready")window.published.push(e.data.text)});</script>
<script src="/agent.js?reader=*"></script>
</body></html>`;

let server, page;
try {
  server = createServer((request, response) => {
    const path = new URL(request.url, "http://127.0.0.1").pathname;
    if (path === "/") {
      response.setHeader("content-type", types.html);
      response.end(shell);
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

  // Switching to local Quarto keeps its root layout selectors and stops
  // imposing the Markdown canvas. Switching back removes its attributes.
  await preview('<html lang="fr"><head><style>body.fullcontent { max-width: 1100px; padding: 7px; } body.fullcontent main { color: rgb(12, 34, 56); }</style></head><body class="fullcontent" id="quarto" onload="window.badHandler=true"><main>Quarto output</main></body></html>', "document");
  await until("Quarto output published", () => page.evaluate('window.published.at(-1) === "Quarto output"'), 5000);
  assert.equal(await page.evaluate('getComputedStyle(document.body).maxWidth'), "1100px");
  assert.equal(await page.evaluate('getComputedStyle(document.body).padding'), "7px");
  assert.equal(await page.evaluate('getComputedStyle(document.querySelector("main")).color'), "rgb(12, 34, 56)");
  assert.equal(await page.evaluate('document.documentElement.lang'), "fr");
  assert.equal(await page.evaluate('document.body.hasAttribute("onload")'), false);

  // No math, nothing fetched.
  await preview("<!doctype html><html><head><title>Plain</title></head><body><p>No formulas here.</p></body></html>");
  await until("plain text published", () => page.evaluate('window.published.at(-1) === "No formulas here."'), 5000);
  assert.equal(await page.evaluate('getComputedStyle(document.body).maxWidth'), "none");
  assert.equal(await page.evaluate('getComputedStyle(document.body).margin'), "0px");
  assert.equal(await page.evaluate('document.body.getBoundingClientRect().width === document.documentElement.clientWidth'), true);
  assert.equal(await page.evaluate('document.body.getBoundingClientRect().height >= innerHeight'), true);
  assert.equal(await page.evaluate('document.body.hasAttribute("class") || document.body.hasAttribute("id") || document.documentElement.hasAttribute("lang")'), false);
  assert.equal(await page.evaluate('document.querySelectorAll("link[href*=katex], script[src*=katex]").length'), 0);

  // Math: KaTeX arrives from this origin, the spans are rendered, and the
  // published text is what the formula now reads as.
  await preview('<!doctype html><html><head><title>Math</title></head><body><p>Let <span data-math-style="inline">x^2 + y_1</span> hold.</p><p><span data-math-style="display">\\sum_{i=1}^n a_i</span></p><p><span data-math-style="inline">\\frac{1}{2}</span></p></body></html>');
  // Display math is wrapped once more, in KaTeX's own display block, so
  // the rendering is looked for anywhere inside the renderer's span.
  const rendered = '[...document.querySelectorAll("span[data-math-style]")].filter((s) => s.querySelector(".katex, .katex-error")).length';
  await until("formulas typeset", () => page.evaluate(`${rendered} === 3`), 15000);
  assert.equal(await page.evaluate('document.querySelectorAll("link[href*=katex]").length'), 1, "one stylesheet");
  assert.equal(await page.evaluate('document.querySelectorAll("script[src*=katex]").length'), 1, "one script");
  assert.equal(await page.evaluate('document.querySelector("span[data-math-style=display] .katex-display") !== null'), true, "display math is displayed");
  assert.equal(await page.evaluate('document.querySelector("span[data-math-style] math") === null'), true, "HTML only: no MathML doubling the text");
  // KaTeX writes zero-width spaces between its pieces, which are not \s.
  const typesetText = String.raw`/Letx2\+y1hold\./.test(window.published.at(-1).replace(/[\s​]+/g, ""))`;
  await until("typeset text published", () => page.evaluate(typesetText), 5000);
  // The faces came from here, and loaded.
  await until("a KaTeX face loaded", () => page.evaluate('document.fonts.check("12px KaTeX_Main")'), 10000);
  const sources = await page.evaluate('performance.getEntriesByType("resource").map(e=>new URL(e.name).pathname).filter(p=>p.includes("katex"))');
  assert.ok(sources.some((p) => p.endsWith("/katex.min.js")), `KaTeX fetched: ${sources}`);
  assert.ok(sources.some((p) => p.includes("/fonts/") && p.endsWith(".woff2")), `a face fetched: ${sources}`);
  assert.ok(sources.every((p) => p.startsWith("/assets/katex-")), `all from this origin's build: ${sources}`);

  // The next repaint is typeset before the text is published: one "ready"
  // for it, and it already reads as typeset.
  const before = await page.evaluate("window.published.length");
  await preview('<!doctype html><html><head><title>Math</title></head><body><p>Now <span data-math-style="inline">z^3</span>.</p></body></html>');
  await until("second repaint published", () => page.evaluate(`window.published.length > ${before}`), 5000);
  assert.match(await page.evaluate(`window.published[${before}]`), /Now z3\./, "typeset in the same breath as the repaint");

  // A formula with a mistake is shown, in KaTeX's own error colour, rather
  // than taking the page down.
  await preview('<!doctype html><html><head><title>Math</title></head><body><p><span data-math-style="inline">\\frac{1</span> and <span data-math-style="inline">ok</span></p></body></html>');
  await until("the bad formula is still on the page", () => page.evaluate(`${rendered} === 2`), 5000);
  assert.equal(await page.evaluate('document.querySelector("span[data-math-style] .katex-error") !== null'), true, "shown as an error");

  console.log("math-browser: KaTeX from this origin, typeset spans, published text, display mode, errors kept passed");
} finally {
  await page?.close?.();
  server?.close();
  rmSync(temporary, { recursive: true, force: true });
}
