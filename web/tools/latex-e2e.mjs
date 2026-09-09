// The routing table, driven end to end: a real `librepaper serve --latex`, a
// real `librepaper local start` when asked for, headless Chromium opening a
// published LaTeX project as its editor, and the stored rendering's
// provenance saying which backend produced it.
//
//   node web/tools/latex-e2e.mjs <binary> <mode> <fixture> [seconds] [mirror]
//
//     mode     local   -- start the local app and pair the page with it
//              vm      -- no local app: Biber must run in the browser VM
//              browser -- neither: a plain browser compile
//     fixture  a directory under latex/corpus/e2e (biber, native) or any
//              directory holding a main.tex
//
// Releases shipping Biber use browser-biber before either fallback. Older
// releases can exercise local or VM bibliography routing. Chromium is
// required; the mirror defaults to the binary's own URL. This check uses
// only its temporary server and data.
//
// Set localStorage `librepaper-latex-debug` (this script does) to see every
// routing decision on the console, which is what the trace below prints.
import { spawn, execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, readFileSync, writeFileSync, statSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const BINARY = resolve(process.argv[2] || "dist/librepaper");
const MODE = process.argv[3] || "local";
const FIXTURE = resolve(process.argv[4] || join(ROOT, "latex", "corpus", "e2e", "biber"));
const WAIT = Number(process.argv[5] || 300);
// A `-` mirror means exercise the binary's compiled-in default. This keeps
// the e2e harness able to verify the deployed mirror without inventing a
// local path for `--latex`.
const MIRROR = process.argv[6] || "-";
const OUT = mkdtempSync(join(tmpdir(), "librepaper-latex-e2e-"));
const PORT = 8600 + Math.floor(Math.random() * 200);
const LOCAL_PORT = 8763;
const BASE = `http://localhost:${PORT}`;
const DEBUG = 9500 + Math.floor(Math.random() * 200);
const HEADERS = { "x-librepaper-client": "shell", "sec-fetch-site": "same-origin" };
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
const data = mkdtempSync(join(tmpdir(), "librepaper-e2e-"));
const config = mkdtempSync(join(tmpdir(), "librepaper-e2e-config-"));
const profile = mkdtempSync(join(tmpdir(), "librepaper-e2e-profile-"));
const serverArgs = ["serve", "--port", String(PORT), "--data", data, "--publishers", "anyone", "--commenters", "anyone"];
if (MIRROR !== "-") serverArgs.push("--latex", MIRROR);
const server = spawn(BINARY, serverArgs, { stdio: ["ignore", "ignore", "ignore"] });
let local = null;
const localOut = [];
if (!["local", "vm", "browser"].includes(MODE)) throw new Error(`unknown mode ${MODE}`);
if (MODE === "local") {
  local = spawn(BINARY, ["local", "start", "--port", String(LOCAL_PORT)], { stdio: ["ignore", "pipe", "pipe"], env: { ...process.env, XDG_CONFIG_HOME: config, XDG_CACHE_HOME: join(config, "cache") } });
  local.stdout.on("data", (c) => localOut.push(String(c)));
  local.stderr.on("data", (c) => localOut.push(String(c)));
}
const chrome = spawn("chromium", ["--headless=new", "--no-sandbox", "--disable-gpu", "--disable-dev-shm-usage", `--user-data-dir=${profile}`, `--remote-debugging-port=${DEBUG}`, "about:blank"], { stdio: "ignore" });
const logs = [];
try {
  for (let i = 0; i < 100; i++) { try { if ((await fetch(`${BASE}/api/config`)).ok) break; } catch {} await wait(150); }
  // Establish the same signed visitor identity a browser would receive. A
  // cookie-less CLI publish is intentionally ownerless, so it cannot mint an
  // editor link for the compile phase below.
  const landing = await fetch(`${BASE}/`);
  const visitor = landing.headers.getSetCookie()
    .find(cookie => cookie.startsWith("librepaper_visitor="))?.split(";", 1)[0] || "";
  if (!visitor) throw new Error("server did not issue a visitor cookie");
  const uploadHeaders = { ...HEADERS, cookie: visitor };
  let body;
  if (statSync(FIXTURE).isDirectory()) {
    body = new FormData();
    body.set("title", "LaTeX smoke");
    body.set("main", "main.tex");
    function addFiles(directory, prefix = "") {
      for (const entry of readdirSync(directory, { withFileTypes: true })) {
        if (entry.name === "logs" || entry.name.startsWith(".")) continue;
        const path = prefix + entry.name;
        if (entry.isSymbolicLink()) throw new Error("unexpected fixture symlink: " + path);
        if (entry.isDirectory()) addFiles(join(directory, entry.name), path + "/");
        else body.append("file", new Blob([readFileSync(join(directory, entry.name))]), path);
      }
    }
    addFiles(FIXTURE);
  } else {
    uploadHeaders["content-type"] = "application/json";
    body = JSON.stringify({ title: "LaTeX smoke", source: readFileSync(FIXTURE, "utf8"), source_format: "latex" });
  }
  const uploaded = await fetch(`${BASE}/api/documents`, {
    method: "POST",
    headers: uploadHeaders,
    body,
  });
  const publication = await uploaded.json();
  if (!uploaded.ok || typeof publication.share_url !== "string") {
    throw new Error(`publish failed: ${JSON.stringify(publication)}`);
  }
  const url = new URL(publication.share_url, BASE).href;
  const publishedUrl = new URL(url);
  const slug = publishedUrl.pathname.split("/").pop();
  // `publish` returns a read link, whose key lives in the fragment so it is
  // never sent during navigation. The browser keeps that key in memory and
  // sends it as a header for document API requests. The smoke harness makes
  // the same read-only API calls while polling, so it must carry the key too.
  const key = new URLSearchParams(publishedUrl.hash.slice(1)).get("k") || "";
  if (!key) throw new Error(`publish did not return a read link: ${url}`);
  const READ_HEADERS = { ...HEADERS, "x-librepaper-key": key };
  // Publishing an anonymous document returns its reader link. Mint a
  // separate editor link for the compile phase; the final navigation below
  // deliberately uses the published reader link, exercising the access-key
  // boundary without granting readers edit rights.
  const share = await fetch(`${BASE}/api/documents/${slug}/share`, {
    method: "POST",
    headers: { ...HEADERS, cookie: visitor, "content-type": "application/json" },
    body: JSON.stringify({ link: { role: "editor", until: "" } }),
  });
  const sharing = await share.json();
  const editPath = sharing.links?.editor?.url;
  if (!share.ok || typeof editPath !== "string") throw new Error(`share did not return an edit link (${share.status}): ${JSON.stringify(sharing)}`);
  const editUrl = new URL(editPath, BASE).href;
  console.log("published", slug, "mode", MODE);
  let token = null;
  if (MODE === "local") {
    let code = null;
    for (let i = 0; i < 60 && !code; i++) {
      try { code = JSON.parse(readFileSync(join(config, "librepaper/local/service.json"), "utf8")).code; } catch {}
      await wait(250);
    }
    if (!code) throw new Error(`no pairing code: ${localOut.join("")}`);
    const health = await (await fetch(`http://127.0.0.1:${LOCAL_PORT}/librepaper/local/v1/health`)).json();
    console.log("local health", JSON.stringify(health));
    const connect = await fetch(`http://127.0.0.1:${LOCAL_PORT}/librepaper/local/v1/connect`, { method: "POST", headers: { "content-type": "application/json", origin: BASE }, body: JSON.stringify({ origin: BASE, project: slug, code }) });
    const pairing = await connect.json();
    console.log("connect", connect.status, JSON.stringify(pairing).slice(0, 80));
    token = { token: pairing.token, expires: pairing.expires, instance: health.instance };
  }
  let endpoint;
  for (let i = 0; i < 100; i++) { try { endpoint = (await (await fetch(`http://127.0.0.1:${DEBUG}/json/version`)).json()).webSocketDebuggerUrl; break; } catch {} await wait(150); }
  const socket = new WebSocket(endpoint);
  await new Promise((r, j) => { socket.onopen = r; socket.onerror = j; });
  let id = 0; const pending = new Map(); const sessions = new Map();
  const send = (method, params = {}, sessionId) => new Promise((resolve, reject) => { const n = ++id; pending.set(n, { resolve, reject }); socket.send(JSON.stringify({ id: n, method, params, ...(sessionId ? { sessionId } : {}) })); });
  socket.onmessage = ({ data: raw }) => {
    const m = JSON.parse(raw);
    if (m.id && pending.has(m.id)) { const p = pending.get(m.id); pending.delete(m.id); m.error ? p.reject(new Error(JSON.stringify(m.error))) : p.resolve(m.result); return; }
    if (m.method === "Target.attachedToTarget") {
      const { sessionId, targetInfo } = m.params; sessions.set(sessionId, targetInfo);
      for (const c of ["Runtime.enable", "Log.enable", "Network.enable"]) send(c, {}, sessionId).catch(() => {});
      send("Target.setAutoAttach", { autoAttach: true, waitForDebuggerOnStart: false, flatten: true }, sessionId).catch(() => {});
      send("Runtime.runIfWaitingForDebugger", {}, sessionId).catch(() => {});
      return;
    }
    const where = m.sessionId ? `${sessions.get(m.sessionId)?.type}:${(sessions.get(m.sessionId)?.url || "").slice(-30)}` : "root";
    if (m.method === "Runtime.exceptionThrown") logs.push(`[${where}] EXCEPTION ${JSON.stringify(m.params.exceptionDetails).slice(0, 500)}`);
    if (m.method === "Runtime.consoleAPICalled") logs.push(`[${where}] console.${m.params.type} ${m.params.args.map((a) => a.value ?? a.description ?? "").join(" ").slice(0, 500)}`);
    if (m.method === "Log.entryAdded") logs.push(`[${where}] log.${m.params.entry.level} ${m.params.entry.text.slice(0, 300)} ${m.params.entry.url || ""}`);
    if (m.method === "Network.responseReceived" && /8763|biber-vm|jobs|capabil|health|connect|renderings/.test(m.params.response.url)) logs.push(`[${where}] ${m.params.response.status} ${m.params.response.url.replace(BASE, "")}`);
  };
  const { targetId } = await send("Target.createTarget", { url: "about:blank" });
  const { sessionId } = await send("Target.attachToTarget", { targetId, flatten: true });
  sessions.set(sessionId, { type: "page", url: "" });
  for (const c of ["Runtime.enable", "Log.enable", "Network.enable"]) await send(c, {}, sessionId);
  await send("Target.setAutoAttach", { autoAttach: true, waitForDebuggerOnStart: false, flatten: true }, sessionId);
  await send("Page.navigate", { url: `${BASE}/` }, sessionId);
  await wait(1500);
  await send("Runtime.evaluate", { expression: `localStorage.setItem("librepaper-latex-debug", "1")` }, sessionId);
  if (token) {
    await send("Runtime.evaluate", { expression: `localStorage.setItem("librepaper-local-pairings", JSON.stringify({ [location.origin + "|" + ${JSON.stringify(slug)}]: ${JSON.stringify(token)} }))` }, sessionId);
  }
  await send("Page.navigate", { url: editUrl }, sessionId);
  const t0 = Date.now();
  let latest = null; let stable = 0;
  while (Date.now() - t0 < WAIT * 1000) {
    await wait(5000);
    latest = await (await fetch(`${BASE}/api/documents/${slug}/renderings/latest`, { headers: READ_HEADERS })).json();
    if (latest.sha) break;
    const status = await send("Runtime.evaluate", { expression: "document.body.innerText.replace(/\\s+/g,' ').slice(0,160)", returnByValue: true }, sessionId).catch(() => null);
    const line = status?.result?.value || "";
    stable = /Compilation failed/.test(line) ? stable + 1 : 0;
    if (stable >= 4) { console.log(`  gave up at ${((Date.now() - t0) / 1000).toFixed(0)}s: ${line}`); break; }
  }
  console.log("latest:", JSON.stringify(latest).slice(0, 400), "after", ((Date.now() - t0) / 1000).toFixed(0), "s");
  if (!latest?.sha) {
    await send("Runtime.evaluate", { expression: `Array.from(document.querySelectorAll("button")).find(b => /diagnostics/i.test(b.textContent + b.getAttribute("aria-label") + b.title))?.click()` }, sessionId);
    await wait(100);
    const details = await send("Runtime.evaluate", { expression: "document.body.innerText", returnByValue: true }, sessionId);
    throw new Error(`No PDF was produced within ${WAIT}s. ${details.result.value}`);
  }
  if (latest.sha) {
    const pdf = await fetch(`${BASE}/api/documents/${slug}/renderings/${latest.sha}`, { headers: READ_HEADERS });
    const bytes = Buffer.from(await pdf.arrayBuffer());
    if (!pdf.ok || bytes.subarray(0, 5).toString() !== "%PDF-") throw new Error("Stored rendering is not a PDF");
    if (MODE === "browser" && latest.provenance?.backend !== "browser") throw new Error("Expected a browser compile, not a local fallback");
    writeFileSync(`${OUT}/rendering.pdf`, bytes);
    let text = "";
    try { text = execFileSync("pdftotext", [`${OUT}/rendering.pdf`, "-"], { encoding: "utf8" }); } catch {}
    console.log("pdf bytes", bytes.length, "text:", text.replace(/\s+/g, " ").slice(0, 200));
    console.log("provenance:", JSON.stringify(latest.provenance));
  }

  // The document link is a reader link. Reopen it after the owner compile and
  // require the app to remain read-only while displaying the stored PDF.
  await send("Page.navigate", { url: "about:blank" }, sessionId);
  await wait(200);
  await send("Page.navigate", { url }, sessionId);
  await wait(1500);
  const access = await send("Runtime.evaluate", { expression: "document.body.innerText", returnByValue: true }, sessionId);
  if (!/Read-only access\./.test(access.result.value)) throw new Error("published read link did not open read-only");
  const compiler = await send("Runtime.evaluate", {
    expression: 'performance.getEntriesByType("resource").some(e => new URL(e.name).pathname.startsWith("/latex/"))', returnByValue: true,
  }, sessionId);
  if (compiler.result.value) throw new Error("read-only page fetched a LaTeX compiler");

  // A stored PDF alone does not prove that the deployed viewer can draw it.
  // Inspect the real document frame, including a cross-origin iframe target.
  let drawn = false;
  const frameSessions = new Map();
  for (let i = 0; i < 100 && !drawn; i++) {
    const expression = '!!document.querySelector(".page canvas") && !!document.querySelector(".textLayer")?.textContent.trim()';
    const { targetInfos } = await send("Target.getTargets");
    const frame = targetInfos.find(t => t.type === "iframe" && t.url.includes(`/${slug}/`));
    if (frame) {
      if (!frameSessions.has(frame.targetId)) frameSessions.set(frame.targetId, (await send("Target.attachToTarget", { targetId: frame.targetId, flatten: true })).sessionId);
      drawn = (await send("Runtime.evaluate", { expression, returnByValue: true }, frameSessions.get(frame.targetId))).result.value;
    } else {
      const { frameTree } = await send("Page.getFrameTree", {}, sessionId);
      const frameId = frameTree.childFrames?.[0]?.frame.id;
      if (frameId) {
        const { executionContextId } = await send("Page.createIsolatedWorld", { frameId, worldName: "latex-smoke" }, sessionId);
        drawn = (await send("Runtime.evaluate", { expression, contextId: executionContextId, returnByValue: true }, sessionId)).result.value;
      }
    }
    if (!drawn) await wait(100);
  }
  if (!drawn) throw new Error("PDF was compiled but the viewer did not draw its pages and selectable text");
  console.log("viewer: PDF pages and selectable text rendered");
  const text = await send("Runtime.evaluate", { expression: "document.body.innerText.replace(/\\s+/g,' ').slice(0,300)", returnByValue: true }, sessionId);
  console.log("page:", text.result.value);
  console.log("--- logs ---");
  for (const line of logs.slice(0, 60)) console.log(line);
  if (local) console.log("--- local ---\n" + localOut.join("").slice(0, 1500));
} catch (error) {
  console.error("E2E FAILED:", error.message);
  for (const line of logs.slice(0, 40)) console.log(line);
  process.exitCode = 1;
} finally {
  chrome.kill(); server.kill(); local?.kill("SIGINT");
  await wait(500);
  for (const dir of [data, config, profile, OUT]) rmSync(dir, { recursive: true, force: true, maxRetries: 3 });
}
