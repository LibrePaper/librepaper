// The web project's Git panel state, over a stand-in for the server: the last
// push found in the history, the Edit tokens an owner sees, the token made for
// a repository and shown once, and revoking one.
import assert from "node:assert/strict";
import { loadRunes } from "../helpers/runes.mjs";

const { createWebGit, ACTION_TOKEN } = await loadRunes(new URL("../../src/lib/reader/web-git.svelte.js", import.meta.url));

const point = (sha, why, extra = {}) => ({ sha, seq: 1, at: "2026-10-25T10:00:00Z", by: "", label: "", why, ...extra });
const idle = () => new Promise((resolve) => setImmediate(resolve));

// A stand-in for the server. `pages` are the history pages newest first.
function server(scene) {
  const calls = [];
  const api = {
    history: async (slug, key, after) => {
      calls.push(["history", slug, key, after]);
      if (scene.historyFails) throw new Error("this document's history is not readable");
      const page = scene.pages[scene.pages.findIndex((entry) => entry.cursor === after)];
      return { labels: page.labels, next: page.next };
    },
    sharing: async (slug) => {
      calls.push(["sharing", slug]);
      if (scene.sharingFails) throw new Error("Could not load sharing settings.");
      return structuredClone(scene.sharing);
    },
    change: async (slug, body) => {
      calls.push(["change", slug, body]);
      if (scene.changeFails) throw new Error("a token needs a name");
      return scene.change(body);
    },
  };
  return { api, calls };
}

const sharing = (tokens = []) => ({ links: [], tokens, can_share: true });
const edit = (id, name = "GitHub Action") => ({ id, role: "editor", name, until: "2027-10-25T00:00:00Z", since: "2026-10-25T00:00:00Z" });

// The newest version that a push made is the last push; its author and label come with it.
{
  const { api, calls } = server({
    pages: [{ cursor: null, next: null, labels: [
      point("c", "cli", { label: "Saved by the CLI" }),
      point("b", "push", { by: "Alice", label: "Reword the greeting", at: "2026-10-24T09:00:00Z" }),
      point("a", "push", { by: "Bob", label: "Older", at: "2026-10-01T09:00:00Z" }),
    ] }],
  });
  const web = createWebGit({ api });
  assert.equal(web.state.loaded, false);
  await web.refresh("paper", { key: "k1" });
  assert.equal(web.state.loaded, true);
  assert.deepEqual(web.state.push, { by: "Alice", at: "2026-10-24T09:00:00Z", label: "Reword the greeting" });
  assert.equal(web.state.pushProblem, "");
  assert.deepEqual(calls, [["history", "paper", "k1", null]], "an editor who is not the owner reads the history and nothing else");
  assert.deepEqual(web.state.tokens, []);
}

// A push by a machine or an unnamed id names nobody.
{
  const { api } = server({ pages: [{ cursor: null, next: null, labels: [point("a", "push", { by: "system", label: "Sync" })] }] });
  const web = createWebGit({ api });
  await web.refresh("paper");
  assert.equal(web.state.push.by, "");
  const ids = server({ pages: [{ cursor: null, next: null, labels: [point("a", "push", { by: "0b1f1c0e-7f64-4d6a-9d2e-5b0c6c5a4e11" })] }] });
  const other = createWebGit({ api: ids.api });
  await other.refresh("paper");
  assert.equal(other.state.push.by, "");
}

// A push further back is found by following the pages, up to a bound; none at all is no push.
{
  const { api, calls } = server({
    pages: [
      { cursor: null, next: 50, labels: [point("e", "cli")] },
      { cursor: 50, next: 20, labels: [point("d", "comment")] },
      { cursor: 20, next: null, labels: [point("c", "push", { by: "Alice", label: "First" })] },
    ],
  });
  const web = createWebGit({ api });
  await web.refresh("paper");
  assert.equal(web.state.push.label, "First");
  assert.deepEqual(calls.map((call) => call[3]), [null, 50, 20]);

  const none = server({ pages: [{ cursor: null, next: null, labels: [point("e", "cli"), point("f", "created")] }] });
  const quiet = createWebGit({ api: none.api });
  await quiet.refresh("paper");
  assert.equal(quiet.state.push, null);
  assert.equal(quiet.state.pushProblem, "");

  // A project with a long history of other saves stops looking after a few pages.
  const long = { pages: Array.from({ length: 10 }, (_, index) => ({ cursor: index === 0 ? null : 100 - index, next: 99 - index, labels: [point(`x${index}`, "cli")] })) };
  const deep = server(long);
  const bounded = createWebGit({ api: deep.api });
  await bounded.refresh("paper");
  assert.equal(deep.calls.length, 5, "five pages, and then it says there is no push");
  assert.equal(bounded.state.push, null);
}

// The owner sees the Edit tokens, and only those.
{
  const { api, calls } = server({
    pages: [{ cursor: null, next: null, labels: [] }],
    sharing: sharing([edit("t1"), { id: "t2", role: "reader", name: "Reader bot" }, { id: "t3", role: "commenter", name: "Commenter bot" }, edit("t4", "Other")]),
  });
  const web = createWebGit({ api });
  await web.refresh("paper", { owner: true });
  assert.deepEqual(web.state.tokens.map((token) => token.id), ["t1", "t4"]);
  assert.deepEqual(calls.map((call) => call[0]).sort(), ["history", "sharing"]);
}

// One of the two reads failing does not take the other down.
{
  const { api } = server({ historyFails: true, sharing: sharing([edit("t1")]), pages: [] });
  const web = createWebGit({ api });
  await web.refresh("paper", { owner: true });
  assert.equal(web.state.loaded, true);
  assert.match(web.state.pushProblem, /not readable/);
  assert.equal(web.state.push, null);
  assert.deepEqual(web.state.tokens.map((token) => token.id), ["t1"]);

  const other = server({ sharingFails: true, pages: [{ cursor: null, next: null, labels: [point("a", "push", { by: "Alice" })] }] });
  const mine = createWebGit({ api: other.api });
  await mine.refresh("paper", { owner: true });
  assert.equal(mine.state.push.by, "Alice");
  assert.match(mine.state.tokensProblem, /Could not load/);
  assert.deepEqual(mine.state.tokens, []);
}

// An answer that arrives after a newer request was made is dropped.
{
  const gates = [];
  const api = {
    history: () => new Promise((resolve) => gates.push(resolve)),
    sharing: async () => sharing([]),
    change: async () => ({}),
  };
  const web = createWebGit({ api });
  const first = web.refresh("paper");
  const second = web.refresh("paper");
  gates[1]({ labels: [point("new", "push", { by: "Alice", label: "New" })], next: null });
  await second;
  gates[0]({ labels: [point("old", "push", { by: "Bob", label: "Old" })], next: null });
  await first;
  assert.equal(web.state.push.label, "New");
}

// Creating a token posts the call the panel promises, keeps the token to show once, and lists the new row.
{
  assert.deepEqual(ACTION_TOKEN, { kind: "token", role: "edit", name: "GitHub Action", until: "365d" });
  const rows = [];
  const { api, calls } = server({
    pages: [{ cursor: null, next: null, labels: [] }],
    sharing: sharing([]),
    change: (body) => {
      if (body.create) {
        rows.push(edit("t9"));
        return { ...sharing(rows), created: { id: "t9", kind: "token", token: "lpt_secret" } };
      }
      rows.splice(rows.findIndex((row) => row.id === body.revoke.id), 1);
      return sharing(rows);
    },
  });
  const web = createWebGit({ api });
  await web.refresh("paper", { owner: true });
  assert.deepEqual(web.state.tokens, []);
  await web.create();
  assert.deepEqual(calls.find((call) => call[0] === "change"), ["change", "paper", { create: { kind: "token", role: "edit", name: "GitHub Action", until: "365d" } }]);
  assert.deepEqual(web.state.reveal, { token: "lpt_secret" });
  assert.deepEqual(web.state.tokens.map((token) => token.id), ["t9"]);
  assert.equal(web.state.busy, "");
  assert.equal(web.state.error, "");

  // Done forgets it; nothing gives it back, not even a read.
  web.hide();
  assert.equal(web.state.reveal, null);
  await web.refresh("paper", { owner: true });
  assert.equal(web.state.reveal, null);
  assert.ok(!JSON.stringify(web.state).includes("lpt_secret"), "the secret is in no other part of the state");

  // Revoking sends the id and lists what is left.
  await web.revoke("t9");
  assert.deepEqual(calls.filter((call) => call[0] === "change")[1], ["change", "paper", { revoke: { id: "t9" } }]);
  assert.deepEqual(web.state.tokens, []);
}

// A failed create says why and shows no token.
{
  const { api } = server({ pages: [{ cursor: null, next: null, labels: [] }], sharing: sharing([]), changeFails: true });
  const web = createWebGit({ api });
  await web.refresh("paper", { owner: true });
  await web.create();
  assert.equal(web.state.error, "a token needs a name");
  assert.equal(web.state.reveal, null);
  assert.equal(web.state.busy, "");
}

// Someone who is not the owner cannot make or revoke a token, whatever the panel is asked.
{
  const { api, calls } = server({ pages: [{ cursor: null, next: null, labels: [] }], sharing: sharing([]), change: () => ({}) });
  const web = createWebGit({ api });
  await web.refresh("paper", { owner: false });
  await web.create();
  await web.revoke("t1");
  assert.equal(calls.some((call) => call[0] === "change" || call[0] === "sharing"), false);
}

// One change at a time, and none after the panel is gone.
{
  const gates = [];
  const calls = [];
  const api = {
    history: async () => ({ labels: [], next: null }),
    sharing: async () => sharing([]),
    change: (slug, body) => { calls.push(body); return new Promise((resolve) => gates.push(resolve)); },
  };
  const web = createWebGit({ api });
  await web.refresh("paper", { owner: true });
  const first = web.create();
  await idle();
  assert.equal(web.state.busy, "create");
  await web.create();
  await web.revoke("t1");
  assert.equal(calls.length, 1);
  web.dispose();
  gates.shift()({ ...sharing([]), created: { token: "lpt_late" } });
  await first;
  assert.equal(web.state.reveal, null, "a token that arrives after the panel is gone is not kept");
}

console.log("web-git: the last push, the Edit tokens, creating one shown once, and revoking it");
