// Repository sync, end to end: the browser's sync library (web/src/lib/forge)
// driving a real `librepaper admin serve` on one side and an in-memory GitHub
// (web/tests/helpers/fake-github.mjs) on the other. The "browser" is a Node
// project session fed by the room's frames, the way tracked-changes.mjs holds
// one; a live edit is a plain Loro edit on that session, which the session
// sends as an ordinary `doc-update`, exactly as typing does.
//
// Each scenario publishes its own document and seeds its own repository.
// Requires a built librepaper binary and LIBREPAPER_TEST_POSTGRES_URL.
import assert from "node:assert/strict";
import { after, test } from "node:test";
import { createProjectSession } from "../../src/lib/project-session.js";
import { gitHub } from "../../src/lib/forge/github.js";
import { connectLink, exportChanges, pull, push } from "../../src/lib/forge/sync.js";
import { startDeployment } from "../helpers/deployment.mjs";
import { startFakeGitHub } from "../helpers/fake-github.mjs";

const fake = await startFakeGitHub();
const deployment = await startDeployment({
  label: "forge_sync",
  advanced: [
    "[forge.github]",
    'app_slug = "librepaper-test"',
    'client_id = "test"',
    'client_secret = "test"',
    `api = ${JSON.stringify(fake.api)}`,
    `web = ${JSON.stringify(fake.web)}`,
    "",
  ].join("\n"),
});
if (!deployment || deployment.unavailable) {
  await fake.stop();
  console.log(`forge-sync: ${deployment?.unavailable || "no librepaper binary; run cargo build"}; skipping`);
  process.exit(0);
}
after(async () => {
  await deployment.stop();
  await fake.stop();
});

const base = deployment.base;
// sync.js builds the commit author's address from the page's host.
globalThis.location = new URL(base);
const config = await (await fetch(`${base}/api/config`)).json();

const wait = async (what, check, timeout = 15000) => {
  const until = Date.now() + timeout;
  for (;;) {
    const found = await check();
    if (found) return found;
    if (Date.now() > until) throw new Error(`timed out waiting for ${what}`);
    await new Promise((done) => setTimeout(done, 50));
  }
};
const pause = (ms) => new Promise((done) => setTimeout(done, ms));

/// A client the way the browser holds one: a socket with the session cookie
/// and a project session fed by the room's frames. `updates` counts the
/// document updates the server has pushed, and `state.pending` is the
/// session's own count of edits the server has not made durable yet.
async function join(slug) {
  const socket = new WebSocket(`${base.replace("http", "ws")}/ws/${slug}`, { headers: { cookie: deployment.cookie } });
  await new Promise((done, fail) => {
    socket.addEventListener("open", done, { once: true });
    socket.addEventListener("error", () => fail(new Error("the socket could not be opened")), { once: true });
  });
  const state = { pending: 0, updates: 0 };
  const session = createProjectSession({
    send: (message) => socket.send(JSON.stringify(message)),
    presenceId: () => "forge-test",
    name: "Tester",
    onState: (next) => { state.pending = next.pending; },
    fetchReference: async (reference) => {
      const response = await fetch(new URL(reference, base), { headers: { cookie: deployment.cookie, "x-librepaper-client": "1" } });
      if (!response.ok) throw new Error(`the document state could not be fetched: ${response.status}`);
      return new Uint8Array(await response.arrayBuffer());
    },
  });
  session.setRules(config);
  const failures = [];
  socket.addEventListener("message", (event) => {
    const frame = JSON.parse(String(event.data));
    const done = (promise) => promise?.catch?.((error) => failures.push(error));
    if (frame.type === "hello") socket.send(JSON.stringify(session.open()));
    else if (frame.type === "doc-state") done(session.start(frame));
    else if (frame.type === "doc-rows") done(session.rows(frame));
    else if (frame.type === "doc-update") { state.updates += 1; session.apply(frame.update); }
    else if (frame.type === "doc-gap") session.gap(frame.vector);
    else if (frame.type === "doc-ack") session.acknowledge(frame.upTo || 0);
    else if (frame.type === "doc-durable") session.durable(frame.vector);
  });
  await wait("the session to join", () => session.joined || failures[0] && Promise.reject(failures[0]));
  return { session, state, close: () => { session.leave(); socket.close(); } };
}

const textAt = (client, path) => {
  const file = client.session.list().find((item) => item.path === path);
  return file ? client.session.textOf(file.id).toString() : null;
};
const paths = (client) => client.session.list().map((file) => file.path);

/// A live edit in the browser: replaces a file's text and waits until the
/// server has the edit, so a pull on the server sees what a push reads here.
async function edit(client, path, text) {
  const file = client.session.list().find((item) => item.path === path);
  assert.ok(file, `${path} is in the project`);
  const body = client.session.textOf(file.id);
  body.delete(0, body.length);
  body.insert(0, text);
  client.session.doc.commit();
  await pause(100);
  await wait("the edit to be saved", () => client.state.pending === 0);
}

const memoryStore = () => {
  const held = new Map();
  return {
    pendingOperation: async (slug) => held.get(slug) ?? null,
    rememberOperation: async (slug, value) => { held.set(slug, value); },
    settleOperation: async (slug) => { held.delete(slug); },
  };
};

// The server half of sync with the test's cookie and absolute URLs. After a
// source change it waits for the server's update to reach the mirror, the way
// the browser's open socket has delivered it by the time the next step reads
// the project.
function apiFor(slug, client) {
  const root = `${base}/api/documents/${slug}`;
  const headers = (extra = {}) => ({ cookie: deployment.cookie, "x-librepaper-client": "shell", origin: base, ...extra });
  const parse = async (response) => {
    const body = await response.json().catch(() => null);
    if (!response.ok) {
      const error = new Error(body?.error || `The server answered ${response.status}.`);
      error.status = response.status;
      error.body = body || {};
      throw error;
    }
    return body;
  };
  const send = (method, path, body) =>
    fetch(`${root}${path}`, { method, headers: headers({ "content-type": "application/json" }), body: JSON.stringify(body) }).then(parse);
  return {
    getLink: () => fetch(`${root}/forge`, { headers: headers() }).then(parse),
    putLink: (body) => send("PUT", "/forge", body),
    async sourceChange(body) {
      const before = client.state.updates;
      const answer = await send("POST", "/source-change", body);
      const changes = [...(body.change.patches || []), ...(body.change.create || []), ...(body.change.delete || []), ...(body.change.put_binary || [])];
      if (changes.length) {
        await wait("the server's update to arrive", () => client.state.updates > before);
        await pause(100);
      }
      return answer;
    },
    uploadAsset: (bytes) => fetch(`${root}/assets`, { method: "PUT", headers: headers(), body: bytes }).then(parse),
    async assetBytes(sha) {
      const response = await fetch(`${root}/assets/${sha}`, { headers: headers() });
      if (!response.ok) await parse(response);
      return new Uint8Array(await response.arrayBuffer());
    },
  };
}

const PAPER = "# Paper\n\nHello world.\n";
const PAPER_FILES = {
  "paper.qmd": PAPER,
  "refs.bib": "@book{a, title={A}}\n",
  Makefile: "all:\n\ttrue\n",
  "renv/activate.R": "# renv\n",
  "data/a.csv": "x,y\n1,2\n",
};

let counter = 0;

/// A published document, a joined client, a seeded repository and a context
/// connected to it, with nothing pulled yet.
async function fixture(files, { connect = true } = {}) {
  counter += 1;
  const repo = `test/repo${counter}`;
  const head = fake.seed(repo, { files });
  const { slug } = await deployment.publish({ title: `Forge ${counter}`, source: "# Placeholder\n", source_format: "markdown" });
  const client = await join(slug);
  const ctxFor = () => ({
    slug,
    session: { doc: client.session.doc, list: () => client.session.list(), textOf: (id) => client.session.textOf(id) },
    rules: config,
    github: gitHub({ token: "fake-token", api: fake.api }),
    api: apiFor(slug, client),
    me: { name: "Tester", handle: "tester" },
    web: fake.web,
    store: memoryStore(),
  });
  const ctx = ctxFor();
  if (connect) {
    await connectLink(ctx, { repositoryId: fake.repo(repo).id, repository: repo, branch: "main", folder: "" });
  }
  return { repo, head, slug, client, ctx, ctxFor, link: async () => (await ctx.api.getLink()).link };
}

async function withFixture(files, action, options) {
  const fx = await fixture(files, options);
  try {
    await action(fx);
  } finally {
    fx.client.close();
  }
}

const conflictPaths = (result) => result.conflicts.map((item) => item.path ?? item);

test("connect then pull brings in project files and leaves the rest", () => withFixture(PAPER_FILES, async (fx) => {
  const result = await pull(fx.ctx);
  assert.equal(result.status, "pulled", JSON.stringify(result));
  await wait("the pulled files", () => ["paper.qmd", "refs.bib", "data/a.csv"].every((path) => paths(fx.client).includes(path)));
  assert.equal(textAt(fx.client, "paper.qmd"), PAPER);
  assert.ok(!paths(fx.client).includes("Makefile"), "Makefile has no accepted extension");
  assert.ok(!paths(fx.client).some((path) => path.startsWith("renv/")), "renv/activate.R is not a project file");
  const link = await fx.link();
  assert.equal(link.synced_commit, fx.head, "the link records the head it pulled");
  assert.ok(link.last_pull_at, "the pull is stamped");
}));

test("a conflict is named, an export branch carries LibrePaper's text, and the merge pulls clean", () => withFixture({ "paper.qmd": PAPER }, async (fx) => {
  await pull(fx.ctx);
  await wait("paper.qmd", () => textAt(fx.client, "paper.qmd") === PAPER);
  const mine = "# Paper\n\nHello LibrePaper.\n";
  await edit(fx.client, "paper.qmd", mine);
  fake.commit(fx.repo, "main", { files: { "paper.qmd": "# Paper\n\nHello GitHub.\n" } });

  const conflicted = await pull(fx.ctx);
  assert.equal(conflicted.status, "conflict", JSON.stringify(conflicted));
  assert.deepEqual(conflictPaths(conflicted), ["paper.qmd"]);
  assert.equal(textAt(fx.client, "paper.qmd"), mine, "a conflicting pull changes nothing");

  const exported = await exportChanges(fx.ctx);
  assert.equal(exported.status, "exported", JSON.stringify(exported));
  assert.match(exported.branch, /^librepaper\//);
  assert.equal(fake.headOf(fx.repo, exported.branch), exported.commit);
  assert.deepEqual(fake.repo(fx.repo).commitOf(exported.commit).parents, [fx.head], "the export sits on the synced commit");
  assert.equal(String(fake.read(fx.repo, exported.branch, "paper.qmd")), mine);
  assert.equal((await fx.link()).export?.branch, exported.branch);

  const resolved = "# Paper\n\nHello both.\n";
  fake.merge(fx.repo, { base: "main", head: exported.branch, files: { "paper.qmd": resolved } });
  const merged = await pull(fx.ctx);
  assert.equal(merged.status, "pulled", JSON.stringify(merged));
  await wait("the resolved text", () => textAt(fx.client, "paper.qmd") === resolved);
  assert.equal((await fx.link()).export, null, "the merged export is consumed");
}));

test("editing again after an export keeps the conflict alive", () => withFixture({ "paper.qmd": PAPER }, async (fx) => {
  await pull(fx.ctx);
  await wait("paper.qmd", () => textAt(fx.client, "paper.qmd") === PAPER);
  await edit(fx.client, "paper.qmd", "# Paper\n\nHello LibrePaper.\n");
  fake.commit(fx.repo, "main", { files: { "paper.qmd": "# Paper\n\nHello GitHub.\n" } });
  assert.equal((await pull(fx.ctx)).status, "conflict");
  const exported = await exportChanges(fx.ctx);
  assert.equal(exported.status, "exported");

  // More work in LibrePaper after the export, then the export is merged.
  const later = "# Paper\n\nHello again from LibrePaper.\n";
  await edit(fx.client, "paper.qmd", later);
  fake.merge(fx.repo, { base: "main", head: exported.branch, files: { "paper.qmd": "# Paper\n\nHello both.\n" } });
  const again = await pull(fx.ctx);
  assert.equal(again.status, "conflict", JSON.stringify(again));
  assert.deepEqual(conflictPaths(again), ["paper.qmd"]);
  assert.equal(textAt(fx.client, "paper.qmd"), later, "the newer edit is not overwritten");
}));

test("push, a commit on GitHub, then push again keeps both changes", () => withFixture({ "paper.qmd": PAPER, "refs.bib": "@book{a}\n" }, async (fx) => {
  await pull(fx.ctx);
  await wait("both files", () => paths(fx.client).includes("refs.bib") && textAt(fx.client, "paper.qmd") === PAPER);

  const first = "# Paper\n\nFirst edit.\n";
  await edit(fx.client, "paper.qmd", first);
  const pushed = await push(fx.ctx, "First edit");
  assert.equal(pushed.status, "pushed", JSON.stringify(pushed));
  const moved = fake.headOf(fx.repo, "main");
  assert.notEqual(moved, fx.head);
  assert.equal(moved, pushed.commit);
  assert.deepEqual(fake.repo(fx.repo).commitOf(moved).parents, [fx.head]);
  assert.equal(String(fake.read(fx.repo, "main", "paper.qmd")), first);

  fake.commit(fx.repo, "main", { files: { "refs.bib": "@book{b}\n" } });
  const second = "# Paper\n\nSecond edit.\n";
  await edit(fx.client, "paper.qmd", second);
  const again = await push(fx.ctx, "Second edit");
  assert.equal(again.status, "pushed", JSON.stringify(again));
  assert.equal(String(fake.read(fx.repo, "main", "paper.qmd")), second);
  assert.equal(String(fake.read(fx.repo, "main", "refs.bib")), "@book{b}\n", "the commit made on GitHub survives the push");
  assert.equal((await fx.link()).synced_commit, fake.headOf(fx.repo, "main"));
}));

test("files outside the project survive a push and an ignore file", () => withFixture(PAPER_FILES, async (fx) => {
  await pull(fx.ctx);
  await wait("paper.qmd", () => textAt(fx.client, "paper.qmd") === PAPER);
  await edit(fx.client, "paper.qmd", "# Paper\n\nEdited.\n");
  assert.equal((await push(fx.ctx)).status, "pushed");
  assert.equal(String(fake.read(fx.repo, "main", "Makefile")), PAPER_FILES.Makefile);
  assert.equal(String(fake.read(fx.repo, "main", "renv/activate.R")), PAPER_FILES["renv/activate.R"]);
  assert.ok(!paths(fx.client).includes("Makefile") && !paths(fx.client).some((path) => path.startsWith("renv/")));

  fake.commit(fx.repo, "main", { files: { ".librepaperignore": "data/\n" } });
  const ignored = await pull(fx.ctx);
  assert.ok(["pulled", "nothing"].includes(ignored.status), JSON.stringify(ignored));
  await edit(fx.client, "paper.qmd", "# Paper\n\nEdited twice.\n");
  assert.equal((await push(fx.ctx)).status, "pushed");
  assert.ok(fake.list(fx.repo, "main").includes("data/a.csv"), "the ignored file stays in the repository");
  assert.equal(String(fake.read(fx.repo, "main", "data/a.csv")), PAPER_FILES["data/a.csv"]);
  assert.equal(String(fake.read(fx.repo, "main", "Makefile")), PAPER_FILES.Makefile);
}));

// Anchoring a comment on a passage needs the editor's anchor records in the
// document, which only the browser's comment machinery writes. The HTTP route
// takes a place the page computed, so there is no honest way to make one here.
test("a comment anchor on text the pull leaves alone is kept", { skip: "creating an anchored comment needs the browser's anchor machinery" }, () => {});

test("two pulls at once apply the change once", () => withFixture({ "paper.qmd": PAPER, "refs.bib": "@book{a}\n" }, async (fx) => {
  await pull(fx.ctx);
  await wait("both files", () => paths(fx.client).includes("refs.bib"));
  const updated = "@book{a}\n@book{c}\n";
  fake.commit(fx.repo, "main", { files: { "refs.bib": updated } });

  const other = fx.ctxFor();
  const settled = await Promise.allSettled([pull(fx.ctx), pull(other)]);
  for (const outcome of settled) assert.equal(outcome.status, "fulfilled", String(outcome.reason?.stack || outcome.reason));
  const statuses = settled.map((outcome) => outcome.value.status);
  assert.ok(statuses.every((status) => ["pulled", "nothing"].includes(status)), statuses.join(","));
  assert.ok(statuses.includes("pulled"), `one of them applied the change: ${statuses}`);
  await wait("the new bibliography", () => textAt(fx.client, "refs.bib") === updated);
  await pause(300);
  assert.equal(textAt(fx.client, "refs.bib"), updated, "applied once, not twice");
  assert.equal((await fx.link()).synced_commit, fake.headOf(fx.repo, "main"));
}));

test("a push whose record was never written is adopted by the next pull", () => withFixture({ "paper.qmd": PAPER }, async (fx) => {
  await pull(fx.ctx);
  await wait("paper.qmd", () => textAt(fx.client, "paper.qmd") === PAPER);
  const mine = "# Paper\n\nPushed by a tab that died.\n";
  await edit(fx.client, "paper.qmd", mine);

  // The GitHub half of a push, by hand, and no putLink: the tab died between
  // moving the branch and recording it.
  const { github } = fx.ctx;
  const synced = (await fx.link()).synced_commit;
  const { tree } = await github.treeOf(fx.repo, synced);
  const blob = await github.createBlob(fx.repo, new TextEncoder().encode(mine));
  const next = await github.createTree(fx.repo, tree, [{ path: "paper.qmd", mode: "100644", type: "blob", sha: blob }]);
  const commit = await github.createCommit(fx.repo, {
    message: "Dead tab", tree: next, parents: [synced], author: { name: "Tester", email: "tester@example.invalid" },
  });
  await github.moveBranch(fx.repo, "main", commit);
  assert.equal((await fx.link()).synced_commit, synced, "the server never heard of the push");

  const result = await pull(fx.ctx);
  assert.equal(result.status, "pulled", JSON.stringify(result));
  assert.equal(result.files, 0, "nothing in the project changes");
  assert.equal((await fx.link()).synced_commit, commit);
  assert.equal(textAt(fx.client, "paper.qmd"), mine);
}));

test("names that differ only by case are refused", () => withFixture({ "A.qmd": "# A\n", "a.qmd": "# a\n" }, async (fx) => {
  const result = await pull(fx.ctx);
  assert.equal(result.status, "refused", JSON.stringify(result));
  const named = JSON.stringify(result.refusals);
  assert.ok(named.includes("A.qmd") && named.includes("a.qmd"), `both names are listed: ${named}`);
  assert.ok(!paths(fx.client).includes("A.qmd") && !paths(fx.client).includes("a.qmd"));
}));

test("a truncated listing is refused", () => withFixture({ "paper.qmd": PAPER }, async (fx) => {
  fake.truncateNextTree();
  const result = await pull(fx.ctx);
  assert.equal(result.status, "refused", JSON.stringify(result));
  assert.ok(JSON.stringify(result.refusals).includes("the repository is too large to compare"), JSON.stringify(result.refusals));
  assert.ok(!paths(fx.client).includes("paper.qmd"));
}));
