// The reader, in a real browser.
//
// The protocol tests in `librepaper/src/tests/` prove the server and
// `collab.js` agree. They say nothing about the page a person actually looks
// at: whether the frame gets painted, whether a reader sees an edit arrive,
// whether the badge says the true thing when the socket is down. That is what
// this does -- headless Chromium over the DevTools protocol, against a real
// `librepaper admin serve` on a temporary directory.
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
import http from "node:http";

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
  ["admin", "serve", "--port", String(PORT), "--data-directory", data, "--publishers", "any", "--commenters", "anyone"],
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
  async evalInFrame(expression, slug, prefix = "raw") {
    const { targetInfos } = await this.send("Target.getTargets");
    const frame = targetInfos.find(
      (one) => one.type === "iframe" && one.url.includes(`/${prefix}/` + slug + "/"),
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

  /// A paged document's PDF pages, as text: pdf.js paints an invisible text
  /// layer behind each page's canvas for selection and search, and that
  /// layer is this frame's only readable trace of what actually rendered --
  /// the bytes themselves arrive over `postMessage`, not as a navigable src
  /// this class can otherwise inspect (see `Preview.svelte`'s `tell`).
  async pdfText(slug) {
    return this.evalInFrame(
      `return [...document.querySelectorAll(".textLayer, [class*=textLayer]")].map((l) => l.textContent).join(" ")`,
      slug,
      "pdf",
    );
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
  // Wide enough that the reader's split layout renders the source pane
  // rather than collapsing to a single narrow-window pane -- the default
  // headless viewport is narrow enough that `.cm-content` mounts but stays
  // a zero-size, unfocusable element.
  await tab.send("Emulation.setDeviceMetricsOverride", { width: 1400, height: 900, deviceScaleFactor: 1, mobile: false });
  for (const cookie of cookies) await tab.send("Network.setCookie", cookie);
  await tab.send("Page.navigate", { url });
  return tab;
}

let root = null;

/* ------------------------------------------------------------- the session */

/// This deployment's real anonymous-owner identity: a throwaway, isolated
/// visit is enough for the shell to mint the browser's own visitor cookie
/// (`issue_visitor` in `signin.rs`), extracted via CDP so node-side requests
/// and other tabs can present the very same cookie a real browser would --
/// ownership of an anonymous upload is keyed on it (`owner()` in
/// `server/mod.rs`). A forged session cookie no longer verifies: sessions
/// are `v1.<payload>.<sig>` with a purpose-separated MAC, and the account
/// must exist in the catalogue, which only the Rust test harness can
/// arrange.
async function mintVisitor() {
  const tab = await openTab(`${BASE}/`, [], { ownProfile: true });
  return until("a visitor cookie", async () => {
    const { cookies } = await tab.send("Network.getCookies", { urls: [BASE] });
    return cookies.find((c) => c.name === "librepaper_visitor")?.value || null;
  });
}

async function publish(body, cookie = "") {
  const headers = { "content-type": "application/json", "x-librepaper-client": "1" };
  if (cookie) headers.cookie = `librepaper_visitor=${cookie}`;
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

  // Isolated so its own cookie survives later identities (owner, alice)
  // overwriting the default context's jar -- this tab is reused well past
  // where those are minted, in the persistence and offline checks below.
  const editorVisitor = await mintVisitor();
  const doc = await publish({
    title: "A Paper",
    source: MARKDOWN,
    source_format: "markdown",
  }, editorVisitor);
  const slug = doc.slug;
  const editor = await openTab(`${BASE}/docs/${slug}`, [
    { name: "librepaper_visitor", value: editorVisitor, domain: "localhost", path: "/" },
  ], { ownProfile: true });
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

  // A live preview must also advance while typing continues without a pause.
  await appendText(editor, "\n\nContinuous typing ");
  let liveWhileTyping = false;
  for (let i = 0; i < 25; i++) {
    await editor.send("Input.insertText", { text: "word " });
    if ((await editor.evalInFrame("return document.body.innerText", slug))?.includes("Continuous typing")) {
      liveWhileTyping = true;
    }
    await wait(100);
  }
  check("the preview advances during continuous typing", liveWhileTyping);

  /* --- 2. read-only live updates ------------------------------------------ */

  // A document owned by somebody, so a second browser is a reader rather than
  // an editor. Reading takes a link, and publishing mints the read one, which
  // is what a reader is handed.
  const owner = await mintVisitor();
  const owned = await publish(
    { title: "Owned", source: "# Owned\n\nOriginal wording.\n", source_format: "markdown" },
    owner,
  );
  check("publishing hands back a read link", /#k=/.test(owned.share_url || ""), JSON.stringify(owned).slice(0, 120));
  const ownerTab = await openTab(`${BASE}/docs/${owned.slug}`, [
    { name: "librepaper_visitor", value: owner, domain: "localhost", path: "/" },
  ]);
  const readerTab = await openTab(`${BASE}${owned.share_url}`, [], { ownProfile: true });
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
  // Owned by the same identity already resident in the default context
  // (`owner`, from the read-link check above) so the tabs below -- opened
  // bare, inheriting that context's cookie jar -- can edit it.
  const notebook = await publish({ title: "Notebook", source: page, source_format: "html" }, owner);
  const notebookTab = await openTab(`${BASE}/docs/${notebook.slug}`, [
    { name: "librepaper_visitor", value: owner, domain: "localhost", path: "/" },
  ]);
  const ran = await until("the notebook's script runs", async () =>
    (await notebookTab.evalInFrame("return document.body.dataset.ran", notebook.slug)) === "yes",
  );
  check("an HTML document's own scripts run in the frame", ran);

  // And an edit to it reaches a reader, which is what the reload is for.
  const secondNotebook = await openTab(`${BASE}/docs/${notebook.slug}`, [
    { name: "librepaper_visitor", value: owner, domain: "localhost", path: "/" },
  ]);
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

  // Pending Annotations only paints once the Collaboration panel has been
  // visited -- an editor's default panel is Files -- so open it before the
  // comment is queued, or the queued draft is never in the DOM to find.
  await editor.eval(`
    const tab = [...document.querySelectorAll("button, a")].find((b) => [b.getAttribute("aria-label"), b.title, b.textContent.trim()].includes("Collaboration"));
    if (!tab) throw new Error("no Collaboration tab");
    tab.click();
    return true;
  `);

  await editor.evalInFrame(`
    parent.postMessage({ librepaper: true, type: "selection",
      selector: { exact: "The first paragraph.", prefix: "", suffix: "", position: 0 }
    }, ${JSON.stringify(BASE)});
    return true;
  `, slug);
  await until("the comment button", () => editor.eval(`return Boolean(document.querySelector("#selectionbar"))`));
  await editor.eval(`document.querySelector("#selectionbar").click(); return true;`);
  await until("the comment form", () => editor.eval(`return Boolean(document.querySelector("#commentForm textarea"))`));
  await editor.eval(`
    const input = document.querySelector("#commentForm textarea");
    input.value = "A comment written offline.";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    document.querySelector("#commentForm").requestSubmit();
    return true;
  `);
  const draftKept = await until("the failed comment is retained", () => editor.eval(`
    const pending = document.querySelector('[aria-label="Unconfirmed comments"]');
    return pending?.querySelector("textarea")?.value === "A comment written offline.";
  `));
  check("an offline comment keeps its full text for retry", draftKept);

  await editor.send("Network.emulateNetworkConditions", {
    offline: false,
    latency: 0,
    downloadThroughput: -1,
    uploadThroughput: -1,
  });
  const recovered = await until(
    "the offline edit reaches the server",
    // From inside the editor's own page, so the visitor cookie that owns
    // this document rides along -- a bare node-side fetch carries none.
    async () => editor.eval(`
      const response = await fetch("/api/documents/${slug}/source");
      const body = await response.json();
      return Boolean(body.source?.includes("Typed while offline."));
    `),
    20000,
  );
  check("what was typed offline reaches the server when the socket returns", recovered);

  await editor.send("Page.reload");
  const reloadedDraft = await until("the draft survives reload and hello", () => editor.eval(`
    return document.querySelector('[aria-label="Unconfirmed comments"] textarea')?.value === "A comment written offline.";
  `));
  check("an unconfirmed comment survives reload and the server snapshot", reloadedDraft);
  const clickedRetry = await until("the retry control", () => editor.eval(`
    const card = document.querySelector('[aria-label="Unconfirmed comments"]');
    const button = [...(card?.querySelectorAll("button") || [])].find((b) => b.textContent.trim() === "Retry");
    if (!button) return false;
    button.click();
    return true;
  `));
  check("the retry control is reachable after reload", Boolean(clickedRetry));
  const confirmed = await until("the retried comment is confirmed", async () => {
    const count = await editor.eval(`
      const response = await fetch("/api/documents/${slug}/comments");
      const data = await response.json();
      return data.comments.filter((comment) => comment.body === "A comment written offline.").length;
    `);
    return count === 1 && await editor.eval(`return !document.querySelector('[aria-label="Unconfirmed comments"]')`);
  });
  check("retry stores one comment and clears the confirmed draft", confirmed);

  /* --- 5. a document that does not compile --------------------------------- */

  // Owned, like the notebook above: only an editor's browser compiles a
  // document locally (`renderers.warm`), and a reader only ever shows a
  // stored PDF -- of which a freshly published document has none.
  const broken = await publish({
    title: "Broken",
    source: "= A Paper\n\n$ x\n",
    source_format: "typst",
  }, owner);
  const brokenTab = await openTab(`${BASE}/docs/${broken.slug}`, [
    { name: "librepaper_visitor", value: owner, domain: "localhost", path: "/" },
  ]);
  const told = await until(
    "the failure is shown",
    async () => {
      // A typst document is paged: its frame is a PDF viewer at
      // `/pdf/<slug>/`, not the `/raw/<slug>/` HTML frame `evalInFrame`
      // looks for, and a PDF.js canvas carries no error text of its own --
      // so what to check is the shell's own error count badge
      // (`.activity-count.errors`, from `errorCount` in Reader.svelte),
      // or its fallback message when this checkout has no typst renderer
      // built at all.
      const errorBadge = await brokenTab.eval(`return document.querySelector(".activity-count.errors")?.textContent || ""`);
      const shell = await brokenTab.eval(`return document.body.innerText`);
      return errorBadge
        ? "told"
        : shell.includes("renderer") || shell.includes("read where")
          ? "no typst renderer built"
          : null;
    },
    20000,
  );
  check("a document that does not compile says so rather than showing nothing", told, told || "");

  /* --- 6. sharing: the key's path through the browser ---------------------- */

  // Sharing behavior that only a browser can check: a key
  // arriving in the fragment, being kept, being cleaned out of the address
  // bar, and being presented on every later request for that document.
  const alice = await mintVisitor();
  const shared = await publish(
    { title: "Under Review", source: "# Under Review\n\nThe draft.\n", source_format: "markdown" },
    alice,
  );
  const minted = await fetch(`${BASE}/api/documents/${shared.slug}/share`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-librepaper-client": "1",
      cookie: `librepaper_visitor=${alice}`,
    },
    body: JSON.stringify({ link: { role: "commenter", until: "180d" } }),
  }).then((r) => r.json());
  const mintedKey = minted.links?.commenter?.key;
  check("the share route mints a key", Boolean(mintedKey), JSON.stringify(minted).slice(0, 120));

  // A reviewer's browser: no account, and the key in the fragment.
  const reviewer = await openTab(`${BASE}/docs/${shared.slug}#k=${mintedKey}`, [], {
    ownProfile: true,
  });
  const kept = await until("the key is kept and the bar cleaned", async () => {
    const state = await reviewer.eval(`
      return {
        hash: location.hash,
        stored: JSON.parse(localStorage.getItem("librepaper-keys") || "{}")["${shared.slug}"] || "",
      };
    `);
    if (!state.stored) return null;
    // The document's own title, not the shell's -- the toolbar shows
    // whatever file is open (the frame's own name) rather than `doc.title`
    // once a source pane is involved, so the draft's own text is the more
    // reliable signal that this reviewer's browser is actually in.
    const frame = await reviewer.evalInFrame("return document.body.innerText", shared.slug);
    return frame?.includes("The draft.") ? state : null;
  });
  check("a link key is taken from the fragment and kept in this browser", kept?.stored === mintedKey);
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
        headers: { "X-LibrePaper-Key": JSON.parse(localStorage.getItem("librepaper-keys"))["${shared.slug}"] },
      });
      return (await response.json()).role;
    `);
    return role || null;
  });
  check("the key carries its role on every later request", asReviewer === "commenter", asReviewer || "");

  // Copying the link puts the key back, so a link copied here is the link
  // that was shared.
  const copied = await reviewer.eval(`
    const keys = JSON.parse(localStorage.getItem("librepaper-keys"));
    return location.origin + "/docs/${shared.slug}#k=" + encodeURIComponent(keys["${shared.slug}"]);
  `);
  check("the copied link carries the key back", copied.endsWith(`#k=${mintedKey}`));

  /* --- 7. an HTML document is served to its reader's frame, and to no one else */

  // Reading takes a credential now, and the documents origin holds none: no
  // sign-in, no link key. It serves a document's bytes only to a frame whose
  // URL carries the short-lived token the reader fetched over the channel
  // that does carry an identity. So a bare fetch gets the empty shell, and
  // the owner's frame gets the page as itself, scripts and all -- which is
  // what the title check below pins.
  const secretPage =
    '<!doctype html><html><head><title>Private Draft</title></head><body>' +
    '<h1>Private Draft</h1><p id="p">the private text</p>' +
    // A mark on the body rather than the title: the agent sets the frame's
    // title from the page's own, so a title the script changed would be
    // put back before anything here could read it.
    '<script>document.body.dataset.ran = "yes"<\/script></body></html>';
  const secret = await publish(
    { title: "Private Draft", source: secretPage, source_format: "html" },
    alice,
  );

  // fetch() refuses to send a caller-set Host header (it is on the Fetch
  // spec's forbidden list, and undici drops it silently), which is exactly
  // the header this request needs to reach the documents origin -- so this
  // one goes over node:http instead.
  const bare = await new Promise((resolve) => {
    const request = http.request(
      { hostname: "localhost", port: PORT, path: `/raw/${secret.slug}/`, headers: { host: `docs.localhost:${PORT}` } },
      (response) => {
        let body = "";
        response.on("data", (chunk) => (body += chunk));
        response.on("end", () => resolve(body));
      },
    );
    request.on("error", () => resolve(""));
    request.end();
  });
  check(
    "the documents origin serves no document's text to a frame with no token",
    !bare.includes("the private text"),
    bare.slice(0, 80),
  );
  check("the empty shell still carries the agent", bare.includes("agent.js"), bare.slice(0, 80));

  const ownerOfSecret = await openTab(`${BASE}/docs/${secret.slug}`, [
    { name: "librepaper_visitor", value: alice, domain: "localhost", path: "/" },
  ]);
  const shown = await until("the document is served into the owner's frame", async () =>
    (await ownerOfSecret.evalInFrame("return document.body.innerText", secret.slug))?.includes(
      "the private text",
    ),
  );
  check(
    "a document is served into its reader's frame with a token",
    shown,
    shown ? "" : `console: ${ownerOfSecret.console.slice(-3).join(" | ")}`,
  );
  const scripted = await ownerOfSecret.evalInFrame("return document.body.dataset.ran", secret.slug);
  check("the served document's own scripts run", scripted === "yes", scripted || "");

  // A stranger gets what a deleted document gets, and is told the one useful
  // thing: that signing in might help.
  const stranger = await openTab(`${BASE}/docs/${secret.slug}`, [], { ownProfile: true });
  const refused = await until("the stranger is refused", async () => {
    const text = await stranger.eval(`return document.body.innerText`);
    return text.includes("not found") ? text : null;
  });
  check("a document with no link tells a stranger nothing", Boolean(refused), (refused || "").slice(0, 90));

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
    { name: "librepaper_visitor", value: alice, domain: "localhost", path: "/" },
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
      const names = [...document.querySelectorAll(".explorer-name")].map((b) => b.textContent.trim());
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
    document.querySelector('button[aria-label="New file"]').click();
    return true;
  `);
  await author.eval(`
    const field = document.querySelector("#new-project-entry");
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set;
    setter.call(field, "lib.typ");
    field.dispatchEvent(new Event("input", { bubbles: true }));
    field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    return true;
  `);
  const two = await until("the second file appears", async () =>
    author.eval(`
      const names = [...document.querySelectorAll(".explorer-name")].map((b) => b.textContent.trim());
      return names.length === 2 ? names : null;
    `),
  );
  check("a file can be added to the directory", Boolean(two), JSON.stringify(two));

  await setText(author, '#let greeting = "from the library"\n');
  await wait(300);

  // Import it from the main file. Choosing a name in the list is what opens
  // it, exactly as a person would.
  await author.eval(`
    document.querySelector('.explorer-row[title="main.typ"]').click();
    return true;
  `);
  await wait(300);
  await setText(author, '#import "lib.typ": greeting\n\n= A Modular Paper\n\n#greeting\n');

  const imported = await until(
    "the import renders",
    async () => (await author.pdfText(paper.slug))?.includes("from the library"),
    20000,
  );
  check("a typst document imports a file beside it and renders", imported);

  // An error in the imported file is an error in *that* file, and choosing it
  // opens the file it is in rather than pointing at a line of the main one.
  await author.eval(`
    document.querySelector('.explorer-row[title="lib.typ"]').click();
    return true;
  `);
  await wait(300);
  await setText(author, "#let greeting = undefined_name\n");
  const badged = await until("the badge says what is wrong", async () =>
    author.eval(`return Boolean(document.querySelector(".activity-count.errors"))`),
    20000,
  );
  check("an error in an imported file is reported", Boolean(badged));

  // Renaming keeps the words: the name moves and the text stays where it is.
  // A double-click on the row starts the rename, the way a person does it --
  // there is no per-row button any more, only a context menu and this.
  await author.eval(`
    const row = document.querySelector('.explorer-row[title="lib.typ"]');
    row.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    return true;
  `);
  await author.eval(`
    const field = document.querySelector('input.name[aria-label^="Rename"]');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set;
    setter.call(field, "helpers.typ");
    field.dispatchEvent(new Event("input", { bubbles: true }));
    field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    return true;
  `);
  const renamed = await until("the rename lands", async () =>
    author.eval(`
      const names = [...document.querySelectorAll(".explorer-name")].map((b) => b.textContent.trim());
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
    headers: { cookie: `librepaper_visitor=${alice}` },
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
    { name: "librepaper_visitor", value: alice, domain: "localhost", path: "/" },
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
      const names = [...document.querySelectorAll(".explorer-name")].map((b) => b.textContent.trim());
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
    headers: { "x-librepaper-client": "1", cookie: `librepaper_visitor=${alice}` },
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

  /* ------------------------------------------------------------- the zip */

  // The whole directory, as a zip, built in this browser from what it already
  // holds: the texts are in the shared document and the figures were fetched
  // to render them, so there is no route to ask and nothing assembled twice.
  //
  // Driven through the control a person uses, and then read back by something
  // that is not the code that wrote it -- which is the only way to find out
  // whether what was produced is a zip or merely bytes that we believe are.
  await illustrator.send("Browser.setDownloadBehavior", {
    behavior: "allow",
    downloadPath: data,
  });
  await illustrator.eval(`
    const button = document.querySelector('.filelist button[aria-label^="Download"]');
    if (!button) throw new Error("no Download control");
    button.click();
    return true;
  `);
  const written = await until("the zip is written", async () => {
    const names = readdirSync(data).filter((name) => name.endsWith(".zip"));
    return names.length ? names[0] : null;
  });
  check("Download writes a zip of the directory", Boolean(written), String(written));
  if (written) {
    const bytes = readFileSync(join(data, written));
    const tail = bytes.subarray(bytes.length - 22);
    const asText = bytes.toString("latin1");
    check(
      "the zip is a zip, and holds every file of the document",
      tail.readUInt32LE(0) === 0x06054b50 &&
        asText.includes("main.md") &&
        asText.includes("one.png"),
      `${bytes.length} bytes`,
    );
  }

  /* ---------------------------------------------------------- vim keys ---- */

  // The setting is this browser's own, in localStorage rather than clicked,
  // because that is how a real visit finds it: already set from last time.
  const vimDoc = await publish({ title: "Vim Keys", source: "start\n", source_format: "markdown" }, alice);
  const vimTab = await openTab(`${BASE}/docs/${vimDoc.slug}`, [
    { name: "librepaper_visitor", value: alice, domain: "localhost", path: "/" },
  ]);
  await until("the vim document's editor is mounted", async () =>
    Boolean(await vimTab.eval(`return Boolean(document.querySelector(".cm-content"))`)),
  );
  await vimTab.eval(`localStorage.setItem("librepaper-keymap", JSON.stringify("vim")); return true;`);
  await vimTab.send("Page.reload");
  await until("Vim's status panel is drawn", async () =>
    Boolean(await vimTab.eval(`return Boolean(document.querySelector(".cm-vim-panel"))`)),
  );

  // `ihello<Esc>`: insert mode, typed, and back to normal.
  await vimKeys(vimTab, "ihello<Esc>");
  const typedHello = await until("hello is typed", async () =>
    (await vimTab.eval(`return document.querySelector(".cm-content").innerText`))?.includes("hello"),
  );
  check("Vim's insert mode types into the document", Boolean(typedHello));

  // `dd`: the line, gone.
  await vimKeys(vimTab, "dd");
  const emptied = await until("dd empties the line", async () =>
    (await vimTab.eval(`return document.querySelector(".cm-content").innerText`))?.includes("hello")
      ? null
      : true,
  );
  check("Vim's dd deletes the line", Boolean(emptied));

  // `u`: the same undo `Mod-z` uses, so the line comes back.
  await vimKeys(vimTab, "u");
  const undone = await until("u undoes the delete", async () =>
    (await vimTab.eval(`return document.querySelector(".cm-content").innerText`))?.includes("hello"),
  );
  check(
    "Vim's u undoes through CodeMirror's own history",
    Boolean(undone),
    undone ? "" : `content: ${JSON.stringify(await vimTab.eval('return document.querySelector(".cm-content").innerText'))} (expected "hello" back; undo appears to have reverted past the delete into the insert as well)`,
  );

  // `:q<Enter>`: the source pane closes, to the document alone.
  await vimKeys(vimTab, ":q<Enter>");
  const closed = await until("the layout closes to the document alone", async () =>
    (await vimTab.eval(`return JSON.parse(localStorage.getItem("librepaper-layout") || "null")`)) === "document"
      ? true
      : null,
  );
  check("Vim's :q closes the source pane", Boolean(closed));

  // Back to editing, and Vim off: `j` is a letter again, not a motion.
  await vimTab.eval(`
    document.querySelector('button[aria-label^="Layout"]').click();
    return true;
  `);
  await until("the source pane reopens", async () =>
    Boolean(await vimTab.eval(`return Boolean(document.querySelector(".cm-content"))`)),
  );
  await vimTab.eval(`localStorage.setItem("librepaper-keymap", JSON.stringify("default")); return true;`);
  await vimTab.send("Page.reload");
  await until("the editor remounts with the keys turned off", async () =>
    Boolean(await vimTab.eval(`return Boolean(document.querySelector(".cm-content"))`)),
  );
  // Give the freshly mounted editor a moment to finish wiring its listeners
  // before the next key arrives.
  await wait(200);
  const noPanel = await vimTab.eval(`return Boolean(document.querySelector(".cm-vim-panel"))`);
  check("turning Vim off removes its status panel", noPanel === false);

  await vimKeys(vimTab, "j");
  const typedJ = await until("j is typed rather than moving the caret", async () =>
    (await vimTab.eval(`return document.querySelector(".cm-content").innerText`))?.includes("j"),
  );
  check("with Vim off, j is a letter rather than a motion", Boolean(typedJ));
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
  // Chromium can still be releasing file handles in its profile directory a
  // moment after the process is asked to exit; one retry after a further
  // pause covers that without masking a real cleanup failure.
  try {
    rmSync(data, { recursive: true, force: true });
  } catch {
    await wait(1000);
    rmSync(data, { recursive: true, force: true });
  }
}

for (const { what, ok, detail } of results) {
  console.log(`browser: ${ok ? "ok  " : "FAIL"}  ${what}${detail ? ` -- ${detail}` : ""}`);
}
if (failures) {
  console.error(`browser: ${failures} of ${results.length} checks failed`);
  process.exit(1);
}
console.log(`browser: ${results.length} checks passed in a real browser`);
