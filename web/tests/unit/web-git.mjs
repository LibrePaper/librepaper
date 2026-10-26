// The web project's Git panel state, over a stand-in for the server: the last
// push found in the history, and a history that cannot be read.
import assert from "node:assert/strict";
import { loadRunes } from "../helpers/runes.mjs";

const { createWebGit } = await loadRunes(new URL("../../src/lib/reader/web-git.svelte.js", import.meta.url));

const point = (sha, why, extra = {}) => ({ sha, seq: 1, at: "2026-10-25T10:00:00Z", by: "", label: "", why, ...extra });

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
  };
  return { api, calls };
}

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
  assert.deepEqual(calls, [["history", "paper", "k1", null]]);
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

// A history that cannot be read is said, and there is no push.
{
  const { api } = server({ historyFails: true, pages: [] });
  const web = createWebGit({ api });
  await web.refresh("paper");
  assert.equal(web.state.loaded, true);
  assert.match(web.state.pushProblem, /not readable/);
  assert.equal(web.state.push, null);
}

// An answer that arrives after a newer request was made is dropped.
{
  const gates = [];
  const api = {
    history: () => new Promise((resolve) => gates.push(resolve)),
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

console.log("web-git: the last push of a web project, and a history that cannot be read");
