// Typing latency in the real app: publishes a long document to a throwaway
// deployment, opens it in headless Chromium, types into the middle of it at a
// human pace, and reports keystroke-to-paint latency and where the main
// thread's time went.
//
//   LIBREPAPER_TEST_POSTGRES_URL=... LIBREPAPER_TEST_BINARY=dist/librepaper \
//     node web/tools/typing-bench.mjs
//
// FORMAT=latex|typst|markdown (latex), LINES=1500, CHARS=150, GAP=100 (ms
// between keystrokes), CPU=4 (Chromium CPU throttling, a stand-in for a
// slower engine), PROFILE=path to write the raw .cpuprofile, TOP=30.
//
// BROWSER=webkit measures in WebKitGTK instead, the desktop app's engine,
// through WEBKIT_WEBDRIVER (its WebKitWebDriver) and the MiniBrowser beside
// it; run it under xvfb-run. WebKit offers no CPU profile over WebDriver, so
// only the latency is reported.
import { readFileSync, readdirSync, writeFileSync, mkdtempSync } from "node:fs";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { spawn } from "node:child_process";
import { browser, until, pause } from "../tests/helpers/browser-driver.mjs";
import { startDeployment } from "../tests/helpers/deployment.mjs";

const root = resolve(import.meta.dirname, "../..");
const FORMAT = process.env.FORMAT || "latex";
const LINES = Number(process.env.LINES || 1500);
const CHARS = Number(process.env.CHARS || 150);
const GAP = Number(process.env.GAP || 100);
const CPU = Number(process.env.CPU || 1);
const TOP = Number(process.env.TOP || 30);
const WEBKIT = process.env.BROWSER === "webkit";

/// The subset of the CDP tab this script uses, over WebDriver to WebKitGTK.
async function webkitTab() {
  const driver = process.env.WEBKIT_WEBDRIVER;
  if (!driver) throw new Error("set WEBKIT_WEBDRIVER to WebKitGTK's WebKitWebDriver");
  const miniBrowser = resolve(driver, "../../libexec/webkit2gtk-4.1/MiniBrowser");
  const port = 24000 + Math.floor(Math.random() * 1000);
  const child = spawn(driver, [`--port=${port}`], { stdio: "ignore" });
  const call = async (method, path, body) => {
    const response = await fetch(`http://127.0.0.1:${port}${path}`, { method, headers: { "content-type": "application/json" }, body: body && JSON.stringify(body) });
    const json = await response.json();
    if (json.value?.error) throw new Error(`${json.value.error}: ${json.value.message}`);
    return json.value;
  };
  await until("WebKitWebDriver", async () => (await fetch(`http://127.0.0.1:${port}/status`)).ok, 20000);
  const { sessionId } = await call("POST", "/session", { capabilities: { alwaysMatch: { "webkitgtk:browserOptions": { binary: miniBrowser, args: ["--automation"] } } } });
  const at = (path) => `/session/${sessionId}${path}`;
  return {
    async evaluate(expression) {
      return call("POST", at("/execute/async"), { script: `const done = arguments[arguments.length - 1]; Promise.resolve(${expression}).then(done, (error) => done({ __error: String(error) }));`, args: [] });
    },
    navigate: (url) => call("POST", at("/url"), { url }),
    resize: (width, height) => call("POST", at("/window/rect"), { width, height }),
    setCookie: (name, value, url) => call("POST", at("/cookie"), { cookie: { name, value, path: "/", domain: new URL(url).hostname, secure: false, httpOnly: true, sameSite: "Lax" } }),
    cookies: () => call("GET", at("/cookie")),
    // A real click at the end of the caret's line, which gives the window and
    // the editor focus the way a person's click would.
    async clickCaret() {
      const { x, y } = await this.evaluate(`(() => {
        const view = document.querySelector(".cm-content").cmTile.view;
        const at = view.coordsAtPos(view.state.selection.main.head);
        return { x: Math.round(at.left), y: Math.round((at.top + at.bottom) / 2) };
      })()`);
      await call("POST", at("/actions"), { actions: [
        { type: "pointer", id: "mouse", parameters: { pointerType: "mouse" }, actions: [
          { type: "pointerMove", x, y, origin: "viewport" }, { type: "pointerDown", button: 0 }, { type: "pointerUp", button: 0 },
        ] },
      ] });
      await call("POST", at("/actions"), { actions: [{ type: "key", id: "keyboard", actions: [{ type: "keyDown", value: "\uE010" }, { type: "keyUp", value: "\uE010" }] }] });
    },
    async type(keys, gap) {
      const actions = keys.flatMap((key) => [{ type: "keyDown", value: key }, { type: "keyUp", value: key }, { type: "pause", duration: gap }]);
      await call("POST", at("/actions"), { actions: [{ type: "key", id: "keyboard", actions }] });
    },
    async close() {
      await call("DELETE", at("")).catch(() => {});
      child.kill();
    },
  };
}

const example = { latex: ["tutorial-latex", "tex"], typst: ["tutorial-typst", "typ"], markdown: ["tutorial-markdown", "md"] }[FORMAT];
if (!example) throw new Error(`unknown FORMAT ${FORMAT}`);

/// The tutorial's main file with its sections' bodies repeated until the
/// document is LINES long, so the editor holds a paper-sized file.
function longDocument() {
  const [dir, ext] = example;
  const main = readFileSync(join(root, "docs/examples", dir, `librepaper.${ext}`), "utf8");
  const sections = readdirSync(join(root, "docs/examples", dir, "sections"))
    .filter((name) => name.endsWith(`.${ext}`))
    .map((name) => readFileSync(join(root, "docs/examples", dir, "sections", name), "utf8"))
    // Section files that pull in images or other files would only add errors.
    .join("\n\n");
  const body = [];
  while (body.join("\n").split("\n").length < LINES) body.push(sections);
  const marker = { latex: "\\end{document}", typst: null, markdown: null }[FORMAT];
  if (marker && main.includes(marker)) return main.replace(marker, `${body.join("\n\n")}\n${marker}`);
  return `${main}\n\n${body.join("\n\n")}\n`;
}

function stats(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const at = (q) => sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))];
  const mean = sorted.reduce((a, b) => a + b, 0) / sorted.length;
  return { n: sorted.length, mean: mean.toFixed(1), p50: at(0.5).toFixed(1), p90: at(0.9).toFixed(1), p99: at(0.99).toFixed(1), max: sorted.at(-1).toFixed(1) };
}

/// Self and inclusive time per function, in milliseconds.
function summarise(profile) {
  const byId = new Map(profile.nodes.map((node) => [node.id, node]));
  const parent = new Map();
  for (const node of profile.nodes) for (const child of node.children || []) parent.set(child, node.id);
  const key = (node) => {
    const { functionName, url, lineNumber, columnNumber } = node.callFrame;
    const file = url ? url.replace(/^.*\/assets\//, "").replace(/^https?:\/\/[^/]+\//, "") : "";
    return `${functionName || "(anonymous)"} ${file}:${lineNumber + 1}:${columnNumber + 1}`;
  };
  const self = new Map();
  const total = new Map();
  const deltas = profile.timeDeltas;
  let busy = 0;
  let span = 0;
  for (let i = 0; i < profile.samples.length; i++) {
    const ms = (deltas[i + 1] ?? deltas[i]) / 1000;
    span += ms;
    const leaf = byId.get(profile.samples[i]);
    const name = leaf.callFrame.functionName;
    if (name === "(idle)" || name === "(program)") continue;
    busy += ms;
    self.set(key(leaf), (self.get(key(leaf)) || 0) + ms);
    const seen = new Set();
    for (let id = leaf.id; id !== undefined; id = parent.get(id)) {
      const k = key(byId.get(id));
      if (seen.has(k)) continue;
      seen.add(k);
      total.set(k, (total.get(k) || 0) + ms);
    }
  }
  const top = (map) => [...map].sort((a, b) => b[1] - a[1]).slice(0, TOP).map(([k, v]) => `${v.toFixed(1).padStart(8)} ms  ${k}`);
  return { busy, span, self: top(self), total: top(total) };
}

const deployment = await startDeployment({ label: "typing_bench" });
if (!deployment || deployment.unavailable) throw new Error(deployment?.unavailable || "no librepaper binary; set LIBREPAPER_TEST_BINARY");
const scratch = mkdtempSync(join(tmpdir(), "librepaper-typing-bench-"));
let tab;
try {
  const source = longDocument();
  const doc = await deployment.publish({ title: "Typing bench", source_format: FORMAT, source });
  tab = WEBKIT ? await webkitTab() : await browser("chromium", join(scratch, "profile"), 19000 + Math.floor(Math.random() * 1000));
  if (!WEBKIT) await tab.resize(1600, 1000);
  const [name, value] = deployment.cookie.split(/=(.*)/s);
  // WebDriver sets a cookie on the page it is on.
  if (WEBKIT) {
    await tab.navigate(deployment.browserBase);
    // A narrow window is the phone layout, with no editor beside the preview.
    // MiniBrowser runs full screen on xvfb-run's screen for that reason.
    await until("a desktop-wide window", async () => { await tab.resize(1600, 1000).catch((e) => console.log("resize:", e.message)); const w = await tab.evaluate("innerWidth"); console.log("innerWidth", w); return w >= 1200; }, 8000);
  }
  await tab.setCookie(name, value, deployment.browserBase);
  // Settings as the preferences page stores them, e.g.
  // PREFS='{"librepaper-keymap":"vim","librepaper-editor-visual":true}'.
  const prefs = JSON.parse(process.env.PREFS || "{}");
  await tab.navigate(deployment.browserBase);
  await tab.evaluate(`(() => { for (const [k, v] of Object.entries(${JSON.stringify(prefs)})) localStorage.setItem(k, JSON.stringify(v)); })()`);
  await tab.navigate(`${deployment.browserBase}/docs/${doc.slug}`);
  try {
    await until("the editor", () => tab.evaluate(`Boolean(document.querySelector(".cm-content")?.cmTile?.view)`), 60000);
  } catch (error) {
    if (tab.cookies) console.log("cookies:", JSON.stringify(await tab.cookies()));
    console.log(await tab.evaluate(`JSON.stringify({ url: location.href, cm: Boolean(document.querySelector(".cm-content")), keys: Object.keys(document.querySelector(".cm-content") || {}), text: document.body.innerText.slice(0, 600) })`));
    throw error;
  }
  // Let the first compile, the index and the proofing settle.
  await pause(6000);
  const lines = await tab.evaluate(`(() => {
    const view = document.querySelector(".cm-content").cmTile.view;
    const line = view.state.doc.line(Math.floor(view.state.doc.lines / 2));
    view.focus();
    view.dispatch({ selection: { anchor: line.to }, scrollIntoView: true });
    return view.state.doc.lines;
  })()`);
  await tab.evaluate(`(() => {
    window.__latency = [];
    window.__events = [];
    document.addEventListener("keydown", (event) => {
      const start = event.timeStamp;
      requestAnimationFrame(() => setTimeout(() => window.__latency.push(performance.now() - start), 0));
    }, true);
    try {
      new PerformanceObserver((list) => {
        for (const entry of list.getEntries()) if (entry.name === "keydown" || entry.name === "keypress" || entry.name === "beforeinput" || entry.name === "input")
          window.__events.push({ name: entry.name, duration: entry.duration, processing: entry.processingEnd - entry.processingStart });
      }).observe({ type: "event", durationThreshold: 16, buffered: false });
    } catch {}
    window.__loaf = [];
    try {
      new PerformanceObserver((list) => {
        for (const entry of list.getEntries()) window.__loaf.push({ duration: entry.duration, scripts: entry.scripts.map((s) => ({ d: s.duration, f: s.sourceFunctionName, u: (s.sourceURL || "").replace(/^.*\\//, ""), inv: s.invoker })) });
      }).observe({ type: "long-animation-frame" });
    } catch {}
  })()`);
  await pause(500);
  const text = "the quick brown fox jumps over the lazy dog ";
  const keys = Array.from({ length: CHARS }, (_, i) => text[i % text.length]);
  let profile = null;
  if (WEBKIT) {
    console.log("focus before:", await tab.evaluate(`document.activeElement?.className + " hasFocus=" + document.hasFocus()`));
    await tab.clickCaret();
    console.log("focus after click:", await tab.evaluate(`document.activeElement?.className + " hasFocus=" + document.hasFocus()`));
    // Vim starts in normal mode; A appends at the end of the line.
    if (prefs["librepaper-keymap"] === "vim") { await tab.type(["A"], 300); }
    await tab.type(keys, GAP);
    await pause(1500);
  } else {
    const press = async (key, code, vk, shift = false) => {
      await tab.command("Input.dispatchKeyEvent", { type: "keyDown", key, text: key, unmodifiedText: key.toLowerCase(), code, windowsVirtualKeyCode: vk, modifiers: shift ? 8 : 0 });
      await tab.command("Input.dispatchKeyEvent", { type: "keyUp", key, code, windowsVirtualKeyCode: vk, modifiers: shift ? 8 : 0 });
    };
    if (prefs["librepaper-keymap"] === "vim") { await press("A", "KeyA", 65, true); await pause(300); }
    if (CPU > 1) await tab.command("Emulation.setCPUThrottlingRate", { rate: CPU });
    await tab.command("Profiler.enable");
    await tab.command("Profiler.setSamplingInterval", { interval: 200 });
    await tab.command("Profiler.start");
    for (const c of keys) {
      const code = c === " " ? "Space" : `Key${c.toUpperCase()}`;
      const vk = c === " " ? 32 : c.toUpperCase().charCodeAt(0);
      await tab.command("Input.dispatchKeyEvent", { type: "keyDown", key: c, text: c, unmodifiedText: c, code, windowsVirtualKeyCode: vk });
      await tab.command("Input.dispatchKeyEvent", { type: "keyUp", key: c, code, windowsVirtualKeyCode: vk });
      await pause(GAP);
    }
    await pause(1500);
    ({ profile } = await tab.command("Profiler.stop"));
    if (CPU > 1) await tab.command("Emulation.setCPUThrottlingRate", { rate: 1 });
    if (process.env.PROFILE) writeFileSync(process.env.PROFILE, JSON.stringify(profile));
  }
  const latency = await tab.evaluate("window.__latency");
  const events = await tab.evaluate("window.__events");
  const loaf = await tab.evaluate("window.__loaf");
  const typed = await tab.evaluate(`document.querySelector(".cm-content").cmTile.view.state.doc.toString().includes("quick brown fox")`);
  console.log(`${WEBKIT ? "webkit" : "chromium"}, format ${FORMAT}, ${lines} lines, ${CHARS} keys every ${GAP} ms, cpu x${CPU}, text arrived: ${typed}`);
  console.log("keydown -> after next frame (ms):", stats(latency));
  if (profile) {
  const summary = summarise(profile);
  const keydowns = events.filter((e) => e.name === "keydown");
  console.log(`event timing >=16ms: ${events.length} entries; keydown ${keydowns.length}`, keydowns.length ? stats(keydowns.map((e) => e.duration)) : "");
  console.log(`main thread busy ${summary.busy.toFixed(0)} ms of ${summary.span.toFixed(0)} ms (${(100 * summary.busy / summary.span).toFixed(0)}%), ${(summary.busy / CHARS).toFixed(1)} ms per key`);
  console.log(`long animation frames: ${loaf.length}`);
  const scripts = new Map();
  for (const frame of loaf) for (const s of frame.scripts) {
    const k = `${s.inv} | ${s.f} ${s.u}`;
    scripts.set(k, (scripts.get(k) || 0) + s.d);
  }
  for (const [k, v] of [...scripts].sort((a, b) => b[1] - a[1]).slice(0, 15)) console.log(`${v.toFixed(0).padStart(7)} ms  ${k}`);
  console.log("\nself time:");
  for (const line of summary.self) console.log(line);
  console.log("\ninclusive time:");
  for (const line of summary.total) console.log(line);
  }
} finally {
  await tab?.close().catch(() => {});
  await deployment.stop();
}
