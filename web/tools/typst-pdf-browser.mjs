// Real browser acceptance: source edits must reach the PDF text layer, twice,
// and a signed-out reader must see the stored result without loading Typst.
// Usage: node web/tools/typst-pdf-browser.mjs [binary] [firefox|chromium|both] [--html]
// --html measures the old renderer with the same edit fixture before migration.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createHmac } from "node:crypto";

const binary = resolve(process.argv[2] || "target/release/komodoc");
const browsers = process.argv[3] && process.argv[3] !== "both" ? [process.argv[3]] : ["firefox", "chromium"];
const baseline = process.argv.includes("--html");
const request = globalThis.fetch;
globalThis.fetch = (url, init = {}) => request(url, { ...init, signal: init.signal || AbortSignal.timeout(5000) });
const pause = (ms) => new Promise((done) => setTimeout(done, ms));
async function until(label, test, timeout = 60000) {
  const deadline = Date.now() + timeout;
  let last;
  while (Date.now() < deadline) {
    try { if (await test()) return; } catch (error) { last = error; }
    await pause(150);
  }
  throw new Error(`${label} timed out${last ? `: ${last.message}` : ""}`);
}

function protocol(socket) {
  let serial = 0;
  const pending = new Map();
  socket.addEventListener("message", ({ data }) => {
    const message = JSON.parse(data);
    const job = pending.get(message.id);
    if (!job) return;
    pending.delete(message.id);
    clearTimeout(job.timer);
    message.error || message.type === "error"
      ? job.reject(new Error(JSON.stringify(message))) : job.resolve(message.result);
  });
  return (method, params = {}, sessionId) => new Promise((resolve, reject) => {
    const id = ++serial;
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`${method} timed out`)); }, 60000);
    pending.set(id, { resolve, reject, timer });
    socket.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
  });
}

async function browser(name, directory, port) {
  mkdirSync(directory, { recursive: true });
  const child = spawn(name, name === "firefox"
    ? ["--headless", "--no-remote", "--profile", directory, "--remote-debugging-port", String(port)]
    : ["--headless=new", "--no-sandbox", "--disable-gpu", "--disable-dev-shm-usage", `--user-data-dir=${directory}`, `--remote-debugging-port=${port}`, "about:blank"],
  { stdio: "ignore" });
  let spawnError;
  child.on("error", (error) => { spawnError = error; });
  let socket;
  try {
    await until(`${name} startup`, async () => {
      if (spawnError) throw spawnError;
      const endpoint = name === "firefox" ? `ws://127.0.0.1:${port}/session`
        : (await (await fetch(`http://127.0.0.1:${port}/json/version`)).json()).webSocketDebuggerUrl;
      socket = new WebSocket(endpoint);
      await new Promise((done, fail) => { socket.onopen = done; socket.onerror = fail; });
      return true;
    }, 20000);
    const send = protocol(socket);
    const close = async () => {
      socket.close();
      child.kill();
      await Promise.race([new Promise((done) => child.once("exit", done)), pause(2000)]);
    };
    if (name === "firefox") {
      await send("session.new", { capabilities: {} });
      const { context } = await send("browsingContext.create", { type: "tab" });
      const evaluate = async (expression, frame = context) => {
        const result = await send("script.evaluate", { expression, target: { context: frame }, awaitPromise: true });
        if (result.type === "exception") throw new Error(result.exceptionDetails.text);
        return result.result.value;
      };
      return {
        close, evaluate,
        navigate: (url) => send("browsingContext.navigate", { context, url, wait: "complete" }),
        text: async () => {
          const tree = await send("browsingContext.getTree", { root: context });
          const frame = tree.contexts[0].children?.[0]?.context;
          return frame ? evaluate("document.body.innerText", frame) : "";
        },
        insert: async (text, replace = false) => {
          await evaluate('document.querySelector(".cm-content").focus()');
          const down = (value) => ({ type: "keyDown", value });
          const up = (value) => ({ type: "keyUp", value });
          await send("input.performActions", { context, actions: [{ type: "key", id: "keyboard", actions: [
            down("\uE009"), down(replace ? "a" : "\uE011"), up(replace ? "a" : "\uE011"), up("\uE009"),
            ...Array.from(text).flatMap((c) => [down(c === "\n" ? "\uE007" : c), up(c === "\n" ? "\uE007" : c)]),
          ] }] });
        },
      };
    }
    const { targetId } = await send("Target.createTarget", { url: "about:blank" });
    const { sessionId } = await send("Target.attachToTarget", { targetId, flatten: true });
    const command = (method, params) => send(method, params, sessionId);
    await command("Page.enable");
    const evaluate = async (expression, contextId) => {
      const result = await command("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true, ...(contextId ? { contextId } : {}) });
      if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description || result.exceptionDetails.text);
      return result.result.value;
    };
    const frames = new Map();
    return {
      close, evaluate,
      navigate: (url) => command("Page.navigate", { url }),
      text: async () => {
        const slug = await evaluate('location.pathname.split("/").pop()');
        const { targetInfos } = await send("Target.getTargets");
        const target = targetInfos.find((one) => one.type === "iframe" && one.url.includes(`/${slug}/`));
        if (target) {
          if (!frames.has(target.targetId)) frames.set(target.targetId, (await send("Target.attachToTarget", { targetId: target.targetId, flatten: true })).sessionId);
          const result = await send("Runtime.evaluate", { expression: "document.body.innerText", returnByValue: true }, frames.get(target.targetId));
          return result.result.value || "";
        }
        const { frameTree } = await command("Page.getFrameTree");
        const frameId = frameTree.childFrames?.[0]?.frame.id;
        if (!frameId) return "";
        const { executionContextId } = await command("Page.createIsolatedWorld", { frameId, worldName: "typst-check" });
        return evaluate("document.body.innerText", executionContextId);
      },
      insert: async (text, replace = false) => {
        await evaluate('document.querySelector(".cm-content").focus()');
        for (const type of ["keyDown", "keyUp"]) await command("Input.dispatchKeyEvent", { type, key: replace ? "a" : "Home", code: replace ? "KeyA" : "Home", modifiers: 2 });
        await command("Input.insertText", { text });
      },
    };
  } catch (error) { socket?.close(); child.kill(); throw error; }
}

for (const name of browsers) {
  const directory = mkdtempSync(join(tmpdir(), "komodoc-typst-pdf-"));
  const port = 20000 + Math.floor(Math.random() * 10000);
  const base = `http://localhost:${port}`;
  const environment = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KOMODOC_")));
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
      "content-type": "application/json", "x-komodoc-client": "1", cookie: `komodoc_session=${cookie}`,
    }, body: JSON.stringify({ title: "Typst PDF acceptance", source_format: "typst", source }) });
    assert.equal(response.status, 201);
    const { slug, share_url: shareUrl } = await response.json();
    tab = await browser(name, join(directory, "browser"), port + 1);
    await tab.navigate(base);
    await until("origin", () => tab.evaluate(`location.origin === ${JSON.stringify(base)}`));
    await tab.evaluate(`document.cookie = ${JSON.stringify(`komodoc_session=${cookie}; path=/`)}`);
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
        const result = await fetch(`${base}/api/documents/${slug}/renderings/latest`, { headers: { "x-komodoc-client": "1", cookie: `komodoc_session=${cookie}` } });
        const latest = await result.json();
        return latest.current;
      }, 80000);
      await tab.evaluate('document.cookie = "komodoc_session=; Max-Age=0; path=/"');
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
