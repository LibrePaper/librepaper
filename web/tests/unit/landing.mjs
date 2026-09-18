// Behavioral checks for the landing page's paginated refresh and deletion
// reconciliation. The VM harness runs the actual function bodies from the
// shipped Svelte component, with only their browser collaborators supplied.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import { FORMATS, starterDocument } from "../../src/lib/starter.js";

const landing = readFileSync(new URL("../../src/components/Landing.svelte", import.meta.url), "utf8");
const body = (start, end) => {
  const from = landing.indexOf(start);
  assert.notEqual(from, -1, `Landing function not found: ${start}`);
  const to = landing.indexOf(end, from);
  assert.notEqual(to, -1, `Landing function boundary not found: ${end}`);
  return landing.slice(from, to);
};

const showList = body("  async function showList()", "  async function deleteSelected");
const reallyDelete = body("  async function reallyDelete()", "  /* --------------------------------------------------------- a new project */");
const create = body("  async function create(event)", "  $effect(() => {");

const context = (values) => vm.createContext({
  Promise,
  Set,
  Map,
  URLSearchParams,
  // A refresh that fails because the browser is offline says nothing: the
  // page has its own offline notice, and a toast per failed poll on top of it
  // is noise. Online is what every case here assumes unless it passes its own
  // `navigator`, which the offline case below does.
  navigator: { onLine: true },
  ...values,
});

// A listing spanning pages follows the server cursor and does not duplicate
// a row if a concurrent update makes it appear on two pages.
{
  const calls = [];
  const problems = [];
  const ctx = context({
    documents: [{ slug: "old" }], counts: new Map(), paths: new Map(),
    SHELL_HEADERS: { "X-Test": "yes" },
    fetch: async (url) => {
      calls.push(url);
      if (calls.length === 1) {
        return { ok: true, json: async () => ({
          documents: [{ slug: "first" }, { slug: "duplicate" }],
          next_cursor: { after_updated: "2026-01-02T00:00:00Z", after_slug: "duplicate" },
        }) };
      }
      return { ok: true, json: async () => ({ documents: [{ slug: "duplicate" }, { slug: "last" }] }) };
    },
    get: async () => ({ comment_count: 0, files: [] }),
    say: (message) => problems.push(message),
  });
  vm.runInContext(showList, ctx);
  await vm.runInContext("showList()", ctx);
  assert.deepEqual(calls, [
    "/api/list",
    "/api/list?after_updated=2026-01-02T00%3A00%3A00Z&after_slug=duplicate",
  ]);
  assert.deepEqual(Array.from(ctx.documents, (doc) => doc.slug), ["first", "duplicate", "last"]);
  assert.deepEqual(problems, []);
}

// A later-page failure reports the refresh problem while leaving the prior
// complete listing intact.
{
  const problems = [];
  let calls = 0;
  const ctx = context({
    documents: [{ slug: "still-visible" }], counts: new Map(), paths: new Map(),
    SHELL_HEADERS: {},
    fetch: async () => {
      calls++;
      return calls === 1
        ? { ok: true, json: async () => ({ documents: [{ slug: "new" }], next_cursor: { after_updated: "t", after_slug: "new" } }) }
        : { ok: false, status: 503 };
    },
    get: async () => ({ comment_count: 0, files: [] }),
    say: (message) => problems.push(message),
  });
  vm.runInContext(showList, ctx);
  await vm.runInContext("showList()", ctx);
  assert.deepEqual(Array.from(ctx.documents, (doc) => doc.slug), ["still-visible"]);
  assert.match(problems[0], /refresh failed/);
}

// Offline, the same failure is silent: `showOfflineProjects` is what the page
// shows instead, and a network error there is expected rather than reportable.
{
  const problems = [];
  const ctx = context({
    documents: [{ slug: "still-visible" }], counts: new Map(), paths: new Map(),
    SHELL_HEADERS: {},
    navigator: { onLine: false },
    fetch: async () => { throw new TypeError("Failed to fetch"); },
    get: async () => ({ comment_count: 0, files: [] }),
    say: (message) => problems.push(message),
  });
  vm.runInContext(showList, ctx);
  assert.equal(await vm.runInContext("showList()", ctx), false);
  assert.deepEqual(problems, []);
  assert.deepEqual(Array.from(ctx.documents, (doc) => doc.slug), ["still-visible"]);
}

// Successful deletion updates only its own state. HTTP and network failures
// retain their selections, while a selection made during the requests
// survives too.
//
// A star is not touched here, in either direction. It is the account's now
// rather than this browser's, and a deleted project is recoverable for seven
// days: unstarring on the way into the trash would mean a project came back
// from it stripped of something nobody asked to change.
{
  const pending = new Map();
  const problems = [];
  const said = [];
  const ctx = context({
    pendingDeletion: ["good", "http-failure", "network-failure"],
    confirming: true,
    selected: new Set(["good", "http-failure", "network-failure"]),
    SHELL_HEADERS: {},
    fetch: (url) => {
      const slug = url.split("/").at(-2);
      const wait = new Promise((resolve, reject) => pending.set(slug, { resolve, reject }));
      return wait;
    },
    say: (message, options) => (options?.kind === "problem" ? problems : said).push(message),
    showList: async () => {},
  });
  vm.runInContext(reallyDelete, ctx);
  const deleting = vm.runInContext("reallyDelete()", ctx);
  await Promise.resolve();
  ctx.selected = new Set(["http-failure", "network-failure", "selected-while-waiting"]);
  pending.get("good").resolve({ ok: true });
  pending.get("http-failure").resolve({ ok: false, status: 500 });
  pending.get("network-failure").reject(new Error("offline"));
  await deleting;
  assert.deepEqual([...ctx.selected].sort(), ["http-failure", "network-failure", "selected-while-waiting"]);
  assert.match(problems[0], /2 projects could not be deleted/);
  // And the one that worked says where it went, because "deleted" and "in the
  // trash for a week" are different promises and only one of them is true.
  assert.match(said[0], /1 project moved to the trash/);
}

// A project is made from its name and its format alone, and the page leaves
// for it rather than returning to the list.
{
  let sent = null;
  const ctx = context({
    naming: true, name: "  A Paper You Can Change  ", format: "quarto", nameError: "", busy: false,
    nameInput: null,
    starterDocument, FORMATS,
    say: () => { throw new Error("a creation failure belongs in the dialog"); },
    upload: async (form) => { sent = form; return { ok: true, json: async () => ({ url: "/docs/a-paper-3f9" }) }; },
    location: { href: "/" },
    Blob, FormData,
  });
  vm.runInContext(create, ctx);
  await vm.runInContext("create({ preventDefault() {} })", ctx);
  assert.equal(ctx.location.href, "/docs/a-paper-3f9");
  assert.equal(sent.get("title"), "A Paper You Can Change", "the name is trimmed before it becomes a title");
  assert.equal(sent.get("file").name, "main.qmd", "the format names the main file");
  assert.match(await sent.get("file").text(), /title: "A Paper You Can Change"/);
}

// An unnamed project is refused here rather than at the server, and nothing
// is sent.
{
  let focused = false;
  const ctx = context({
    naming: true, name: "   ", format: "markdown", nameError: "", busy: false,
    nameInput: { focus: () => { focused = true; } },
    starterDocument, FORMATS,
    say: () => {},
    upload: async () => { throw new Error("an unnamed project must not be sent"); },
    location: { href: "/" }, Blob, FormData,
  });
  vm.runInContext(create, ctx);
  await vm.runInContext("create({ preventDefault() {} })", ctx);
  assert.equal(ctx.nameError, "Give the project a name.");
  assert.equal(focused, true, "the field that is wrong is the field the cursor lands in");
  assert.equal(ctx.location.href, "/", "a refused creation stays on the list");
}

// A refusal is reported in the dialog that asked -- where the name is still on
// screen -- rather than in a corner, and the dialog stays open.
{
  const ctx = context({
    naming: true, name: "Paper", format: "markdown", nameError: "", busy: false, nameInput: null,
    starterDocument, FORMATS,
    say: () => { throw new Error("a creation failure belongs in the dialog"); },
    upload: async () => ({ ok: false, json: async () => ({ error: "you may not publish here" }) }),
    location: { href: "/" }, Blob, FormData,
  });
  vm.runInContext(create, ctx);
  await vm.runInContext("create({ preventDefault() {} })", ctx);
  assert.equal(ctx.nameError, "you may not publish here");
  assert.equal(ctx.naming, true, "the dialog stays open so the name can be changed");
  assert.equal(ctx.busy, false);
}

console.log("landing: pagination, refresh preservation, silent offline refresh, project creation and deletion failure checks passed");
