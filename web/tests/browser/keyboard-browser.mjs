// The keyboard, used the way a person uses it. axe-core reads what a page
// declares, and it cannot see where focus goes when a key is pressed. This
// test presses real keys and checks the focus rules that only show up then:
// where focus lands when a dialog opens, whether it comes back to the
// control that opened the dialog when the dialog closes, whether Tab can
// reach the panel icons at all, and whether Tab leaves the editor.
//
// Keys go through the Chrome DevTools protocol, not through dispatchEvent. A
// synthetic KeyboardEvent runs the page's key handlers but does not move
// focus, so a test built on one would pass for a focus bug that a real Tab
// press exposes straight away.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";
import { contentType, loroAlias } from "../helpers/loro.mjs";
import { KATEX_DEFINE, serveKatex } from "../helpers/katex.mjs";

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const temp = mkdtempSync(join(tmpdir(), "librepaper-keyboard-"));
const out = join(temp, "build");

const room = join(temp, "room.js");
writeFileSync(room, `
import { LoroDoc, LoroText } from "loro-crdt";
const encode = b => btoa(String.fromCharCode(...b));
const server = new LoroDoc();
const files = server.getMap("files"), paths = server.getMap("paths");
const text = files.setContainer("main", new LoroText()); text.insert(0, "A paragraph of source.");
paths.set("main", "main.html");
const notes = files.setContainer("notes", new LoroText()); notes.insert(0, "Notes");
paths.set("notes", "notes.html");
server.commit();
const update = encode(server.export({ mode: "update" }));
const vector = encode(server.oplogVersion().encode());
export function openRoom(slug, {onMessage, onConnected}) {
  window.roomReceive = onMessage;
  queueMicrotask(() => onConnected(true));
  return {
    send(message) { if (message.type === "doc-open") queueMicrotask(() => onMessage({type:"doc-state", protocol:"librepaper.room.v3", vector, durableVector: vector, updates:[update]})); return {ok:true}; },
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
        if (path.startsWith('/api/templates')) return Response.json({ templates: [] });
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
        if (path.endsWith('/config')) return Response.json({ extensions: ['.html', '.md'], text_extensions: ['.html', '.md'], asset_extensions: ['.png'], derived_extensions: [] });
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

let server, tab;

// Two animation frames is long enough for Svelte to flush the state a key
// press changed and for the dialog library to finish moving focus, which it
// does on the frame after it opens or closes.
const settle = () => tab.evaluate("new Promise(done => requestAnimationFrame(() => requestAnimationFrame(done)))");

// One key press as the browser sees it: down, then up. Enter carries its
// text, because Chrome only turns Enter into a click on a button when the
// key down names a carriage return. The other keys carry none, and the
// modifiers are a bitmask: Alt=1, Ctrl=2, Meta=4, Shift=8.
const press = async (key, { code = key, keyCode, modifiers = 0 } = {}) => {
  const text = key === "Enter" ? "\r" : undefined;
  await tab.command("Input.dispatchKeyEvent", { type: "keyDown", key, code, windowsVirtualKeyCode: keyCode, modifiers, text });
  await tab.command("Input.dispatchKeyEvent", { type: "keyUp", key, code, windowsVirtualKeyCode: keyCode, modifiers });
  await settle();
};

const focused = () => tab.evaluate("(() => { const e = document.activeElement; return e ? (e.getAttribute('aria-label') || e.textContent.trim().slice(0, 60) || e.tagName) : null })()");

// The element that has focus before a key opens something is tagged, so
// the test can say afterwards whether focus came back to that same element
// and not merely to somewhere near it.
const markOrigin = () => tab.evaluate("(() => { for (const node of document.querySelectorAll('[data-keyboard-origin]')) delete node.dataset.keyboardOrigin; document.activeElement.dataset.keyboardOrigin = 'yes'; })()");
const atOrigin = () => tab.evaluate("document.activeElement?.dataset.keyboardOrigin === 'yes'");
const focusOn = (selector) => tab.evaluate(`document.querySelector(${JSON.stringify(selector)}).focus()`);
const openDialogs = () => tab.evaluate("document.querySelectorAll('[role=\"dialog\"][data-state=\"open\"]').length");
const focusInDialog = () => tab.evaluate("Boolean(document.activeElement?.closest('[role=\"dialog\"][data-state=\"open\"]'))");

const ESCAPE = { code: "Escape", keyCode: 27 };
const TAB = { code: "Tab", keyCode: 9 };
const ENTER = { code: "Enter", keyCode: 13 };

try {
  for (const [name, screen] of Object.entries(SCREENS)) {
    const entry = join(temp, `${name}.js`);
    writeFileSync(entry, screen.entry);
    await build({
      configFile: false, define: KATEX_DEFINE, root: join(root, "web"), resolve: { alias: loroAlias },
      plugins: [tailwindcss(), svelte(), { name: "mock-room", enforce: "pre", resolveId(id) { if (/(^|\/)room\.js$/.test(id)) return room; } }],
      build: { outDir: out, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "check.js" } },
      logLevel: "error",
    });

    // The shell as the server sends it, with the entry pointed at this bundle
    // and the renderer manifest emptied, as the accessibility test does.
    const shell = readFileSync(join(root, screen.shell), "utf8")
      .replace(screen.script, "/check.js")
      .replace("__MODULES__", "{}")
      .replace("__FONTS__", "{}");
    server?.close();
    server = createServer((request, response) => {
      if (serveKatex(request, response)) return;
      const path = new URL(request.url, "http://127.0.0.1").pathname;
      if (path === "/" || path === "/docs/paper") {
        response.setHeader("content-type", "text/html");
        response.end(shell);
        return;
      }
      if (path.startsWith("/raw/")) { response.setHeader("content-type", "text/html"); response.end("<body>A paragraph of source.</body>"); return; }
      try {
        response.setHeader("content-type", contentType(path));
        response.end(readFileSync(join(out, path.slice(1))));
      } catch { response.statusCode = 404; response.end(); }
    });
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
    const url = `http://127.0.0.1:${server.address().port}${name === "reader" ? "/docs/paper" : "/"}`;

    tab ??= await browser("chromium", join(temp, "profile"), 28000 + Math.floor(Math.random() * 1000));
    await tab.navigate(url);
    // After the navigation, not before: the metrics override does not survive
    // one, and a Reader that thinks it is on a phone is a different screen.
    await tab.resize(1280, 900);
    await until(`${name} ready`, () => tab.evaluate(screen.ready), 20000);

    if (name === "landing") {
      // New project is a page inside the landing, not a dialog. Enter from
      // its button must put focus inside the page, or a keyboard user is left
      // on a button that has visibly changed the screen.
      await focusOn('[aria-label="New project"]');
      await markOrigin();
      await press("Enter", ENTER);
      assert.ok(await tab.evaluate("Boolean(document.activeElement?.closest('.new-page'))"), "Enter on New project moves focus into the New project page, so the next key is typed into it and not into the page behind");

      // The page has no Escape handler, so it is left through its own Cancel
      // button. Focus must go back to the New project button that opened it.
      await tab.evaluate("document.querySelector('.new-project-cancel').click()");
      await settle();
      assert.equal(await tab.evaluate("document.querySelectorAll('.new-page').length"), 0, "Cancel closes the New project page");
      assert.ok(await atOrigin(), `closing the New project page returns focus to the New project button, not to ${await focused()}`);

      // Settings is a page-style dialog, the one kind on this screen where
      // Escape is the way out. Enter opens it, Escape closes it.
      await tab.evaluate("[...document.querySelectorAll('button')].find(button => button.textContent.trim() === 'Settings').focus()");
      await markOrigin();
      await press("Enter", ENTER);
      assert.ok(await openDialogs() > 0, "Enter on Settings opens a dialog");
      assert.ok(await focusInDialog(), `focus moves into the Settings dialog when it opens, not to ${await focused()}`);
      await press("Escape", ESCAPE);
      assert.equal(await openDialogs(), 0, "Escape closes the Settings dialog");
      assert.ok(await atOrigin(), `closing Settings returns focus to the Settings button, not to ${await focused()}`);
    }

    if (name === "reader") {
      // Start from nothing focused, so the walk is the one a keyboard user
      // takes from the top of the page rather than from wherever the editor
      // took focus on load.
      await tab.evaluate("document.activeElement?.blur()");
      const visited = [];
      let reached = false;
      for (let step = 0; step < 40 && !reached; step++) {
        await press("Tab", TAB);
        visited.push(await focused());
        reached = await tab.evaluate("Boolean(document.activeElement?.closest('.activity-sections'))");
      }
      assert.ok(reached, `Tab from the top of the reader never reaches the panel icons; focus went through ${JSON.stringify(visited)}`);

      // A panel icon that is not already open, so pressing it is a change a
      // keyboard user can see. The open state is aria-expanded on the icon,
      // and the panel it controls stops being hidden.
      const label = await tab.evaluate("[...document.querySelectorAll('.activity-sections .icon-control')].find(node => node.getAttribute('aria-pressed') !== 'true')?.getAttribute('aria-label') ?? null");
      assert.ok(label, "at least one panel icon is not already open");
      const iconSelector = `.activity-sections [aria-label=${JSON.stringify(label)}]`;
      const icon = `document.querySelector(${JSON.stringify(iconSelector)})`;
      await focusOn(iconSelector);
      await press("Enter", ENTER);
      const panel = await tab.evaluate(`(() => { const node = ${icon}; const slot = document.getElementById(node.getAttribute('aria-controls')); return { expanded: node.getAttribute('aria-expanded'), shown: Boolean(slot) && !slot.hidden }; })()`);
      assert.ok(panel.expanded === "true" && panel.shown, `Enter on the ${label} icon opens its panel, but aria-expanded is ${panel.expanded} and the panel is ${panel.shown ? "shown" : "hidden"}`);

      // Ctrl+Alt+P opens the command palette over the reader. Its search box
      // must take focus, so the person can type a command without a click.
      await focusOn(iconSelector);
      await markOrigin();
      await press("p", { code: "KeyP", keyCode: 80, modifiers: 3 });
      assert.ok(await openDialogs() > 0, `Ctrl+Alt+P from the ${label} icon opens the command palette as a dialog`);
      assert.ok(await tab.evaluate("document.activeElement === document.querySelector('[role=\"dialog\"][data-state=\"open\"] input[aria-label=\"Command\"]')"), `the command palette opens with focus in its Command field, not on ${await focused()}`);
      await press("Escape", ESCAPE);
      assert.equal(await openDialogs(), 0, "Escape closes the command palette");
      assert.ok(await atOrigin(), `closing the command palette returns focus to the ${label} icon, not to ${await focused()}`);

      // Ctrl+Alt+/ opens the keyboard shortcuts. The same contract applies:
      // focus goes into the dialog and comes back to the icon on Escape.
      await focusOn(iconSelector);
      await markOrigin();
      await press("/", { code: "Slash", keyCode: 191, modifiers: 3 });
      assert.ok(await openDialogs() > 0, `Ctrl+Alt+/ from the ${label} icon opens the keyboard shortcuts as a dialog`);
      assert.ok(await focusInDialog(), `the keyboard shortcuts dialog opens with focus inside it, not on ${await focused()}`);
      await press("Escape", ESCAPE);
      assert.equal(await openDialogs(), 0, "Escape closes the keyboard shortcuts dialog");
      assert.ok(await atOrigin(), `closing the keyboard shortcuts returns focus to the ${label} icon, not to ${await focused()}`);

      // The editor is a trap unless Tab leaves it. CodeMirror lets Escape
      // hand the next Tab to the page, so Escape then Tab must land outside
      // the editor. A Reader that took Escape for itself would fail here.
      await tab.evaluate("document.querySelector('.cm-content').focus()");
      await press("Escape", ESCAPE);
      await press("Tab", TAB);
      assert.ok(!(await tab.evaluate("Boolean(document.activeElement?.closest('.cm-editor'))")), `Escape then Tab from the source leaves the editor, but focus is still in it at ${await focused()}`);

      assert.deepEqual(await tab.evaluate("window.testErrors"), [], "the reader raised no errors while it was used from the keyboard");
    }
  }
  console.log("keyboard-browser: landing New project and Settings, reader Tab to the panel icons, Enter opens a panel, Ctrl+Alt+P and Ctrl+Alt+/ take focus and return it on Escape, Escape then Tab leaves the editor");
} finally {
  await tab?.close();
  server?.close();
  removeTemporary(temp);
}
