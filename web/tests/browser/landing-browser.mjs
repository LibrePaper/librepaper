// The project list at phone width is a different control layout, not a wide
// table squeezed into a narrow window. Exercise its row menu and its actions
// against the real Landing component, stylesheet and page shell.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until } from "../helpers/browser-driver.mjs";

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const temp = mkdtempSync(join(tmpdir(), "librepaper-landing-browser-"));
const entry = join(temp, "entry.js"), out = join(temp, "build");
const documents = [
  { slug: "owned", title: "My paper", role: "owner", owner: "You", favorite: false, files: 3, updated_at: "2026-09-30T12:00:00Z" },
  { slug: "shared", title: "Shared paper", role: "editor", owner: "Alex", favorite: false, files: 2, updated_at: "2026-09-28T12:00:00Z" },
  { slug: "older", title: "Older draft", role: "owner", owner: "You", favorite: false, files: 1, updated_at: "2026-09-20T12:00:00Z" },
];
let server, tab;
try {
  writeFileSync(entry, `
    import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
    import Landing from ${JSON.stringify(join(root, "web/src/components/Landing.svelte"))};
    import { mount } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
    window.testCalls = [];
    window.failShared = false;
    const docs = ${JSON.stringify(documents)};
    for (const doc of docs) doc.shared_hidden = sessionStorage.getItem('shared-hidden:' + doc.slug) === 'true';
    const trash = [{ slug: 'gone', title: 'Old project', role: 'owner', updated_at: '2026-09-10T12:00:00Z', purge_due: '2026-10-10T12:00:00Z' }];
    globalThis.fetch = async (url, init = {}) => {
      const path = String(url);
      window.testCalls.push({ path, method: init.method || 'GET', body: typeof init.body === 'string' ? init.body : null });
      if (path === '/api/config') return Response.json({ extensions: ['.md'], text_extensions: ['.md'], asset_extensions: ['.png'], derived_extensions: [] });
      if (path === '/api/me') return Response.json({ name: 'Tester', can_publish: true, providers: [] });
      if (path.startsWith('/api/list')) return Response.json({ documents: docs });
      if (path === '/api/trash') return Response.json({ documents: trash });
      const parts = path.split('/');
      const slug = parts[3], action = parts[4];
      const match = slug && action ? [path, slug, action] : null;
      if (match && match[2] === 'favorite') {
        const doc = docs.find(item => item.slug === match[1]);
        if (doc) doc.favorite = init.method === 'POST';
        return Response.json({ ok: true });
      }
      if (match && match[2] === 'shared') {
        if (window.failShared) return Response.json({ error: 'could not save shared preference' }, { status: 500 });
        const doc = docs.find(item => item.slug === match[1]);
        if (doc) {
          doc.shared_hidden = init.method === 'DELETE';
          sessionStorage.setItem('shared-hidden:' + doc.slug, String(doc.shared_hidden));
        }
        return Response.json({ ok: true });
      }
      if (match && match[2] === 'rename') {
        const doc = docs.find(item => item.slug === match[1]);
        if (doc) doc.title = JSON.parse(init.body).title;
        return Response.json({ ok: true });
      }
      if (match && match[2] === 'delete') return Response.json({ ok: true });
      if (match && match[2] === 'untrash') return Response.json({ ok: true });
      if (match && match[2] === 'purge') return Response.json({ ok: true });
      if (path.startsWith('/api/documents/')) return Response.json({ comment_count: 0, files: [] });
      return Response.json({});
    };
    mount(Landing, { target: document.body });
  `);
  await build({ configFile: false, root: join(root, "web"), plugins: [tailwindcss(), svelte()],
    build: { outDir: out, emptyOutDir: true, minify: false, lib: { entry, formats: ["es"], fileName: () => "check.js", cssFileName: "librepaper-web" } }, logLevel: "error" });
  const shell = '<!doctype html><html lang="en" data-theme="librepaper"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/librepaper-web.css"></head><body><script type="module" src="/check.js"></script></body></html>';
  server = createServer((request, response) => {
    const path = new URL(request.url, "http://127.0.0.1").pathname;
    if (path === "/") { response.setHeader("content-type", "text/html"); response.end(shell); return; }
    try { response.setHeader("content-type", path.endsWith(".css") ? "text/css" : "text/javascript"); response.end(readFileSync(join(out, path.slice(1)))); }
    catch { response.statusCode = 404; response.end(); }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = 31000 + Math.floor(Math.random() * 1000);
  tab = await browser("chromium", join(temp, "profile"), port);
  await tab.resize(1100, 850);
  await tab.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("project list", () => tab.evaluate("document.querySelectorAll('.projects-table tbody tr').length === 3"));

  // Use Chromium's keyboard input path so focus and menu behavior are
  // exercised together.
  const keyCodes = { Enter: 13, ArrowDown: 40, Escape: 27 };
  const key = async (value) => {
    await tab.command("Input.dispatchKeyEvent", { type: "keyDown", key: value, code: value, windowsVirtualKeyCode: keyCodes[value] });
    await tab.command("Input.dispatchKeyEvent", { type: "keyUp", key: value, code: value, windowsVirtualKeyCode: keyCodes[value] });
    await tab.evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
  };

  const flush = () => tab.evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
  const click = async (selector) => {
    await tab.evaluate(`(() => { const node = document.querySelector(${JSON.stringify(selector)}); if (!node) throw new Error('missing ' + ${JSON.stringify(selector)}); node.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'mouse' })); node.click(); })()`);
    await flush();
  };
  const trigger = (slug) => `.project-actions-trigger[data-project-slug="${slug}"]`;
  const openActions = '.explorer-menu[data-state="open"] [data-project-action]';
  const menuActions = () => tab.evaluate(`[...document.querySelectorAll(${JSON.stringify(openActions)})].map(item => item.dataset.projectAction)`);
  const row = (slug) => `document.querySelector(${JSON.stringify(trigger(slug))})?.closest('tr')`;
  const rowHeight = (slug) => tab.evaluate(`${row(slug)}?.getBoundingClientRect().height || 0`);
  const visible = (selector) => tab.evaluate(`(() => { const node = document.querySelector(${JSON.stringify(selector)}); return !!node?.getClientRects().length && getComputedStyle(node).visibility !== 'hidden'; })()`);

  for (const width of [320, 390, 760]) {
    await tab.resize(width, 850);
    await flush();
    assert.equal(await tab.evaluate("document.querySelectorAll('.project-actions-trigger').length"), 3, `${width}px: one action menu per project`);
    assert.equal(await tab.evaluate("document.querySelectorAll('.mobile-updated').length"), 0, `${width}px: no inline relative dates`);
    assert.equal(await tab.evaluate("Boolean(document.querySelector('th[data-column=updated]'))"), false, `${width}px: updated column is hidden`);
    assert.ok(await rowHeight("owned") <= 65, `${width}px: ordinary rows stay compact (${await rowHeight("owned")}px)`);
    assert.equal(await tab.evaluate("[...document.querySelectorAll('.activity-sections .icon-control')].filter(button => button.querySelector('.compact-label')).length"), 6, `${width}px: all six bottom rail labels remain`);
    assert.equal(await tab.evaluate("(() => { const rail = document.querySelector('.activity-sections'); return rail.scrollWidth <= rail.clientWidth; })()"), true, `${width}px: the bottom rail fits without scrolling`);
    const navGap = await tab.evaluate(`(() => { const actions=document.querySelector('.nav-actions'); const project=document.querySelector('.nav-new'); const account=document.querySelector('.nav-actions .account'); return { gap:getComputedStyle(actions).columnGap, between:Math.round(account.getBoundingClientRect().left-project.getBoundingClientRect().right) }; })()`);
    assert.deepEqual(navGap, { gap: "4px", between: 4 }, `${width}px: new-project and account controls keep a 4px gap`);
    await click(trigger("owned"));
    await until("owned actions menu", () => tab.evaluate(`Boolean(document.querySelector(${JSON.stringify(openActions)}))`));
    const bounds = await tab.evaluate(`(() => { const menu=document.querySelector('.explorer-menu[data-state="open"]'); const rect=menu?.getBoundingClientRect(); return rect && {left:rect.left,right:rect.right,width:innerWidth,text:menu.innerText}; })()`);
    assert.ok(bounds && bounds.left >= -1 && bounds.right <= width + 1, `${width}px: action menu stays in viewport (${JSON.stringify(bounds)})`);
    assert.match(bounds.text, /Rename|Fork|Move to trash/, `${width}px: action names remain visible`);
    await key("Escape");
    await flush();
    await tab.evaluate(`(() => { const select=document.querySelector('#project-sort'); select.value='title:asc'; select.dispatchEvent(new Event('change',{bubbles:true})); })()`);
    await flush();
    const titleSort = await tab.evaluate(`[...document.querySelectorAll('.projects-table a.title-line')].map(link=>link.textContent.trim())`);
    assert.deepEqual(titleSort, ["My paper", "Older draft", "Shared paper"], `${width}px: title sorting still works`);
    await tab.evaluate(`(() => { const select=document.querySelector('#project-sort'); select.value='updated:desc'; select.dispatchEvent(new Event('change',{bubbles:true})); })()`);
    await flush();
    const updateSort = await tab.evaluate(`[...document.querySelectorAll('.projects-table a.title-line')].map(link=>link.textContent.trim())`);
    assert.deepEqual(updateSort, ["My paper", "Shared paper", "Older draft"], `${width}px: updated sorting still works`);
  }
  if (process.env.LIBREPAPER_LANDING_SCREENSHOT) {
    await tab.resize(390, 850);
    await flush();
    const { data } = await tab.command("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
    writeFileSync(process.env.LIBREPAPER_LANDING_SCREENSHOT, Buffer.from(data, "base64"));
  }

  // Desktop retains the metadata and its full action row.
  await tab.resize(1100, 850);
  await flush();
  assert.equal(await visible('th[data-column="updated"]'), true, "desktop restores the Updated column");
  assert.equal(await tab.evaluate("document.querySelectorAll('.project-actions-trigger').length"), 0, "desktop uses inline actions");
  assert.equal(await tab.evaluate("document.querySelectorAll('.project-actions').length"), 3, "desktop restores actions for all rows");

  // Keyboard users can open, move within and dismiss the menu with focus
  // returning to the row trigger.
  await tab.resize(390, 850);
  await flush();
  await tab.evaluate(`document.querySelector(${JSON.stringify(trigger("owned"))}).focus()`);
  await key("Enter");
  await until("keyboard opened the actions menu", () => tab.evaluate(`Boolean(document.querySelector(${JSON.stringify(openActions)}))`));
  await key("ArrowDown");
  assert.equal(await tab.evaluate(`Boolean(document.querySelector(${JSON.stringify(openActions + "[data-highlighted]")}))`), true, "ArrowDown highlights a menu action");
  await key("Escape");
  await flush();
  assert.equal(await tab.evaluate(`document.activeElement === document.querySelector(${JSON.stringify(trigger("owned"))})`), true, "Escape returns focus to the trigger");

  // Shared work can be removed from this personal list, forked or starred,
  // but cannot be renamed or trashed.
  await tab.resize(390, 850);
  await flush();
  await click('.rail-item button[aria-label="Shared with me"]');
  await until("shared projects", () => tab.evaluate(`Boolean(document.querySelector(${JSON.stringify(trigger("shared"))}))`));
  await click(trigger("shared"));
  const sharedActions = await menuActions();
  assert.deepEqual(sharedActions.sort(), ["favorite", "fork", "hide-shared"], "shared project offers remove, fork and favorite");
  await click('.explorer-menu[data-state="open"] [data-project-action="favorite"]');
  await until("favorite saved", () => tab.evaluate("testCalls.some(call => call.path === '/api/documents/shared/favorite' && call.method === 'POST')"));

  // A failed request leaves the project in Shared. A successful one hides it
  // and offers an immediate Undo; the preference can also be cleared from
  // All projects after navigating away.
  await tab.evaluate("window.failShared = true");
  await click(trigger("shared"));
  assert.equal(await tab.evaluate("document.querySelector('.explorer-menu[data-state=open] [data-project-action=favorite] .menuitem-label')?.textContent"), "Remove from favorites", "favorite state is reflected in its action");
  await click('.explorer-menu[data-state="open"] [data-project-action="hide-shared"]');
  await until("failed shared visibility request", () => tab.evaluate("testCalls.some(call => call.path === '/api/documents/shared/shared' && call.method === 'DELETE')"));
  await flush();
  assert.equal(await tab.evaluate(`Boolean(document.querySelector(${JSON.stringify(trigger("shared"))}))`), true, "failed removal leaves the shared project visible");
  await tab.evaluate("window.failShared = false");
  await click(trigger("shared"));
  await click('.explorer-menu[data-state="open"] [data-project-action="hide-shared"]');
  await until("shared project removed", () => tab.evaluate(`!document.querySelector(${JSON.stringify(trigger("shared"))})`));
  assert.equal(await tab.evaluate("testCalls.some(call => call.path === '/api/documents/shared/shared' && call.method === 'DELETE')"), true, "remove sends DELETE to the shared preference endpoint");
  await until("shared removal undo", () => tab.evaluate("[...document.querySelectorAll('button')].some(button => button.textContent.trim() === 'Undo')"));
  await tab.evaluate("[...document.querySelectorAll('button')].find(button => button.textContent.trim() === 'Undo').click()");
  await flush();
  await until("shared project restored by undo", () => tab.evaluate(`Boolean(document.querySelector(${JSON.stringify(trigger("shared"))}))`));
  assert.equal(await tab.evaluate("testCalls.some(call => call.path === '/api/documents/shared/shared' && call.method === 'POST')"), true, "Undo restores the shared preference with POST");
  await key("Escape");
  await flush();

  // The desktop row action removes the same document. Its preference survives
  // a reload, and All projects offers a direct way to show it in Shared again.
  await click('.rail-item button[aria-label="Projects"]');
  await until("all projects after undo", () => tab.evaluate(`Boolean(document.querySelector(${JSON.stringify(trigger("owned"))}))`));
  await tab.resize(1100, 850);
  await flush();
  await click('.rail-item button[aria-label="Shared with me"]');
  const removeAction = '.project-actions button[aria-label="Remove Shared paper from Shared"]';
  await until("desktop shared project", () => tab.evaluate(`Boolean(document.querySelector(${JSON.stringify(removeAction)}))`));
  await click(removeAction);
  await until("desktop shared project removed", () => tab.evaluate(`!document.querySelector(${JSON.stringify(removeAction)})`));
  await click('.rail-item button[aria-label="Projects"]');
  const showAction = '.project-actions button[aria-label="Show Shared paper in Shared"]';
  await until("desktop show action", () => tab.evaluate(`Boolean(document.querySelector(${JSON.stringify(showAction)}))`));
  const currentUrl = await tab.evaluate("location.href");
  await tab.navigate(currentUrl);
  await until("hidden preference after reload", () => tab.evaluate(`Boolean(document.querySelector(${JSON.stringify(showAction)}))`));
  await click(showAction);
  await until("desktop restore request", () => tab.evaluate("testCalls.some(call => call.path === '/api/documents/shared/shared' && call.method === 'POST')"));
  await click('.rail-item button[aria-label="Shared with me"]');
  await until("shared project restored from All projects", () => tab.evaluate(`Boolean(document.querySelector(${JSON.stringify(removeAction)}))`));
  await click('.rail-item button[aria-label="Projects"]');
  await tab.resize(390, 850);
  await flush();

  await click(trigger("owned"));
  const ownedActions = await menuActions();
  assert.deepEqual(ownedActions.sort(), ["favorite", "fork", "rename", "trash"], "owned project exposes its available actions");
  await click('.explorer-menu[data-state="open"] [data-project-action="rename"]');
  await until("rename dialog", () => tab.evaluate("Boolean(document.querySelector('#rename-project input'))"));
  await tab.evaluate(`(() => { const input=document.querySelector('#rename-project input'); input.value='Renamed paper'; input.dispatchEvent(new Event('input',{bubbles:true})); document.querySelector('#rename-project').requestSubmit(); })()`);
  await until("rename saved", () => tab.evaluate("document.querySelector('.projects-table a.title-line')?.textContent === 'Renamed paper'"));
  assert.equal(await tab.evaluate("testCalls.some(call => call.path === '/api/documents/owned/rename' && JSON.parse(call.body).title === 'Renamed paper')"), true, "rename sends the new title for the original slug");

  await click(trigger("owned"));
  await click('.explorer-menu[data-state="open"] [data-project-action="fork"]');
  await until("fork dialog", () => tab.evaluate("Boolean(document.querySelector('#copy-project input'))"));
  assert.equal(await tab.evaluate("document.querySelector('#copy-project input').value"), "Renamed paper (copy 1)", "fork opens with a usable suggested name");
  await key("Escape");
  await flush();

  // A canceled confirmation must not send a delete. Confirming does, then the
  // refreshed list remains usable.
  await click(trigger("owned"));
  await click('.explorer-menu[data-state="open"] [data-project-action="trash"]');
  await until("trash confirmation", () => tab.evaluate("Boolean([...document.querySelectorAll('[role=dialog][data-state=open]')].find(dialog => dialog.innerText.includes('seven days')))"));
  const deletesBefore = await tab.evaluate("testCalls.filter(call => call.path === '/api/documents/owned/delete').length");
  await tab.evaluate("[...document.querySelectorAll('[role=dialog][data-state=open] button')].find(button => button.textContent.trim() === 'Cancel').click()");
  await until("trash confirmation closed", () => tab.evaluate("!document.querySelector('[role=dialog][data-state=open]')"));
  await flush();
  assert.equal(await tab.evaluate("testCalls.filter(call => call.path === '/api/documents/owned/delete').length"), deletesBefore, "cancel leaves the project alone");
  await click(trigger("owned"));
  await click('.explorer-menu[data-state="open"] [data-project-action="trash"]');
  await until("second trash confirmation", () => tab.evaluate("Boolean([...document.querySelectorAll('[role=dialog][data-state=open]')].find(dialog => dialog.innerText.includes('seven days')))"));
  await tab.evaluate("[...document.querySelectorAll('[role=dialog][data-state=open] button')].find(button => button.textContent.trim() === 'Move to trash').click()");
  await until("trash request", () => tab.evaluate("testCalls.some(call => call.path === '/api/documents/owned/delete' && call.method === 'POST')"));
  await until("trash confirmation closed after delete", () => tab.evaluate("!document.querySelector('[role=dialog][data-state=open]')"));

  // Trash rows keep restore and permanent-delete reachable through the same
  // narrow menu; restore leaves the trash after its request succeeds.
  await tab.evaluate("[...document.querySelectorAll('.rail-item button')].find(button => button.getAttribute('aria-label') === 'Trash').click()");
  await until("trash loaded", () => tab.evaluate(`Boolean(document.querySelector(${JSON.stringify(trigger("gone"))}))`));
  await click('.project-actions-trigger[data-project-slug="gone"]');
  const trashActions = await menuActions();
  assert.ok(trashActions.includes("restore"), "trash menu offers restore");
  await click('.explorer-menu[data-state="open"] [data-project-action="restore"]');
  await until("project restored", () => tab.evaluate("testCalls.some(call => call.path === '/api/documents/gone/untrash')"));

  console.log("landing-browser: compact project actions, row density, responsive metadata, keyboard access and project operations passed");
} finally {
  await tab?.close();
  await new Promise((resolve) => server ? server.close(resolve) : resolve());
  rmSync(temp, { recursive: true, force: true });
}
