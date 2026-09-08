// Behavioral checks for the landing page's paginated refresh and deletion
// reconciliation. The VM harness runs the actual function bodies from the
// shipped Svelte component, with only their browser collaborators supplied.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const landing = readFileSync(new URL("../src/components/Landing.svelte", import.meta.url), "utf8");
const body = (start, end) => {
  const from = landing.indexOf(start);
  assert.notEqual(from, -1, `Landing function not found: ${start}`);
  const to = landing.indexOf(end, from);
  assert.notEqual(to, -1, `Landing function boundary not found: ${end}`);
  return landing.slice(from, to);
};

const showList = body("  async function showList()", "  async function deleteSelected");
const reallyDelete = body("  async function reallyDelete()", "  /* ------------------------------------------------------------ file picking */");

const context = (values) => vm.createContext({
  Promise,
  Set,
  Map,
  URLSearchParams,
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
    problem: (message) => problems.push(message),
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
    problem: (message) => problems.push(message),
  });
  vm.runInContext(showList, ctx);
  await vm.runInContext("showList()", ctx);
  assert.deepEqual(Array.from(ctx.documents, (doc) => doc.slug), ["still-visible"]);
  assert.match(problems[0], /refresh failed/);
}

// Successful deletion updates only its own state. HTTP and network failures
// retain their selections and favorites, while a selection made during the
// requests survives too.
{
  const pending = new Map();
  const problems = [];
  const ctx = context({
    pendingDeletion: ["good", "http-failure", "network-failure"],
    confirming: true,
    favorites: new Set(["good", "http-failure", "network-failure"]),
    selected: new Set(["good", "http-failure", "network-failure"]),
    FAVORITES: "favorites",
    SHELL_HEADERS: {},
    fetch: (url) => {
      const slug = url.split("/").at(-2);
      const wait = new Promise((resolve, reject) => pending.set(slug, { resolve, reject }));
      return wait;
    },
    write: () => {},
    problem: (message) => problems.push(message),
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
  assert.deepEqual([...ctx.favorites].sort(), ["http-failure", "network-failure"]);
  assert.deepEqual([...ctx.selected].sort(), ["http-failure", "network-failure", "selected-while-waiting"]);
  assert.match(problems[0], /Could not delete 2 projects/);
}

console.log("landing: pagination, refresh preservation, and deletion failure checks passed");
