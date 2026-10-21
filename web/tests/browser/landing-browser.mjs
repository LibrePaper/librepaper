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
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const temp = mkdtempSync(join(tmpdir(), "librepaper-landing-browser-"));
const entry = join(temp, "entry.js"), out = join(temp, "build");
const documents = [
  { slug: "owned", title: "My paper", role: "owner", owner: "You", tags: ["thesis"], files: 3, updated_at: "2026-09-30T12:00:00Z" },
  { slug: "shared", title: "Shared paper", role: "editor", owner: "Alex", tags: [], files: 2, updated_at: "2026-09-28T12:00:00Z" },
  { slug: "older", title: "Older draft", role: "owner", owner: "You", tags: [], files: 1, updated_at: "2026-09-20T12:00:00Z" },
];
// The account's tags as the listing carries them: Favorites, which is built in,
// and the ones the reader made.
const tagList = [
  { id: "favorites", name: "Favorites", colour: "yellow", builtin: true },
  { id: "thesis", name: "Thesis", colour: "blue", builtin: false },
  { id: "teaching", name: "Teaching", colour: "bluish-green", builtin: false },
];
let server, tab;
try {
  writeFileSync(entry, `
    import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
    import Landing from ${JSON.stringify(join(root, "web/src/components/Landing.svelte"))};
    import { mount } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
    window.testCalls = [];
    window.failShared = false;
    window.tagList = ${JSON.stringify(tagList)};
    const docs = ${JSON.stringify(documents)};
    for (const doc of docs) doc.shared_hidden = sessionStorage.getItem('shared-hidden:' + doc.slug) === 'true';
    if (sessionStorage.getItem('landing-many-tags') === 'yes') {
      for (let i = 1; i <= 100; i++) window.tagList.push({ id: 'many-' + i, name: 'Many tag ' + i, colour: 'blue', builtin: false });
      docs.find(doc => doc.slug === 'older').tags.push('many-100');
    }
    const trash = [{ slug: 'gone', title: 'Old project', role: 'owner', updated_at: '2026-09-10T12:00:00Z', purge_due: '2026-10-10T12:00:00Z' }];
    globalThis.fetch = async (url, init = {}) => {
      const path = String(url);
      window.testCalls.push({ path, method: init.method || 'GET', body: typeof init.body === 'string' ? init.body : null });
      if (path === '/api/config') return Response.json({ extensions: ['.md'], text_extensions: ['.md'], asset_extensions: ['.png'], derived_extensions: [] });
      if (path === '/api/me') return Response.json({ name: 'Tester', can_publish: true, providers: [] });
      if (path.startsWith('/api/list')) return Response.json({ documents: docs, tags: window.tagList });
      if (path === '/api/trash') return Response.json({ documents: trash });
      if (path === '/api/tags' && init.method === 'POST') {
        const body = JSON.parse(init.body);
        const tag = { id: 'tag-' + (window.tagList.length + 1), name: body.name, colour: body.colour, builtin: false };
        window.tagList.push(tag);
        return Response.json({ tag }, { status: 201 });
      }
      const parts = path.split('/');
      const slug = parts[3], action = parts[4];
      const match = slug && action ? [path, slug, action] : null;
      if (match && match[2] === 'tags') {
        const doc = docs.find(item => item.slug === match[1]);
        const id = parts[5];
        if (doc) doc.tags = init.method === 'PUT' ? [...new Set([...doc.tags, id])] : doc.tags.filter(each => each !== id);
        return Response.json({ slug: match[1], tag: id, on: init.method === 'PUT' });
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
  const searchButton = 'body > nav button[aria-label="Search projects"]';
  const searchField = 'body > nav input.project-search';
  const closeSearch = 'body > nav button[aria-label="Close search"]';
  const scrollList = (top) => tab.evaluate(`(() => { const list = document.querySelector('.projects'); list.scrollTop = ${top}; list.dispatchEvent(new Event('scroll')); })()`);
  const barState = () => tab.evaluate(`({ nav: document.querySelector('body > nav').classList.contains('bar-hidden'), main: document.querySelector('main#main').classList.contains('bar-hidden') })`);
  // At phone width the places live in the Menu, not on the rail.
  const goPlace = async (id) => {
    await click("body > nav .compact-panels-trigger");
    await until("places menu", () => tab.evaluate(`Boolean(document.querySelector('.explorer-menu[data-state="open"]'))`));
    await click(`.explorer-menu[data-state="open"] [data-panel-id="${id}"]`);
  };

  for (const width of [320, 390, 760]) {
    await tab.resize(width, 850);
    await flush();
    assert.equal(await tab.evaluate("document.querySelectorAll('.project-actions-trigger').length"), 3, `${width}px: one action menu per project`);
    assert.equal(await tab.evaluate("document.querySelectorAll('.mobile-updated').length"), 0, `${width}px: no inline relative dates`);
    assert.equal(await tab.evaluate("Boolean(document.querySelector('th[data-column=updated]'))"), false, `${width}px: updated column is hidden`);
    assert.ok(await rowHeight("owned") <= 65, `${width}px: ordinary rows stay compact (${await rowHeight("owned")}px)`);
    assert.equal(await tab.evaluate("document.querySelectorAll('.activity-sections').length"), 0, `${width}px: there is no bottom rail`);
    assert.equal(await tab.evaluate(`Boolean(document.querySelector('body > nav .compact-panels-trigger[aria-label="Menu"]'))`), true, `${width}px: the top bar has the Menu button`);
    assert.equal(await tab.evaluate(`Boolean(document.querySelector('body > nav button[aria-label="Search projects"]'))`), true, `${width}px: the top bar has the search button`);
    assert.equal(await tab.evaluate(`Boolean(document.querySelector('body > nav input.project-search'))`), false, `${width}px: no search field until the button is pressed`);
    assert.equal(await tab.evaluate(`(() => { const node = document.querySelector('.nav-actions .account'); return !node || node.getClientRects().length === 0; })()`), true, `${width}px: the account avatar is hidden (it lives in the Menu)`);
    assert.equal(await tab.evaluate("getComputedStyle(document.querySelector('.nav-actions')).columnGap"), "8px", `${width}px: the nav actions keep the editor's 8px gap`);
    await click(searchButton);
    await until("search field open", () => tab.evaluate(`Boolean(document.querySelector(${JSON.stringify(searchField)}))`));
    assert.equal(await tab.evaluate(`document.activeElement === document.querySelector(${JSON.stringify(searchField)})`), true, `${width}px: the search field is focused`);
    await click(closeSearch);
    await until("search field closed", () => tab.evaluate(`!document.querySelector(${JSON.stringify(searchField)})`));
    assert.equal(await tab.evaluate(`Boolean(document.querySelector(${JSON.stringify(searchButton)}))`), true, `${width}px: closing search brings the search button back`);
    // The list scrolls itself; the bar hides on scrolling down and returns on scrolling up.
    // The spacer keeps these scrolls far from the end, where the bar holds still.
    await tab.evaluate(`(() => { const spacer = document.createElement('div'); spacer.id = 'scroll-spacer'; spacer.style.cssText = 'flex: none; height: 2000px'; document.querySelector('.projects').append(spacer); })()`);
    await scrollList(10);
    await flush();
    await scrollList(300);
    await flush();
    assert.deepEqual(await barState(), { nav: true, main: true }, `${width}px: scrolling the list down hides the bar`);
    await scrollList(100);
    await flush();
    assert.deepEqual(await barState(), { nav: false, main: false }, `${width}px: scrolling the list up shows the bar`);
    await tab.evaluate("document.querySelector('.projects').scrollTop = 0");
    await tab.evaluate("document.querySelector('#scroll-spacer').remove()");
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

  // The Menu's Trash item shows how many projects are in the trash, once the
  // trash has been read: like the desktop rail's badge, it is fetched on a visit.
  await tab.resize(390, 850);
  await flush();
  await goPlace("trash");
  await until("trash place", () => tab.evaluate(`document.querySelector('.place-heading')?.textContent.trim() === "Trash"`));
  await goPlace("projects");
  await until("projects place", () => tab.evaluate(`document.querySelector('.place-heading')?.textContent.trim() === "Projects"`));
  await click("body > nav .compact-panels-trigger");
  await until("places menu for the trash count", () => tab.evaluate(`Boolean(document.querySelector('.explorer-menu[data-state="open"] [data-panel-id="trash"]'))`));
  await until("trash count in the Menu", () => tab.evaluate(`document.querySelector('.explorer-menu[data-state="open"] [data-panel-id="trash"] .menuitem-count')?.textContent.trim() === "1"`));
  await key("Escape");
  await flush();

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
  await goPlace("shared");
  await until("shared projects", () => tab.evaluate(`Boolean(document.querySelector(${JSON.stringify(trigger("shared"))}))`));
  await click(trigger("shared"));
  const sharedActions = await menuActions();
  assert.deepEqual(sharedActions.sort(), ["favorite", "fork", "hide-shared"], "shared project offers remove, fork and favorite");
  await click('.explorer-menu[data-state="open"] [data-project-action="favorite"]');
  await until("favorite saved", () => tab.evaluate("testCalls.some(call => call.path === '/api/documents/shared/tags/favorites' && call.method === 'PUT')"));

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
  await goPlace("projects");
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
  await goPlace("trash");
  await until("trash loaded", () => tab.evaluate(`Boolean(document.querySelector(${JSON.stringify(trigger("gone"))}))`));
  await click('.project-actions-trigger[data-project-slug="gone"]');
  const trashActions = await menuActions();
  assert.ok(trashActions.includes("restore"), "trash menu offers restore");
  await click('.explorer-menu[data-state="open"] [data-project-action="restore"]');
  await until("project restored", () => tab.evaluate("testCalls.some(call => call.path === '/api/documents/gone/untrash')"));

  // Tags: the rail filters by one, a project's menu puts one on and takes it
  // off, the rail makes a new one, and search finds a project by a tag's name.
  // Menu items are placed a frame after they open, so each click waits for a
  // rect on screen rather than for the item to exist.
  const onScreen = (selector) => tab.evaluate(`(() => { const node = document.querySelector(${JSON.stringify(selector)}); if (!node) return false; const rect = node.getBoundingClientRect(); return rect.width > 0 && rect.height > 0 && rect.left >= 0 && rect.top >= 0 && rect.right <= innerWidth && rect.bottom <= innerHeight; })()`);
  const clickOnScreen = async (label, selector) => {
    await until(label, () => onScreen(selector));
    await click(selector);
  };
  const titles = () => tab.evaluate(`[...document.querySelectorAll('.projects-table a.title-line')].map(link => link.textContent.trim())`);
  const placeHeading = () => tab.evaluate(`document.querySelector('.place-heading')?.textContent.trim()`);
  await tab.resize(1100, 850);
  await flush();

  // The rail's one Tags control opens a menu of the account's tags.
  const railTags = '.activity-sections button[aria-label="Tags"]';
  await until("tags control in the rail", () => tab.evaluate(`Boolean(document.querySelector(${JSON.stringify(railTags)}))`));
  await click(railTags);
  await clickOnScreen("thesis item on screen", '.explorer-menu[data-state="open"] [data-rail-tag="thesis"]');
  await until("thesis place", () => tab.evaluate(`document.querySelector('.place-heading')?.textContent.trim() === "Thesis"`));
  assert.deepEqual(await titles(), ["Renamed paper"], "the rail filters the list to projects with the tag");
  assert.match(await tab.evaluate("location.search"), /[?&]tag=thesis(&|$)/, "the tag is in the address");
  assert.equal(await tab.evaluate("new URLSearchParams(location.search).has('in')"), false, "a tag leaves the place parameter alone");
  assert.equal(await tab.evaluate(`document.querySelector(${JSON.stringify(railTags)}).getAttribute('aria-pressed')`), "true", "the Tags control is pressed while a tag is the place");
  await click('.rail-item button[aria-label="Projects"]');
  await until("projects after the tag", () => tab.evaluate("document.querySelectorAll('.projects-table tbody tr').length === 3"));
  assert.equal(await tab.evaluate("new URLSearchParams(location.search).has('tag')"), false, "a place removes the tag from the address");

  // The project's own menu puts the tag on and takes it off, and the row shows
  // the chip as soon as the server has agreed.
  const olderRow = () => tab.evaluate(`[...document.querySelectorAll('.projects-table tbody tr')].find(row => row.textContent.includes('Older draft'))?.querySelector('.tag-chip')?.textContent.trim() ?? ''`);
  const toggleTeaching = async (wanted) => {
    await click('button[aria-label="Tags for Older draft"]');
    await clickOnScreen("tag menu item on screen", '.explorer-menu[data-state="open"] [data-project-tag="teaching"]');
    await until(wanted ? "teaching put on" : "teaching taken off", () => tab.evaluate(`testCalls.some(call => call.path === '/api/documents/older/tags/teaching' && call.method === '${wanted ? "PUT" : "DELETE"}')`));
  };
  await toggleTeaching(true);
  await until("teaching chip shown", async () => (await olderRow()) === "Teaching");
  await toggleTeaching(false);
  await until("teaching chip removed", async () => (await olderRow()) === "");
  await toggleTeaching(true);
  await until("teaching chip back", async () => (await olderRow()) === "Teaching");

  // The chip is a button that opens its tag as the place.
  await click('.projects-table tbody tr button.tag-chip[aria-label="Show projects tagged Teaching"]');
  await until("teaching place from its chip", async () => (await placeHeading()) === "Teaching");
  assert.match(await tab.evaluate("location.search"), /[?&]tag=teaching(&|$)/, "the chip puts the tag in the address");
  assert.deepEqual(await titles(), ["Older draft"], "the chip filters the list to projects with the tag");
  await click('.rail-item button[aria-label="Projects"]');
  await until("projects after the chip", () => tab.evaluate("document.querySelectorAll('.projects-table tbody tr').length === 3"));

  // A new tag is made from the rail's Tags menu, and the dialog says it is the account's.
  await click(railTags);
  await clickOnScreen("new tag item on screen", '.explorer-menu[data-state="open"] [data-rail-action="new-tag"]');
  await until("new tag dialog", () => tab.evaluate("Boolean(document.querySelector('#new-tag input'))"));
  await tab.evaluate(`(() => { const input = document.querySelector('#new-tag input'); input.value = 'Draft'; input.dispatchEvent(new Event('input', { bubbles: true })); document.querySelector('#new-tag').requestSubmit(); })()`);
  assert.equal(await tab.evaluate("testCalls.some(call => call.path === '/api/tags' && call.method === 'POST' && JSON.parse(call.body).name === 'Draft')"), true, "New tag posts the name to the account");
  await click(railTags);
  await until("draft tag in the Tags menu", () => tab.evaluate(`[...document.querySelectorAll('.explorer-menu[data-state="open"] [data-rail-tag]')].some(item => item.textContent.includes('Draft'))`));
  await key("Escape");

  // Search finds a project by the name of a tag it has, and not by anything else.
  const searchInput = 'body > nav input.project-search';
  await tab.evaluate(`(() => { const field = document.querySelector(${JSON.stringify(searchInput)}); field.value = 'teaching'; field.dispatchEvent(new Event('input', { bubbles: true })); })()`);
  await until("search by tag name", async () => JSON.stringify(await titles()) === JSON.stringify(["Older draft"]));
  await tab.evaluate(`(() => { const field = document.querySelector(${JSON.stringify(searchInput)}); field.value = ''; field.dispatchEvent(new Event('input', { bubbles: true })); })()`);
  await until("search cleared", () => tab.evaluate("document.querySelectorAll('.projects-table tbody tr').length === 3"));

  // An address naming a tag the account does not have shows Projects instead.
  await tab.navigate(`http://127.0.0.1:${server.address().port}/?tag=missing`);
  await until("unknown tag falls back to Projects", () => tab.evaluate(`!new URLSearchParams(location.search).has('tag') && document.querySelectorAll('.projects-table tbody tr').length === 3`));
  assert.equal(await placeHeading(), "Projects", "an unknown tag shows Projects");

  // Widths stored by a desktop drag must not reach the phone layout. The
  // actions column is not resizable, and the title takes the row up to the menu.
  // The table reads its stored widths on mount, so seed them, then load the
  // Projects place again at desktop width before going to phone width.
  await tab.resize(1100, 850);
  await tab.evaluate(`localStorage.setItem('librepaper:table-widths', JSON.stringify({ 'projects:projects': { mark: 44, title: 120, owner: 210, files: 90, updated: 130, actions: 150 } }))`);
  await tab.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("project list after seeding widths", () => tab.evaluate("document.querySelectorAll('.projects-table tbody tr').length > 0"));
  await tab.resize(390, 850);
  await flush();
  await flush();
  const phoneWidths = await tab.evaluate(`(() => { const width = (selector) => document.querySelector(selector)?.getBoundingClientRect().width ?? 0; return { actions: width('th[data-column=actions]'), title: width('th[data-column=title]'), table: width('.projects-table') }; })()`);
  assert.ok(Math.abs(phoneWidths.actions - 48) <= 1, "390px: a stored desktop width does not widen the menu column");
  assert.ok(phoneWidths.title >= phoneWidths.table - 44 - 48 - 2, "390px: the title takes the row up to the menu");
  await tab.evaluate("localStorage.removeItem('librepaper:table-widths')");

  // Between the phone and tablet widths Owner and Files go, so the title keeps
  // room for more than a word. The table keeps the widths it read on mount, so
  // the page loads again without the ones seeded above.
  await tab.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("project list without stored widths", () => tab.evaluate("document.querySelectorAll('.projects-table tbody tr').length > 0"));
  await tab.resize(820, 850);
  await flush();
  await flush();
  assert.equal(await visible("th[data-column=owner]"), false, "820px: Owner and Files give the title room");
  assert.equal(await visible("th[data-column=files]"), false, "820px: Owner and Files give the title room");
  assert.equal(await visible("th[data-column=updated]"), true, "820px: the date stays");
  const titleWidth = await tab.evaluate("document.querySelector('th[data-column=title]')?.getBoundingClientRect().width ?? 0");
  assert.ok(titleWidth >= 250, `820px: the title is wider than a word (${titleWidth}px)`);
  await tab.resize(1100, 850);
  await flush();
  await flush();
  assert.equal(await visible("th[data-column=owner]"), true, "1100px: Owner comes back");

  // A hundred tags: each menu and the Manage dialog scroll inside the window
  // rather than running past it. The entry adds the tags on load when this
  // flag is set, so the page is loaded again to list them.
  await tab.evaluate("sessionStorage.setItem('landing-many-tags', 'yes')");
  await tab.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("project list with many tags", () => tab.evaluate("document.querySelectorAll('.projects-table tbody tr').length === 3"));
  await tab.resize(1100, 850);
  await flush();
  const scrolls = (selector) => tab.evaluate(`(() => { const node = document.querySelector(${JSON.stringify(selector)}); if (!node) return 'missing'; const rect = node.getBoundingClientRect(); return { fits: rect.bottom <= innerHeight && rect.top >= 0, scrollable: node.scrollHeight > node.clientHeight + 1 }; })()`);
  const fitsNow = async (selector) => (await scrolls(selector))?.fits === true;

  const railList = '.explorer-menu[data-state="open"] .tags-menu-items';
  const manyLast = '.explorer-menu[data-state="open"] [data-rail-tag="many-100"]';
  await click(railTags);
  await until("rail tags list in the window", () => fitsNow(railList));
  const railShape = await scrolls(railList);
  assert.equal(railShape.fits, true, "the rail's Tags list fits the window");
  assert.equal(railShape.scrollable, true, "the rail's Tags list scrolls");
  await tab.evaluate(`document.querySelector(${JSON.stringify(manyLast)}).scrollIntoView({ block: 'nearest' })`);
  await clickOnScreen("last tag on screen", manyLast);
  await until("last tag place", async () => (await placeHeading()) === "Many tag 100");
  await click('.rail-item button[aria-label="Projects"]');
  await until("projects after many tags", () => tab.evaluate("document.querySelectorAll('.projects-table tbody tr').length === 3"));

  const rowMenu = '.explorer-menu[data-state="open"]';
  const newTagItem = '.explorer-menu[data-state="open"] [data-project-tag="new"]';
  await click('button[aria-label="Tags for Older draft"]');
  await until("row tag menu in the window", () => fitsNow(rowMenu));
  const rowShape = await scrolls(rowMenu);
  assert.equal(rowShape.fits, true, "the row's tag menu fits the window");
  assert.equal(rowShape.scrollable, true, "the row's tag menu scrolls");
  await tab.evaluate(`document.querySelector(${JSON.stringify(newTagItem)}).scrollIntoView({ block: 'nearest' })`);
  assert.equal(await onScreen(newTagItem), true, "New tag can be reached in the row's tag menu");
  await key("Escape");
  await flush();

  // Manage tags opens a dialog whose list scrolls inside it, with Done in view.
  const manageDialog = '[role="dialog"][data-state="open"]';
  const checkManage = async (label) => {
    await until(`${label} manage dialog in the window`, () => fitsNow(manageDialog));
    const shape = await scrolls(manageDialog);
    assert.equal(shape.fits, true, `${label}: the Manage tags dialog fits the window`);
    const listScrolls = await tab.evaluate("(() => { const list = document.querySelector('.tag-manage').parentElement; return list.scrollHeight > list.clientHeight + 1; })()");
    assert.equal(listScrolls, true, `${label}: the tag list scrolls inside the dialog`);
    const doneOnScreen = await tab.evaluate(`(() => { const button = [...document.querySelectorAll(${JSON.stringify(manageDialog + " button")})].find(each => each.textContent.trim() === 'Done'); if (!button) return false; const rect = button.getBoundingClientRect(); return rect.width > 0 && rect.bottom <= innerHeight && rect.top >= 0; })()`);
    assert.equal(doneOnScreen, true, `${label}: Done is on screen`);
    await key("Escape");
    await until(`${label} manage dialog closed`, () => tab.evaluate(`!document.querySelector(${JSON.stringify(manageDialog)})`));
  };
  const manageItem = '.explorer-menu[data-state="open"] [data-rail-action="manage-tags"]';
  await click(railTags);
  await until("manage tags item rendered", () => tab.evaluate(`Boolean(document.querySelector(${JSON.stringify(manageItem)}))`));
  await tab.evaluate(`document.querySelector(${JSON.stringify(manageItem)}).scrollIntoView({ block: 'nearest' })`);
  await clickOnScreen("manage tags on screen", manageItem);
  await until("manage tags dialog", () => tab.evaluate("Boolean(document.querySelector('.tag-manage'))"));
  await checkManage("1100px");

  // At phone width the same dialog opens from the Menu.
  await tab.resize(390, 844);
  await flush();
  const phoneManage = '.explorer-menu[data-state="open"] [data-workspace-action="manage-tags"]';
  await click("body > nav .compact-panels-trigger");
  await until("phone manage tags item rendered", () => tab.evaluate(`Boolean(document.querySelector(${JSON.stringify(phoneManage)}))`));
  await tab.evaluate(`document.querySelector(${JSON.stringify(phoneManage)}).scrollIntoView({ block: 'nearest' })`);
  await clickOnScreen("phone manage tags on screen", phoneManage);
  await until("phone manage tags dialog", () => tab.evaluate("Boolean(document.querySelector('.tag-manage'))"));
  await checkManage("390px");

  console.log("landing-browser: compact project actions, row density, responsive metadata, keyboard access and project operations passed");
} finally {
  await tab?.close();
  await new Promise((resolve) => server ? server.close(resolve) : resolve());
  removeTemporary(temp);
}
