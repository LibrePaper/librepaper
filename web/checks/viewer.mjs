// The PDF viewer, and a comment anchored into one, in a real browser.
//
// `docs/specs/latex.md`'s bet, in one sentence: "a LaTeX document is a hard case
// for the same anchoring, not a new anchoring". This is what decides whether
// that is true. It draws the corpus' `article/` -- written to carry a
// hyphenated line end, a page break inside a sentence, a footnote and an `fi`
// ligature on purpose -- into the viewer page the server serves on the
// documents origin, walks it with the agent that walks every other document,
// and asks `anchorOne` for each of those four passages. What it reports is
// the miss rate, because the spec accepts tolerance and a number is the only
// honest way to say how much was needed.
//
// It needs a PDF, which needs a TeX Live. Without one it skips with a message
// rather than failing, the way the mirror checks do:
// `node latex/tools/texlive.mjs --pdf` is what makes it run.
//
// Headless Chromium over the DevTools protocol, the same way
// `browser-smoke.mjs` and `latex-check.mjs` do it.

import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, extname } from "node:path";

const HERE = dirname(new URL(import.meta.url).pathname);
const REPO = dirname(dirname(HERE));
const SHELL = join(REPO, "web", "dist");
const CORPUS = join(REPO, "latex", "corpus");

const ARTICLE = join(CORPUS, "article", "main.pdf");
const PAPER = join(CORPUS, "paper", "main.pdf");

if (!existsSync(join(SHELL, "viewer.html"))) {
  console.log("viewer: no built shell at web/dist; skipping (run `make web`)");
  process.exit(0);
}
if (!existsSync(ARTICLE) || !existsSync(PAPER)) {
  console.log("viewer: no corpus PDFs; skipping (run `node latex/tools/texlive.mjs --pdf`)");
  process.exit(0);
}

let failures = 0;
const results = [];
function check(what, condition, detail = "") {
  results.push({ what, ok: Boolean(condition), detail });
  if (!condition) failures += 1;
}

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/* -------------------------------------------------------------- the server */

// The shell as the Rust server serves it on the documents origin: the pages
// and their assets, `/agent.js`, and the agent injected into `viewer.html`.
// Anything this gets wrong is a difference between the test and the server, so
// it is kept to the two rules that matter -- the agent goes in before
// `</body>`, and the assets are reachable from the same origin, which is what
// the page's own `script-src 'self'` requires.
const PORT = 8700 + Math.floor(Math.random() * 300);
const BASE = `http://localhost:${PORT}`;

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".pdf": "application/pdf",
  ".wasm": "application/wasm",
  ".svg": "image/svg+xml",
};

// A parent for the frame. The agent posts to `parent` and takes messages from
// `parent` only, so a viewer with nothing above it is a viewer nothing can be
// sent to; this is the sidebar's half, cut down to what the checks need.
const HARNESS = `<!doctype html><meta charset="utf-8"><body>
<iframe id="frame" src="/viewer.html" style="width:900px;height:700px;border:0"></iframe>
<script type="module">
import { anchorOne, flatten } from "/src/lib/anchor.js";
const frame = document.getElementById("frame");
window.seen = { ready: [], selection: [], regions: [] };
addEventListener("message", (event) => {
  const message = event.data;
  if (!message || message.komodoc !== true) return;
  if (message.type === "ready") window.seen.ready.push(message.text);
  if (message.type === "selection") window.seen.selection.push(message.selector);
  if (message.type === "regions-unplaceable") window.seen.regions.push(message);
});
window.sendPdf = async (path) => {
  const bytes = await fetch(path).then((r) => r.arrayBuffer());
  window.seen.ready.length = 0;
  frame.contentWindow.postMessage({ komodoc: true, type: "preview", pdf: bytes }, "*", [bytes]);
};
window.text = () => window.seen.ready.at(-1) || "";
// The sidebar's own job: take a selector, find it in the published text, and
// send the offsets back for painting. Exactly what \`Reader.svelte\` does.
window.anchor = (selector) => {
  const text = window.text();
  return anchorOne(text, selector, flatten(text));
};
window.paint = (ranges) =>
  frame.contentWindow.postMessage({ komodoc: true, type: "highlight", ranges }, "*");
window.paintRegions = (regions) =>
  frame.contentWindow.postMessage({ komodoc: true, type: "regions", regions }, "*");
window.doc = () => frame.contentDocument;
window.ready = true;
</script></body>`;

const server = createServer((request, response) => {
  const path = new URL(request.url, BASE).pathname;
  if (path === "/" || path === "/harness.html") {
    response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
    response.end(HARNESS);
    return;
  }
  if (path === "/pdf/article") return sendFile(response, ARTICLE);
  if (path === "/pdf/paper") return sendFile(response, PAPER);
  // The sources the harness imports directly -- the anchoring is the module
  // the reader uses, not a copy of it.
  if (path.startsWith("/src/")) return sendFile(response, join(REPO, "web", path.slice(1)));
  const file = join(SHELL, path.replace(/^\/+/, ""));
  if (!file.startsWith(SHELL) || !existsSync(file)) {
    response.writeHead(404).end("not found");
    return;
  }
  if (path === "/viewer.html") {
    const page = readFileSync(file, "utf8").replace(
      "</body>",
      `<script src="/agent.js?reader=${BASE}"></script></body>`,
    );
    response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
    response.end(page);
    return;
  }
  sendFile(response, file);
});

function sendFile(response, file) {
  if (!existsSync(file)) {
    response.writeHead(404).end("not found");
    return;
  }
  response.writeHead(200, {
    "content-type": TYPES[extname(file)] || "application/octet-stream",
  });
  response.end(readFileSync(file));
}

/* ------------------------------------------------------------- the browser */

const CHROME = ["chromium", "chromium-browser", "google-chrome", "google-chrome-stable"];
const DEBUG_PORT = 9700 + Math.floor(Math.random() * 300);
let chrome = null;
let socket = null;

async function endpoint() {
  for (let tries = 0; tries < 200; tries++) {
    try {
      const version = await fetch(`http://127.0.0.1:${DEBUG_PORT}/json/version`).then((r) =>
        r.json(),
      );
      return version.webSocketDebuggerUrl;
    } catch {
      await wait(150);
    }
  }
  throw new Error("chromium never opened its debugging port");
}

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
        this.console.push(JSON.stringify(message.params).slice(0, 300));
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
  async eval(expression) {
    const result = await this.send("Runtime.evaluate", {
      expression: `(async () => { ${expression} })()`,
      awaitPromise: true,
      returnByValue: true,
    });
    if (result.exceptionDetails) {
      throw new Error(JSON.stringify(result.exceptionDetails).slice(0, 500));
    }
    return result.result.value;
  }
}

/* ------------------------------------------------------- the four passages */

// Each is a W3C text-quote selector of the kind the sidebar stores, written
// against the *source* rather than against anything pdf.js produced -- which
// is the point: a person quoting this document quotes what they read, and the
// viewer's job is to make the PDF read the same way.
const CASES = [
  {
    name: "a sentence broken across the page break",
    selector: {
      exact:
        "which is precisely the case a text-quote anchor has to re-anchor across, since pdf.js will hand the reader two runs of text",
      prefix: "page boundary by TeX, ",
      suffix: " with a gap",
    },
    spansPages: true,
  },
  {
    name: "a quotation across the hyphenated line end",
    selector: {
      exact: "because incomprehensibility is not a word that fits in a column",
      prefix: "next line, ",
      suffix: ". That hyphen",
    },
    spansLines: true,
  },
  {
    name: "a quotation containing the fi ligature",
    selector: {
      exact: "efficient",
      prefix: "The word ",
      suffix: " carries an",
    },
  },
  {
    name: "a quotation inside the footnote",
    selector: {
      exact: "A footnote, so the anchoring has a text node that is nowhere near its mark on the page.",
      prefix: "vanish.",
      suffix: "",
    },
    inFootnote: true,
  },
];

/* ------------------------------------------------------------------ the run */

async function run() {
  for (const candidate of CHROME) {
    try {
      chrome = spawn(
        candidate,
        [
          "--headless=new",
          `--remote-debugging-port=${DEBUG_PORT}`,
          "--no-sandbox",
          "--disable-gpu",
          "--disable-dev-shm-usage",
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
  socket = new WebSocket(await endpoint());
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve);
    socket.addEventListener("error", reject);
  });
  const root = new Tab(socket, null);
  const { targetId } = await root.send("Target.createTarget", { url: "about:blank" });
  const { sessionId } = await root.send("Target.attachToTarget", { targetId, flatten: true });
  const tab = new Tab(socket, sessionId);
  await tab.send("Runtime.enable");
  await tab.send("Log.enable");
  await tab.send("Page.enable");
  await tab.send("Page.navigate", { url: `${BASE}/harness.html` });
  for (let tries = 0; tries < 200; tries++) {
    if (await tab.eval("return Boolean(window.ready && window.doc && window.doc())")) break;
    await wait(100);
  }

  /* --- nothing until a message arrives ---------------------------------- */

  const empty = await tab.eval(`
    return { note: window.doc().body.textContent.trim(), canvases: window.doc().querySelectorAll("canvas").length };
  `);
  check(
    "a viewer with no message shows 'nothing to show yet' and no pages",
    /nothing to show yet/.test(empty.note) && empty.canvases === 0,
    JSON.stringify(empty),
  );

  /* --- the article, drawn ------------------------------------------------ */

  await tab.eval(`await window.sendPdf("/pdf/article");`);
  await settle(tab);

  // `VIEWER_DUMP=<path>` writes the joined text out. Not part of any check:
  // it is how the representation below was arrived at, and how the next
  // person will argue with it.
  if (process.env.VIEWER_DUMP) {
    writeFileSync(process.env.VIEWER_DUMP, await tab.eval("return window.text()"));
  }

  const drawn = await tab.eval(`
    const doc = window.doc();
    const pages = [...doc.querySelectorAll(".page")];
    return {
      canvases: doc.querySelectorAll(".page canvas").length,
      spans: pages.map((page) => page.querySelectorAll(".textLayer span:not(.gap)").length),
      published: window.text().length,
      viewerPages: frame.contentWindow.komodocViewer?.pages ?? 0,
    };
  `);
  check("the article draws 3 pages", drawn.canvases === 3, JSON.stringify(drawn));
  check(
    "every page has a text layer with spans in it",
    drawn.spans.length === 3 && drawn.spans.every((n) => n > 0),
    JSON.stringify(drawn.spans),
  );
  check(
    "the agent republished the pages as one text",
    drawn.published > 1500,
    `${drawn.published} characters`,
  );
  check(
    "the viewer reports its page count for the caret lock to use",
    drawn.viewerPages === 3,
    String(drawn.viewerPages),
  );

  // HTML-era figure-region records remain records after a document moves to
  // PDF, but the PDF text layer has no honest image-to-page coordinate map.
  // The agent must preserve and label such a region instead of attaching it
  // to page one or silently dropping it.
  await tab.eval(`
    window.seen.regions.length = 0;
    window.paintRegions([{ id: "legacy-figure", digest: "old-image", index: 0, x: 10, y: 10, w: 20, h: 20 }]);
    await new Promise((resolve) => setTimeout(resolve, 100));
  `);
  const regionResult = await tab.eval(`return window.seen.regions.at(-1) || null;`);
  check(
    "an old figure region is explicitly unplaceable in a PDF",
    regionResult?.reason === "pdf" && regionResult.ids.includes("legacy-figure"),
    JSON.stringify(regionResult),
  );

  /* --- the joined text reads as prose ------------------------------------ */

  const prose = await tab.eval(`
    const text = window.text();
    return {
      sentence: text.includes("This sentence begins on one page and, because of the vertical space above it"),
      hyphenated: text.includes("incomprehensibility"),
      ligature: text.includes("efficient") && !/[\\uFB00-\\uFB06]/.test(text),
      sample: text.slice(text.indexOf("Literate programming"), text.indexOf("Literate programming") + 120),
    };
  `);
  check(
    "the sentence across the page break is one run of words in the joined text",
    prose.sentence,
    prose.sample,
  );
  check(
    "the hyphenated line end joins into one word",
    prose.hyphenated,
    prose.hyphenated ? "" : "the word is still split at its hyphen",
  );
  check(
    "the fi ligature reads as two letters and no U+FB01 survives",
    prose.ligature,
    prose.ligature ? "" : "a ligature code point reached the joined text",
  );

  /* --- the four passages, anchored --------------------------------------- */

  let misses = 0;
  for (const one of CASES) {
    const found = await tab.eval(`
      const selector = ${JSON.stringify(one.selector)};
      const at = window.anchor(selector);
      if (!at) return { ok: false };
      const viewer = frame.contentWindow.komodocViewer;
      window.paint([{ id: "case", start: at.start, end: at.end, motivation: "commenting" }]);
      await new Promise((r) => setTimeout(r, 120));
      // A mark inside a run separator is at the page's origin, zero-sized
      // and clipped, so it says nothing about where the passage sits; only
      // the marks over real runs do.
      const marks = [...window.doc().querySelectorAll("mark[data-komodoc]")].filter(
        (m) => !m.closest("span.gap"),
      );
      const tops = [...new Set(marks.map((m) => Math.round(m.getBoundingClientRect().top)))];
      const pageOf = (offset) => viewer.pageForOffset(offset);
      const bottoms = marks.map((m) => m.closest(".page")?.getBoundingClientRect());
      return {
        ok: true,
        start: at.start,
        end: at.end,
        startPage: pageOf(at.start),
        endPage: pageOf(at.end - 1),
        marks: marks.length,
        lines: tops.length,
        // Where the marks sit down the page, as a fraction of its height:
        // a footnote is at the foot, and nothing in the body is.
        depth: marks.map((m, i) =>
          bottoms[i] ? (m.getBoundingClientRect().top - bottoms[i].top) / bottoms[i].height : 0,
        ),
        text: window.text().slice(at.start, at.end),
      };
    `);
    if (!found.ok) misses += 1;
    check(`${one.name}: anchors`, found.ok, found.ok ? "" : "anchorOne found nothing");
    if (!found.ok) continue;
    check(
      `${one.name}: the passage is painted`,
      found.marks > 0,
      `${found.marks} mark(s) over ${found.lines} line(s)`,
    );
    if (one.spansPages) {
      check(
        `${one.name}: it really crosses a page boundary`,
        found.endPage > found.startPage,
        `pages ${found.startPage}..${found.endPage}`,
      );
    }
    if (one.spansLines) {
      check(
        `${one.name}: the mark touches the spans on both lines`,
        found.lines >= 2,
        `${found.lines} distinct line(s)`,
      );
    }
    if (one.inFootnote) {
      check(
        `${one.name}: it lands in the footnote at the foot of the page, not in the body`,
        found.depth.every((d) => d > 0.75),
        found.depth.map((d) => d.toFixed(2)).join(" "),
      );
    }
  }
  const rate = ((misses / CASES.length) * 100).toFixed(0);
  console.log(`viewer: anchoring miss rate ${rate}% (${misses} of ${CASES.length} passages)`);
  check("no passage of the four is orphaned", misses === 0, `${rate}% missed`);

  /* --- a selection, as the sidebar receives it --------------------------- */

  const selected = await tab.eval(`
    const doc = window.doc();
    const spans = [...doc.querySelectorAll(".page[data-page='1'] .textLayer span:not(.gap)")]
      .filter((s) => s.textContent.trim().length > 1);
    // Two spans with something between them, so the quote has to be cut from
    // the joined text rather than from either node.
    const first = spans[3];
    const last = spans[6];
    const range = doc.createRange();
    range.setStart(first.firstChild, 0);
    range.setEnd(last.firstChild, last.firstChild.data.length);
    const selection = frame.contentWindow.getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
    doc.dispatchEvent(new Event("selectionchange"));
    window.seen.selection.length = 0;
    doc.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
    await new Promise((r) => setTimeout(r, 300));
    return window.seen.selection.at(-1);
  `);
  check(
    "a selection across two spans becomes a text-quote selector",
    selected && typeof selected.exact === "string" && selected.exact.length > 0,
    JSON.stringify(selected).slice(0, 160),
  );
  if (selected?.exact) {
    check(
      "its exact text is the visible words, with one whitespace between them",
      !/\s\s/.test(selected.exact) && !/^\s|\s$/.test(selected.exact),
      JSON.stringify(selected.exact),
    );
    const round = await tab.eval(`
      return Boolean(window.anchor(${JSON.stringify(selected)}));
    `);
    check("and the selector it produced anchors back into the same text", round, "");
  }

  /* --- a second PDF replaces the pages and the table ---------------------- */

  await tab.eval(`await window.sendPdf("/pdf/paper");`);
  await settle(tab);
  const second = await tab.eval(`
    const doc = window.doc();
    const text = window.text();
    return {
      canvases: doc.querySelectorAll(".page canvas").length,
      viewerPages: frame.contentWindow.komodocViewer?.pages ?? 0,
      stillArticle: text.includes("incomprehensibility"),
      anchors: Boolean(window.anchor({ exact: "incomprehensibility", prefix: "", suffix: "" })),
    };
  `);
  check("a second preview replaces the pages", second.canvases === 4, JSON.stringify(second));
  check(
    "and replaces the table: the first document's words are gone",
    !second.stillArticle && !second.anchors,
    JSON.stringify(second),
  );
  check(
    "and the page index follows it",
    second.viewerPages === 4,
    String(second.viewerPages),
  );

  await root.send("Target.closeTarget", { targetId });
}

/// Wait for the agent's republish. It debounces its observer by 250 ms, and a
/// render of several pages trips it more than once, so this waits for the
/// published text to stop growing rather than for a fixed delay.
async function settle(tab) {
  let last = -1;
  for (let tries = 0; tries < 100; tries++) {
    await wait(150);
    const now = await tab.eval("return window.text().length");
    if (now > 0 && now === last) return;
    last = now;
  }
}

server.listen(PORT);
try {
  await run();
} catch (error) {
  check("the viewer check ran", false, String(error).slice(0, 500));
} finally {
  try {
    socket?.close();
  } catch {}
  chrome?.kill();
  server.close();
  await wait(200);
}

for (const { what, ok, detail } of results) {
  console.log(`viewer: ${ok ? "ok  " : "FAIL"}  ${what}${detail ? ` -- ${detail}` : ""}`);
}
if (failures) {
  console.error(`viewer: ${failures} of ${results.length} checks failed`);
  process.exit(1);
}
console.log(`viewer: ${results.length} checks passed in a real browser`);
