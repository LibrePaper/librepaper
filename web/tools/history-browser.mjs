// End-to-end history acceptance against an isolated server and real browsers.
// Usage: node web/tools/history-browser.mjs [binary] [firefox|chromium|both]
import assert from "node:assert/strict";
import { browser, until, pause } from "./browser-driver.mjs";
import { spawn, execFile } from "node:child_process";
import { promisify } from "node:util";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const binary = resolve(process.argv[2] || "target/debug/librepaper");
const browsers = process.argv[3] && process.argv[3] !== "both"
  ? [process.argv[3]] : ["firefox", "chromium"];
const original = "# History acceptance\n\nThe red fox watches the quiet river.\n\nA paragraph to remove.\n";
const revised = "# History acceptance\n\nThe blue fox watches the quiet river.\n\nA newly added paragraph.\n";
const run = promisify(execFile);

// Runs a same-origin fetch from inside a page so the visitor cookie the
// landing page issued that browser (HttpOnly, so node can't read it) rides
// along automatically -- the anonymous-upload ownership this deployment
// actually uses. See crates/librepaper/src/server/signin.rs VISITOR_COOKIE
// and owner() in server/mod.rs.
async function pageRequest(tab, path, method = "GET", body) {
  const script = `(async () => {
    const response = await fetch(${JSON.stringify(path)}, {
      method: ${JSON.stringify(method)},
      headers: { "content-type": "application/json", "x-librepaper-client": "1" },
      ${body !== undefined ? `body: ${JSON.stringify(JSON.stringify(body))},` : ""}
    });
    const text = await response.text();
    return JSON.stringify({ ok: response.ok, status: response.status, text });
  })()`;
  const raw = await tab.evaluate(script);
  const { ok, status, text } = JSON.parse(raw);
  let result;
  try { result = JSON.parse(text); } catch { result = text; }
  assert.ok(ok, `${method} ${path}: ${status} ${text}`);
  return result;
}

for (const name of browsers) {
  const directory = mkdtempSync(join(tmpdir(), "librepaper-history-browser-"));
  const port = 20000 + Math.floor(Math.random() * 10000);
  const base = `http://localhost:${port}`;
  const environment = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("LIBREPAPER_")));
  const server = spawn(binary, ["serve", "--port", String(port), "--data", join(directory, "data"), "--publishers", "anyone", "--commenters", "anyone"], {
    stdio: ["ignore", "ignore", "pipe"], env: environment,
  });
  let serverError = "";
  server.stderr.on("data", (bytes) => { serverError += bytes; });
  let owner, reader;
  try {
    await until("server", async () => {
      if (server.exitCode !== null) throw new Error(`server exited: ${serverError}`);
      return (await fetch(`${base}/api/config`)).ok;
    }, 15000);

    // The owner is whoever the visitor cookie names: open the landing page
    // first so the shell mints one, then create the document from inside
    // that same page.
    owner = await browser(name, join(directory, "owner"), port + 1);
    // Wide enough that the reader's split layout renders the source pane
    // rather than collapsing to a single narrow-window pane.
    await owner.resize(1400, 900);
    await owner.navigate(base);
    const { slug, share_url: shareUrl } = await pageRequest(owner, "/api/documents", "POST", {
      title: "History acceptance", source_format: "markdown", source: original,
    });
    const historyPath = `/api/documents/${slug}/history`;
    const first = (await pageRequest(owner, historyPath)).checkpoints[0];
    assert.ok(first?.sha, "publishing records the original checkpoint");
    await pageRequest(owner, `${historyPath}/${first.sha}`, "PATCH", { label: "Original draft" });
    await owner.navigate(`${base}/docs/${slug}`);
    await until("editor", () => owner.evaluate('!!document.querySelector(".cm-content")'));
    await until("initial source synchronized", () => owner.evaluate('document.querySelector(".cm-content")?.textContent.includes("red fox")'));
    await until("initial preview", async () => (await owner.text()).includes("red fox"));
    await owner.insert(revised, true);
    await until("revised preview", async () => (await owner.text()).includes("blue fox"));
    await until("edited source reached the server", async () => {
      const snapshot = await pageRequest(owner, `/api/documents/${slug}/snapshot`);
      return snapshot.texts?.[snapshot.main] === revised || snapshot.source === revised;
    }, 15000);
    console.log(`${name}: revised source synchronized`);
    await pageRequest(owner, `/api/documents/${slug}/comments`, "POST", {
      type: "comment", exact: "blue fox", body: "Review the revision.",
    });
    await until("edited source checkpoint", async () =>
      (await pageRequest(owner, historyPath)).checkpoints.some(point => point.sha !== first.sha), 15000);
    const revisions = (await pageRequest(owner, historyPath)).checkpoints;
    const second = revisions.at(-1);
    assert.notEqual(second.sha, first.sha, "comment checkpoints the edited source");
    await pageRequest(owner, `${historyPath}/${second.sha}`, "PATCH", { label: "Revised draft" });
    const { stdout: diff } = await run(binary, ["diff", slug, first.sha.slice(0, 7), second.sha.slice(0, 7),
      "--server", base, "--key", `${base}${shareUrl}`], { env: environment });
    assert.match(diff, /-The red fox/);
    assert.match(diff, /\+The blue fox/);

    // An independent read-link visit must offer comparison without an editor
    // and without the visitor cookie the owner's page carries -- a separate
    // browser profile, rather than trying to clear an HttpOnly cookie from
    // script.
    reader = await browser(name, join(directory, "reader"), port + 2);
    await reader.resize(1400, 900);
    await reader.navigate(`${base}${shareUrl}`);
    await until("reader preview", async () => (await reader.text()).includes("blue fox"));
    assert.equal(await reader.evaluate('!!document.querySelector(".cm-content")'), false);
    await reader.evaluate('document.querySelector("button[aria-label=History]")?.click()');
    await until("reader history", () => reader.evaluate('document.body.innerText.includes("Original draft")'));
    await reader.evaluate('window.historyFrameMessages = []; addEventListener("message", event => { if (event.data?.type?.startsWith("semantic-redlines-")) historyFrameMessages.push(event.data); });');
    await reader.evaluate(`(() => {
      const button = document.querySelector('li[data-sha=${JSON.stringify(first.sha)}] button[aria-label="Compare since this version"]');
      if (!button) throw new Error('missing compare control');
      button.click();
    })()`);
    await until("visible word diff", () => reader.evaluate(`(() => {
      const text = document.querySelector('.history-hunks')?.textContent || '';
      return text.includes('red') && text.includes('blue') && text.includes('river');
    })()`));
    await until("rendered change mapping", async () => {
      const messages = JSON.parse(await reader.evaluate('JSON.stringify(window.historyFrameMessages)'));
      assert.ok(!messages.some(message => message.type === "semantic-redlines-rejected"), JSON.stringify(messages));
      return messages.some(message => message.type === "semantic-redlines-painted");
    });
    assert.equal(await reader.evaluate('!!document.querySelector("button[aria-label^=Restore]")'), false,
      "reader has no restore control");
    await reader.evaluate(`(() => {
      const button = [...document.querySelectorAll('.history-files button')].find(button => button.textContent.trim() === 'main.md');
      if (!button) throw new Error('missing changed-file control');
      button.click();
    })()`);
    await until("reader source diff", () => reader.evaluate(`(() => {
      const diff = document.querySelector('.history-file-diff');
      return diff?.querySelector('del')?.textContent.includes('red') && diff?.querySelector('ins')?.textContent.includes('blue');
    })()`));

    // A checkpoint link carries both the old version and its access key.
    const historical = new URL(`${base}${shareUrl}`);
    historical.searchParams.set("at", first.sha);
    await reader.navigate(historical.href);
    await until("historical URL", () => reader.evaluate(`location.search.includes(${JSON.stringify(first.sha)})`));
    console.log(`${name}: opened checkpoint URL`);
    await until("historical reader preview", async () => (await reader.text()).includes("red fox"));
    assert.ok(!(await reader.text()).includes("blue fox"), "historical link displays the earlier rendering");
    await reader.evaluate(`(() => {
      window.historyCopied = '';
      Object.defineProperty(navigator.clipboard, 'writeText', { configurable: true, value: async (text) => { window.historyCopied = text; } });
      document.querySelector('button[aria-label="Copy the link to this version"]').click();
    })()`);
    await until("copied checkpoint link", () => reader.evaluate('Boolean(window.historyCopied)'));
    const copied = new URL(await reader.evaluate('window.historyCopied'));
    assert.equal(copied.searchParams.get("at"), first.sha);
    assert.equal(copied.hash, historical.hash, "copied checkpoint retains share key");

    // Comparing from an old preview must capture the actual current tree.
    await until("current comparison control", () => reader.evaluate(`([...document.querySelectorAll('button')].some(button => button.textContent.trim() === 'Compare with current'))`));
    await reader.evaluate(`(() => {
      const button = [...document.querySelectorAll('button')].find(button => button.textContent.trim() === 'Compare with current');
      if (!button) throw new Error('missing current comparison');
      button.click();
    })()`);
    await until("captured current preview", async () => (await reader.text()).includes("blue fox"));
    assert.equal(await reader.evaluate('document.querySelector(\'[aria-label="Historical version"]\')?.textContent.includes("Current version")'), true);
    const later = revised.replace("blue fox", "green fox");
    await owner.insert(later, true);
    await until("newer edits notification", () => reader.evaluate('document.body.innerText.includes("Newer edits available")'));
    assert.ok((await reader.text()).includes("blue fox"), "comparison stays frozen after a peer edit");
    assert.ok(!(await reader.text()).includes("green fox"), "new peer text does not enter the frozen preview");
    await reader.evaluate(`([...document.querySelectorAll('button')].find(button => button.textContent.includes('Newer edits available'))).click()`);
    await until("refreshed comparison preview", async () => (await reader.text()).includes("green fox"));
    await owner.insert(revised, true);
    await until("owner returned to revised source", async () => (await owner.text()).includes("blue fox"));

    // The editor restores through the same route that the CLI uses.
    await owner.navigate(`${base}/docs/${slug}?at=${first.sha}`);
    await until("historical owner preview", async () => (await owner.text()).includes("red fox"));
    await until("restore control", () => owner.evaluate(`([...document.querySelectorAll('button')].some(button => button.textContent.trim() === 'Restore this version'))`));
    await owner.evaluate(`(() => {
      const button = [...document.querySelectorAll('button')].find(button => button.textContent.trim() === 'Restore this version');
      if (!button) throw new Error('missing restore control');
      button.click();
    })()`);
    await until("restore confirmation", () => owner.evaluate('document.body.innerText.includes("Restore this version?")'));
    await owner.evaluate(`([...document.querySelectorAll('button')].find(button => button.textContent.trim() === 'Restore version')).click()`);
    await until("restore event", async () => (await pageRequest(owner, historyPath)).checkpoints.at(-1)?.why === "restore");
    const restored = (await pageRequest(owner, historyPath)).checkpoints.at(-1);
    assert.equal(restored.why, "restore");
    assert.notEqual(restored.sha, first.sha, "restore records a new event");
    assert.equal(restored.parent, second.sha, "restore follows the version it replaced");
    const restoredTree = await pageRequest(owner, `${historyPath}/${restored.sha}`);
    assert.equal(restoredTree.texts[restoredTree.main], original);
    const { stdout: response } = await run(binary, ["export", slug, "--format", "response",
      "--server", base, "--key", `${base}${shareUrl}`], { env: environment });
    assert.match(response, /\*\*Now(?: \(source\))?:\*\*.*red fox/, "response export quotes the replacement passage");
    await owner.navigate(`${base}/docs/${slug}`);
    await until("restored source loaded", () => owner.evaluate('document.querySelector(".cm-content")?.textContent.includes("red fox")'));
    // A fresh editor visit can remember the source-only pane. Explicitly
    // reveal the document before asserting its visible preview text.
    await owner.evaluate('document.querySelector(\'nav[aria-label="Workspace view"] button[aria-label="Document"]\')?.click()');
    await until("restored live preview", async () => (await owner.text()).includes("red fox"));
    console.log(`${name}: history links, reader access, revision capture, and live restore passed`);
  } catch (error) {
    if (owner) console.error(name, "owner frame:", await owner.text().catch(() => "frame unavailable"));
    if (owner) console.error(name, "owner:", await owner.evaluate("document.body.innerText.slice(-4000)").catch(() => "page unavailable"));
    if (reader) console.error(name, "reader:", await reader.evaluate("location.href + '\\n' + document.body.innerText.slice(-4000)").catch(() => "page unavailable"));
    throw error;
  } finally {
    await owner?.close();
    await reader?.close();
    server.kill();
    await Promise.race([new Promise((done) => server.once("exit", done)), pause(2000)]);
    rmSync(directory, { recursive: true, force: true });
  }
}
