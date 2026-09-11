import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { createServer } from "node:http";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { browser, until } from "../tools/browser-driver.mjs";

const directory = mkdtempSync(join(tmpdir(), "librepaper-quarto-outbox-"));
const source = readFileSync(new URL("../src/lib/results-pending.js", import.meta.url));
const artifactSource = readFileSync(new URL("../src/lib/results-artifact.js", import.meta.url));
const resultsPendingSource = readFileSync(new URL("../src/lib/results-pending.js", import.meta.url));
const resultsArtifactSource = readFileSync(new URL("../src/lib/results-artifact.js", import.meta.url));
const interactiveSource = readFileSync(new URL("../src/lib/results-interactive.js", import.meta.url));
const identitySource = readFileSync(new URL("../src/lib/engines/identity.js", import.meta.url));
// `results-artifact.js` reaches for the shared SHA-256 helper, so the page has
// to be able to resolve it too: one unresolvable import fails the whole inline
// module script, and the symptom is the outbox never appearing at all.
const digestSource = readFileSync(new URL("../src/lib/digest.js", import.meta.url));
const requests = [];
const server = createServer((request, response) => {
  requests.push(request.url);
  response.setHeader("content-type", request.url.endsWith(".js") ? "text/javascript" : "text/html");
  response.end(request.url === "/pending.js" ? source
    : request.url === "/artifact.js" ? artifactSource
      : request.url === "/results-pending.js" ? resultsPendingSource
        : request.url === "/results-artifact.js" ? resultsArtifactSource
          : request.url === "/results-interactive.js" ? interactiveSource
            : request.url === "/engines/identity.js" ? identitySource
              : request.url === "/digest.js" ? digestSource
                : '<!doctype html><script type="module">import * as pending from "/pending.js"; import * as artifact from "/artifact.js"; import * as results from "/results-artifact.js"; window.pending = pending; window.artifact = artifact; window.results = results;</script>');
});
await new Promise((done) => server.listen(0, "127.0.0.1", done));
const base = `http://127.0.0.1:${server.address().port}`;
let tab;
try {
  tab = await browser(process.env.BROWSER || "firefox", join(directory, "profile"), 30000 + Math.floor(Math.random() * 10000));
  await tab.navigate(base);
  await until("outbox module", () => tab.evaluate("Boolean(window.pending)"));
  const payload = { manifest: { schema:"librepaper-quarto-bundle/v1", document_id:"paper", render_id:"render-1" }, blobs:[{data:"saved bytes"}], expected_generation:7 };
  await tab.evaluate(`pending.savePendingResults("paper", ${JSON.stringify(payload)}).then(() => true)`);
  await tab.navigate("about:blank");
  await tab.navigate(base);
  await until("reloaded outbox module", () => tab.evaluate("Boolean(window.pending)"));
  const restored = JSON.parse(await tab.evaluate('pending.loadPendingResults("paper").then(JSON.stringify)'));
  assert.deepEqual(restored, payload, "completed output and original selection generation survive reload");
  assert.equal(await tab.evaluate('pending.loadPendingResults("another-paper").then((value) => value === null)'), true);
  assert.equal(await tab.evaluate(`pending.savePendingResults("another-paper", ${JSON.stringify(payload)}).then(() => false, () => true)`), true, "wrong document scope is refused");
  await tab.evaluate('pending.clearPendingResults("paper", "old-render").then(() => true)');
  assert.equal(await tab.evaluate('pending.loadPendingResults("paper").then((value) => value.manifest.render_id)'), "render-1", "late acknowledgements cannot clear a different render");
  await tab.evaluate('pending.clearPendingResults("paper", "render-1").then(() => true)');
  assert.equal(await tab.evaluate('pending.loadPendingResults("paper").then((value) => value === null)'), true);
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
    const rendered = await artifact.prepareResultsArtifact(manifest, new TextEncoder().encode(${JSON.stringify(html)}), (item) => new TextEncoder().encode(resources[item.path]));
    const nested = await (await fetch(rendered.assets["styles/nested/theme.css"])).text();
    const main = await (await fetch(rendered.assets["styles/main.css"])).text();
    const fontRewritten = nested.includes(rendered.assets["styles/fonts/Font One.woff2"] + "#face");
    const importRewritten = main.includes(rendered.assets["styles/nested/theme.css"]);
    const safe = !/<iframe|<script| onload=/i.test(rendered.html) && rendered.html.includes("data:image/svg+xml;base64,");
    const download = await fetch(rendered.downloadUrl);
    const downloadSafe = download.headers.get("content-type").startsWith("text/plain") && (await download.text()) === ${JSON.stringify(html)};
    const pdfText = ${JSON.stringify(pdfText)};
    const pdfManifest = ${JSON.stringify(pdfManifest)};
    const pdf = await artifact.prepareResultsArtifact(pdfManifest, new TextEncoder().encode(pdfText), () => new Uint8Array());
    const pdfPreview = pdf.kind === "pdf" && new TextDecoder().decode(pdf.bytes) === pdfText;
    const pdfDownload = await fetch(pdf.downloadUrl);
    const pdfDownloadSafe = pdfDownload.headers.get("content-type").startsWith("application/pdf") && (await pdfDownload.text()) === pdfText;
    pdf.dispose();
    rendered.dispose();
    let refused = false;
    try { await artifact.prepareResultsArtifact(manifest, new TextEncoder().encode(${JSON.stringify(html)}), () => new Uint8Array()); } catch { refused = true; }
    return JSON.stringify({fontRewritten, importRewritten, safe, refused, downloadSafe, pdfPreview, pdfDownloadSafe});
  })()`));
  assert.deepEqual(result, {fontRewritten:true, importRewritten:true, safe:true, refused:true, downloadSafe:true, pdfPreview:true, pdfDownloadSafe:true});
  console.log("quarto outbox: reload recovery, document scope, and late acknowledgement checks passed");
  const isolation = await tab.evaluate(`(async () => {
    const source = '<script>let isolated=false; try { parent.document.body; } catch { isolated=true; } fetch("/credential-probe").then(() => parent.postMessage({widget:false}, "*"), () => parent.postMessage({widget:isolated}, "*"));<\/script>';
    const bytes = new TextEncoder().encode(source);
    const sha256 = [...new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))].map(x=>x.toString(16).padStart(2,"0")).join("");
    const rendered = await artifact.prepareResultsArtifact({artifact:{kind:"html",entrypoint:"widget.html",mime:"text/html",sha256,size:bytes.length},assets:[]}, bytes, () => null);
    const frame = document.createElement("iframe"); frame.sandbox="allow-scripts";
    const result = new Promise(resolve => { window.addEventListener("message", function listener(event) { if(event.source === frame.contentWindow && event.data && "widget" in event.data) { window.removeEventListener("message", listener); resolve(event.data.widget && event.origin === "null"); } }); });
    frame.srcdoc=rendered.page("widget.html", true); document.body.append(frame);
    const safe=await result; frame.remove(); rendered.dispose(); return safe;
  })()`);
  assert.equal(isolation, true, "widget scripts execute with opaque origin and blocked fetch");
  console.log("quarto artifact: nested CSS/font closure, spaces, active content isolation, and digest checks passed");

  // The ordinary Reader output wraps this static page in an empty-sandbox
  // iframe before posting it through the document-origin agent. Keep that
  // boundary covered separately from the opt-in widget test above: a relative
  // URL that is absent from the verified inventory must not become a request
  // against the document/application origin.
  requests.length = 0;
  const staticHtml = '<!doctype html><script>parent.postMessage({ran:true}, "*"); fetch("/credential-probe-static");</script><a href="/credential-probe-static">probe</a>';
  const staticBytes = new TextEncoder().encode(staticHtml);
  const staticSha = [...new Uint8Array(await crypto.subtle.digest("SHA-256", staticBytes))].map(x => x.toString(16).padStart(2, "0")).join("");
  const staticManifest = { artifact:{kind:"html",entrypoint:"report.html",mime:"text/html",sha256:staticSha,size:staticBytes.length}, assets:[] };
  const opaque = await tab.evaluate(`(async () => {
    const bytes = new TextEncoder().encode(${JSON.stringify(staticHtml)});
    const prepared = await results.prepareResultsArtifact(${JSON.stringify(staticManifest)}, bytes, () => null);
    const frame = document.createElement("iframe");
    frame.sandbox = "";
    frame.referrerPolicy = "no-referrer";
    const page = prepared.page("report.html", false);
    frame.srcdoc = page;
    document.body.append(frame);
    await new Promise(resolve => { frame.onload = resolve; setTimeout(resolve, 250); });
    const inaccessible = frame.contentDocument === null;
    frame.remove();
    prepared.dispose();
    return JSON.stringify({inaccessible, rewritten: !page.includes("credential-probe-static")});
  })()`);
  const staticIsolation = JSON.parse(opaque);
  assert.deepEqual(staticIsolation, {inaccessible:true, rewritten:true}, "static saved output stays opaque and rewrites unresolved resources");
  assert.equal(requests.some((url) => url === "/credential-probe-static"), false, "static output cannot request an app-origin resource");
  console.log("quarto artifact: static output sandbox and unresolved-resource isolation passed");
} finally {
  await tab?.close();
  await new Promise((done) => server.close(done));
  rmSync(directory, { recursive:true, force:true });
}
