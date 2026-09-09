import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { createServer } from "node:http";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { browser, until } from "../tools/browser-driver.mjs";

const directory = mkdtempSync(join(tmpdir(), "librepaper-quarto-outbox-"));
const source = readFileSync(new URL("../src/lib/quarto-pending.js", import.meta.url));
const artifactSource = readFileSync(new URL("../src/lib/quarto-artifact.js", import.meta.url));
const server = createServer((request, response) => {
  response.setHeader("content-type", request.url.endsWith(".js") ? "text/javascript" : "text/html");
  response.end(request.url === "/pending.js" ? source : request.url === "/artifact.js" ? artifactSource : '<!doctype html><script type="module">import * as pending from "/pending.js"; import * as artifact from "/artifact.js"; window.pending = pending; window.artifact = artifact;</script>');
});
await new Promise((done) => server.listen(0, "127.0.0.1", done));
const base = `http://127.0.0.1:${server.address().port}`;
let tab;
try {
  tab = await browser(process.env.BROWSER || "firefox", join(directory, "profile"), 30000 + Math.floor(Math.random() * 10000));
  await tab.navigate(base);
  await until("outbox module", () => tab.evaluate("Boolean(window.pending)"));
  const payload = { manifest: { schema:"librepaper-quarto-bundle/v1", document_id:"paper", render_id:"render-1" }, blobs:[{data:"saved bytes"}], expected_generation:7 };
  await tab.evaluate(`pending.savePendingQuarto("paper", ${JSON.stringify(payload)}).then(() => true)`);
  await tab.navigate("about:blank");
  await tab.navigate(base);
  await until("reloaded outbox module", () => tab.evaluate("Boolean(window.pending)"));
  const restored = JSON.parse(await tab.evaluate('pending.loadPendingQuarto("paper").then(JSON.stringify)'));
  assert.deepEqual(restored, payload, "completed output and original selection generation survive reload");
  assert.equal(await tab.evaluate('pending.loadPendingQuarto("another-paper").then((value) => value === null)'), true);
  assert.equal(await tab.evaluate(`pending.savePendingQuarto("another-paper", ${JSON.stringify(payload)}).then(() => false, () => true)`), true, "wrong document scope is refused");
  await tab.evaluate('pending.clearPendingQuarto("paper", "old-render").then(() => true)');
  assert.equal(await tab.evaluate('pending.loadPendingQuarto("paper").then((value) => value.manifest.render_id)'), "render-1", "late acknowledgements cannot clear a different render");
  await tab.evaluate('pending.clearPendingQuarto("paper", "render-1").then(() => true)');
  assert.equal(await tab.evaluate('pending.loadPendingQuarto("paper").then((value) => value === null)'), true);
  const html = '<!doctype html><link rel="stylesheet" href="styles/main.css"><body onload="alert(1)"><img src="fig/Figure%20One.png"><iframe src="evil.html"></iframe><svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script><circle r="2"/></svg></body>';
  const resources = {
    "styles/main.css":'@import "nested/theme.css";',
    "styles/nested/theme.css":'@font-face { src: url("../fonts/Font%20One.woff2?v=1#face"); }',
    "styles/fonts/Font One.woff2":"test font bytes",
    "fig/Figure One.png":"test image bytes",
  };
  const descriptor = (path, content, mime) => ({ path, size:Buffer.byteLength(content), sha256:createHash("sha256").update(content).digest("hex"), mime });
  const manifest = { artifact:{ ...descriptor("report.html", html, "text/html"), entrypoint:"report.html", kind:"html" }, assets:Object.entries(resources).map(([path, content]) => descriptor(path, content, path.endsWith(".css") ? "text/css" : path.endsWith(".png") ? "image/png" : "font/woff2")) };
  const pdfText = "PDF fixture bytes";
  const pdfManifest = { artifact:{ ...descriptor("report.pdf", pdfText, "application/pdf"), entrypoint:"report.pdf", kind:"pdf" }, assets:[] };
  const result = JSON.parse(await tab.evaluate(`(async () => {
    const manifest = ${JSON.stringify(manifest)};
    const resources = ${JSON.stringify(resources)};
    const rendered = await artifact.prepareQuartoArtifact(manifest, new TextEncoder().encode(${JSON.stringify(html)}), (item) => new TextEncoder().encode(resources[item.path]));
    const nested = await (await fetch(rendered.assets["styles/nested/theme.css"])).text();
    const main = await (await fetch(rendered.assets["styles/main.css"])).text();
    const fontRewritten = nested.includes(rendered.assets["styles/fonts/Font One.woff2"] + "#face");
    const importRewritten = main.includes(rendered.assets["styles/nested/theme.css"]);
    const safe = !/<iframe|<script| onload=/i.test(rendered.html) && rendered.html.includes("data:image/svg+xml;base64,");
    const download = await fetch(rendered.downloadUrl);
    const downloadSafe = download.headers.get("content-type").startsWith("text/plain") && (await download.text()) === ${JSON.stringify(html)};
    const pdfText = ${JSON.stringify(pdfText)};
    const pdfManifest = ${JSON.stringify(pdfManifest)};
    const pdf = await artifact.prepareQuartoArtifact(pdfManifest, new TextEncoder().encode(pdfText), () => new Uint8Array());
    const pdfPreview = pdf.kind === "pdf" && new TextDecoder().decode(pdf.bytes) === pdfText;
    const pdfDownload = await fetch(pdf.downloadUrl);
    const pdfDownloadSafe = pdfDownload.headers.get("content-type").startsWith("application/pdf") && (await pdfDownload.text()) === pdfText;
    pdf.dispose();
    rendered.dispose();
    let refused = false;
    try { await artifact.prepareQuartoArtifact(manifest, new TextEncoder().encode(${JSON.stringify(html)}), () => new Uint8Array()); } catch { refused = true; }
    return JSON.stringify({fontRewritten, importRewritten, safe, refused, downloadSafe, pdfPreview, pdfDownloadSafe});
  })()`));
  assert.deepEqual(result, {fontRewritten:true, importRewritten:true, safe:true, refused:true, downloadSafe:true, pdfPreview:true, pdfDownloadSafe:true});
  console.log("quarto outbox: reload recovery, document scope, and late acknowledgement checks passed");
  console.log("quarto artifact: nested CSS/font closure, spaces, active content isolation, and digest checks passed");
} finally {
  await tab?.close();
  await new Promise((done) => server.close(done));
  rmSync(directory, { recursive:true, force:true });
}
