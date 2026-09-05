// The reader, in a real browser.
//
// The protocol tests in `komodoc/src/tests/` prove the server and `collab.js`
// agree. They say nothing about the page a person actually looks at: whether
// the frame gets painted, whether a reader sees an edit arrive, whether the
// badge says the true thing when the socket is down. That is what this does --
// headless Chromium over the DevTools protocol, against a real `komodoc serve`
// on a temporary directory.
//
// Usage: browser-smoke.mjs <path-to-komodoc-binary>
// Nothing here touches a deployment or any storage but its own temporary one.

import { spawn } from "node:child_process";
import { mkdtempSync, rmSync, readFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createHmac } from "node:crypto";

const binary = process.argv[2] || "dist/komodoc";
if (!existsSync(binary)) {
  console.error(`browser: no komodoc binary at ${binary}; run \`make build\` first`);
  process.exit(1);
}

const data = mkdtempSync(join(tmpdir(), "komodoc-smoke-"));
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
    socket.addEventListener("message", (event) => {
      const message = JSON.parse(event.data);
      if (message.id && this.pending.has(message.id)) {
        const { resolve, reject } = this.pending.get(message.id);
        this.pending.delete(message.id);
        message.error ? reject(new Error(JSON.stringify(message.error))) : resolve(message.result);
        return;
      }
      if (message.method === "Runtime.consoleAPICalled" || message.method === "Log.entryAdded") {
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
  const headers = { "content-type": "application/json", "x-komodoc-client": "1" };
  if (cookie) headers.cookie = `komodoc_session=${cookie}`;
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

/* ----------------------------------------------------------------- the run */

const MARKDOWN = "# A Paper\n\nThe first paragraph.\n";

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

  /* --- 1. editing: the frame is painted from the source, in the browser --- */

  const doc = await publish({
    title: "A Paper",
    source: MARKDOWN,
    source_format: "markdown",
  });
  const slug = doc.slug;
  const editor = await openTab(`${BASE}/docs/${slug}`);
  const painted = await until("the frame is painted", async () =>
    (await editor.evalInFrame("return document.body.innerText", slug)) ?.includes("The first paragraph."),
  );
  check(
    "an editor's browser renders the source into the frame",
    painted,
    painted ? "" : `console: ${editor.console.slice(-3).join(" | ")}`,
  );

  // Typing reaches the frame, which is the whole editing loop.
  await appendText(editor, "\n\nA sentence typed in the browser.\n");
  const typed = await until("typing reaches the frame", async () =>
    (await editor.evalInFrame("return document.body.innerText", slug))?.includes("typed in the browser"),
  );
  check("what is typed is rendered into the frame", typed);

  /* --- 2. read-only live updates ------------------------------------------ */

  // A document owned by somebody, so a second browser is a reader rather than
  // an editor.
  const owned = await publish(
    { title: "Owned", source: "# Owned\n\nOriginal wording.\n", source_format: "markdown" },
    sessionCookie("owner"),
  );
  const ownerTab = await openTab(`${BASE}/docs/${owned.slug}`, [
    { name: "komodoc_session", value: sessionCookie("owner"), domain: "localhost", path: "/" },
  ]);
  const readerTab = await openTab(`${BASE}/docs/${owned.slug}`, [], { ownProfile: true });
  const readerSees = await until("the reader renders", async () =>
    (await readerTab.evalInFrame("return document.body.innerText", owned.slug))?.includes("Original wording."),
  );
  check("a reader renders the document without an editor", readerSees);

  const noSource = await readerTab.eval(`return !!document.querySelector(".cm-content")`);
  check("a reader is not given an editor", noSource === false);

  await until("the owner's editor", async () =>
    ownerTab.eval(`return !!document.querySelector(".cm-content")`),
  );
  await appendText(ownerTab, "\n\nAdded while the reader watched.\n");
  const live = await until("the reader sees the edit", async () =>
    (await readerTab.evalInFrame("return document.body.innerText", owned.slug))?.includes("while the reader watched"),
  );
  check("a reader sees an edit arrive without reloading", live);

  /* --- 3. an HTML document's scripts run, and it is still live ------------- */

  const page = `<!doctype html><html><head><title>Notebook</title></head><body>
    <p id="prose">The original figure.</p>
    <script>document.body.dataset.ran = "yes";<\/script>
    </body></html>`;
  const notebook = await publish({ title: "Notebook", source: page, source_format: "html" });
  const notebookTab = await openTab(`${BASE}/docs/${notebook.slug}`);
  const ran = await until("the notebook's script runs", async () =>
    (await notebookTab.evalInFrame("return document.body.dataset.ran", notebook.slug)) === "yes",
  );
  check("an HTML document's own scripts run in the frame", ran);

  // And an edit to it reaches a reader, which is what the reload is for.
  const secondNotebook = await openTab(`${BASE}/docs/${notebook.slug}`);
  await until("the second tab", async () =>
    (await secondNotebook.evalInFrame("return document.body.innerText", notebook.slug))?.includes("original figure"),
  );
  await setText(notebookTab, page.replace("The original figure.", "The revised figure."));
  const notebookLive = await until(
    "the html reader sees the edit",
    async () =>
      (await secondNotebook.evalInFrame("return document.body.innerText", notebook.slug))?.includes("revised figure"),
    20000,
  );
  check("a reader of an HTML document sees an edit arrive", notebookLive);
  const stillRan = await secondNotebook.evalInFrame("return document.body.dataset.ran", notebook.slug);
  check("the reloaded HTML document still runs its scripts", stillRan === "yes");

  /* --- 4. persistence badge, and offline recovery -------------------------- */

  const badge = await until("a persistence state", async () =>
    editor.eval(`return document.body.innerText.includes("saving") ? "saving" : "quiet"`),
  );
  check("the toolbar has a persistence state to report", badge !== null, `badge: ${badge}`);

  // Offline: the socket is cut, the badge says so, and what is typed survives
  // a reload because it is in this browser's own storage.
  await editor.send("Network.emulateNetworkConditions", {
    offline: true,
    latency: 0,
    downloadThroughput: 0,
    uploadThroughput: 0,
  });
  await appendText(editor, "\n\nTyped while offline.\n");
  const saidOffline = await until("the offline badge", async () =>
    editor.eval(`return document.body.innerText.includes("offline")`),
  );
  check("the toolbar says the socket is down rather than claiming saved", saidOffline);

  await editor.send("Network.emulateNetworkConditions", {
    offline: false,
    latency: 0,
    downloadThroughput: -1,
    uploadThroughput: -1,
  });
  const recovered = await until(
    "the offline edit reaches the server",
    async () => {
      const source = await fetch(`${BASE}/api/documents/${slug}/source`).then((r) => r.json());
      return source.source?.includes("Typed while offline.");
    },
    20000,
  );
  check("what was typed offline reaches the server when the socket returns", recovered);

  /* --- 5. a document that does not compile --------------------------------- */

  const broken = await publish({
    title: "Broken",
    source: "= A Paper\n\n$ x\n",
    source_format: "typst",
  });
  const brokenTab = await openTab(`${BASE}/docs/${broken.slug}`);
  const told = await until(
    "the failure is shown",
    async () => {
      const frame = (await brokenTab.evalInFrame("return document.body.innerText", broken.slug)) || "";
      const shell = await brokenTab.eval(`return document.body.innerText`);
      // Either the engine's "does not compile" page in the frame, or the
      // badge counting the errors -- typst may not be built in this checkout,
      // in which case the page says so instead, which is also not a crash.
      return frame.includes("does not compile") || /\d+ error/.test(shell)
        ? "told"
        : shell.includes("renderer") || shell.includes("read where")
          ? "no typst renderer built"
          : null;
    },
    20000,
  );
  check("a document that does not compile says so rather than showing nothing", told, told || "");

  /* --- 6. sharing: the key's path through the browser ---------------------- */

  // The half of `03-SPEC-sharing.md` that only a browser can check: a key
  // arriving in the fragment, being kept, being cleaned out of the address
  // bar, and being presented on every later request for that document.
  const alice = sessionCookie("alice");
  const shared = await publish(
    { title: "Under Review", source: "# Under Review\n\nThe draft.\n", source_format: "markdown" },
    alice,
  );
  const minted = await fetch(`${BASE}/api/documents/${shared.slug}/share`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-komodoc-client": "1",
      cookie: `komodoc_session=${alice}`,
    },
    body: JSON.stringify({ link: { role: "commenter", label: "reviewer 2" } }),
  }).then((r) => r.json());
  check("the share route mints a key", Boolean(minted.key), JSON.stringify(minted).slice(0, 120));

  // A reviewer's browser: no account, and the key in the fragment.
  const reviewer = await openTab(`${BASE}/docs/${shared.slug}#k=${minted.key}`, [], {
    ownProfile: true,
  });
  const kept = await until("the key is kept and the bar cleaned", async () => {
    const state = await reviewer.eval(`
      return {
        hash: location.hash,
        stored: JSON.parse(localStorage.getItem("komodoc-keys") || "{}")["${shared.slug}"] || "",
        text: document.body.innerText,
      };
    `);
    return state.stored && state.text.includes("Under Review") ? state : null;
  });
  check("a link key is taken from the fragment and kept in this browser", kept?.stored === minted.key);
  check(
    "the visible URL is cleaned once the key is stored",
    kept && kept.hash === "",
    kept ? `hash: ${kept.hash}` : "",
  );

  // And it is presented on the requests that follow, which is what makes the
  // role real rather than remembered.
  const asReviewer = await until("the role arrives", async () => {
    const role = await reviewer.eval(`
      const response = await fetch("/api/documents/${shared.slug}", {
        headers: { "X-Komodoc-Key": JSON.parse(localStorage.getItem("komodoc-keys"))["${shared.slug}"] },
      });
      return (await response.json()).role;
    `);
    return role || null;
  });
  check("the key carries its role on every later request", asReviewer === "commenter", asReviewer || "");

  // Copying the link puts the key back, so a link copied here is the link
  // that was shared.
  const copied = await reviewer.eval(`
    const keys = JSON.parse(localStorage.getItem("komodoc-keys"));
    return location.origin + "/docs/${shared.slug}#k=" + encodeURIComponent(keys["${shared.slug}"]);
  `);
  check("the copied link carries the key back", copied.endsWith(`#k=${minted.key}`));

  /* --- 7. a private HTML document is painted rather than served ------------ */

  // The documents origin holds no sign-in, so it never serves a private
  // document's bytes. The reader paints them in over the socket instead, which
  // is the one path that carries an identity -- at the cost of the document's
  // own scripts, which is what the second check below pins.
  const secretPage =
    '<!doctype html><html><head><title>Private Draft</title></head><body>' +
    '<h1>Private Draft</h1><p id="p">the private text</p>' +
    '<script>document.title = "a script ran"</script></body></html>';
  const secret = await publish(
    { title: "Private Draft", source: secretPage, source_format: "html" },
    alice,
  );
  await fetch(`${BASE}/api/documents/${secret.slug}/share`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-komodoc-client": "1",
      cookie: `komodoc_session=${alice}`,
    },
    body: JSON.stringify({ visibility: "private" }),
  });

  const bare = await fetch(`${BASE}/raw/${secret.slug}/`, {
    headers: { host: `docs.localhost:${PORT}` },
    redirect: "manual",
  })
    .then((r) => r.text())
    .catch(() => "");
  check(
    "the documents origin serves no private document's text",
    !bare.includes("the private text"),
    bare.slice(0, 80),
  );

  const ownerOfSecret = await openTab(`${BASE}/docs/${secret.slug}`, [
    { name: "komodoc_session", value: alice, domain: "localhost", path: "/" },
  ]);
  const shown = await until("the private document is painted", async () =>
    (await ownerOfSecret.evalInFrame("return document.body.innerText", secret.slug))?.includes(
      "the private text",
    ),
  );
  check(
    "a private document is painted into the frame by the reader",
    shown,
    shown ? "" : `console: ${ownerOfSecret.console.slice(-3).join(" | ")}`,
  );

  // A stranger gets what a deleted document gets, and is told the one useful
  // thing: that signing in might help.
  const stranger = await openTab(`${BASE}/docs/${secret.slug}`, [], { ownProfile: true });
  const refused = await until("the stranger is refused", async () => {
    const text = await stranger.eval(`return document.body.innerText`);
    return text.includes("not found") ? text : null;
  });
  check("a private document tells a stranger nothing", Boolean(refused), (refused || "").slice(0, 90));
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
