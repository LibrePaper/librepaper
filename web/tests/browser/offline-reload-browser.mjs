// Does what somebody typed survive closing the tab, when nothing can be
// fetched back from the server?
//
// This is contracts 1 and 7 at the point where they are actually felt: a laptop
// lid closes on a train, the tab is reopened later, and the paragraph is still
// there. `tests/unit/offline-persistence.mjs` asks the storage adapter that
// question directly; this asks the page, which is a different question,
// because the editor has to bind to what was hydrated rather than to the empty
// document it made while waiting.
//
// The room here never sends any document state. That is not a simulation of
// being offline -- there is genuinely nothing to receive, so text on the page
// after a reload can only have come from IndexedDB. A test that let the server
// answer would pass whether or not persistence worked at all.
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until } from "../../tools/browser-driver.mjs";

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const temp = mkdtempSync(join(tmpdir(), "librepaper-offline-"));
const out = join(temp, "build");
const TYPED = "words typed while the server was unreachable";

// A room that connects and then says nothing. `doc-open` is answered with
// silence rather than with state, which is what makes this test mean anything.
const room = join(temp, "room.js");
writeFileSync(room, `
export function openRoom(slug, {onConnected}) {
  queueMicrotask(() => onConnected(true));
  return { send() { return {ok:true}; }, sendLive() { return {ok:true}; }, close() {} };
}
`);

// The page under test: join a project with an identity, so the local cache is
// keyed and used, and expose just enough for the driver to type and to read.
const entry = join(temp, "entry.js");
writeFileSync(entry, `
import { join } from ${JSON.stringify(join(root, "web/src/lib/collab.js"))};
// Sending goes nowhere: there is no server in this test, only the local
// cache, which is the whole point.
const session = join({
  slug: "paper",
  createdAt: "2026-01-01T00:00:00Z",
  mayEdit: true,
  send: () => ({ ok: true }),
  onState: (state) => { window.latestState = state; },
});
window.session = session;
window.state = () => window.latestState;
window.acknowledgeAll = () => session.acknowledge(Number.MAX_SAFE_INTEGER);
window.markDurable = () => {
  const bytes = session.doc.oplogVersion().encode();
  session.durable(btoa(String.fromCharCode(...bytes)));
};
// A real server always names its protocol on this frame (§6.1); this stands
// in for "nothing else arrived" rather than for a server that predates the
// handshake, which no longer exists to simulate.
window.ready = session.start({ protocol: "librepaper.room.v3", vector: "", durableVector: "", updates: [] }).then(() => true);
window.mainText = () => {
  const files = session.doc.getMap("files");
  const id = session.doc.getMap("meta").get("main");
  return id ? (files.get(id)?.toString() ?? "") : "";
};
window.type = (words) => {
  const id = session.addText("main.md", words);
  session.doc.getMap("meta").set("main", id);
  session.doc.commit();
  return session.persist();
};
`);

await build({
  configFile: false,
  root: join(root, "web"),
  plugins: [tailwindcss(), svelte(), { name: "mock-room", enforce: "pre", resolveId: (id) => (/(^|\/)room\.js$/.test(id) ? room : undefined) }],
  build: { outDir: out, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "check.js" } },
  logLevel: "error",
});

const page = `<!doctype html><meta charset="utf-8"><body><script type="module" src="/check.js"></script></body>`;
const server = createServer((request, response) => {
  const path = new URL(request.url, "http://127.0.0.1").pathname;
  if (path === "/" || path === "/docs/paper") {
    response.setHeader("content-type", "text/html");
    return response.end(page);
  }
  try {
    response.setHeader("content-type", path.endsWith(".css") ? "text/css" : "text/javascript");
    response.end(readFileSync(join(out, path.slice(1))));
  } catch {
    response.statusCode = 404;
    response.end("");
  }
});
const port = 28000 + Math.floor(Math.random() * 1000);
await new Promise((listening) => server.listen(port, "127.0.0.1", listening));
const url = `http://127.0.0.1:${port}/docs/paper`;

let tab;
const failures = [];
const check = (ok, what) => { if (!ok) failures.push(what); };
try {
  // The profile directory is kept across both loads, because that is where
  // IndexedDB lives. A fresh profile would make this test prove nothing.
  tab = await browser("chromium", join(temp, "profile"), 29000 + Math.floor(Math.random() * 500));
  await tab.navigate(url);
  await until("first load", () => tab.evaluate("window.ready"), 30000);

  await tab.evaluate(`window.type(${JSON.stringify(TYPED)})`);
  check(await tab.evaluate("window.mainText()") === TYPED, "the words are there before the reload");
  // `persist` resolves when the local store has taken it, so there is no sleep
  // here to be flaky about.
  await tab.evaluate("window.session.persist()");

  await tab.navigate(url);
  await until("second load", () => tab.evaluate("window.ready"), 30000);
  const after = await tab.evaluate("window.mainText()");
  check(after === TYPED, `the words survived the reload (got ${JSON.stringify(after)})`);
  check((await tab.evaluate("window.state()?.pending")) > 0, "restored work remains pending until the server's durable vector covers it");
  await tab.evaluate("window.acknowledgeAll()");
  check((await tab.evaluate("window.state()?.pending")) > 0, "a catch-up acknowledgement does not prove the restored work is durable");
  await tab.evaluate("window.markDurable()");
  check((await tab.evaluate("window.state()?.pending")) === 0, "the durable vector clears the restored pending work");
} finally {
  await tab?.close?.();
  server.close();
  rmSync(temp, { recursive: true, force: true });
}

if (failures.length) {
  console.error("offline-reload:\n  " + failures.join("\n  "));
  process.exit(1);
}
console.log("offline-reload: what was typed is still there after a reload with no server to ask");
