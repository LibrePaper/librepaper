// Real browser acceptance: source edits must reach the PDF text layer, twice,
// and a signed-out reader must see the stored result without loading Typst.
// Usage: node web/tools/typst-pdf-browser.mjs [binary] [firefox|chromium|both] [--html]
// --html measures the old renderer with the same edit fixture before migration.
import assert from "node:assert/strict";
import { browser, until, pause } from "./browser-driver.mjs";
import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createHmac } from "node:crypto";

const binary = resolve(process.argv[2] || "target/release/librepaper");
const browsers = process.argv[3] && process.argv[3] !== "both" ? [process.argv[3]] : ["firefox", "chromium"];
const baseline = process.argv.includes("--html");
const request = globalThis.fetch;
globalThis.fetch = (url, init = {}) => request(url, { ...init, signal: init.signal || AbortSignal.timeout(5000) });
for (const name of browsers) {
  const directory = mkdtempSync(join(tmpdir(), "librepaper-typst-pdf-"));
  const port = 20000 + Math.floor(Math.random() * 10000);
  const base = `http://localhost:${port}`;
  const environment = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("LIBREPAPER_")));
  const server = spawn(binary, ["serve", "--port", String(port), "--data", join(directory, "data"), "--publishers", "anyone", "--commenters", "anyone"], { stdio: ["ignore", "ignore", "pipe"], env: environment });
  let serverError = "";
  server.stderr.on("data", (bytes) => { serverError += bytes; });
  let tab;
  try {
    await until("server", async () => {
      if (server.exitCode !== null) throw new Error(`server exited: ${serverError}`);
      return (await fetch(`${base}/api/config`)).ok;
    }, 15000);
    const secret = Buffer.from(readFileSync(join(directory, "data/session.key"), "utf8").trim(), "hex");
    const payload = Buffer.from(`owner|owner|${Math.floor(Date.now() / 1000) + 3600}`).toString("base64url");
    const cookie = `${payload}.${createHmac("sha256", secret).update(payload).digest("base64url")}`;
    const source = "= Typst PDF acceptance\n\nOriginal paragraph with office ligatures and café.\n\n$ y = x^2 $\n";
    const response = await fetch(`${base}/api/documents`, { method: "POST", headers: {
      "content-type": "application/json", "x-librepaper-client": "1", cookie: `librepaper_session=${cookie}`,
    }, body: JSON.stringify({ title: "Typst PDF acceptance", source_format: "typst", source }) });
    assert.equal(response.status, 201);
    const { slug, share_url: shareUrl } = await response.json();
    tab = await browser(name, join(directory, "browser"), port + 1);
    await tab.navigate(base);
    await until("origin", () => tab.evaluate(`location.origin === ${JSON.stringify(base)}`));
    await tab.evaluate(`document.cookie = ${JSON.stringify(`librepaper_session=${cookie}; path=/`)}`);
    const started = performance.now();
    await tab.navigate(`${base}/docs/${slug}`);
    await until("initial preview", async () => (await tab.text()).includes("Original paragraph"));
    const timings = { initial_ms: Math.round(performance.now() - started) };
    if (!baseline) assert.match(await tab.evaluate('document.querySelector("iframe").src'), /\/pdf\//);
    for (const [index, marker] of ["First visible revision.", "Second visible revision."].entries()) {
      const start = performance.now();
      await tab.insert(`${marker}\n\n`);
      await until(marker, async () => (await tab.text()).includes(marker));
      timings[`edit_${index + 1}_ms`] = Math.round(performance.now() - start);
    }
    console.log(name, baseline ? "HTML baseline" : "PDF", JSON.stringify(timings));
    if (!baseline) {
      const before = await tab.text();
      await tab.insert('#panic("acceptance failure")\n');
      await until("compile diagnostic", () => tab.evaluate(`!!document.querySelector('button[aria-label^="Diagnostics:"]')`));
      assert.equal(await tab.text(), before, "compile failure must retain the last successful PDF");
      await tab.insert(`Recovered preview.\n\nSecond visible revision.\n\nFirst visible revision.\n\n${source}`, true);
      await until("recovered source", () => tab.evaluate('!document.querySelector(".cm-content").innerText.includes("acceptance failure")'));
      await until("recovered preview", async () => (await tab.text()).includes("Recovered preview."));
      console.log(name, "waiting for the quiet-period artifact upload");
      await until("stored current PDF", async () => {
        const result = await fetch(`${base}/api/documents/${slug}/renderings/latest`, { headers: { "x-librepaper-client": "1", cookie: `librepaper_session=${cookie}` } });
        const latest = await result.json();
        return latest.current;
      }, 80000);
      await tab.evaluate('document.cookie = "librepaper_session=; Max-Age=0; path=/"');
      assert.ok(shareUrl, "published fixture must provide its reader link");
      // A fragment-only navigation would leave the mounted owner's session
      // running. Start a new page so the read-link identity is resolved anew.
      await tab.navigate("about:blank");
      await until("reader navigation", () => tab.evaluate('location.href === "about:blank"'));
      await tab.navigate(`${base}${shareUrl}`);
      await until("stored reader preview", async () => (await tab.text()).includes("Recovered preview."));
      assert.equal(await tab.evaluate('!!document.querySelector(".cm-content")'), false, "reader must not open an editor");
      assert.equal(await tab.evaluate('performance.getEntriesByType("resource").some(e => e.name.includes("/wasm/typst."))'), false, "reader must not download Typst");
      console.log(name, "PASS: repeated edits, failure recovery, stored reader PDF without compiler");
    }
  } catch (error) {
    if (tab) {
      console.error(name, "shell:", await tab.evaluate("document.body.innerText.slice(-2500)").catch(() => "unavailable"));
      console.error(name, "preview:", (await tab.text().catch(() => "unavailable")).slice(0, 1000));
    }
    throw error;
  } finally {
    await tab?.close();
    server.kill();
    await Promise.race([new Promise((done) => server.once("exit", done)), pause(2000)]);
    rmSync(directory, { recursive: true, force: true });
  }
}
