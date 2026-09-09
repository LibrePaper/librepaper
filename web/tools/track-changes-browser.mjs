// Track changes, in a real browser: suggest through the reader, accept from
// the card, redlines since a checkpoint, and the CLI against the same server.
// Not part of `bun run check`: it needs the built binary and a chromium.
// Run: node web/tools/track-changes-browser.mjs dist/librepaper

// The reader, in a real browser.
//
// The protocol tests in `librepaper/src/tests/` prove the server and
// `collab.js` agree. They say nothing about the page a person actually looks
// at: whether the frame gets painted, whether a reader sees an edit arrive,
// whether the badge says the true thing when the socket is down. That is what
// this does -- headless Chromium over the DevTools protocol, against a real
// `librepaper serve` on a temporary directory.
//
// Usage: browser-smoke.mjs <path-to-librepaper-binary>
// Nothing here touches a deployment or any storage but its own temporary one.

import { spawn } from "node:child_process";
import {
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
  existsSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createHmac } from "node:crypto";

const binary = process.argv[2] || "dist/librepaper";
if (!existsSync(binary)) {
  console.error(`browser: no librepaper binary at ${binary}; run \`make build\` first`);
  process.exit(1);
}

const data = mkdtempSync(join(tmpdir(), "librepaper-smoke-"));
const PORT = 8200 + Math.floor(Math.random() * 300);
const BASE = `http://localhost:${PORT}`;
let failures = 0;
const results = [];

function check(what, condition, detail = "") {
  results.push({ what, ok: Boolean(condition), detail });
  if (!condition) failures += 1;
}

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function until(what, predicate, timeout = 15000) {
  const deadline = Date.now() + timeout;
  let last;
  while (Date.now() < deadline) {
    try {
      last = await predicate();
      if (last) return last;
    } catch (error) {
      last = String(error);
    }
    await wait(150);
  }
  return null;
}

/* ------------------------------------------------------------- the server */

const server = spawn(
  binary,
  ["serve", "--port", String(PORT), "--data", data, "--publishers", "anyone", "--commenters", "anyone"],
  { stdio: ["ignore", "pipe", "pipe"] },
);
const serverLog = [];
server.stdout.on("data", (chunk) => serverLog.push(String(chunk)));
server.stderr.on("data", (chunk) => serverLog.push(String(chunk)));

/* ------------------------------------------------------------- the browser */

let chrome = null;
const CHROME = ["chromium", "chromium-browser", "google-chrome", "google-chrome-stable"];

async function connect(port) {
  for (let tries = 0; tries < 100; tries++) {
    try {
      const list = await fetch(`http://127.0.0.1:${port}/json/version`).then((r) => r.json());
      return list.webSocketDebuggerUrl;
    } catch {
      await wait(150);
    }
  }
  throw new Error("chromium never opened its debugging port");
}

/// One tab, with just enough of the DevTools protocol to drive a page.
class Tab {
  constructor(socket, sessionId) {
    this.socket = socket;
    this.sessionId = sessionId;
    this.next = 1;
    this.pending = new Map();
    this.console = [];
    // Every URL this page asked the network for, so a check can say that
    // something was fetched once and not twice -- which is the only way to
    // show that a cache is doing its job rather than merely being present.
    this.requests = [];
    socket.addEventListener("message", (event) => {
      const message = JSON.parse(event.data);
      if ((message.sessionId || null) !== (this.sessionId || null)) return;
      if (message.id && this.pending.has(message.id)) {
        const { resolve, reject } = this.pending.get(message.id);
        this.pending.delete(message.id);
        message.error ? reject(new Error(JSON.stringify(message.error))) : resolve(message.result);
        return;
      }
      if (message.method === "Network.requestWillBeSent") {
        this.requests.push(message.params?.request?.url || "");
      }
      if (
        message.method === "Runtime.consoleAPICalled" ||
        message.method === "Log.entryAdded" ||
        // An uncaught exception is the thing worth knowing when a page does
        // not come up, and it arrives on its own channel rather than as a
        // console message. Without this a broken component shows up only as
        // "no editor mounted", which says what did not happen and not why.
        message.method === "Runtime.exceptionThrown"
      ) {
        this.console.push(JSON.stringify(message.params).slice(0, 400));
      }
    });
  }

  send(method, params = {}) {
    const id = this.next++;
    const payload = { id, method, params };
    if (this.sessionId) payload.sessionId = this.sessionId;
    this.socket.send(JSON.stringify(payload));
    return new Promise((resolve, reject) => this.pending.set(id, { resolve, reject }));
  }

  /// Evaluates in the page and returns the value. `await` works.
  async eval(expression) {
    const result = await this.send("Runtime.evaluate", {
      expression: `(async () => { ${expression} })()`,
      awaitPromise: true,
      returnByValue: true,
    });
    if (result.exceptionDetails) {
      throw new Error(JSON.stringify(result.exceptionDetails).slice(0, 300));
    }
    return result.result.value;
  }

  /// The same, inside the document frame -- which is a different origin, so it
  /// is reached as its own target rather than through the parent. The slug
  /// picks this tab's frame out of every tab's: `Target.getTargets` answers
  /// for the whole browser, and several documents are open at once here.
  async evalInFrame(expression, slug) {
    const { targetInfos } = await this.send("Target.getTargets");
    const frame = targetInfos.find(
      (one) => one.type === "iframe" && one.url.includes("/raw/" + slug + "/"),
    );
    if (!frame) return null;
    const { sessionId } = await this.send("Target.attachToTarget", {
      targetId: frame.targetId,
      flatten: true,
    });
    const inner = new Tab(this.socket, sessionId);
    await inner.send("Runtime.enable");
    return inner.eval(expression);
  }
}

let socket = null;

async function openTab(url, cookies = [], { ownProfile = false } = {}) {
  // A browser context of its own means a cookie jar of its own. Without it a
  // tab opened after a signed-in one is signed in too, and a test that means
  // to be an anonymous reader is quietly the owner.
  const context = ownProfile
    ? (await root.send("Target.createBrowserContext")).browserContextId
    : undefined;
  const { targetId } = await root.send("Target.createTarget", {
    url: "about:blank",
    ...(context ? { browserContextId: context } : {}),
  });
  const { sessionId } = await root.send("Target.attachToTarget", { targetId, flatten: true });
  const tab = new Tab(socket, sessionId);
  await tab.send("Page.enable");
  await tab.send("Runtime.enable");
  await tab.send("Log.enable");
  await tab.send("Network.enable");
  for (const cookie of cookies) await tab.send("Network.setCookie", cookie);
  await tab.send("Page.navigate", { url });
  return tab;
}

let root = null;

/* ------------------------------------------------------------- the session */

/// The cookie a real sign-in produces, minted here with the deployment's own
/// key so a test can be somebody. The scheme is `auth.rs`: base64url of
/// "login|id|expiry", a dot, and base64url of its HMAC.
function sessionCookie(login) {
  const key = Buffer.from(readFileSync(join(data, "session.key"), "utf8").trim(), "hex");
  const payload = Buffer.from(`${login}|${login}|${Math.floor(Date.now() / 1000) + 3600}`)
    .toString("base64url");
  const signature = createHmac("sha256", key).update(payload).digest("base64url");
  return `${payload}.${signature}`;
}

async function publish(body, cookie = "") {
  const headers = { "content-type": "application/json", "x-librepaper-client": "1" };
  if (cookie) headers.cookie = `librepaper_session=${cookie}`;
  const response = await fetch(`${BASE}/api/documents`, {
    method: "POST",
    headers,
    body: JSON.stringify(body),
  });
  const payload = await response.json();
  if (response.status !== 201) throw new Error(`publish failed: ${response.status} ${JSON.stringify(payload)}`);
  return payload;
}

/// Types the way a person does: focus the editor, put the caret where it
/// belongs, and send real key and text events. Reaching into CodeMirror's
/// internals would test this script's knowledge of CodeMirror rather than the
/// editor.
async function focusEditor(tab) {
  await tab.eval(`
    const editor = document.querySelector(".cm-content");
    if (!editor) throw new Error("no editor mounted");
    editor.focus();
    return true;
  `);
}

async function press(tab, key, code, modifiers = 0) {
  for (const type of ["keyDown", "keyUp"]) {
    await tab.send("Input.dispatchKeyEvent", { type, key, code, modifiers });
  }
}

/// Appends at the end of the document.
async function appendText(tab, text) {
  await focusEditor(tab);
  await press(tab, "End", "End", 2); // Ctrl-End: the end of the document
  await tab.send("Input.insertText", { text });
}

/// Replaces the whole document.
async function setText(tab, text) {
  await focusEditor(tab);
  await press(tab, "a", "KeyA", 2); // Ctrl-A
  await tab.send("Input.insertText", { text });
}

// The names Vim mode reads out of `<...>` in a key sequence below. Vim's own
// command-line input reads `keyCode` for Enter and Escape rather than `key`,
// so those two carry the legacy code CDP does not infer on its own.
const VIM_SPECIAL = {
  Esc: { key: "Escape", keyCode: 27 },
  Enter: { key: "Enter", keyCode: 13 },
};

/// Sends a sequence of real keydown/keyup pairs, the way `ihello<Esc>` is
/// read: one character at a time, with `<Esc>` and `<Enter>` as named keys.
/// Vim mode reads the keyboard, not a paste, so `Input.insertText` -- which
/// is what `appendText` and `setText` use -- would never reach it; a letter
/// has to carry `text` for the browser to also treat it as typed, same as
/// Puppeteer's own `type()` does, so that a key Vim does not claim still
/// inserts its letter.
async function vimKeys(tab, sequence) {
  await focusEditor(tab);
  for (const [token, name] of sequence.matchAll(/<(\w+)>|[\s\S]/g)) {
    if (name) {
      const special = VIM_SPECIAL[name];
      if (!special) throw new Error(`vimKeys: no such key <${name}>`);
      const { key, keyCode } = special;
      const event = { key, windowsVirtualKeyCode: keyCode, nativeVirtualKeyCode: keyCode };
      await tab.send("Input.dispatchKeyEvent", { type: "keyDown", ...event });
      await tab.send("Input.dispatchKeyEvent", { type: "keyUp", ...event });
    } else {
      await tab.send("Input.dispatchKeyEvent", { type: "keyDown", key: token, text: token });
      await tab.send("Input.dispatchKeyEvent", { type: "keyUp", key: token });
    }
    // Entering command-line mode moves the focus to the panel's own input,
    // which a handler does asynchronously; the next key is for whichever
    // element ends up focused.
    await wait(20);
  }
}

/* ----------------------------------------------------------------- the run */

const MARKDOWN = "# A Paper\n\nThe first paragraph.\n";
const slug0 = (d) => d.slug;

async function run() {
  await until("the server", async () => (await fetch(`${BASE}/api/config`)).ok);

  for (const candidate of CHROME) {
    try {
      chrome = spawn(
        candidate,
        [
          "--headless=new",
          "--remote-debugging-port=9333",
          "--no-sandbox",
          "--disable-gpu",
          "--disable-dev-shm-usage",
          `--user-data-dir=${join(data, "chrome")}`,
          "about:blank",
        ],
        { stdio: "ignore" },
      );
      break;
    } catch {
      chrome = null;
    }
  }
  if (!chrome) throw new Error("no chromium to drive");
  const endpoint = await connect(9333);
  socket = new WebSocket(endpoint);
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve);
    socket.addEventListener("error", reject);
  });
  root = new Tab(socket, null);

  /* --- A. suggest through the reader, accept from the card ---------------- */

  const owner = sessionCookie("vincent");
  const doc = await publish({ title: "A Paper", source: MARKDOWN, source_format: "markdown" }, owner);
  const readKey = (doc.share_url || "").split("#k=")[1] || "";
  const api = (path) => fetch(`${BASE}${path}`, { headers: { "x-librepaper-client": "1", "x-librepaper-key": readKey } }).then((r) => r.json());
  const minted = await fetch(`${BASE}/api/documents/${slug0(doc)}/share`, { method: "POST", headers: { "content-type": "application/json", "x-librepaper-client": "1", cookie: `librepaper_session=${owner}` }, body: JSON.stringify({ link: { role: "editor", until: "" } }) }).then((r) => r.json());
  const editKey = minted.key || (minted.link || "").split("#k=")[1] || "";
  const slug = doc.slug;
  const editor = await openTab(`${BASE}/docs/${slug}`, [{ name: "librepaper_session", value: owner, url: BASE }]);
  const painted = await until("the frame is painted", async () =>
    (await editor.evalInFrame("return document.body.innerText", slug))?.includes("The first paragraph."),
  );
  check("the frame is painted", painted, painted ? "" : `console: ${editor.console.slice(-3).join(" | ")}`);

  // The comments panel, then the Suggest tool.
  await editor.eval(`const el = [...document.querySelectorAll("button, a")].find((b) => [b.getAttribute("aria-label"), b.title, b.textContent.trim()].includes("Comments")); if (!el) throw new Error("no Comments tab among " + [...document.querySelectorAll("button, a")].map((b) => b.getAttribute("aria-label") || b.title || b.textContent.trim()).join(",")); el.click(); return true;`);
  await until("the suggest tool", () => editor.eval(`return Boolean(document.querySelector('[aria-label="Suggest"]'))`));
  await editor.eval(`document.querySelector('[aria-label="Suggest"]').click(); return true;`);

  await editor.evalInFrame(`
    parent.postMessage({ librepaper: true, type: "selection",
      selector: { exact: "The first paragraph.", prefix: "", suffix: "", position: 0 }
    }, ${JSON.stringify(BASE)});
    return true;
  `, slug);
  await until("the selection bar", () => editor.eval(`return Boolean(document.querySelector("#selectionbar"))`));
  const barText = await editor.eval(`return document.querySelector("#selectionbar").textContent.trim()`);
  check("the selection bar offers Suggest", barText === "Suggest", barText);
  await editor.eval(`document.querySelector("#selectionbar").click(); return true;`);
  await until("the suggest form", () => editor.eval(`return document.querySelectorAll("#commentForm textarea").length === 2`));
  const prefill = await editor.eval(`return document.querySelector("#commentForm textarea").value`);
  check("the proposal is prefilled with the passage", prefill === "The first paragraph.", prefill);
  await editor.eval(`
    const [proposed, note] = document.querySelectorAll("#commentForm textarea");
    proposed.value = "The opening paragraph.";
    proposed.dispatchEvent(new Event("input", { bubbles: true }));
    note.value = "clearer";
    note.dispatchEvent(new Event("input", { bubbles: true }));
    document.querySelector("#commentForm").requestSubmit();
    return true;
  `);

  const listed = await until("the suggestion is stored", async () => {
    const { comments } = await api(`/api/documents/${slug}/comments`);
    const one = comments.find((c) => c.motivation === "editing");
    return one && !one.temp_id ? one : null;
  });
  check("the stored suggestion carries its proposal and a source anchor",
    listed?.proposed === "The opening paragraph." && listed?.source?.path && listed?.body === "clearer",
    JSON.stringify(listed).slice(0, 200));

  const painted2 = await until("the suggestion is painted", () => editor.evalInFrame(`
    const mark = document.querySelector("mark[data-proposed]");
    return mark ? { proposed: mark.dataset.proposed, strike: getComputedStyle(mark).textDecorationLine } : null;
  `, slug));
  check("the frame strikes the passage and carries the proposal",
    painted2?.proposed === "The opening paragraph." && painted2?.strike.includes("line-through"), JSON.stringify(painted2));

  const cardDiff = await until("the card diff", () => editor.eval(`
    const p = document.querySelector(".suggestion-diff");
    return p ? { del: p.querySelector("del")?.textContent, ins: p.querySelector("ins")?.textContent } : null;
  `));
  check("the card shows a word diff", cardDiff?.del === "first" && cardDiff?.ins === "opening", JSON.stringify(cardDiff));

  await editor.eval(`[...document.querySelectorAll("button")].find((b) => b.textContent.trim() === "Accept").click(); return true;`);
  const applied = await until("the accept lands in the frame", async () =>
    (await editor.evalInFrame("return document.body.innerText", slug))?.includes("The opening paragraph."),
  );
  check("accepting applies the proposal to the live document", applied);
  const decided = await until("the card says accepted", () => editor.eval(`return document.body.innerText.includes("Accepted")`));
  check("the card reports the acceptance", decided);
  const { checkpoints } = await api(`/api/documents/${slug}/history`);
  const acceptPoint = checkpoints.find((p) => p.why === "accept");
  check("an accept checkpoint is recorded", Boolean(acceptPoint), checkpoints.map((p) => p.why).join(","));
  const after = (await api(`/api/documents/${slug}/comments`)).comments.find((c) => c.motivation === "editing");
  check("the comment is resolved with outcome accepted in that checkpoint",
    after?.outcome === "accepted" && after?.resolved === true && after?.resolved_in === acceptPoint?.sha, JSON.stringify(after).slice(0, 200));
  const sourceNow = await api(`/api/documents/${slug}/source`);
  check("the source itself changed", (sourceNow.source || "").includes("The opening paragraph."), sourceNow.source);

  /* --- B. redlines since the first checkpoint ----------------------------- */

  await editor.eval(`[...document.querySelectorAll("button, a")].find((b) => [b.getAttribute("aria-label"), b.title, b.textContent.trim()].includes("History")).click(); return true;`);
  await until("the history panel", () => editor.eval(`return Boolean(document.querySelector("select.select"))`));
  const first = checkpoints[0].sha;
  await editor.eval(`
    const select = document.querySelector("select.select");
    select.value = ${JSON.stringify(first)};
    select.dispatchEvent(new Event("change", { bubbles: true }));
    return select.value;
  `);
  await until("the change list", () => editor.eval(`return document.body.innerText.includes("opening")`));
  await editor.eval(`
    const box = [...document.querySelectorAll("input[type=checkbox]")].find((b) => b.closest("label")?.textContent.includes("Show in document"));
    if (!box) throw new Error("no toggle");
    box.click();
    return box.checked;
  `);
  const redlined = await until("redlines in the frame", () => editor.evalInFrame(`
    const ins = document.querySelector("mark.librepaper-ins");
    const del = document.querySelector("mark.librepaper-del");
    return ins || del ? { ins: ins?.textContent, del: del?.dataset.deleted, who: (ins || del).title, text: document.body.innerText } : null;
  `, slug));
  check("insertions and deletions are painted inline", redlined?.ins === "opening" && redlined?.del === "first", JSON.stringify(redlined).slice(0, 200));
  check("redlines add no text to the document", !(redlined?.text || "").includes("first"), redlined?.text);

  await editor.eval(`const el = [...document.querySelectorAll("button, a")].find((b) => [b.getAttribute("aria-label"), b.title, b.textContent.trim()].includes("Comments")); if (!el) throw new Error("no Comments tab among " + [...document.querySelectorAll("button, a")].map((b) => b.getAttribute("aria-label") || b.title || b.textContent.trim()).join(",")); el.click(); return true;`);
  const cleared = await until("redlines cleared", async () => !(await editor.evalInFrame(`return Boolean(document.querySelector("mark.librepaper-ins, mark.librepaper-del"))`, slug)));
  check("leaving the history panel clears the redlines", cleared);

  /* --- C. the CLI against the same server --------------------------------- */

  const { execFileSync, spawnSync } = await import("node:child_process");
  const cli = (args) => spawnSync(binary, [...args, "--server", BASE, "--key", editKey], { encoding: "utf8", env: { ...process.env, HOME: data } });
  const suggested = cli(["suggest", slug, "--find", "opening paragraph", "--replace", "second paragraph", "--note", "from the terminal"]);
  check("librepaper suggest prints a comment id", suggested.status === 0 && /^[0-9a-f-]{20,}\s*$/.test(suggested.stdout), `${suggested.status} ${suggested.stdout} ${suggested.stderr}`.slice(0, 200));
  const twice = cli(["suggest", slug, "--find", "a", "--replace", "x"]);
  check("librepaper suggest refuses an ambiguous passage with a count", twice.status !== 0 && /occurs [0-9]+ times/.test(twice.stderr), twice.stderr.slice(0, 200));
  const accepted = cli(["accept", slug, suggested.stdout.trim()]);
  check("librepaper accept applies it", accepted.status === 0 && /^accepted, resolved in [0-9a-f]{7}/.test(accepted.stdout), `${accepted.status} ${accepted.stdout} ${accepted.stderr}`.slice(0, 200));
  const sourceAfterCli = await api(`/api/documents/${slug}/source`);
  check("the CLI acceptance reached the source", (sourceAfterCli.source || "").includes("The second paragraph."), sourceAfterCli.source);
  const again = cli(["accept", slug, suggested.stdout.trim()]);
  check("accepting twice is refused", again.status === 1 && /already accepted/.test(again.stderr), `${again.status} ${again.stderr}`.slice(0, 200));
}
/* ------------------------------------------------------------------ report */

try {
  await run();
} catch (error) {
  check("the smoke test ran", false, String(error).slice(0, 300));
  console.error(serverLog.slice(-5).join(""));
} finally {
  try {
    socket?.close();
  } catch {}
  chrome?.kill();
  server.kill();
  await wait(300);
  rmSync(data, { recursive: true, force: true });
}

for (const { what, ok, detail } of results) {
  console.log(`browser: ${ok ? "ok  " : "FAIL"}  ${what}${detail ? ` -- ${detail}` : ""}`);
}
if (failures) {
  console.error(`browser: ${failures} of ${results.length} checks failed`);
  process.exit(1);
}
console.log(`browser: ${results.length} checks passed in a real browser`);
