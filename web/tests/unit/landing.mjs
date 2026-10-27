// Behavioral checks for the landing page's paginated refresh and deletion
// reconciliation. The VM harness runs the actual function bodies from the
// shipped Svelte component, with only their browser collaborators supplied.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

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
const requestPaths = body("  const pathRequests = new Map();", "  async function learnPaths()");
const tagging = body("  const tagQueues = new Map();", "  // The two things the rail's tag list offers beneath the tags.");
const tagEditing = body("  const tagEdits = new Map();", "  function renameTag(");

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
  // The New project page is closed and the place is the project list unless a case says otherwise.
  newTab: "",
  place: "projects",
  // Every listing page carries the account's tags, which the refresh keeps
  // in the order the rail shows them. No case here is about tags.
  tags: [],
  sortTags: (list) => list,
  // A loaded listing warms the document page. No case here is about that.
  warmReader: () => {},
  readDefaultPreview: () => "html",
  location: { origin: "https://example.test" },
  me: {},
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

// Offline, the same failure is silent: `showLocalProjects` is what the page
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

// The file search asks each project for its paths with `files=1`. Only an
// editor's detail response carries that list, so the request has to ask for it.
{
  const urls = [];
  const ctx = context({
    documents: [], counts: new Map(), paths: new Map(),
    SHELL_HEADERS: {},
    get: async (url) => {
      urls.push(url);
      return { comment_count: 0, files: ["paper.md"] };
    },
    say: () => {},
  });
  vm.runInContext(requestPaths, ctx);
  assert.deepEqual(await vm.runInContext('requestPaths({ slug: "alpha" })', ctx), ["paper.md"]);
  assert.equal(urls.length, 1);
  assert.ok(urls.every((url) => url.endsWith("?files=1")), urls.join(", "));
}

// Tag requests for one project and tag go out one at a time, and once nothing
// is in flight the row shows what the server last confirmed.
{
  const deferred = () => { let resolve; const promise = new Promise((done) => { resolve = done; }); return { promise, resolve }; };
  const tick = () => new Promise((done) => setTimeout(done, 0));
  const harness = () => {
    const calls = [];
    const problems = [];
    const ctx = context({
      SHELL_HEADERS: {},
      hasTag: (doc, id) => (doc.tags || []).includes(id),
      fetch: (url, options) => {
        const answer = deferred();
        calls.push({ url, method: options.method, answer });
        return answer.promise;
      },
      say: (message) => problems.push(message),
    });
    vm.runInContext(tagging, ctx);
    return { ctx, calls, problems };
  };

  // Add, then remove before the add is answered. The removal waits for the
  // add, so it fails after the add succeeded and the row keeps the saved tag.
  {
    const { ctx, calls } = harness();
    ctx.doc = { slug: "p", tags: [] };
    const adding = vm.runInContext('setTag(doc, "t", true)', ctx);
    const removing = vm.runInContext('setTag(doc, "t", false)', ctx);
    assert.deepEqual([...ctx.doc.tags], [], "the row shows the latest choice at once");
    await tick();
    assert.equal(calls.length, 1, "the removal waits for the add");
    calls[0].answer.resolve({ ok: true });
    await tick();
    assert.equal(calls.length, 2);
    assert.equal(calls[1].method, "DELETE");
    calls[1].answer.resolve({ ok: false, status: 500 });
    await Promise.all([adding, removing]);
    assert.deepEqual([...ctx.doc.tags], ["t"], "a failed removal leaves the tag the server has");
  }

  // Add and remove both fail: the row ends as the server holds it, untagged.
  {
    const { ctx, calls, problems } = harness();
    ctx.doc = { slug: "p", tags: [] };
    const adding = vm.runInContext('setTag(doc, "t", true)', ctx);
    const removing = vm.runInContext('setTag(doc, "t", false)', ctx);
    await tick();
    calls[0].answer.resolve({ ok: false, status: 500 });
    await tick();
    calls[1].answer.resolve({ ok: false, status: 500 });
    await Promise.all([adding, removing]);
    assert.deepEqual([...ctx.doc.tags], [], "two failures never show a tag the server does not have");
    assert.equal(problems.length, 2);
  }
}

// Edits to one tag go out one at a time. Picking vermillion and then blue
// again on a blue tag is two saves, and the tag ends blue.
{
  const tick = () => new Promise((done) => setTimeout(done, 0));
  const calls = [];
  const ctx = context({
    SHELL_HEADERS: {},
    tags: [{ id: "t", name: "Thesis", colour: "blue", builtin: false }],
    sortTags: (list) => list,
    fetch: (url, options) => new Promise((resolve) => {
      calls.push({ body: JSON.parse(options.body), resolve });
    }),
    say: () => {},
  });
  vm.runInContext(tagEditing, ctx);
  vm.runInContext('editTag("t", "colour", "vermillion", "failed", "x")', ctx);
  vm.runInContext('editTag("t", "colour", "blue", "failed", "x")', ctx);
  assert.equal(ctx.tags[0].colour, "blue", "the last pick shows at once");
  await tick();
  assert.equal(calls.length, 1, "the second save waits for the first");
  calls[0].resolve({ ok: true, json: async () => ({ tag: { id: "t", name: "Thesis", colour: "vermillion", builtin: false } }) });
  await tick(); await tick();
  assert.equal(calls.length, 2);
  assert.deepEqual(calls[1].body, { colour: "blue" });
  calls[1].resolve({ ok: true, json: async () => ({ tag: { id: "t", name: "Thesis", colour: "blue", builtin: false } }) });
  await tick(); await tick();
  assert.equal(ctx.tags[0].colour, "blue");
}

console.log("landing: pagination, refresh preservation, silent offline refresh, deletion failure, tag ordering checks passed");
