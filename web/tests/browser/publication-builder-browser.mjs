// The display bundle is assembled in Chromium because its DOMParser is the
// parser that turns a captured rendering into the public HTML page.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFileSync, mkdtempSync, rmSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { browser, until } from "../../tools/browser-driver.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const types = { ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript" };
const shell = `<!doctype html><script type="module">
  import { buildDisplayBundle } from "/src/lib/publication-builder.js";
  window.bundle = async (html, tree) => {
    const bytes = (entries = {}) => Object.fromEntries(Object.entries(entries).map(([path, value]) => [path, new Uint8Array(value)]));
    return buildDisplayBundle(html, { ...tree, assets: bytes(tree.assets) });
  };
</script>`;
let server, page;
const temporary = mkdtempSync(join(tmpdir(), "librepaper-publication-builder-"));

try {
  server = createServer((request, response) => {
    const pathname = new URL(request.url, "http://127.0.0.1").pathname;
    if (pathname === "/") { response.setHeader("content-type", "text/html"); response.end(shell); return; }
    const file = normalize(join(root, pathname));
    if (!file.startsWith(root) || !file.startsWith(join(root, "src"))) { response.statusCode = 404; response.end(); return; }
    try { response.setHeader("content-type", types[extname(file)] || "application/octet-stream"); response.end(readFileSync(file)); }
    catch { response.statusCode = 404; response.end(); }
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  page = await browser("chromium", join(temporary, "profile"), 31000 + Math.floor(Math.random() * 10000));
  await page.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("publication builder loaded", () => page.evaluate("typeof window.bundle === 'function'"), 5000);

  const run = (html, tree) => page.evaluate(`window.bundle(${JSON.stringify(html)}, ${JSON.stringify(tree)}).then(value => ({ ok: true, value: { ...value, assets: value.assets.map(asset => ({ ...asset, bytes: Array.from(asset.bytes) })) } }), error => ({ ok: false, error: error.message }))`);
  const tree = {
    main: "article/index.html",
    texts: {
      "article/styles/site.css": '@import "theme.css"; /* url("../private/comment.png") */ .label::before { content: "url(../private/quoted.png)" } .hero { background: url("../figures/diagram.png#detail") }',
      "article/styles/theme.css": '@font-face { src: url("../fonts/text.woff2") }',
      "article/boot.js": 'import("./private.js")',
      "sources/refs.bib": "@book{private}",
      "sources/article.tex": "\\input{private}",
    },
    assets: {
      "article/figures/diagram.png": [137, 80, 78, 71],
      "article/figures/copy.png": [137, 80, 78, 71],
      "article/fonts/text.woff2": [119, 79, 70, 50],
      "article/private/comment.png": [1, 2, 3, 4],
      "article/private/quoted.png": [5, 6, 7, 8],
      "sources/private.map": [1, 2, 3],
    },
    digests: { "sources/refs.bib": "a".repeat(64) },
  };
  const html = '<!doctype html><html><head><link rel="stylesheet" href="styles/site.css"></head><body><img src="figures/diagram.png"><img src="figures/copy.png" srcset="figures/diagram.png 1x, figures/copy.png 2x"></body></html>';
  const first = await run(html, tree);
  assert.equal(first.ok, true, first.error);
  assert.match(first.value.html, /loading="lazy"/);
  assert.doesNotMatch(first.value.html, /sources\/|private\.map|refs\.bib/);
  assert.equal(first.value.assets.some((asset) => /sources\/|private\.map|refs\.bib/.test(asset.path)), false, "unused compiler inputs never enter the public manifest");
  assert.equal(first.value.assets.filter((asset) => asset.mime === "image/png").length, 1, "identical binary images share one public object");
  assert.equal(first.value.assets.some((asset) => [1, 2, 3, 4, 5, 6, 7, 8].includes(asset.bytes[0])), false, "CSS comments and quoted content do not publish private image references");
  assert.equal(first.value.assets.filter((asset) => asset.mime === "text/css").length, 2, "referenced CSS and its import are public");
  const css = first.value.assets.filter((asset) => asset.mime === "text/css").map((asset) => new TextDecoder().decode(new Uint8Array(asset.bytes))).join("\n");
  const font = first.value.assets.find((asset) => asset.mime === "font/woff2");
  const image = first.value.assets.find((asset) => asset.mime === "image/png");
  const fontReference = css.match(/url\(["']?([^"')]+\.woff2)["']?\)/)?.[1];
  const imageReference = css.match(/url\(["']?([^"')]+\.png#detail)["']?\)/)?.[1];
  assert.ok(fontReference && imageReference, "nested CSS retains its referenced font and image");
  const stylesheetUrl = "https://docs.example/published/slug/assets/stylesheet.css";
  assert.equal(new URL(fontReference, stylesheetUrl).pathname, `/published/slug/${font.path}`);
  assert.equal(new URL(imageReference, stylesheetUrl).pathname, `/published/slug/${image.path}`);
  const changed = await run(html.replace("<body>", "<body><p>edited text</p>"), tree);
  assert.equal(changed.ok, true, changed.error);
  assert.deepEqual(changed.value.assets.filter((asset) => asset.mime === "image/png").map((asset) => asset.path), first.value.assets.filter((asset) => asset.mime === "image/png").map((asset) => asset.path), "text edits retain binary object names");
  const math = await run('<p>For <span data-math-style="inline">x^2</span>.</p>', tree);
  assert.equal(math.ok, true, math.error);
  assert.match(math.value.html, /data-math-style="inline"/, "renderer math markup remains publishable without copying app-owned KaTeX files");

  for (const [label, badHtml, expected] of [
    ["missing files", '<img src="missing.png">', "Missing display dependency: article/missing.png"],
    ["source maps", '<style>/*# sourceMappingURL=style.css.map */</style>', "Remove source map references"],
    ["compiler inputs", '<object data="../sources/refs.bib"></object>', "Unsupported public asset type: sources/refs.bib"],
    ["private module imports", '<script type="module">import "./private.js"</script>', "Bundle relative module imports before publishing: inline script"],
    ["script module imports", '<script src="boot.js"></script>', "Bundle relative module imports before publishing: article/boot.js"],
    ["runtime-only assets", '<link rel="stylesheet" href="/assets/katex.css">', "Unsupported display dependency: /assets/katex.css"],
  ]) {
    const result = await run(badHtml, tree);
    assert.equal(result.ok, false, `${label} unexpectedly published`);
    assert.match(result.error, new RegExp(expected));
  }
  console.log("publication-builder-browser: public dependency closure, binary deduplication and actionable rejections passed");
} finally {
  await page?.close?.();
  server?.close();
  rmSync(temporary, { recursive: true, force: true });
}
