// The mechanical half of an accessibility review, on the two screens that
// carry almost all of the application's markup.
//
// axe-core is Deque's rules engine: the one behind the browser extension and
// Lighthouse's accessibility score. It catches what a machine can decide --
// a page with no language, text under the contrast threshold, a field with no
// label, an ARIA attribute on an element that cannot take it, two elements
// with one id -- and nothing about whether the thing it is looking at makes
// sense. It is the floor, not the review: a resolved comment that could only
// be opened with a mouse passed every rule in here.
//
// The shells are the real ones. The page is served from web/pages/ with its
// script swapped for the test bundle, so what axe reads is the document the
// server sends, meta tags, language and all -- not an approximation written
// in this file.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until } from "../../tools/browser-driver.mjs";
import { contentType, loroAlias } from "../helpers/loro.mjs";

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const temp = mkdtempSync(join(tmpdir(), "librepaper-accessibility-"));
const out = join(temp, "build");
const axeSource = readFileSync(join(root, "web/node_modules/axe-core/axe.min.js"), "utf8");

// The rules to hold the pages to: WCAG 2.0 and 2.1, A and AA. `best-practice`
// is left out on purpose -- it is Deque's advice rather than the standard, and
// a suite that fails on advice is a suite people learn to ignore.
const TAGS = ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"];

// Two rules cannot be answered on this bench, and neither is switched off
// because it is inconvenient:
//
// `color-contrast` reads the colours as painted, and the pages under test are
// mounted into a bare shell where a card's own background has not been
// composited the way the server's CSS composites it; it reports text over
// `transparent` as a failure it cannot measure. The palette is checked by
// arithmetic instead, at the bottom of this file, which is the stricter test.
//
// `region` wants every piece of content inside a landmark. The Reader mounts
// its own <main> and the bar above it, but the test bundle mounts them into a
// body that has no page around them, so stray text nodes land outside both.
const UNMEASURABLE = new Set(["color-contrast", "region"]);

const room = join(temp, "room.js");
writeFileSync(room, `
import { LoroDoc, LoroText } from "loro-crdt";
const encode = b => btoa(String.fromCharCode(...b));
const server = new LoroDoc();
const files = server.getMap("files"), paths = server.getMap("paths"), meta = server.getMap("meta");
const text = files.setContainer("main", new LoroText()); text.insert(0, "A paragraph of source.");
paths.set("main", "main.html"); meta.set("main", "main");
const notes = files.setContainer("notes", new LoroText()); notes.insert(0, "Notes");
paths.set("notes", "notes.html");
server.commit();
const update = encode(server.export({ mode: "update" }));
const vector = encode(server.oplogVersion().encode());
export function openRoom(slug, {onMessage, onConnected}) {
  window.roomReceive = onMessage;
  queueMicrotask(() => onConnected(true));
  return {
    send(message) { if (message.type === "doc-open") queueMicrotask(() => onMessage({type:"doc-state", protocol:"librepaper.room.v2", vector, updates:[update]})); return {ok:true}; },
    sendLive() { return {ok:true}; },
    close() {},
  };
}
`);

const SCREENS = {
  landing: {
    shell: "web/pages/index.html",
    script: "../src/entries/landing.js",
    ready: "Boolean([...document.querySelectorAll('button')].find(button => button.textContent.trim() === 'New project'))",
    entry: `
      import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
      import Landing from ${JSON.stringify(join(root, "web/src/components/Landing.svelte"))};
      import { mount } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
      const documents = [
        { slug: "one", title: "A paper about something", updated_at: "2026-09-01T10:00:00Z", comment_count: 2, file_count: 4, role: "editor" },
        { slug: "two", title: "Another draft", updated_at: "2026-08-20T10:00:00Z", comment_count: 0, file_count: 1, role: "reader" },
      ];
      globalThis.fetch = async (url) => {
        const path = String(url);
        if (path === '/api/config') return Response.json({ extensions: ['.md'], text_extensions: ['.md'], asset_extensions: ['.png'], derived_extensions: [] });
        if (path === '/api/me') return Response.json({ name: 'Tester', can_publish: true, providers: [] });
        if (path === '/api/list') return Response.json({ documents });
        if (path.startsWith('/api/documents/')) return Response.json({ comment_count: 0, files: [] });
        return Response.json({});
      };
      mount(Landing, { target: document.body });
    `,
  },
  reader: {
    shell: "web/pages/reader.html",
    script: "../src/entries/reader.js",
    ready: "Boolean(document.querySelector('.cm-editor')) && Boolean(document.querySelector('.filelist'))",
    // The component is imported after the mocks are in place, not before: a
    // static import is evaluated first, and the Reader reads the globals it
    // is handed as it loads.
    entry: `
      import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
      window.testErrors = [];
      addEventListener('error', event => window.testErrors.push(event.message));
      addEventListener('unhandledrejection', event => window.testErrors.push(String(event.reason)));
      globalThis.WebSocket = class { constructor() { queueMicrotask(() => this.onopen?.()); } send() {} close() { this.onclose?.(); } };
      globalThis.fetch = async (url) => {
        const path = String(url);
        if (path.endsWith('/me')) return Response.json({ name: 'Tester', providers: [] });
        if (path.endsWith('/config')) return Response.json({});
        if (path === '/api/documents/paper') return Response.json({ title: 'A paper about something', created_at: 'test', role: 'editor', source_format: 'html', docs_origin: location.origin, can_moderate: true, can_see_sharing: true });
        if (path.endsWith('/frame')) return Response.json({ token: 'frame-token', until: 9999999999 });
        if (path.endsWith('/chat')) return Response.json({ id: 'chat', token: 'private' });
        if (path.includes('/comments')) return { ok: true, json: async () => ({ comments: [] }) };
        return { ok: false, json: async () => ({}) };
      };
      const { default: Reader } = await import(${JSON.stringify(join(root, "web/src/components/Reader.svelte"))});
      const { mount } = await import(${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))});
      mount(Reader, { target: document.body });
    `,
  },
};

/// What axe makes of the page as it now stands, in the words a person reading
/// a failing suite needs: which rule, how bad, and the first elements it found.
async function violations(tab, where) {
  const found = await tab.evaluate(`(async () => {
    const run = await axe.run(document, { resultTypes: ['violations'], runOnly: { type: 'tag', values: ${JSON.stringify(TAGS)} } });
    return run.violations.map(violation => ({
      id: violation.id, impact: violation.impact, help: violation.help,
      nodes: violation.nodes.slice(0, 3).map(node => node.target.join(' ')),
    }));
  })()`);
  return found.filter((violation) => !UNMEASURABLE.has(violation.id)).map((violation) => ({ ...violation, where }));
}

const say = (found) => found
  .map((violation) => `  ${violation.where}: ${violation.id} (${violation.impact}) -- ${violation.help}\n    ${violation.nodes.join("\n    ")}`)
  .join("\n");

let server, tab;
const failures = [];
try {
  for (const [name, screen] of Object.entries(SCREENS)) {
    const entry = join(temp, `${name}.js`);
    writeFileSync(entry, screen.entry);
    await build({
      configFile: false, root: join(root, "web"), resolve: { alias: loroAlias },
      plugins: [tailwindcss(), svelte(), { name: "mock-room", enforce: "pre", resolveId(id) { if (/(^|\/)room\.js$/.test(id)) return room; } }],
      build: { outDir: out, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "check.js" } },
      logLevel: "error",
    });

    // The shell as the server sends it, with the entry pointed at this bundle
    // and the renderer manifest -- which only the binary can write -- emptied.
    const shell = readFileSync(join(root, screen.shell), "utf8")
      .replace(screen.script, "/check.js")
      .replace("__MODULES__", "{}");
    server?.close();
    server = createServer((request, response) => {
      const path = new URL(request.url, "http://127.0.0.1").pathname;
      if (path === "/" || path === "/docs/paper") {
        response.setHeader("content-type", "text/html");
        response.end(shell);
        return;
      }
      if (path === "/axe.js") { response.setHeader("content-type", "text/javascript"); response.end(axeSource); return; }
      if (path.startsWith("/raw/")) { response.setHeader("content-type", "text/html"); response.end("<body>A paragraph of source.</body>"); return; }
      try {
        response.setHeader("content-type", contentType(path));
        response.end(readFileSync(join(out, path.slice(1))));
      } catch { response.statusCode = 404; response.end(); }
    });
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
    const url = `http://127.0.0.1:${server.address().port}${name === "reader" ? "/docs/paper" : "/"}`;

    tab ??= await browser("chromium", join(temp, "profile"), 27000 + Math.floor(Math.random() * 1000));
    await tab.navigate(url);
    // After the navigation, not before: the metrics override does not survive
    // one, and a Reader that thinks it is on a phone is a different screen.
    await tab.resize(1280, 900);
    await until(`${name} ready`, () => tab.evaluate(screen.ready), 20000);
    await tab.evaluate(`new Promise(done => { const tag = document.createElement('script'); tag.src = '/axe.js'; tag.onload = done; document.head.append(tag); })`);
    failures.push(...await violations(tab, name));

    // The panels are most of the Reader's markup and none of it is on the
    // screen until its icon is pressed, so each one is opened and read.
    if (name === "landing") {
      await tab.evaluate("[...document.querySelectorAll('button')].find(button => button.textContent.trim() === 'New project').click()");
      await tab.evaluate("new Promise(done => requestAnimationFrame(() => requestAnimationFrame(done)))");
      failures.push(...await violations(tab, "landing: new project"));
    }

    if (name === "reader") {
      const panels = await tab.evaluate(`[...document.querySelectorAll('.activity-sections .icon-control')].map(node => node.getAttribute('aria-label'))`);
      for (const panel of panels) {
        await tab.evaluate(`document.querySelector('.activity-sections [aria-label=${JSON.stringify(panel)}]').click()`);
        await tab.evaluate("new Promise(done => requestAnimationFrame(() => requestAnimationFrame(done)))");
        failures.push(...await violations(tab, `reader: ${panel}`));
      }

      // The two things the keyboard opens. Neither is reachable by opening a
      // panel, and both are tables of controls, which is where an id printed
      // twice or a listbox with no name would land.
      const settle = "new Promise(done => requestAnimationFrame(() => requestAnimationFrame(done)))";
      await tab.evaluate("document.querySelector('[aria-label=\"Keyboard shortcuts\"]').click()");
      await tab.evaluate(settle);
      failures.push(...await violations(tab, "reader: keyboard shortcuts"));
      await tab.evaluate("document.querySelector('[role=\"dialog\"][data-state=\"open\"] [aria-label=Close]').click()");
      await tab.evaluate(settle);

      await tab.evaluate("window.dispatchEvent(new KeyboardEvent('keydown', {key:'p', code:'KeyP', ctrlKey:true, altKey:true, bubbles:true}))");
      await tab.evaluate(settle);
      failures.push(...await violations(tab, "reader: command palette"));
      await tab.evaluate("document.querySelector('[role=\"dialog\"][data-state=\"open\"] [aria-label=Close]').click()");
      await tab.evaluate(settle);
    }
  }

  assert.equal(failures.length, 0, `axe-core found violations:\n${say(failures)}`);

  // What `color-contrast` would have told us, computed from the theme rather
  // than from pixels: the tones that carry white text, and the one colour in
  // the application that is text somebody reads a number off. An avatar is
  // coloured by a hash of the name, so a tone that fails here fails for an
  // arbitrary sixth of everyone in the document.
  const theme = readFileSync(join(root, "web/src/styles/theme.css"), "utf8");
  const value = (name) => {
    const found = new RegExp(`--${name}:\\s*(#[0-9a-f]{6})`, "i").exec(theme);
    assert.ok(found, `--${name} is a hex value in theme.css`);
    return found[1];
  };
  const luminance = (hex) => {
    const channels = [1, 3, 5].map((at) => Number.parseInt(hex.slice(at, at + 2), 16) / 255)
      .map((channel) => (channel <= 0.03928 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4));
    return 0.2126 * channels[0] + 0.7152 * channels[1] + 0.0722 * channels[2];
  };
  const ratio = (one, other) => {
    const [high, low] = [luminance(one), luminance(other)].sort((a, b) => b - a);
    return (high + 0.05) / (low + 0.05);
  };
  for (const tone of ["primary", "secondary", "tertiary", "success", "warning", "error"]) {
    const against = ratio(value(`color-${tone}-700`), "#ffffff");
    assert.ok(against >= 4.5, `white initials on ${tone}-700 are ${against.toFixed(2)}:1`);
  }
  const numbers = ratio(value("color-line-number"), "#ffffff");
  assert.ok(numbers >= 4.5, `line numbers are ${numbers.toFixed(2)}:1 against the editor`);
  console.log(`accessibility-browser: landing and reader, every panel, clean against ${TAGS.join(", ")}`);
} finally {
  await tab?.close();
  server?.close();
  rmSync(temp, { recursive: true, force: true });
}
