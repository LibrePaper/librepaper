// The routing table, driven end to end: a real `komodoc serve --latex`, a
// real `komodoc local start` when asked for, headless Chromium opening a
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
// Expected: latex/corpus/e2e/biber with `local` stores a PDF whose
// provenance says `local-biber` in a few seconds; with `vm` it says
// `vm-biber` after the guest has booted and Biber has run (a minute or two
// on a cold cache); latex/corpus/e2e/native with `local` says
// `backend: local`, because the mirror lacks pgfplots and the paired app
// compiles the project natively. It needs the mirror at latex/mirror and
// Chromium; it touches no deployment and no data but its own.
//
// Set localStorage `komodoc-latex-debug` (this script does) to see every
// routing decision on the console, which is what the trace below prints.
import { spawn, execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const BINARY = resolve(process.argv[2] || "dist/komodoc");
const MODE = process.argv[3] || "local";
const FIXTURE = resolve(process.argv[4] || join(ROOT, "latex", "corpus", "e2e", "biber"));
const WAIT = Number(process.argv[5] || 300);
const MIRROR = process.argv[6] || `${ROOT}/latex/mirror`;
const OUT = mkdtempSync(join(tmpdir(), "komodoc-latex-e2e-"));
const PORT = 8600 + Math.floor(Math.random() * 200);
const LOCAL_PORT = 8763;
const BASE = `http://localhost:${PORT}`;
const DEBUG = 9500 + Math.floor(Math.random() * 200);
const HEADERS = { "x-komodoc-client": "1", "sec-fetch-site": "same-origin" };
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
const data = mkdtempSync(join(tmpdir(), "komodoc-e2e-"));
const config = mkdtempSync(join(tmpdir(), "komodoc-e2e-config-"));
const profile = mkdtempSync(join(tmpdir(), "komodoc-e2e-profile-"));
const server = spawn(BINARY, ["serve", "--port", String(PORT), "--data", data, "--publishers", "anyone", "--commenters", "anyone", "--latex", MIRROR], { stdio: ["ignore", "ignore", "ignore"] });
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
  const published = execFileSync(BINARY, ["publish", FIXTURE, "--server", BASE], { encoding: "utf8" });
  const url = published.trim().split("\n").pop();
  const slug = new URL(url).pathname.split("/").pop();
  console.log("published", slug, "mode", MODE);
  let token = null;
  if (MODE === "local") {
    let code = null;
    for (let i = 0; i < 60 && !code; i++) {
      try { code = JSON.parse(readFileSync(join(config, "komodoc/local/service.json"), "utf8")).code; } catch {}
      await wait(250);
    }
    if (!code) throw new Error(`no pairing code: ${localOut.join("")}`);
    const health = await (await fetch(`http://127.0.0.1:${LOCAL_PORT}/komodoc/local/v1/health`)).json();
    console.log("local health", JSON.stringify(health));
    const connect = await fetch(`http://127.0.0.1:${LOCAL_PORT}/komodoc/local/v1/connect`, { method: "POST", headers: { "content-type": "application/json", origin: BASE }, body: JSON.stringify({ origin: BASE, project: slug, code }) });
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
  await send("Runtime.evaluate", { expression: `localStorage.setItem("komodoc-latex-debug", "1")` }, sessionId);
  if (token) {
    await send("Runtime.evaluate", { expression: `localStorage.setItem("komodoc-local-pairings", JSON.stringify({ [location.origin + "|" + ${JSON.stringify(slug)}]: ${JSON.stringify(token)} }))` }, sessionId);
  }
  await send("Page.navigate", { url }, sessionId);
  const t0 = Date.now();
  let latest = null; let stable = 0;
  while (Date.now() - t0 < WAIT * 1000) {
    await wait(5000);
    latest = await (await fetch(`${BASE}/api/documents/${slug}/renderings/latest`, { headers: HEADERS })).json();
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
    const pdf = await fetch(`${BASE}/api/documents/${slug}/renderings/${latest.sha}`, { headers: HEADERS });
    const bytes = Buffer.from(await pdf.arrayBuffer());
    if (!pdf.ok || bytes.subarray(0, 5).toString() !== "%PDF-") throw new Error("Stored rendering is not a PDF");
    if (MODE === "browser" && latest.provenance?.backend !== "browser") throw new Error("Expected a browser compile, not a local fallback");
    writeFileSync(`${OUT}/rendering.pdf`, bytes);
    let text = "";
    try { text = execFileSync("pdftotext", [`${OUT}/rendering.pdf`, "-"], { encoding: "utf8" }); } catch {}
    console.log("pdf bytes", bytes.length, "text:", text.replace(/\s+/g, " ").slice(0, 200));
    console.log("provenance:", JSON.stringify(latest.provenance));
  }

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
  for (const line of logs.filter((l) => !/\/texlive\/[^/]+\/[0-9a-f]{2}\//.test(l)).slice(0, 60)) console.log(line);
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
