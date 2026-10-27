// The preview follows the reader's paper in a real Chromium: a theme message
// sets the dark class and the paper colours, a document's own data-theme and
// author styles win over it, and highlight marks are painted again in the
// new paper's wash when the paper changes.
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
assertFreshDist("preview-paper-browser");
const temporary = mkdtempSync(join(tmpdir(), "librepaper-preview-paper-browser-"));

const types = { js: "text/javascript", css: "text/css", woff2: "font/woff2", html: "text/html" };
// The page is its own parent: the frame script takes messages from `parent`,
// which at the top level is the window itself, so the check plays the
// sidebar. The query string names this page's real origin, which is known
// only once the server has picked its port.
const shell = (origin) => `<!doctype html><html><head><meta charset="utf-8"></head><body>
<script>window.published=[];addEventListener("message",e=>{if(e.data&&e.data.librepaper&&e.data.type==="ready")window.published.push(e.data.text)});</script>
<script src="/frame.js?reader=${encodeURIComponent(origin)}"></script>
</body></html>`;

let server, page;
try {
  server = createServer((request, response) => {
    const path = new URL(request.url, "http://127.0.0.1").pathname;
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
  const theme = (mode, paper) => page.evaluate(`postMessage(${JSON.stringify({librepaper:true,type:"theme",mode,paper})},"*")`);
  const highlight = (ranges) => page.evaluate(`postMessage(${JSON.stringify({librepaper:true,type:"highlight",ranges})},"*")`);

  // Before any theme message the paper is light, and so is the preview.
  await preview("<!doctype html><html><head><title>P</title></head><body><p>Paper text.</p></body></html>");
  await until("paper text published", () => page.evaluate('window.published.at(-1) === "Paper text."'), 5000);
  assert.equal(await page.evaluate('document.documentElement.classList.contains("librepaper-dark")'), false);
  assert.equal(await page.evaluate('getComputedStyle(document.body).backgroundColor'), "rgb(255, 255, 255)");
  assert.equal(await page.evaluate('getComputedStyle(document.body).color'), "rgb(0, 0, 0)");
  assert.equal(await page.evaluate('document.documentElement.dataset.theme'), "light");
  console.log("preview-paper-browser: light by default, before any theme message");

  // Dark paper: the class, the paper colours, and the scheme follow it. Images
  // stay white so a transparent plot drawn for white paper stays readable.
  await theme("dark", "dark");
  await until("dark class on", () => page.evaluate('document.documentElement.classList.contains("librepaper-dark")'), 5000);
  await preview('<!doctype html><html><head><title>P</title></head><body><p>Paper text.</p><img id="plot" src="data:image/gif;base64,R0lGODlhAQABAAAAACw="></body></html>');
  await until("plot is in the preview", () => page.evaluate('document.getElementById("plot") !== null'), 5000);
  assert.equal(await page.evaluate('getComputedStyle(document.body).backgroundColor'), "rgb(28, 31, 30)");
  assert.equal(await page.evaluate('getComputedStyle(document.body).color'), "rgb(228, 231, 229)");
  assert.equal(await page.evaluate('getComputedStyle(document.documentElement).colorScheme'), "dark");
  assert.equal(await page.evaluate('document.documentElement.dataset.theme'), "dark");
  assert.equal(await page.evaluate('getComputedStyle(document.getElementById("plot")).backgroundColor'), "rgb(255, 255, 255)");
  console.log("preview-paper-browser: dark paper colours, scheme and data-theme, image stays white");

  // A new preview does not lose the paper.
  await preview("<!doctype html><html><head><title>P</title></head><body><p>Still on paper.</p></body></html>");
  await until("second preview published", () => page.evaluate('window.published.at(-1) === "Still on paper."'), 5000);
  assert.equal(await page.evaluate('document.documentElement.classList.contains("librepaper-dark")'), true);
  console.log("preview-paper-browser: dark paper survives a new preview");

  // The document's own data-theme wins, and the frame's comes back without it.
  await preview('<html data-theme="light"><head></head><body><p>Own theme.</p></body></html>');
  await until("own theme published", () => page.evaluate('window.published.at(-1) === "Own theme."'), 5000);
  assert.equal(await page.evaluate('document.documentElement.dataset.theme'), "light");
  assert.equal(await page.evaluate('document.documentElement.classList.contains("librepaper-dark")'), true);
  await preview("<!doctype html><html><head><title>P</title></head><body><p>Frame theme.</p></body></html>");
  await until("frame theme published", () => page.evaluate('window.published.at(-1) === "Frame theme."'), 5000);
  assert.equal(await page.evaluate('document.documentElement.dataset.theme'), "dark");
  console.log("preview-paper-browser: the document's own data-theme wins, and the frame's returns without one");

  // Author styles are kept: the document's body rules are not overridden by
  // the preview canvas.
  await preview('<!doctype html><html><head><title>P</title><style>body{background:rgb(1, 2, 3);color:rgb(4, 5, 6)}</style></head><body><p>Author styled.</p></body></html>', "document");
  await until("author styled published", () => page.evaluate('window.published.at(-1) === "Author styled."'), 5000);
  assert.equal(await page.evaluate('getComputedStyle(document.body).backgroundColor'), "rgb(1, 2, 3)");
  assert.equal(await page.evaluate('getComputedStyle(document.body).color'), "rgb(4, 5, 6)");
  console.log("preview-paper-browser: author body styles are kept over the paper");

  // Always light: the dark class goes, and the flow preview is white again.
  await theme("dark", "light");
  await until("dark class off", () => page.evaluate('!document.documentElement.classList.contains("librepaper-dark")'), 5000);
  await preview("<!doctype html><html><head><title>P</title></head><body><p>Light again.</p></body></html>");
  await until("light again published", () => page.evaluate('window.published.at(-1) === "Light again."'), 5000);
  assert.equal(await page.evaluate('getComputedStyle(document.body).backgroundColor'), "rgb(255, 255, 255)");
  console.log("preview-paper-browser: always light removes the dark paper");

  // Marks are painted again in the wash of the paper they are on.
  await theme("dark", "dark");
  await until("dark class back on", () => page.evaluate('document.documentElement.classList.contains("librepaper-dark")'), 5000);
  await preview("<!doctype html><html><head><title>P</title></head><body><p>Marked words here.</p></body></html>");
  await until("marked text published", () => page.evaluate('window.published.at(-1) === "Marked words here."'), 5000);
  await highlight([{ id: "a1", start: 0, end: 6, motivation: "commenting" }]);
  await until("mark painted", () => page.evaluate('document.querySelector("mark[data-librepaper]") !== null'), 5000);
  const darkWash = await page.evaluate('document.querySelector("mark[data-librepaper]").style.background');
  await theme("dark", "light");
  await until("mark repainted for light paper", () => page.evaluate(`document.querySelector("mark[data-librepaper]")?.style.background !== ${JSON.stringify(darkWash)}`), 5000);
  const lightWash = await page.evaluate('document.querySelector("mark[data-librepaper]").style.background');
  assert.notEqual(lightWash, darkWash);
  console.log(`preview-paper-browser: marks repaint with the paper (${darkWash} then ${lightWash})`);

  console.log("preview-paper-browser: paper colours, class, data-theme, author styles and marks follow the reader passed");
} finally {
  await page?.close?.();
  server?.close();
  removeTemporary(temporary);
}
