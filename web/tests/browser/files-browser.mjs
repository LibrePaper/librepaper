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
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-files-check-"));
const output = join(temporary, "build");
const profile = join(temporary, "chrome");
const entry = join(temporary, "entry.js");
const port = 19000 + Math.floor(Math.random() * 1000);

const source = `
import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
import { tick, createRawSnippet } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import Files from ${JSON.stringify(join(root, "web/src/components/reader/Files.svelte"))};
import Comments from ${JSON.stringify(join(root, "web/src/components/Comments.svelte"))};
import History from ${JSON.stringify(join(root, "web/src/components/reader/History.svelte"))};
import Diagnostics from ${JSON.stringify(join(root, "web/src/components/reader/Diagnostics.svelte"))};
import Share from ${JSON.stringify(join(root, "web/src/components/reader/Share.svelte"))};
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
  const target = row(path), bounds = target.getBoundingClientRect();
  target.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, button: 2, clientX: bounds.left + 40, clientY: bounds.top + 8 })); await flush();
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
  check(!document.querySelector('.explorer-more'), 'no collapsed action menus');
  check(!document.querySelector('.explorer .panel-title'), 'no visible panel title');
  for (const label of ['New file', 'New folder', 'Upload files', 'Collapse all folders', 'Download project']) {
    const control = button(label);
    check(control && control.getClientRects().length, label + ' is visible');
    check(document.querySelector('.panel-actions').contains(control), label + ' remains in the toolbar');
    check(control.querySelector('svg path, svg rect'), label + ' has an actual icon');
  }
  check(!button('Rename selected item') && !button('Move selected items') && !button('Set selected file as main'), 'no redundant selection toolbar');
  check(!document.querySelector('[aria-label="Main file"]'), 'no favorite-looking star');
  row('main.tex').dispatchEvent(new MouseEvent('dblclick', { bubbles: true })); await flush();
  await name('Rename main.tex', 'renamed-main.tex');
  check(session.paths.get(main) === 'renamed-main.tex', 'double click renames a file');
  row('renamed-main.tex').dispatchEvent(new MouseEvent('dblclick', { bubbles: true })); await flush();
  await name('Rename renamed-main.tex', 'main.tex');
  check(row('main.tex').getBoundingClientRect().height <= 40, 'compact file rows');
  check(document.querySelector('.explorer-row svg').getBoundingClientRect().width <= 24, 'compact icons');
  button('New folder').click(); await flush(); await name('New folder name', 'new');
  check(session.folders().includes('new'), 'created empty folder');
  row('new').click(); await flush(); button('New folder').click(); await flush(); await name('New folder name', 'sub');
  check(session.folders().includes('new/sub'), 'created nested folder');
  row('new/sub').dispatchEvent(new MouseEvent('dblclick', { bubbles: true })); await flush();
  await name('Rename new/sub', 'renamed');
  check(session.folders().includes('new/renamed'), 'double click renames a folder');
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
  row('main.tex').dispatchEvent(new MouseEvent('dblclick', { bubbles: true })); await flush();
  check(!button('Rename main.tex'), 'read-only double click cannot rename');
  return { passed: true, files: session.list().map((file) => file.path) };
};
window.sharingSetup = async () => {
  component.$destroy(); session.leave();
  document.body.style.width = '100%';
  document.body.style.height = '100vh';
  document.body.style.paddingTop = '0';
  window.shareRequests = [];
  window.copiedLink = '';
  let linkNumber = 0;
  let snapshot = { slug: 'paper', can_share: true, url: '/docs/paper',
    owner: { name: 'Alice', provider: 'github' },
    links: { reader: null, commenter: null, editor: null },
    edit_needs_signin: false, comment_needs_signin: false };
  window.expireShareLink = (role) => { snapshot.links[role].expired = true; };
  window.fetch = async (_url, options = {}) => {
    if (options.method !== 'POST') return new Response(JSON.stringify(snapshot));
    const body = JSON.parse(options.body);
    window.shareRequests.push(body);
    if (body.link) {
      const token = body.link.role + "-token-" + (++linkNumber);
      snapshot.links = { ...snapshot.links, [body.link.role]: {
        key: token,
        url: 'https://example.test/docs/paper#k=' + token,
        until: body.link.until === 'never' ? null : '2027-03-05T00:00:00Z',
        label: body.link.label || '',
        budget: body.link.budget,
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
    tools: snippet('<div><button>View</button><button>Share</button></div>'),
  } });
  window.testShare = createClassComponent({ component: Share, target: document.body, props: { open: true, slug: 'paper' } });
  await flush();
};
window.sharingCheck = async () => {
  // The links are the whole of the sharing: there is no access setting beside
  // them, and the owner has no link of their own here; they open the document
  // from the main page.
  check(!button('Copy your link'), 'the owner is not offered a link of their own');
  check(!document.querySelector('[aria-label="General access"]'), 'no general access setting');
  check(!button('Copy Read link'), 'a read link has to be created before it can be copied');
  check(!document.querySelector('input[readonly]'), 'raw URLs are hidden');
  button('Create Read link').click(); await flush();
  button('Copy Read link').click(); await flush();
  check(window.copiedLink.includes('#k=reader-token-'), 'a read link carries its key');
  const editLabel = button('Edit link label');
  editLabel.value = 'CI'; editLabel.dispatchEvent(new Event('input', { bubbles: true }));
  const editBudget = button('Edit link budget');
  editBudget.value = '8'; editBudget.dispatchEvent(new Event('input', { bubbles: true }));
  button('Create Edit link').click(); await flush();
  check(window.shareRequests.at(-1).link.label === 'CI', 'the link label reaches the share change');
  check(window.shareRequests.at(-1).link.budget === 8, 'the link budget reaches the share change');
  check(document.body.textContent.includes('CI') && document.body.textContent.includes('8 comments/hour'), 'link metadata is shown on its row');
  check(button('Copy Edit link'), 'new edit link can be copied');
  button('Copy Edit link').click(); await flush();
  check(window.copiedLink.includes('#k='), 'copies access token link');
  window.expireShareLink('editor');
  window.testShare.$set({ open: false }); await flush();
  window.testShare.$set({ open: true }); await flush();
  check(button('Replace Edit link'), 'expired links offer replacement');
  button('Replace Edit link').click(); await flush();
  check(button('Copy Edit link'), 'replacement link can be copied');
  button('Revoke Edit link').click(); await flush();
  check(!button('Copy Edit link') && button('Create Edit link'), 'revoking restores link creation');
  check(button('Edit link label').value === '' && button('Edit link budget').value === '', 'revoking drops link metadata');
  button('Copy Read link').click(); await flush();
  const previousReadLink = window.copiedLink;
  button('Revoke Read link').click(); await flush();
  check(!button('Copy Read link') && button('Create Read link'), 'read revocation restores creation');
  check(window.shareRequests.at(-1).revoke === 'reader', 'read revocation uses the reader role');
  button('Create Read link').click(); await flush();
  button('Copy Read link').click(); await flush();
  check(window.copiedLink !== previousReadLink, 'recreating issues a new read link');
  check(window.shareRequests.every((body) => !('visibility' in body)), 'the pane never sends a visibility');
  window.testShare.$set({ open: false }); await flush();
  // The bar carries no status of its own any more: warnings go in the
  // Reader's status row under it, where they are readable, not squeezed
  // between the file name and the icons.
  check(!document.querySelector('nav [role="status"]'), 'the bar prints no status text');
  return true;
};

window.shareSidebarCheck = async () => {
  window.testShare.$set({ open: true, inline: true, onclose: () => window.testShare.$set({ open: false }) });
  await flush();
  const panel = document.querySelector('.share-sidebar');
  check(panel && !panel.closest('[role="dialog"]'), 'sharing opens inside a panel');
  panel.style.width = '384px';
  panel.style.height = '320px';
  await flush();
  check(panel.scrollWidth <= panel.clientWidth + 1, 'sharing controls fit the fixed sidebar width');
  check(panel.scrollHeight > panel.clientHeight, 'long sharing settings scroll in the panel');
  panel.style.width = '192px'; await flush();
  check(panel.scrollWidth <= panel.clientWidth + 1, 'controls wrap when workspace is narrow');
  button('Copy Read link').click(); await flush();
  check(window.copiedLink.includes('#k=reader-token-'), 'sidebar copies the read link');
  check(!panel.textContent.includes('Document link'), 'no duplicate document link');
  check(!panel.querySelector('input[readonly]'), 'sidebar hides raw URL fields');
  const writeClipboard = navigator.clipboard.writeText;
  navigator.clipboard.writeText = async () => { throw new Error('blocked'); };
  button('Copy Read link').click(); await flush();
  check(button('Link to copy manually')?.value.includes('#k=reader-token-'), 'blocked clipboard exposes selectable fallback');
  navigator.clipboard.writeText = writeClipboard;
  button('Copy Read link').click(); await flush();
  check(!button('Link to copy manually'), 'successful copy hides manual fallback');
  check(!button('Close sharing'), 'sharing uses the activity rail to close like other sidebars');
  return true;
};

window.panelTypographyCheck = async () => {
  const samples = [];
  const mounted = [];
  for (const [component, props] of [[Files, {}], [Comments, {}], [History, {}], [Diagnostics, {}], [Share, { open: true, inline: true, slug: 'paper' }]]) {
    const host = document.createElement('div');
    host.style.cssText = 'display:flex;flex-direction:column;width:360px;height:500px';
    document.body.append(host);
    const instance = createClassComponent({ component, target: host, props });
    mounted.push({ instance, host });
    await flush();
    const panel = host.querySelector('.panel');
    const title = host.querySelector('h2.sr-only');
    check(panel && title, 'every panel retains an accessible heading');
    check(!host.querySelector('.panel-header, .panel-title'), 'no visible title row');
    const titleStyle = getComputedStyle(title);
    check(titleStyle.position === 'absolute' && title.getBoundingClientRect().height <= 1, 'accessible heading takes no layout space');
    const bodyStyle = getComputedStyle(panel);
    samples.push([bodyStyle.fontFamily, bodyStyle.fontSize, bodyStyle.lineHeight, bodyStyle.padding]);
  }
  check(samples.every((sample) => JSON.stringify(sample) === JSON.stringify(samples[0])), 'all five sidebar panels have identical base typography and padding');
  for (const { instance, host } of mounted) { instance.$destroy(); host.remove(); }
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
    response.end('<!doctype html><html data-theme="librepaper"><head><link rel="stylesheet" href="/files-check.css"></head><body style="width:340px;height:720px"><script type="module" src="/files-check.js"></script></body></html>');
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
  assert.equal(await evaluate("shareSidebarCheck()"), true);
  assert.equal(await evaluate("panelTypographyCheck()"), true);
  if (process.env.SHARE_SCREENSHOT) {
    await send("Emulation.setDeviceMetricsOverride", { width: 900, height: 900, deviceScaleFactor: 1, mobile: false });
    await evaluate("document.querySelector('.share-sidebar').style.cssText = 'width:384px;height:800px;background:var(--color-sidebar)' ");
    const shot = await send("Page.captureScreenshot", { format: "png" });
    writeFileSync(process.env.SHARE_SCREENSHOT, Buffer.from(shot.result.data, "base64"));
  }
  console.log("files-browser: creation, nesting, rename, keyboard, drag/drop, move dialog, duplication, delete, uploads, collisions, read-only, sharing and navbar passed");
} finally {
  socket?.close();
  browser?.kill();
  if (server?.listening) await new Promise((resolve) => server.close(resolve));
  rmSync(temporary, { recursive: true, force: true });
}
