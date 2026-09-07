// End-to-end history acceptance against an isolated server and real browsers.
// Usage: node web/tools/history-browser.mjs [binary] [firefox|chromium|both]
import assert from "node:assert/strict";
import { browser, until, pause } from "./browser-driver.mjs";
import { spawn, execFile } from "node:child_process";
import { promisify } from "node:util";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createHmac } from "node:crypto";

const binary = resolve(process.argv[2] || "target/debug/komodoc");
const browsers = process.argv[3] && process.argv[3] !== "both"
  ? [process.argv[3]] : ["firefox", "chromium"];
const original = "# History acceptance\n\nThe red fox watches the quiet river.\n\nA paragraph to remove.\n";
const revised = "# History acceptance\n\nThe blue fox watches the quiet river.\n\nA newly added paragraph.\n";
const run = promisify(execFile);

for (const name of browsers) {
  const directory = mkdtempSync(join(tmpdir(), "komodoc-history-browser-"));
  const port = 20000 + Math.floor(Math.random() * 10000);
  const base = `http://localhost:${port}`;
  const environment = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KOMODOC_")));
  const server = spawn(binary, ["serve", "--port", String(port), "--data", join(directory, "data"), "--publishers", "anyone", "--commenters", "anyone"], {
    stdio: ["ignore", "ignore", "pipe"], env: environment,
  });
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
    const headers = { "content-type": "application/json", "x-komodoc-client": "1", cookie: `komodoc_session=${cookie}` };
    const request = async (path, method = "GET", body) => {
      const response = await fetch(`${base}${path}`, { method, headers, ...(body ? { body: JSON.stringify(body) } : {}) });
      const result = await response.json();
      assert.ok(response.ok, `${method} ${path}: ${response.status} ${JSON.stringify(result)}`);
      return result;
    };
    const { slug, share_url: shareUrl } = await request("/api/documents", "POST", {
      title: "History acceptance", source_format: "markdown", source: original,
    });
    const historyPath = `/api/documents/${slug}/history`;
    const first = (await request(historyPath)).checkpoints[0];
    assert.ok(first?.sha, "publishing records the original checkpoint");
    await request(`${historyPath}/${first.sha}`, "PATCH", { label: "Original draft" });
    tab = await browser(name, join(directory, "browser"), port + 1);
    await tab.navigate(base);
    await tab.evaluate(`document.cookie = ${JSON.stringify(`komodoc_session=${cookie}; path=/`)}`);
    await tab.navigate(`${base}/docs/${slug}`);
    await until("editor", () => tab.evaluate('!!document.querySelector(".cm-content")'));
    await tab.insert(revised, true);
    await until("revised preview", async () => (await tab.text()).includes("blue fox"));
    await request(`/api/documents/${slug}/comments`, "POST", {
      type: "comment", exact: "blue fox", body: "Review the revision.",
    });
    const revisions = (await request(historyPath)).checkpoints;
    const second = revisions.at(-1);
    assert.notEqual(second.sha, first.sha, "comment checkpoints the edited source");
    await request(`${historyPath}/${second.sha}`, "PATCH", { label: "Revised draft" });
    const { stdout: diff } = await run(binary, ["diff", slug, first.sha.slice(0, 7), second.sha.slice(0, 7),
      "--server", base, "--key", `${base}${shareUrl}`], { env: environment });
    assert.match(diff, /-The red fox/);
    assert.match(diff, /\+The blue fox/);

    // An independent read-link visit must offer comparison without an editor.
    await tab.evaluate('document.cookie = "komodoc_session=; Max-Age=0; path=/"');
    await tab.navigate("about:blank");
    await tab.navigate(`${base}${shareUrl}`);
    await until("reader preview", async () => (await tab.text()).includes("blue fox"));
    assert.equal(await tab.evaluate('!!document.querySelector(".cm-content")'), false);
    await tab.evaluate('document.querySelector("button[aria-label=History]")?.click()');
    await until("reader history", () => tab.evaluate('document.body.innerText.includes("Original draft")'));
    await tab.evaluate(`(() => {
      const select = document.querySelector('.history-changes select');
      if (!select) throw new Error('missing baseline selector');
      select.value = ${JSON.stringify(first.sha)};
      select.dispatchEvent(new Event('change', { bubbles: true }));
    })()`);
    await until("visible word diff", () => tab.evaluate(`(() => {
      const text = document.querySelector('.history-hunks')?.textContent || '';
      return text.includes('red') && text.includes('blue') && text.includes('river');
    })()`));
    assert.equal(await tab.evaluate('!!document.querySelector("button[aria-label^=Restore]")'), false,
      "reader has no restore control");
    await tab.evaluate(`(() => {
      const button = [...document.querySelectorAll('.history-changes button')].find(button => button.textContent.trim() === 'main.md');
      if (!button) throw new Error('missing changed-file control');
      button.click();
    })()`);
    await until("reader source diff", () => tab.evaluate(`(() => {
      const diff = document.querySelector('.history-file-diff');
      return diff?.querySelector('del')?.textContent.includes('red') && diff?.querySelector('ins')?.textContent.includes('blue');
    })()`));

    // A checkpoint link carries both the old version and its access key.
    const historical = new URL(`${base}${shareUrl}`);
    historical.searchParams.set("at", first.sha);
    await tab.navigate("about:blank");
    await tab.navigate(historical.href);
    await until("historical reader preview", async () => (await tab.text()).includes("red fox"));
    assert.ok(!(await tab.text()).includes("blue fox"), "historical link displays the earlier rendering");
    await tab.evaluate(`(() => {
      window.historyCopied = '';
      Object.defineProperty(navigator.clipboard, 'writeText', { configurable: true, value: async (text) => { window.historyCopied = text; } });
      document.querySelector('button[aria-label="Copy the link to this version"]').click();
    })()`);
    await until("copied checkpoint link", () => tab.evaluate('Boolean(window.historyCopied)'));
    const copied = new URL(await tab.evaluate('window.historyCopied'));
    assert.equal(copied.searchParams.get("at"), first.sha);
    assert.equal(copied.hash, historical.hash, "copied checkpoint retains share key");

    // The editor restores through the same route that the CLI uses.
    await tab.evaluate(`document.cookie = ${JSON.stringify(`komodoc_session=${cookie}; path=/`)}`);
    await tab.navigate("about:blank");
    await tab.navigate(`${base}/docs/${slug}?at=${first.sha}`);
    await until("historical owner preview", async () => (await tab.text()).includes("red fox"));
    await tab.evaluate(`(() => {
      const button = [...document.querySelectorAll('button')].find(button => button.textContent.trim() === 'Restore this version');
      if (!button) throw new Error('missing restore control');
      button.click();
    })()`);
    await until("restore event", async () => (await request(historyPath)).checkpoints.at(-1)?.why === "restore");
    const restored = (await request(historyPath)).checkpoints.at(-1);
    assert.equal(restored.why, "restore");
    assert.notEqual(restored.sha, first.sha, "restore records a new event");
    assert.equal(restored.parent, second.sha, "restore follows the version it replaced");
    const restoredTree = await request(`${historyPath}/${restored.sha}`);
    assert.equal(restoredTree.texts[restoredTree.main], original);
    const { stdout: response } = await run(binary, ["export", slug, "--format", "response",
      "--server", base, "--key", `${base}${shareUrl}`], { env: environment });
    assert.match(response, /\*\*Now:\*\*.*red fox/, "response export quotes the replacement passage");
    await tab.navigate(`${base}/docs/${slug}`);
    await until("restored live preview", async () => (await tab.text()).includes("red fox"));
    console.log(`${name}: history links, reader access, revision capture, and live restore passed`);
  } catch (error) {
    if (tab) console.error(name, await tab.evaluate("document.body.innerText.slice(-4000)").catch(() => "page unavailable"));
    throw error;
  } finally {
    await tab?.close();
    server.kill();
    await Promise.race([new Promise((done) => server.once("exit", done)), pause(2000)]);
    rmSync(directory, { recursive: true, force: true });
  }
}
