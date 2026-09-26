// The review queue, in a real browser.
//
// A proposal is a branch and a decision names one of its hunks (SPEC-loro.md
// §3.3, §5.1), so the panel's unit is a hunk, not a proposal: an agent run
// that touches four passages is four cards and four answers, and the document
// does not move until the last of them is given (§5.1a).
//
// What is pinned here is what a reviewer can actually do -- see the words a
// change would take out beside the ones it would put in, walk the queue from
// the keyboard, answer the change they are looking at, and be stopped from
// answering one whose text has moved underneath it. The verbs belong to that
// one change; ticking several, and answering for the whole queue, are things
// asked for from the menu rather than controls the panel wears all day. The panel had no coverage at all while it still
// carried the deleted revision mechanism, which is how it came to be wired to
// nothing without that showing up anywhere.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until } from "../../tools/browser-driver.mjs";
import { contentType, loroAlias } from "../helpers/loro.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-changes-"));
const entry = join(temporary, "entry.js");
writeFileSync(entry, `
import ${JSON.stringify(join(root, "src/styles/app.css"))};
import { mount, unmount } from ${JSON.stringify(join(root, "node_modules/svelte/src/index-client.js"))};
import { tick } from ${JSON.stringify(join(root, "node_modules/svelte/src/index-client.js"))};
import Changes from ${JSON.stringify(join(root, "src/components/reader/Changes.svelte"))};

window.decided = [];
window.revealed = [];

// Two proposals over two files. The first is an agent run: two hunks, so
// answering one of them leaves the other waiting and the paper unchanged.
// The second is one hunk whose base has moved, which the panel must refuse
// to let anybody answer rather than answer against the wrong words.
const rows = [
  { id: 'run#0', proposal: 'run', hunk: 0, __from: 'proposal', author: 'ada@example.org',
    file_id: 'f1', path: 'paper.md', position: 4, before: 'cat', after: 'tabby',
    created_at: new Date(Date.now() - 60000).toISOString() },
  { id: 'run#1', proposal: 'run', hunk: 1, __from: 'proposal', author: 'ada@example.org',
    file_id: 'f2', path: 'notes.md', position: 0, before: '', after: 'A new line.\\n',
    created_at: new Date(Date.now() - 60000).toISOString() },
  { id: 'moved#0', proposal: 'moved', hunk: 0, __from: 'proposal', author: 'grace@example.org',
    file_id: 'f1', path: 'paper.md', position: 40, before: '', after: 'late',
    stale: 'The text this was written against has changed.',
    created_at: new Date(Date.now() - 30000).toISOString() },
];

const comments = [
  { id: 'c1', motivation: 'editing', exact: 'sat', proposed: 'perched',
    author: 'hopper@example.org', path: 'paper.md', source: { path: 'paper.md', exact: 'sat' } },
];

const files = [{ id: 'f1', path: 'paper.md' }, { id: 'f2', path: 'notes.md' }];

const handlers = {
  onproposaldecide: (item, action) => {
    window.decided.push([item.proposal, item.hunk, action]);
    return Promise.resolve({ ok: true });
  },
  onproposalreveal: (item) => window.revealed.push(item.id),
  onaccept: (comment) => { window.decided.push(['comment', comment.id, 'accept']); return Promise.resolve({ ok: true }); },
  onreject: (comment) => { window.decided.push(['comment', comment.id, 'reject']); return Promise.resolve({ ok: true }); },
  onreveal: (comment) => window.revealed.push(comment.id),
  onproposalpreview: (ids) => { window.asked = ids; },
};

let component = null;
window.show = async (props = {}) => {
  if (component) await unmount(component);
  window.decided = []; window.revealed = []; window.asked = null;
  component = mount(Changes, { target: document.body, props: {
    proposals: rows, comments, files, canReview: true, canModerate: true, ...handlers, ...props,
  } });
  await tick();
};

window.rows = () => [...document.querySelectorAll('.change-row')].map((row) => ({
  id: row.querySelector('.row-main')?.id?.replace('change-', '') || '',
  removed: row.querySelector('.deletion')?.textContent?.trim() || '',
  added: row.querySelector('.insertion')?.textContent?.trim() || '',
  active: row.classList.contains('active'),
  blocked: row.classList.contains('blocked'),
}));
window.count = () => document.querySelector('.changes-meta')?.textContent || '';
window.feedback = () => document.querySelector('.panel-status')?.textContent || '';
const press = (node) => {
  for (const type of ['pointerdown', 'pointerup', 'click']) {
    node.dispatchEvent(new PointerEvent(type, { bubbles: true, composed: true, button: 0, isPrimary: true, pointerType: 'mouse' }));
  }
};
const settle = async () => { await tick(); await new Promise((resolve) => setTimeout(resolve, 60)); await tick(); };
const mainOf = (id) => {
  const node = [...document.querySelectorAll('.row-main')].find((one) => one.id === 'change-' + id);
  if (!node) throw new Error('no row ' + id);
  return node;
};
// Only the change being looked at offers an answer, so reaching one means
// selecting its row first -- which is what a reviewer does with the click
// that opens it.
window.look = async (id) => { mainOf(id).click(); await settle(); };
window.act = async (id, action) => {
  await window.look(id);
  const main = mainOf(id);
  const scope = main.closest('.contested-option') || main.closest('.change-row');
  scope.querySelector('.row-action.' + action).click();
};
// What the selected change offers: whether either verb can be pressed at all.
window.actions = async (id) => {
  await window.look(id);
  const scope = mainOf(id).closest('.contested-option') || mainOf(id).closest('.change-row');
  const accept = scope.querySelector('.row-action.accept');
  const reject = scope.querySelector('.row-action.reject');
  return accept && reject ? { accept: accept.disabled, reject: reject.disabled } : null;
};
window.key = (key) => {
  document.querySelector('.changes-panel')
    .dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true }));
};
// The infrequent things live behind the ••• , so the test opens it the way a
// reviewer does rather than reaching past it. Only the menu actually on show
// counts: the filter beside it keeps its own items in the page whether or not
// anybody has opened it.
const openItems = () => [...document.querySelectorAll('[data-part="content"][data-state="open"] [data-part="item"]')];
window.menuItems = async () => {
  press(document.querySelector('.changes-menu'));
  await settle();
  return openItems().map((node) => node.textContent.trim());
};
window.menu = async (label) => {
  press(document.querySelector('.changes-menu'));
  await settle();
  const item = openItems().find((node) => node.textContent.trim() === label);
  if (!item) throw new Error('no menu item ' + label);
  press(item);
  await settle();
};

// Two proposals over the same words, which the Reader marks with a shared
// contested id (SPEC-loro.md §5.3). The panel's job is to stand them
// together, and to leave an unrelated change alone.
window.rivals = [
  { id: 'alice#0', proposal: 'alice', hunk: 0, __from: 'proposal', author: 'alice@example.org',
    file_id: 'f1', path: 'paper.md', position: 4, before: 'cat', after: 'tabby', contested: 'alice#0',
    created_at: new Date(Date.now() - 7200000).toISOString() },
  { id: 'bob#0', proposal: 'bob', hunk: 0, __from: 'proposal', author: 'bob@example.org',
    file_id: 'f1', path: 'paper.md', position: 4, before: 'cat sat', after: 'dog lay', contested: 'alice#0',
    created_at: new Date(Date.now() - 1200000).toISOString() },
  { id: 'carol#0', proposal: 'carol', hunk: 0, __from: 'proposal', author: 'carol@example.org',
    file_id: 'f2', path: 'notes.md', position: 0, before: '', after: 'Elsewhere.',
    created_at: new Date(Date.now() - 600000).toISOString() },
];

window.groups = () => [...document.querySelectorAll('.changes-list > .change-row')].map((row) => ({
  contested: row.classList.contains('contested'),
  head: row.querySelector('.contested-head')?.textContent?.trim() || '',
  options: [...row.querySelectorAll('.contested-option .row-main')].map((node) => node.id.replace('change-', '')),
  id: row.querySelector('.row-main')?.id?.replace('change-', '') || '',
}));
window.pick = (id) => {
  const node = [...document.querySelectorAll('.row-main')].find((n) => n.id === 'change-' + id);
  node.closest('.row-head').querySelector('.row-pick').click();
};
window.barButton = (word) => [...document.querySelectorAll('.select-bar .bar-action')]
  .find((node) => node.textContent.trim().startsWith(word)) || null;
window.selectedCount = () => document.querySelector('.select-bar .select-count')?.textContent?.trim() || '';
window.picks = () => document.querySelectorAll('.row-pick').length;
// Everything answered already: the queue's empty state, which offers to show
// what was decided rather than a pair of verbs with nothing to do.
window.settled = rows.map((row) => ({ ...row, status: 'accepted', resolved: true }));
window.empty = () => ({
  text: document.querySelector('.changes-empty')?.textContent?.trim() || '',
  link: document.querySelector('.changes-empty .empty-link')?.textContent?.trim() || '',
  actions: document.querySelectorAll('.row-action').length,
});
window.ready = true;
await window.show();
`);

let server, page;
try {
  await build({
    configFile: false, root, plugins: [svelte(), tailwindcss()], logLevel: "error",
    resolve: { alias: loroAlias },
    build: { outDir: join(temporary, "build"), lib: { entry, formats: ["es"], fileName: () => "panel.js" } },
  });
  server = createServer((request, response) => {
    const file = request.url === "/panel.js" ? "panel.js" : request.url === "/style.css" ? "librepaper-web.css" : null;
    response.setHeader("content-type", file ? contentType(file) : "text/html");
    response.end(file
      ? readFileSync(join(temporary, "build", file))
      : '<!doctype html><html data-theme="librepaper"><head><link rel="stylesheet" href="/style.css"><style>body{display:flex;height:700px;width:380px;overflow:hidden}</style></head><body><script type="module" src="/panel.js"></script></body></html>');
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  page = await browser("chromium", join(temporary, "profile"), 26000 + Math.floor(Math.random() * 10000));
  await page.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("panel", () => page.evaluate("window.ready"), 15000);

  // One card per hunk, and the words on both sides of each decision. A
  // reviewer choosing whether to lose a sentence has to be able to read it.
  const rows = await page.evaluate("window.rows()");
  // File order, then offset within the file -- the order a reviewer reads the
  // paper in, not the order the proposals happen to have arrived in. A
  // suggestion is anchored by its words rather than by an offset, so it sorts
  // to the end of its file rather than to the top of it.
  assert.deepEqual(rows.map((row) => row.id), ["run#0", "moved#0", "suggestion:c1", "run#1"],
    "every hunk is its own card, in reading order, with suggestions among them");
  assert.equal(rows[0].removed, "− cat", "the words a change would take out stay readable");
  assert.equal(rows[0].added, "+ tabby", "beside the ones it would put in");
  assert.equal(rows.find((row) => row.id === "run#1").removed, "", "a pure insertion shows nothing struck through");
  assert.match(await page.evaluate("window.count()"), /4 pending/);

  // A hunk whose base has moved is shown and cannot be answered: the words at
  // those offsets are no longer the words the author proposed changing.
  const moved = rows.find((row) => row.id === "moved#0");
  assert.equal(moved.blocked, true, "a stale hunk is marked as needing attention");
  assert.deepEqual(await page.evaluate('window.actions("moved#0")'), { accept: true, reject: true },
    "and neither verb can be pressed on it");
  // The verbs belong to the change being looked at and to no other: a queue
  // of twenty is not twenty invitations to answer something unread.
  assert.equal(await page.evaluate("document.querySelectorAll('.row-action').length"), 2,
    "only the selected change carries a pair of buttons");

  // Answering a hunk names the proposal and the index within it, because that
  // is the only name for a hunk that means the same on both sides (§5.2).
  await page.evaluate('window.act("run#0", "accept")');
  await until("accepted", async () => (await page.evaluate("window.decided")).length > 0, 4000);
  assert.deepEqual(await page.evaluate("window.decided"), [["run", 0, "accept"]]);

  // And the queue moves on to the next card awaiting an answer, which is the
  // next one in the paper rather than the next one in this proposal.
  await until("advanced", async () => (await page.evaluate("window.rows()")).some((row) => row.id === "moved#0" && row.active), 4000);

  // The keyboard walks the same queue.
  await page.evaluate("window.show()");
  await until("reset", async () => (await page.evaluate("window.decided")).length === 0, 4000);
  await page.evaluate('window.key("j")');
  await until("moved down", async () => (await page.evaluate("window.rows()")).some((row) => row.id === "moved#0" && row.active), 4000);
  // A stale card refuses the keyboard as it refuses the button, and says why.
  await page.evaluate('window.key("a")');
  await until("refused", async () => /has changed/.test(await page.evaluate("window.feedback()")), 4000);
  assert.deepEqual(await page.evaluate("window.decided"), [], "a stale hunk is not answered by pressing A at it");
  await page.evaluate('window.key("j")');await page.evaluate('window.key("j")');
  await until("at the last card", async () => (await page.evaluate("window.rows()")).some((row) => row.id === "run#1" && row.active), 4000);
  await page.evaluate('window.key("r")');
  await until("rejected by key", async () => (await page.evaluate("window.decided")).length > 0, 4000);
  assert.deepEqual(await page.evaluate("window.decided"), [["run", 1, "reject"]]);

  // A suggestion is a proposal with a remark on it (§1.2), and is decided
  // through the same queue rather than a mechanism of its own.
  await page.evaluate("window.show()");
  await until("reset again", async () => (await page.evaluate("window.decided")).length === 0, 4000);
  await page.evaluate('window.act("suggestion:c1", "accept")');
  await until("suggestion decided", async () => (await page.evaluate("window.decided")).length > 0, 4000);
  assert.deepEqual(await page.evaluate("window.decided"), [["comment", "c1", "accept"]]);

  // Without the right to review, the queue is readable and inert.
  await page.evaluate("window.show({ canReview: false, canModerate: false })");
  await until("read-only", async () => (await page.evaluate("window.rows()")).length === 4, 4000);
  for (const id of ["run#0", "run#1", "suggestion:c1"]) {
    assert.deepEqual(await page.evaluate("window.actions(" + JSON.stringify(id) + ")"), { accept: true, reject: true },
      "a reader sees what is proposed and cannot answer it");
  }
  // Rival changes stand together (§5.3). Two proposals over the same words are
  // one question with two answers, and the reviewer sees them as such rather
  // than meeting the second one several cards later.
  await page.evaluate("window.show({ proposals: window.rivals, comments: [] })");
  await until("rivals shown", async () => (await page.evaluate("window.groups()")).length > 0, 4000);
  const groups = await page.evaluate("window.groups()");
  assert.equal(groups.length, 2, "the two rivals are one entry, and the unrelated change is its own");
  assert.equal(groups[0].contested, true, "the rival pair is marked as contested");
  assert.deepEqual(groups[0].options, ["alice#0", "bob#0"], "both answers are inside the one card");
  assert.match(groups[0].head, /2 proposals change the same text/);
  assert.match(groups[0].head, /still to answer/, "and the reviewer is told the others remain");
  assert.equal(groups[1].contested, false, "a change nobody contests is an ordinary card");
  assert.equal(groups[1].id, "carol#0");
  assert.match(await page.evaluate("window.count()"), /1 contested/);

  // Both rivals are still answerable on their own: grouping them presents the
  // choice, it does not take the decision away or make it a single action.
  await page.evaluate('window.act("bob#0", "accept")');
  await until("rival answered", async () => (await page.evaluate("window.decided")).length > 0, 4000);
  assert.deepEqual(await page.evaluate("window.decided"), [["bob", 0, "accept"]]);

  // Reading a chosen set of proposals as prose (§5.3). Ticking changes selects
  // the proposals they belong to -- a proposal is a branch and a reading
  // applies it whole -- so two hunks of one proposal count once.
  //
  // None of it is on show until it is asked for: ticks and the verbs that go
  // with them are a mode entered from the menu, so an ordinary review is a
  // queue of changes rather than a queue of changes wearing checkboxes.
  await page.evaluate("window.show({ proposals: window.rivals, comments: [] })");
  await until("queue drawn", async () => (await page.evaluate("window.rows()")).length === 2, 4000);
  assert.equal(await page.evaluate("window.picks()"), 0, "no checkboxes until selection is asked for");
  assert.equal(await page.evaluate("!!window.barButton('Accept')"), false, "and no bulk verbs standing above the queue");
  await page.evaluate("window.menu('Select multiple')");
  await until("selecting", async () => (await page.evaluate("window.picks()")) === 3, 4000);
  assert.equal(await page.evaluate("window.selectedCount()"), "0 selected");
  assert.equal(await page.evaluate("window.barButton('Read').disabled"), true,
    "with nothing ticked there is nothing to read");
  await page.evaluate('window.pick("alice#0")');
  await page.evaluate('window.pick("carol#0")');
  await until("two chosen", async () => (await page.evaluate("window.selectedCount()")) === "2 selected", 4000);
  await page.evaluate("window.barButton('Read').click()");
  await until("asked to read", async () => (await page.evaluate("window.asked")) !== null, 4000);
  assert.deepEqual((await page.evaluate("window.asked")).sort(), ["alice", "carol"],
    "the reading names proposals, not hunks");

  // The same ticks answer several changes at once. Here a tick means the one
  // hunk rather than the whole proposal -- a decision names a hunk (§5.2).
  // Nothing is answered that a card would not let this caller answer one at a
  // time.
  await page.evaluate("window.show()");
  await page.evaluate("window.menu('Select multiple')");
  await until("verbs offered", async () => await page.evaluate("!!window.barButton('Accept')"), 4000);
  assert.equal(await page.evaluate("window.barButton('Accept').disabled"), true,
    "with nothing ticked there is nothing to answer for");
  await page.evaluate('window.pick("moved#0")');
  await until("one stale ticked", async () => (await page.evaluate("window.selectedCount()")) === "1 selected", 4000);
  assert.equal(await page.evaluate("window.barButton('Accept').disabled"), true,
    "a stale hunk on its own leaves the verb with nothing it may touch");
  await page.evaluate('window.pick("run#0")');
  await until("two ticked", async () => (await page.evaluate("window.selectedCount()")) === "2 selected", 4000);
  await page.evaluate("window.barButton('Accept').click()");
  await until("bulk accepted", async () => (await page.evaluate("window.decided")).length > 0, 4000);
  assert.deepEqual(await page.evaluate("window.decided"), [["run", 0, "accept"]],
    "the stale hunk is left alone rather than answered against words that moved");
  assert.match(await page.evaluate("window.feedback()"), /excluded/,
    "and the reviewer is told it was held back");
  assert.equal(await page.evaluate("window.picks()"), 0,
    "the ticks are spent and the mode closes behind them");

  // Two hunks of one proposal are two answers, and rejecting says so once.
  await page.evaluate("window.show()");
  await page.evaluate("window.menu('Select multiple')");
  await until("reset for reject", async () => (await page.evaluate("window.picks()")) > 0, 4000);
  await page.evaluate('window.pick("run#0")');
  await page.evaluate('window.pick("run#1")');
  await until("two answerable", async () => (await page.evaluate("window.selectedCount()")) === "2 selected", 4000);
  await page.evaluate("window.barButton('Reject').click()");
  await until("bulk rejected", async () => (await page.evaluate("window.decided")).length === 2, 4000);
  assert.deepEqual((await page.evaluate("window.decided")).map((entry) => entry[1]).sort(), [0, 1],
    "both hunks of the proposal are answered, each by its own index");
  assert.match(await page.evaluate("window.feedback()"), /2 changes rejected/);

  // The whole queue at once, from the menu, and only after being asked twice:
  // a decision is recorded and broadcast at once (§5.1), so there is no taking
  // it back once given.
  await page.evaluate("window.show()");
  await until("queue back", async () => (await page.evaluate("window.rows()")).length === 4, 4000);
  await page.evaluate("window.menu('Reject all pending changes')");
  await until("asked to confirm", async () => /Reject all 3\?/.test(await page.evaluate("document.querySelector('.confirm-bar .select-count')?.textContent || ''")), 4000);
  assert.deepEqual(await page.evaluate("window.decided"), [], "nothing is answered by opening the menu item");
  await page.evaluate("[...document.querySelectorAll('.confirm-bar .bar-action')].find((node) => node.textContent.trim() === 'Reject all').click()");
  await until("all rejected", async () => (await page.evaluate("window.decided")).length === 3, 4000);
  assert.match(await page.evaluate("window.feedback()"), /excluded/,
    "the stale hunk is held back from the whole-queue verb too");

  // A reader is offered neither the verbs nor a tick that leads nowhere: this
  // queue has no reading to offer without onproposalpreview.
  await page.evaluate("window.show({ canReview: false, canModerate: false, onproposalpreview: undefined })");
  await until("inert for a reader", async () => (await page.evaluate("window.picks()")) === 0, 4000);
  const items = await page.evaluate("window.menuItems()");
  assert.deepEqual(items, [], "a reader is offered no verb and no mode they cannot use");

  // Nothing left to answer: a sentence, and a way to see what was decided --
  // not two verbs greyed out over an empty queue.
  await page.evaluate("window.show({ proposals: window.settled, comments: [] })");
  await until("the queue is empty", async () => (await page.evaluate("window.empty()")).text.length > 0, 4000);
  const empty = await page.evaluate("window.empty()");
  assert.match(empty.text, /No pending changes/);
  assert.equal(empty.link, "View resolved changes", "and the way to see them is offered");
  assert.equal(empty.actions, 0, "with no verb standing over nothing");
  await page.evaluate("document.querySelector('.changes-empty .empty-link').click()");
  await until("resolved shown", async () => (await page.evaluate("window.rows()")).length === 3, 4000);

  console.log("changes-browser: one card per hunk, verbs on the change being read, rival changes stand together, ticking is a mode, the whole queue asks first, stale hunks inert");
} finally {
  await page?.close();
  server?.close();
  rmSync(temporary, { recursive: true, force: true });
}
