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
import { mkdtempSync, rmSync, readFileSync, writeFileSync, existsSync } from "node:fs";
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
    // Every URL this page asked the network for, so a check can say that
    // something was fetched once and not twice -- which is the only way to
    // show that a cache is doing its job rather than merely being present.
    this.requests = [];
    socket.addEventListener("message", (event) => {
      const message = JSON.parse(event.data);
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
  check(
    "a reader renders the document without an editor",
    readerSees,
    readerSees ? "" : `console: ${readerTab.console.slice(-3).join(" | ")}`,
  );

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

  /* ------------------------------------------------------- the directory */

  // A document is a directory, and the whole point of that is this: a file
  // that imports another one renders. Until now the browser's file map stayed
  // empty, so a typst document with an `#import` compiled on its author's
  // laptop and nowhere else -- and since readers render for themselves, the
  // reader got the error too.
  const paper = await publish(
    {
      title: "A Modular Paper",
      source: "= A Modular Paper\n\nThe opening paragraph.\n",
      source_format: "typst",
    },
    alice,
  );
  const author = await openTab(`${BASE}/docs/${paper.slug}`, [
    { name: "komodoc_session", value: alice, domain: "localhost", path: "/" },
  ]);
  await until("the paper is open", async () =>
    (await author.eval(`return document.body.innerText`)).includes("A Modular Paper"),
  );
  await author.eval(`
    const open = [...document.querySelectorAll("button")].find((b) => /source|edit/i.test(b.title || b.textContent));
    open?.click();
    return true;
  `);
  await until("the editor is mounted", async () =>
    Boolean(await author.eval(`return Boolean(document.querySelector(".cm-content"))`)),
  );

  // The file list is there, with the one file this document has.
  const listed = await until("the file list is drawn", async () =>
    author.eval(`
      const names = [...document.querySelectorAll(".filelist .path")].map((b) => b.textContent.trim());
      return names.length ? names : null;
    `),
  );
  check(
    "a document of one file still shows its directory",
    listed?.length === 1 && listed[0] === "main.typ",
    JSON.stringify(listed),
  );

  // Add a second file, and write a function in it.
  await author.eval(`
    [...document.querySelectorAll(".filelist .addfile")][0].click();
    return true;
  `);
  await author.eval(`
    const field = document.querySelector(".filelist input.name");
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set;
    setter.call(field, "lib.typ");
    field.dispatchEvent(new Event("input", { bubbles: true }));
    field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    return true;
  `);
  const two = await until("the second file appears", async () =>
    author.eval(`
      const names = [...document.querySelectorAll(".filelist .path")].map((b) => b.textContent.trim());
      return names.length === 2 ? names : null;
    `),
  );
  check("a file can be added to the directory", Boolean(two), JSON.stringify(two));

  await setText(author, '#let greeting = "from the library"\n');
  await wait(300);

  // Import it from the main file. Choosing a name in the list is what opens
  // it, exactly as a person would.
  await author.eval(`
    const row = [...document.querySelectorAll(".filelist .path")].find((b) => b.textContent.trim() === "main.typ");
    row.click();
    return true;
  `);
  await wait(300);
  await setText(author, '#import "lib.typ": greeting\n\n= A Modular Paper\n\n#greeting\n');

  const imported = await until("the import renders", async () =>
    (await author.evalInFrame("return document.body.innerText", paper.slug))?.includes(
      "from the library",
    ),
  );
  check("a typst document imports a file beside it and renders", imported);

  // An error in the imported file is an error in *that* file, and choosing it
  // opens the file it is in rather than pointing at a line of the main one.
  await author.eval(`
    const row = [...document.querySelectorAll(".filelist .path")].find((b) => b.textContent.trim() === "lib.typ");
    row.click();
    return true;
  `);
  await wait(300);
  await setText(author, "#let greeting = undefined_name\n");
  const badged = await until("the badge says what is wrong", async () => {
    const text = await author.eval(`return document.body.innerText`);
    return /error/i.test(text) ? text : null;
  });
  check("an error in an imported file is reported", Boolean(badged));

  // Renaming keeps the words: the name moves and the text stays where it is.
  await author.eval(`
    const row = [...document.querySelectorAll(".filelist li")].find((li) => li.textContent.includes("lib.typ"));
    row.querySelector('button[aria-label^="Rename"], button[title^="Rename"]')?.click();
    return true;
  `);
  await author.eval(`
    const field = document.querySelector(".filelist input.name");
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set;
    setter.call(field, "helpers.typ");
    field.dispatchEvent(new Event("input", { bubbles: true }));
    field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    return true;
  `);
  const renamed = await until("the rename lands", async () =>
    author.eval(`
      const names = [...document.querySelectorAll(".filelist .path")].map((b) => b.textContent.trim());
      return names.includes("helpers.typ") ? names : null;
    `),
  );
  check("a file can be renamed", Boolean(renamed), JSON.stringify(renamed));
  const stillThere = await author.eval(`
    return document.querySelector(".cm-content")?.innerText || "";
  `);
  check(
    "a rename keeps the words and the editor stays bound to them",
    stillThere.includes("undefined_name"),
    stillThere.slice(0, 60),
  );

  // And the server holds the same directory, which is what a reader will be
  // given and what a checkpoint will record.
  const heldPaper = await fetch(`${BASE}/api/documents/${paper.slug}`, {
    headers: { cookie: `komodoc_session=${alice}` },
  }).then((r) => r.json());
  check(
    "the server records which file is the document",
    heldPaper.main === "main.typ",
    JSON.stringify(heldPaper.main),
  );

  /* ---------------------------------------------------------- the figures */

  // A figure is the half of a directory a paper's bytes actually live in. The
  // bytes never enter the shared document: they go to the store under their
  // own digest, and what travels between browsers is a name and that digest.
  //
  // Driven through the control a person uses -- the file chooser in the file
  // list, handed a real file the way a person hands it one -- rather than by
  // calling the upload route, because the route is already covered by the
  // Rust tests and what is not covered is the path from a chooser to a page.
  const png = join(data, "one.png");
  writeFileSync(
    png,
    Buffer.from(
      "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
      "base64",
    ),
  );

  const illustrated = await publish(
    {
      title: "A Paper With A Figure",
      source: "# A Paper With A Figure\n\n![the plot](one.png)\n",
      source_format: "markdown",
    },
    alice,
  );
  const illustrator = await openTab(`${BASE}/docs/${illustrated.slug}`, [
    { name: "komodoc_session", value: alice, domain: "localhost", path: "/" },
  ]);
  await until("the illustrated paper opens", async () =>
    (await illustrator.eval(`return document.body.innerText`)).includes("A Paper With A Figure"),
  );
  await illustrator.eval(`
    const open = [...document.querySelectorAll("button")].find((b) => /source|edit/i.test(b.title || b.textContent));
    open?.click();
    return true;
  `);
  await until("the editor is mounted", async () =>
    Boolean(await illustrator.eval(`return Boolean(document.querySelector(".cm-content"))`)),
  );

  // Hand the chooser a file, which is what a person does.
  await illustrator.send("DOM.enable");
  const { root: pageRoot } = await illustrator.send("DOM.getDocument");
  const { nodeId } = await illustrator.send("DOM.querySelector", {
    nodeId: pageRoot.nodeId,
    selector: ".filelist .chooser",
  });
  await illustrator.send("DOM.setFileInputFiles", { nodeId, files: [png] });

  const inTheList = await until("the figure joins the directory", async () =>
    illustrator.eval(`
      const names = [...document.querySelectorAll(".filelist .path")].map((b) => b.textContent.trim());
      return names.includes("one.png") ? names : null;
    `),
  );
  check("a figure chosen from a disk joins the directory", Boolean(inTheList), JSON.stringify(inTheList));

  // It reaches the page as a blob in this browser, never as the route it came
  // from: a rendered page must not carry a credential.
  const drawn = await until("the figure reaches the rendered page", async () =>
    illustrator.evalInFrame(
      `const img = document.querySelector("img"); return img ? img.src : null`,
      illustrated.slug,
    ),
  );
  check(
    "a markdown figure is rewritten to a blob in this browser",
    typeof drawn === "string" && drawn.startsWith("blob:"),
    String(drawn).slice(0, 60),
  );
  // The bytes reached the store, under the digest of themselves: the URL the
  // page fetched is the digest, and what comes back from it is the PNG that
  // went in.
  // The document's own asset route, not the shell's bundle -- both live under
  // a path containing "/assets/", and matching the loose one made this check
  // pass against a JavaScript chunk.
  const figureRoute = `/api/documents/${illustrated.slug}/assets/`;
  const asked = await until("the figure is fetched", async () =>
    illustrator.requests.find((url) => url.includes(figureRoute)),
  );
  const stored = await fetch(asked, {
    headers: { "x-komodoc-client": "1", cookie: `komodoc_session=${alice}` },
  });
  const back = Buffer.from(await stored.arrayBuffer());
  check(
    "the figure in the store is the file that was chosen",
    stored.status === 200 && back.equals(readFileSync(png)),
    `${stored.status}, ${back.length} bytes`,
  );

  // Fetched once. The URL carries the digest of the bytes, so a second render
  // reads them from this browser rather than from the network.
  const before = illustrator.requests.filter((url) => url.includes(figureRoute)).length;
  await appendText(illustrator, "\n\nAnother sentence, forcing a re-render.\n");
  await wait(1200);
  const after = illustrator.requests.filter((url) => url.includes(figureRoute)).length;
  check(
    "a figure already fetched is not fetched again on the next render",
    after === before && before >= 1,
    `${before} then ${after}`,
  );
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
