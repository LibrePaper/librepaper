// The review queue, in a real browser.
//
// A proposal is a branch and a decision names one of its hunks (SPEC-loro.md
// §3.3, §5.1), so the panel's unit is a hunk, not a proposal: an agent run
// that touches four passages is four cards and four answers, and the document
// does not move until the last of them is given (§5.1a).
//
// What is pinned here is what a reviewer can actually do -- see the words a
// change would take out beside the ones it would put in, walk the queue from
// the keyboard, answer a card, and be stopped from answering one whose text
// has moved underneath it. The panel had no coverage at all while it still
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
  acceptDisabled: row.querySelector('.row-action.accept')?.disabled ?? null,
}));
window.count = () => document.querySelector('.changes-meta')?.textContent || '';
window.feedback = () => document.querySelector('.panel-status')?.textContent || '';
window.nav = () => document.querySelector('.queue-nav span')?.textContent || '';
window.act = (id, action) => {
  const main = [...document.querySelectorAll('.row-main')].find((node) => node.id === 'change-' + id);
  if (!main) throw new Error('no row ' + id);
  // Inside a contested group each option carries its own buttons, so scope to
  // the option when there is one and to the card when there is not.
  const scope = main.closest('.contested-option') || main.closest('.change-row');
  scope.querySelector('.row-action.' + action).click();
};
window.key = (key) => {
  document.querySelector('.changes-panel')
    .dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true }));
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
window.previewButton = () => document.querySelector('.preview-bar button');
window.barButton = (word) => [...document.querySelectorAll('.preview-bar button')]
  .find((node) => node.textContent.trim().startsWith(word)) || null;
window.picks = () => document.querySelectorAll('.row-pick').length;
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
  assert.equal(moved.acceptDisabled, true, "and cannot be accepted from the queue");

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
  await until("read-only", async () => (await page.evaluate("window.rows()")).every((row) => row.acceptDisabled === true), 4000);
  assert.equal((await page.evaluate("window.rows()")).length, 4, "a reader still sees what is proposed");
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

  await page.evaluate("window.show({ proposals: window.rivals, comments: [] })");
  await until("preview offered", async () => await page.evaluate("!!window.previewButton()"), 4000);
  assert.equal(await page.evaluate("window.previewButton().disabled"), true,
    "with nothing ticked there is nothing to read");
  await page.evaluate('window.pick("alice#0")');
  await page.evaluate('window.pick("carol#0")');
  await until("two chosen", async () => /2 proposals/.test(await page.evaluate("window.previewButton().textContent")), 4000);
  await page.evaluate("window.previewButton().click()");
  await until("asked to read", async () => (await page.evaluate("window.asked")) !== null, 4000);
  assert.deepEqual((await page.evaluate("window.asked")).sort(), ["alice", "carol"],
    "the reading names proposals, not hunks");


  // The same ticks answer several changes at once. Here a tick means the one
  // hunk rather than the whole proposal -- a decision names a hunk (§5.2) --
  // and the verb says so by counting changes where the reading counts
  // proposals. Nothing is answered that a card would not let this caller
  // answer one at a time.
  await page.evaluate("window.show()");
  await until("verbs offered", async () => await page.evaluate("!!window.barButton('Accept')"), 4000);
  assert.equal(await page.evaluate("window.barButton('Accept').disabled"), true,
    "with nothing ticked there is nothing to answer for");
  await page.evaluate('window.pick("run#0")');
  await page.evaluate('window.pick("moved#0")');
  // Two ticked, one of them stale: the verb counts only what it would touch.
  await until("one answerable", async () => /Accept 1 change$/.test((await page.evaluate("window.barButton('Accept').textContent")).trim()), 4000);
  await page.evaluate("window.barButton('Accept').click()");
  await until("bulk accepted", async () => (await page.evaluate("window.decided")).length > 0, 4000);
  assert.deepEqual(await page.evaluate("window.decided"), [["run", 0, "accept"]],
    "the stale hunk is left alone rather than answered against words that moved");
  assert.match(await page.evaluate("window.feedback()"), /excluded/,
    "and the reviewer is told it was held back");

  // Two hunks of one proposal are two answers, and rejecting says so once.
  await page.evaluate("window.show()");
  await until("reset for reject", async () => (await page.evaluate("window.decided")).length === 0, 4000);
  await page.evaluate('window.pick("run#0")');
  await page.evaluate('window.pick("run#1")');
  await until("two answerable", async () => /Reject 2 changes$/.test((await page.evaluate("window.barButton('Reject').textContent")).trim()), 4000);
  await page.evaluate("window.barButton('Reject').click()");
  await until("bulk rejected", async () => (await page.evaluate("window.decided")).length === 2, 4000);
  assert.deepEqual((await page.evaluate("window.decided")).map((entry) => entry[1]).sort(), [0, 1],
    "both hunks of the proposal are answered, each by its own index");
  assert.match(await page.evaluate("window.feedback()"), /2 changes rejected/);
  assert.equal(await page.evaluate("window.barButton('Reject').disabled"), true,
    "and the ticks are spent, so the verb cannot be pressed twice");

  // A reader is offered neither the verbs nor a tick that leads nowhere: this
  // queue has no reading to offer without onproposalpreview.
  await page.evaluate("window.show({ canReview: false, canModerate: false, onproposalpreview: undefined })");
  await until("inert for a reader", async () => (await page.evaluate("window.picks()")) === 0, 4000);
  assert.equal(await page.evaluate("!!window.barButton('Accept')"), false,
    "a reader is not offered a verb they cannot use");

  console.log("changes-browser: one card per hunk, rival changes stand together, a chosen set reads as prose or is answered together, stale hunks inert");
} finally {
  await page?.close();
  server?.close();
  rmSync(temporary, { recursive: true, force: true });
}
