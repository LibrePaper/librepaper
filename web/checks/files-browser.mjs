// Browser regression checks for the Skeleton file explorer and real shared operations.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(here));
const temporary = mkdtempSync(join(tmpdir(), "komodoc-files-check-"));
const output = join(temporary, "build");
const profile = join(temporary, "chrome");
const entry = join(temporary, "entry.js");
const port = 19000 + Math.floor(Math.random() * 1000);

const source = `
import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
import { tick, createRawSnippet } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import Files from ${JSON.stringify(join(root, "web/src/components/Files.svelte"))};
import Share from ${JSON.stringify(join(root, "web/src/components/Share.svelte"))};
import Nav from ${JSON.stringify(join(root, "web/src/components/Nav.svelte"))};
import { join as joinSession } from ${JSON.stringify(join(root, "web/src/lib/collab.js"))};
import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};
const session = joinSession({ send() {}, mayEdit: true });
const rules = { text_extensions: [".tex", ".md"], asset_extensions: [".png"], max_path: 200 };
const main = session.addText("main.tex", "main");
session.setMain(main);
const child = session.addText("chapters/one.tex", "chapter");
session.addFolder("empty", rules);
const component = createClassComponent({ component: Files, target: document.body, props: {
  files: session.list(), folders: session.folders(), rules, mayEdit: true, open: main,
  onadd: (path) => session.addText(path), onmkdir: (path) => session.addFolder(path, rules),
  onrelocate: (...args) => session.relocate(args[0], args[1], rules, args[2]),
  ondelete: (entries) => session.removeEntries(entries),
  onduplicate: (entry, path) => session.duplicateEntry(entry, path, rules),
  onopen: (file) => component.$set({ open: file.id }),
  ontext: async (file, path) => session.addText(path, await file.text()),
} });
session.onFiles(() => component.$set({ files: session.list(), folders: session.folders() }));
const flush = async () => { await tick(); await new Promise((resolve) => setTimeout(resolve, 80)); await tick(); };
const button = (label) => document.querySelector('[aria-label="' + label + '"]');
const row = (path) => document.querySelector('.explorer-row[title="' + path + '"]');
const check = (condition, message) => { if (!condition) throw new Error(message); };
const key = (target, key) => target.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }));
const name = async (label, value) => { const input = button(label); check(input, 'missing input ' + label + ' inputs: ' + [...document.querySelectorAll('input')].map(x=>x.outerHTML).join(' ') + ' menus: ' + [...document.querySelectorAll('[role=menu][data-state=open]')].map(x=>x.outerHTML).join(' ')); input.value = value; input.dispatchEvent(new Event('input', { bubbles: true })); key(input, 'Enter'); await flush(); };
const menu = async (path, text) => {
  button('Actions for ' + path).click(); await flush();
  const item = [...document.querySelectorAll('[role="menuitem"]')].find((item) => item.textContent.trim().startsWith(text) && item.getClientRects().length);
  check(item, 'missing menu item ' + text); item.dispatchEvent(new PointerEvent('pointermove', { pointerType: 'mouse', bubbles: true })); await flush(); item.click(); await flush();
};
window.contextCheck = async (lower = false) => {
  document.body.style.paddingTop = lower ? "190px" : "64px";
  await flush();
  const target = row('main.tex');
  const rect = target.getBoundingClientRect();
  const x = rect.left + 90, y = rect.top + rect.height / 2;
  target.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, button: 2, clientX: x, clientY: y }));
  await flush();
  const menu = document.querySelector('[role="menu"][data-state="open"]');
  check(menu, 'right click opens the file menu');
  const bounds = menu.getBoundingClientRect();
  check(bounds.left >= 0 && bounds.top >= 0 && bounds.right <= innerWidth && bounds.bottom <= innerHeight, 'context menu remains inside viewport');
  check(Math.min(Math.abs(bounds.top - y), Math.abs(bounds.bottom - y)) < 20, 'context menu stays next to right click');
  check(Math.abs(bounds.left - x) < 20, 'context menu has pointer horizontal position');
  check(getComputedStyle(menu).fontFamily === getComputedStyle(target).fontFamily, 'consistent menu font');
  check(getComputedStyle(menu).fontSize === getComputedStyle(target).fontSize, 'consistent menu font size');
  const item = menu.querySelector('[role="menuitem"]');
  const itemRect = item.getBoundingClientRect();
  check(menu.contains(document.elementFromPoint(itemRect.left + 8, itemRect.top + 8)), 'context menu is above page content');
  return true;
};
window.filesCheck = async () => {
  await flush();
  check(document.querySelector('[role="tree"]'), 'Skeleton tree mounted');
  check(!document.querySelector('.explorer-root'), 'no redundant root row');
  check(row('main.tex').getBoundingClientRect().height <= 40, 'compact file rows');
  check(document.querySelector('.explorer-row svg').getBoundingClientRect().width <= 24, 'compact icons');
  button('New folder').click(); await flush(); await name('New folder name', 'new');
  check(session.folders().includes('new'), 'created empty folder');
  row('new').click(); await flush(); button('New folder').click(); await flush(); await name('New folder name', 'sub');
  check(session.folders().includes('new/sub'), 'created nested folder');
  await menu('new/sub', 'Rename'); await name('Rename new/sub', 'renamed');
  check(session.folders().includes('new/renamed'), 'renamed folder from menu');
  row('chapters').click(); await flush(); row('chapters/one.tex').click(); await flush();
  key(row('chapters/one.tex'), 'F2'); await flush(); await name('Rename chapters/one.tex', 'two.tex');
  check(session.paths.get(child) === 'chapters/two.tex', 'F2 renamed file');
  const transfer = new DataTransfer();
  row('chapters').dispatchEvent(new DragEvent('dragstart', { dataTransfer: transfer, bubbles: true, cancelable: true }));
  row('empty').dispatchEvent(new DragEvent('drop', { dataTransfer: transfer, bubbles: true, cancelable: true }));
  await flush(); check(session.paths.get(child) === 'empty/chapters/two.tex', 'drag moved entire folder');
  const toTop = new DataTransfer();
  row('empty/chapters').dispatchEvent(new DragEvent('dragstart', { dataTransfer: toTop, bubbles: true, cancelable: true }));
  document.querySelector('.explorer').dispatchEvent(new DragEvent('drop', { dataTransfer: toTop, bubbles: true, cancelable: true }));
  await flush(); check(session.paths.get(child) === 'chapters/two.tex', 'drop on sidebar moves to top level');
  await menu('chapters', 'Move to');
  const confirmMove = [...document.querySelectorAll('button')].find((button) => button.textContent.trim() === 'Move');
  check(confirmMove, 'move dialog opened'); confirmMove.click(); await flush();
  check(session.paths.get(child) === 'chapters/two.tex', 'Move to root');
  await menu('chapters/two.tex', 'Duplicate');
  check(session.list().some((file) => file.path === 'chapters/two (2).tex'), 'duplicated file');
  await menu('new', 'Delete');
  const cancel = [...document.querySelectorAll('button')].find((button) => button.textContent.trim() === 'Cancel' && button.getClientRects().length);
  cancel.click(); await flush(); check(session.folders().includes('new'), 'cancel preserves folders');
  await menu('new', 'Delete');
  const confirmDelete = [...document.querySelectorAll('button')].find((button) => button.textContent.trim() === 'Delete' && button.getClientRects().length);
  confirmDelete.click(); await flush(); check(!session.folders().includes('new/renamed'), 'recursive delete');
  const upload = new DataTransfer(); upload.items.add(new File(['uploaded'], 'upload.tex', { type: 'text/plain' }));
  row('empty').dispatchEvent(new DragEvent('drop', { dataTransfer: upload, bubbles: true, cancelable: true }));
  await flush(); check(session.list().some((file) => file.path === 'empty/upload.tex'), 'external upload to folder');
  row('empty').dispatchEvent(new DragEvent('drop', { dataTransfer: upload, bubbles: true, cancelable: true }));
  await flush();
  const both = [...document.querySelectorAll('button')].find((button) => button.textContent.trim() === 'Keep both' && button.getClientRects().length);
  check(both, 'upload collision dialog'); both.click(); await flush();
  check(session.list().some((file) => file.path === 'empty/upload (2).tex'), 'upload keep both');
  component.$set({ mayEdit: false }); await flush();
  check(!button('New folder'), 'read-only toolbar');
  check(!document.querySelector('[draggable="true"]'), 'read-only drag disabled');
  return { passed: true, files: session.list().map((file) => file.path) };
};
window.sharingSetup = async () => {
  component.$destroy(); session.leave();
  document.body.style.width = '100%';
  document.body.style.height = '100vh';
  document.body.style.paddingTop = '0';
  window.shareRequests = [];
  window.copiedLink = '';
  let snapshot = { slug: 'paper', can_share: true, visibility: 'private', listing: true,
    owner: { name: 'Alice', provider: 'github' },
    links: { reader: null, commenter: null, editor: null },
    edit_needs_signin: false, comment_needs_signin: false };
  window.fetch = async (_url, options = {}) => {
    if (options.method !== 'POST') return new Response(JSON.stringify(snapshot));
    const body = JSON.parse(options.body);
    window.shareRequests.push(body);
    if (body.visibility === 'listed') return new Response(JSON.stringify({ error: 'Listing disabled' }), { status: 403 });
    if (body.visibility) snapshot.visibility = body.visibility;
    if (body.link) {
      snapshot.links = { ...snapshot.links, [body.link.role]: {
        key: 'test-access-token',
        url: 'https://example.test/docs/paper#k=test-access-token',
        until: body.link.until === 'never' ? null : '2027-03-05T00:00:00Z',
        expired: false,
      } };
    }
    if (body.revoke) snapshot.links = { ...snapshot.links, [body.revoke]: null };
    return new Response(JSON.stringify(snapshot));
  };
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async (value) => { window.copiedLink = value; } } });
  const snippet = (html) => createRawSnippet(() => ({ render: () => html }));
  window.testNav = createClassComponent({ component: Nav, target: document.body, props: {
    children: snippet('<span>Example project</span>'),
    tools: snippet('<div><button>Layout</button><button>Share</button></div>'),
    status: snippet('<small class="badge preset-tonal-warning">Changes are only saved in this browser</small>'),
  } });
  window.testShare = createClassComponent({ component: Share, target: document.body, props: { open: true, slug: 'paper' } });
  await flush();
};
window.sharingCheck = async () => {
  const visibleButton = (text) => [...document.querySelectorAll('button')].find((element) => element.textContent.trim() === text && element.getClientRects().length);
  check(document.querySelector('[aria-label="General access"]').value === 'private', 'restricted access shown');
  const copy = visibleButton('Copy link');
  const rect = copy.getBoundingClientRect();
  check(rect.top >= 0 && rect.bottom <= innerHeight, 'copy action visible on short viewport');
  copy.click(); await flush(); check(window.copiedLink.endsWith('/docs/paper'), 'copies document link');
  const editSection = document.querySelector('[aria-label="Edit link expiry"]').closest('section');
  [...editSection.querySelectorAll('button')].find((button) => button.textContent.trim() === 'Create').click();
  await flush();
  const link = document.querySelector('[aria-label="Edit link"]');
  check(link?.value.includes('#k='), 'new access link available');
  visibleButton('Copy').click(); await flush(); check(window.copiedLink === link.value, 'copies access token link');
  const editLinkSection = link.closest('section');
  [...editLinkSection.querySelectorAll('button')].find((button) => button.textContent.trim() === 'Turn off').click();
  await flush();
  check(!document.querySelector('[aria-label="Edit link"]'), 'revoked key removed');
  const visibility = document.querySelector('[aria-label="General access"]');
  visibility.value = 'listed'; visibility.dispatchEvent(new Event('change', { bubbles: true })); await flush();
  check(visibility.value === 'private', 'refused access change restores displayed setting');
  visibleButton('Done').click(); await flush();
  const status = document.querySelector('.nav-status').getBoundingClientRect();
  const actions = document.querySelector('.nav-actions').getBoundingClientRect();
  check(status.right <= actions.left, 'warnings do not split or overlap action icons');
  check(!document.querySelector('.nav-actions .badge'), 'status outside action group');
  return true;
};

window.filesCheckReady = true;
`;

writeFileSync(entry, source);
let server;
let browser;
let socket;
try {
  await build({
    configFile: false,
    root: join(root, "web"),
    plugins: [tailwindcss(), svelte()],
    logLevel: "error",
    build: {
      outDir: output,
      emptyOutDir: true,
      lib: { entry, formats: ["es"], fileName: () => "files-check.js", cssFileName: "files-check" },
    },
  });

  server = createServer((request, response) => {
    if (request.url === "/files-check.css") {
      response.setHeader("Content-Type", "text/css");
      response.end(readFileSync(join(output, "files-check.css")));
      return;
    }
    if (request.url === "/files-check.js") {
      response.setHeader("Content-Type", "text/javascript");
      response.end(readFileSync(join(output, "files-check.js")));
      return;
    }
    response.setHeader("Content-Type", "text/html");
    response.end('<link rel="stylesheet" href="/files-check.css"><body style="width:340px;height:720px"><script type="module" src="/files-check.js"></script></body>');
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  const httpPort = typeof address === "object" && address ? address.port : 0;
  if (!httpPort) throw new Error("local editor server did not start");

  browser = spawn("chromium", [
    "--headless=new",
    "--no-sandbox",
    "--disable-gpu",
    `--user-data-dir=${profile}`,
    `--remote-debugging-port=${port}`,
    "about:blank",
  ], { stdio: "ignore" });
  const wait = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));
  let browserInfo;
  for (let attempt = 0; attempt < 100; attempt++) {
    try {
      browserInfo = await fetch(`http://127.0.0.1:${port}/json`).then((answer) => answer.json());
      break;
    } catch {
      await wait(100);
    }
  }
  const page = browserInfo?.find((target) => target.type === "page");
  if (!page) throw new Error("Chromium did not expose a page target");
  socket = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, { once: true });
    socket.addEventListener("error", reject, { once: true });
  });

  let sequence = 0;
  const pending = new Map();
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(event.data);
    const resolve = pending.get(message.id);
    if (resolve) {
      pending.delete(message.id);
      resolve(message);
    }
  });
  const send = (method, params = {}) => new Promise((resolve) => {
    const id = ++sequence;
    pending.set(id, resolve);
    socket.send(JSON.stringify({ id, method, params }));
  });
  const evaluate = async (expression) => {
    const message = await send("Runtime.evaluate", {
      expression,
      awaitPromise: true,
      returnByValue: true,
    });
    if (message.result?.exceptionDetails) throw new Error(JSON.stringify(message.result.exceptionDetails));
    return message.result.result.value;
  };

  await send("Page.navigate", { url: `http://127.0.0.1:${httpPort}/` });
  let ready = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    ready = await evaluate("Boolean(window.filesCheckReady && document.querySelector('[role=tree]'))");
    if (ready) break;
    await wait(100);
  }
  assert.equal(ready, true, "Editor did not mount in Chromium");
  assert.equal(await evaluate("contextCheck()"), true);
  if (process.env.FILES_MENU_SCREENSHOT) {
    const menuScreenshot = await send("Page.captureScreenshot", { format: "png" });
    writeFileSync(process.env.FILES_MENU_SCREENSHOT, Buffer.from(menuScreenshot.result.data, "base64"));
  }
  await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape" });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape" });
  assert.equal(await evaluate("contextCheck(true)"), true);
  await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape" });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape" });
  await evaluate("document.body.style.paddingTop = '0px'");
  const screenshot = await send("Page.captureScreenshot", { format: "png" });
  if (process.env.FILES_SCREENSHOT) writeFileSync(process.env.FILES_SCREENSHOT, Buffer.from(screenshot.result.data, "base64"));
  const result = await evaluate("filesCheck()");
  assert.equal(result.passed, true);
  await evaluate("sharingSetup()");
  if (process.env.SHARE_SCREENSHOT) {
    await send("Emulation.setDeviceMetricsOverride", { width: 900, height: 820, deviceScaleFactor: 1, mobile: false });
    await evaluate("new Promise(r => setTimeout(r, 100))");
    const shot = await send("Page.captureScreenshot", { format: "png" });
    writeFileSync(process.env.SHARE_SCREENSHOT, Buffer.from(shot.result.data, "base64"));
  }
  await send("Emulation.setDeviceMetricsOverride", { width: 780, height: 437, deviceScaleFactor: 1, mobile: false });
  assert.equal(await evaluate("sharingCheck()"), true);
  console.log("files-browser: creation, nesting, rename, keyboard, drag/drop, move dialog, duplication, delete, uploads, collisions, read-only, sharing and navbar passed");
} finally {
  socket?.close();
  browser?.kill();
  if (server?.listening) await new Promise((resolve) => server.close(resolve));
  rmSync(temporary, { recursive: true, force: true });
}
