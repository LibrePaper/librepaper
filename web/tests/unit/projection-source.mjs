// The reader's projection source: fetch, etag, the deferred refresh, and the
// tree a reader without a session is given. The fetcher and the timers are
// scripted here, so each step lands exactly when the check says it does.
import assert from "node:assert/strict";
import { m } from "../../src/lib/paraglide/messages.js";
import { loadRunes } from "../helpers/runes.mjs";

const { createProjectionSource } = await loadRunes(new URL("../../src/lib/reader/projection-source.svelte.js", import.meta.url));

const project = (digest, extra = {}) => ({ project_digest: digest, texts: {}, changes: [], tree: { files: {}, settings: null }, ...extra });

function response({ status = 200, body = {}, etag = null } = {}) {
  return {
    status,
    ok: status >= 200 && status < 300,
    headers: { get: (name) => (name.toLowerCase() === "etag" ? etag : null) },
    json: async () => body,
  };
}

const flush = () => new Promise((resolve) => setImmediate(resolve));

// `replies` is a queue the test fills before each request; `timers` records
// every timer the source starts, and the test fires them by hand.
function harness(options = {}) {
  const requests = [];
  const replies = [];
  const timers = [];
  const seen = { projects: [], problems: [] };
  const source = createProjectionSource({
    slug: "doc",
    key: "k",
    fetcher: (url, init) => {
      requests.push({ url, headers: init.headers });
      return replies.shift();
    },
    setTimer: (fn, ms) => {
      const timer = { fn, ms, cleared: false, fired: false };
      timers.push(timer);
      return timer;
    },
    clearTimer: (timer) => { if (timer) timer.cleared = true; },
    onProject: (next) => seen.projects.push(next),
    onProblem: (message) => seen.problems.push(message),
    ...options,
  });
  const live = () => timers.filter((timer) => !timer.cleared && !timer.fired);
  const fire = () => {
    const [timer] = live();
    assert.ok(timer, "expected a live timer to fire");
    timer.fired = true;
    timer.fn();
  };
  return { source, requests, replies, timers, seen, live, fire };
}

// 1. The first refresh stores the projection and reports it once.
{
  const h = harness();
  h.replies.push(Promise.resolve(response({ body: project("v1"), etag: '"e1"' })));
  await h.source.refresh();
  assert.equal(h.requests[0].url, "/api/documents/doc/project");
  assert.equal(h.requests[0].headers["If-None-Match"], undefined);
  assert.equal(h.source.state.project.project_digest, "v1");
  assert.equal(h.source.state.unreadable, "");
  assert.equal(h.seen.projects.length, 1);
}

// 2. The next refresh sends the etag back; without an etag header the quoted
// project digest stands in for it.
{
  const h = harness();
  h.replies.push(Promise.resolve(response({ body: project("v1"), etag: '"e1"' })));
  await h.source.refresh();
  h.replies.push(Promise.resolve(response({ body: project("v2") })));
  await h.source.refresh();
  assert.equal(h.requests[1].headers["If-None-Match"], '"e1"');
  assert.equal(h.source.state.project.project_digest, "v2");
  h.replies.push(Promise.resolve(response({ body: project("v3") })));
  await h.source.refresh();
  assert.equal(h.requests[2].headers["If-None-Match"], '"v2"');
}

// 3. A 304 changes nothing and reports nothing.
{
  const h = harness();
  h.replies.push(Promise.resolve(response({ body: project("v1"), etag: '"e1"' })));
  await h.source.refresh();
  const before = h.source.state.project;
  h.replies.push(Promise.resolve(response({ status: 304 })));
  await h.source.refresh();
  assert.equal(h.source.state.project, before);
  assert.equal(h.seen.projects.length, 1);
  assert.deepEqual(h.seen.problems, []);
}

// 4. A failure before any projection sets the unreadable message; a failure
// after one leaves the held projection alone. Both are reported.
{
  const h = harness();
  h.replies.push(Promise.resolve(response({ status: 500, body: { error: "boom" } })));
  await h.source.refresh();
  assert.equal(h.source.state.project, null);
  assert.equal(h.source.state.unreadable, "boom");
  assert.deepEqual(h.seen.problems, ["boom"]);
}
{
  const h = harness();
  h.replies.push(Promise.resolve(response({ body: project("v1") })));
  await h.source.refresh();
  h.replies.push(Promise.resolve(response({ status: 500, body: { error: "boom" } })));
  await h.source.refresh();
  assert.equal(h.source.state.project.project_digest, "v1");
  assert.equal(h.source.state.unreadable, "");
  assert.deepEqual(h.seen.problems, ["boom"]);
}

// 5. A 503 says what the body says.
{
  const h = harness();
  h.replies.push(Promise.resolve(response({ status: 503, body: { error: "over the limit" } })));
  await h.source.refresh();
  assert.equal(h.source.state.unreadable, "over the limit");
  assert.deepEqual(h.seen.problems, ["over the limit"]);
}

// 6. A refresh asked for while one is in flight shares it; the follow-up is
// scheduled when the first lands, and firing that timer fetches again.
{
  const h = harness();
  let land;
  h.replies.push(new Promise((resolve) => { land = resolve; }));
  const first = h.source.refresh();
  const second = h.source.refresh();
  assert.equal(first, second);
  assert.equal(h.requests.length, 1);
  land(response({ body: project("v1") }));
  await first;
  assert.equal(h.live().length, 1);
  h.replies.push(Promise.resolve(response({ body: project("v2") })));
  h.fire();
  assert.equal(h.requests.length, 2);
  await flush();
  assert.equal(h.source.state.project.project_digest, "v2");
  assert.equal(h.seen.projects.length, 2);
}

// 7. A refresh scheduled while hidden starts no timer; resume starts one once
// the tab is visible.
{
  let hidden = true;
  const h = harness({ hidden: () => hidden });
  h.source.schedule();
  assert.equal(h.timers.length, 0);
  hidden = false;
  h.source.resume();
  assert.equal(h.live().length, 1);
  assert.equal(h.timers[0].ms, 1000);
}

// 8. An editor (active() false) fetches nothing, directly or from a timer.
{
  let active = true;
  const h = harness({ active: () => active });
  active = false;
  await h.source.refresh();
  assert.equal(h.requests.length, 0);
  active = true;
  h.source.schedule();
  active = false;
  h.fire();
  await flush();
  assert.equal(h.requests.length, 0);
}

// 9. A response that lands after dispose() is dropped, and dispose clears the
// timer that was pending.
{
  const h = harness();
  let land;
  h.replies.push(new Promise((resolve) => { land = resolve; }));
  const pending = h.source.refresh();
  h.source.dispose();
  land(response({ body: project("v1") }));
  await pending;
  assert.equal(h.source.state.project, null);
  assert.equal(h.source.state.unreadable, "");
  assert.equal(h.seen.projects.length, 0);
  assert.deepEqual(h.seen.problems, []);
}
{
  const h = harness();
  h.source.schedule();
  h.source.dispose();
  assert.equal(h.timers[0].cleared, true);
}

// 10. The tree a reader with no session is given.
{
  const h = harness();
  assert.deepEqual(h.source.tree(), { main: "", texts: {}, digests: {} });
  h.replies.push(Promise.resolve(response({
    body: project("v1", {
      texts: { "main.tex": "hello" },
      tree: {
        files: {
          "main.tex": { kind: "text", id: "t1" },
          "fig.png": { kind: "asset", digest: "d1" },
          "empty.png": { kind: "asset" },
        },
        settings: { x: 1 },
      },
    }),
  })));
  await h.source.refresh();
  assert.deepEqual(h.source.tree(), {
    main: "",
    texts: { "main.tex": "hello" },
    retained: {},
    digests: { "fig.png": "d1" },
    settings: { x: 1 },
  });
  h.replies.push(Promise.resolve(response({
    body: project("v2", {
      texts: { "main.tex": "hello" },
      retained: { "analysis.R": "x <- 1" },
      tree: { files: { "main.tex": { kind: "text", id: "t1" } } },
    }),
  })));
  await h.source.refresh();
  assert.deepEqual(h.source.tree().retained, { "analysis.R": "x <- 1" });
}

// A 503 with no body text falls back to the generic load failure, since the
// error always carries a message by the time it reaches the catch.
{
  const h = harness();
  h.replies.push(Promise.resolve(response({ status: 503 })));
  await h.source.refresh();
  assert.equal(h.source.state.unreadable, m.readerc_project_load_failed());
}
