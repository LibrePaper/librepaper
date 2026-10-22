// Git sync, end to end: the page's sync library (web/src/lib/gitsync) driving a
// real `librepaper admin serve` on one side and a real detached local service
// (`librepaper local`, isolated state) on the other, which runs the system
// `git` against real bare repositories. A second ordinary clone, driven with
// plain `git`, plays the other side of each scenario. The "browser" is a Node
// project session fed by the room's frames, the way tracked-changes.mjs holds
// one; a live edit is a plain Loro edit on that session, which the session
// sends as an ordinary `doc-update`, exactly as typing does.
//
// Local remotes need a grant, which the app gets from a native folder chooser.
// The chooser cannot run headless, so the test writes the same grant record
// the chooser creates into the service's isolated state; the service reads it
// again on every request.
//
// Each scenario publishes its own document and seeds its own repository.
// Requires a built librepaper binary, git 2.38 or newer and
// LIBREPAPER_TEST_POSTGRES_URL.
import assert from "node:assert/strict";
import { execFile, execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { after, test } from "node:test";
import { promisify } from "node:util";
import { createProjectSession } from "../../src/lib/project-session.js";
import { createBridge } from "../../src/lib/gitsync/bridge.js";
import { applyResolution, compare, connect, outgoing, pull, push } from "../../src/lib/gitsync/sync.js";
import { deploymentBinary, startDeployment } from "../helpers/deployment.mjs";

const run = promisify(execFile);

const deployment = await startDeployment({ label: "git_sync" });
if (!deployment || deployment.unavailable) {
  console.log(`git-sync: ${deployment?.unavailable || "no librepaper binary; run cargo build"}; skipping`);
  process.exit(0);
}
const binary = deploymentBinary();
const base = deployment.base;
const origin = new URL(base).origin;
// sync.js builds the commit author's address from the page's host.
globalThis.location = new URL(base);
const config = await (await fetch(`${base}/api/config`)).json();

// ---------------------------------------------------------------- helpers

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

/// Bounds a sync call, so a hang names itself instead of stalling the run.
async function limit(what, promise, timeout = 90000) {
  let timer;
  try {
    return await Promise.race([
      promise,
      new Promise((_, fail) => { timer = setTimeout(() => fail(new Error(`timed out during ${what}`)), timeout); }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

const root = await mkdtemp(join(tmpdir(), "librepaper-git-sync-"));
const gitConfig = join(root, "gitconfig");
writeFileSync(gitConfig, [
  "[user]", "\tname = Test Committer", "\temail = committer@example.invalid",
  "[init]", "\tdefaultBranch = main",
  "[protocol \"file\"]", "\tallow = always",
  "",
].join("\n"));
const gitEnv = { ...process.env, HOME: root, GIT_CONFIG_GLOBAL: gitConfig, GIT_CONFIG_NOSYSTEM: "1", GIT_TERMINAL_PROMPT: "0" };

function git(cwd, ...args) {
  return execFileSync("git", args, { cwd, env: gitEnv, stdio: ["ignore", "pipe", "pipe"] });
}

let gitMissing = null;
try {
  const version = git(root, "--version").toString().match(/(\d+)\.(\d+)/);
  if (!version || Number(version[1]) < 2 || (Number(version[1]) === 2 && Number(version[2]) < 38)) {
    gitMissing = `git ${version?.[0] ?? "of unknown version"} is older than 2.38`;
  }
} catch {
  gitMissing = "git is not installed";
}
if (gitMissing) {
  console.log(`git-sync: ${gitMissing}; skipping`);
  await deployment.stop();
  await rm(root, { recursive: true, force: true });
  process.exit(0);
}

// ---------------------------------------------------------------- the local service

// Isolated state in the way local-service.mjs does it: nothing here touches
// the user's installation.
const state = join(root, "state");
const localState = join(state, "librepaper", "local");
const grantsPath = join(localState, "git-grants.json");
mkdirSync(localState, { recursive: true });
writeFileSync(grantsPath, JSON.stringify({ grants: [] }));
const listener = createServer();
await new Promise((done) => listener.listen(0, "127.0.0.1", done));
const sidecarPort = listener.address().port;
await new Promise((done) => listener.close(done));
const sidecarEnv = {
  ...gitEnv,
  XDG_STATE_HOME: state,
  XDG_CACHE_HOME: join(root, "cache"),
  XDG_CONFIG_HOME: join(root, "config"),
  LIBREPAPER_LOCAL_PORT: String(sidecarPort),
  LIBREPAPER_SERVER: `${base}/`,
};
delete sidecarEnv.DISPLAY;
delete sidecarEnv.WAYLAND_DISPLAY;
const local = (...args) => run(binary, ["local", ...args], { env: sidecarEnv, timeout: 30000 });

let sidecar;
try {
  sidecar = JSON.parse((await local("endpoint")).stdout.trim().split("\n").at(-1));
} catch (error) {
  await local("stop").catch(() => {});
  await deployment.stop();
  await rm(root, { recursive: true, force: true });
  throw error;
}
assert.ok(sidecar.token, "the endpoint carries the local credential");
const sidecarBase = `${sidecar.address}librepaper/local`;

after(async () => {
  await local("stop").catch(() => {});
  await deployment.stop();
  await rm(root, { recursive: true, force: true });
});

/// The request function of the page's bridge, as `gitRequest` in
/// web/src/lib/desktop/client.js does it for one project.
function requestFor(slug) {
  return async (op, body = {}) => {
    const response = await fetch(`${sidecarBase}/git/${op}`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${sidecar.token}`,
        "X-LibrePaper-Origin": origin,
        "content-type": "application/json",
      },
      body: JSON.stringify({ ...body, project: slug }),
      signal: AbortSignal.timeout(80000),
    });
    const parsed = await response.json().catch(() => null);
    if (!response.ok) {
      const error = new Error(parsed?.error || `The Git service answered ${response.status}.`);
      error.status = response.status;
      error.kind = parsed?.kind;
      error.body = parsed;
      throw error;
    }
    return parsed;
  };
}

/// The grant the folder chooser would have recorded for `path`.
function addGrant(slug, path) {
  const file = JSON.parse(readFileSync(grantsPath, "utf8"));
  const grant = `g-${randomBytes(16).toString("hex")}`;
  file.grants.push({ grant, origin, project: slug, path, created_at: Math.floor(Date.now() / 1000) });
  writeFileSync(grantsPath, JSON.stringify(file), { mode: 0o600 });
  return grant;
}

/// The private repository directory the service keeps for a connection id,
/// found by its record rather than by recomputing the directory name.
function privateRepository(connectionId) {
  const gitRoot = join(localState, "git");
  for (const hash of readdirSync(gitRoot)) {
    for (const name of readdirSync(join(gitRoot, hash))) {
      const record = join(gitRoot, hash, name, "connection.json");
      if (existsSync(record) && JSON.parse(readFileSync(record, "utf8")).connection_id === connectionId) {
        return join(gitRoot, hash, name, "repo.git");
      }
    }
  }
  throw new Error(`no private repository for connection ${connectionId}`);
}

// ---------------------------------------------------------------- the project side

/// A client the way the browser holds one: a socket with the session cookie
/// and a project session fed by the room's frames. `state.pending` is the
/// session's own count of edits the server has not made durable yet.
async function join_(slug) {
  const socket = new WebSocket(`${base.replace("http", "ws")}/ws/${slug}`, { headers: { cookie: deployment.cookie } });
  await new Promise((done, fail) => {
    socket.addEventListener("open", done, { once: true });
    socket.addEventListener("error", () => fail(new Error("the socket could not be opened")), { once: true });
  });
  const state = { pending: 0 };
  const session = createProjectSession({
    send: (message) => socket.send(JSON.stringify(message)),
    presenceId: () => `git-test-${randomBytes(4).toString("hex")}`,
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
    else if (frame.type === "doc-update") session.apply(frame.update);
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
const paths = (client) => client.session.list().map((file) => file.path).sort();

/// Waits until the server has every edit the session made, so an operation
/// that reads the document on the server sees what the page holds.
async function settle(client) {
  await pause(100);
  await wait("the edit to be saved", () => client.state.pending === 0);
}

/// A live edit in the browser: replaces `from` with `to` inside a file, so the
/// rest of the text keeps its place, and waits until the server has it.
async function replaceIn(client, path, from, to) {
  const file = client.session.list().find((item) => item.path === path);
  assert.ok(file, `${path} is in the project`);
  const body = client.session.textOf(file.id);
  const at = body.toString().indexOf(from);
  assert.ok(at >= 0, `${path} contains ${JSON.stringify(from)}`);
  body.delete(at, from.length);
  body.insert(at, to);
  client.session.doc.commit();
  await settle(client);
}

/// Makes the project hold exactly `files` (a map from path to text).
async function shape(client, files) {
  for (const [path, text] of Object.entries(files)) {
    if (!client.session.list().some((item) => item.path === path)) client.session.addText(path, text);
  }
  for (const file of client.session.list()) {
    if (!(file.path in files)) client.session.removeFile(file.id);
  }
  await settle(client);
  for (const [path, text] of Object.entries(files)) {
    await wait(`${path} in the project`, () => textAt(client, path) === text);
  }
}

// The server half of sync with the test's cookie and absolute URLs. After a
// source change sync.js itself waits for the update to reach the session.
function apiFor(slug) {
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
    getLink: () => fetch(`${root}/sync`, { headers: headers() }).then(parse),
    putLink: (body) => send("PUT", "/sync", body),
    async deleteLink() {
      const response = await fetch(`${root}/sync`, { method: "DELETE", headers: headers() });
      if (!response.ok) await parse(response);
    },
    sourceChange: (body) => send("POST", "/source-change", body),
    uploadAsset: (bytes) => fetch(`${root}/assets`, { method: "PUT", headers: headers(), body: bytes }).then(parse),
    async assetBytes(sha) {
      const response = await fetch(`${root}/assets/${sha}`, { headers: headers() });
      if (!response.ok) await parse(response);
      return new Uint8Array(await response.arrayBuffer());
    },
  };
}

// ---------------------------------------------------------------- the repository side

let counter = 0;

/// A fresh bare repository and an ordinary clone of it, which plays the other
/// side: whoever else pushes to the branch.
function otherSide() {
  counter += 1;
  const bare = join(root, `remote${counter}.git`);
  const clone = join(root, `clone${counter}`);
  git(root, "init", "--bare", "--quiet", "-b", "main", bare);
  git(root, "init", "--quiet", "-b", "main", clone);
  git(clone, "config", "core.autocrlf", "false");
  git(clone, "remote", "add", "origin", bare);
  const headOf = () => {
    try {
      return git(bare, "rev-parse", "--verify", "--quiet", "refs/heads/main").toString().trim();
    } catch {
      return null;
    }
  };
  return {
    bare,
    clone,
    headOf,
    /// Writes `files` (a map from path to string, Buffer or null to delete),
    /// commits on top of the branch as it is now, pushes, and answers the id.
    commit(files, message = "Other side") {
      if (headOf()) {
        git(clone, "fetch", "--quiet", "origin");
        git(clone, "checkout", "--quiet", "-B", "main", "origin/main");
      }
      for (const [path, content] of Object.entries(files)) {
        const target = join(clone, path);
        if (content === null) {
          rmSync(target, { force: true });
        } else {
          mkdirSync(dirname(target), { recursive: true });
          writeFileSync(target, content);
        }
      }
      git(clone, "add", "--all");
      git(clone, "commit", "--quiet", "-m", message);
      git(clone, "push", "--quiet", "origin", "main");
      return headOf();
    },
    /// The bytes of a file on the branch, or null.
    show(path, ref = "main") {
      try {
        return git(bare, "cat-file", "blob", `${ref}:${path}`);
      } catch {
        return null;
      }
    },
    read(path, ref = "main") {
      return this.show(path, ref)?.toString("utf8") ?? null;
    },
    parents(commit) {
      return git(bare, "rev-list", "--parents", "-n", "1", commit).toString().trim().split(" ").slice(1);
    },
  };
}

const sorted = (list) => [...list].sort();

/// A published document, a joined client, a seeded repository and a context
/// for it, with nothing connected yet. `project` is what the project holds
/// before connecting, `remote` what the repository's branch holds (nothing
/// when it is empty).
async function fixture({ remote = {}, project = {}, folder = "" }) {
  const repo = otherSide();
  if (Object.keys(remote).length) repo.commit(remote, "Seed");
  const { slug } = await deployment.publish({ title: `Git ${counter}`, source: "# Placeholder\n", source_format: "markdown" });
  const client = await join_(slug);
  await shape(client, project);
  const grant = addGrant(slug, repo.bare);
  const git_ = createBridge(requestFor(slug));
  const contextFor = (holder) => ({
    slug,
    session: {
      doc: holder.session.doc,
      list: () => holder.session.list(),
      textOf: (id) => holder.session.textOf(id),
    },
    rules: config,
    git: git_,
    api: apiFor(slug),
    me: { name: "Tester", handle: "tester" },
    arrivalMs: 20000,
  });
  const ctx = contextFor(client);
  const clients = [client];
  const fx = {
    slug,
    repo,
    client,
    ctx,
    git: git_,
    draft: { remote: { grant }, label: "", branch: "main", folder },
    contextFor,
    /// A second page on the same document, with its own session.
    async another() {
      const second = await join_(slug);
      clients.push(second);
      return { client: second, ctx: contextFor(second) };
    },
    link: async () => (await ctx.api.getLink()).link,
    closeAll: () => clients.forEach((one) => one.close()),
  };
  return fx;
}

async function withFixture(options, action) {
  const fx = await fixture(options);
  try {
    await action(fx);
  } finally {
    fx.closeAll();
  }
}

/// Connects a fixture whose project already matches the repository and checks
/// that nothing changed.
async function linked(fx) {
  const result = await limit("connect", connect(fx.ctx, fx.draft, "remote"));
  assert.ok(["pulled", "nothing"].includes(result.status), JSON.stringify(result));
  const link = await fx.link();
  assert.equal(link.checkpoint, fx.repo.headOf(), "the link records the head it synced with");
  return link;
}

const TEST = { timeout: 240000 };

// ---------------------------------------------------------------- 1. first connection

const INTRO_REMOTE = "# Intro\n\nFrom the repository.\n";
const INTRO_LOCAL = "# Intro\n\nFrom LibrePaper.\n";
const BIB = "@book{a, title={A}}\n";
const NOTES = "Notes kept in LibrePaper.\n";

const firstConnection = () => ({
  remote: { "paper/intro.qmd": INTRO_REMOTE, "paper/refs.bib": BIB, "README.md": "# Repository\n" },
  project: { "intro.qmd": INTRO_LOCAL, "notes.md": NOTES },
  folder: "paper",
});

test("1a. first connection keeping the repository's copies", TEST, () => withFixture(firstConnection(), async (fx) => {
  const head = fx.repo.headOf();
  const looked = await limit("compare", compare(fx.ctx, fx.draft));
  assert.deepEqual(looked.refusals, []);
  assert.deepEqual(looked.incoming, ["refs.bib"], JSON.stringify(looked));
  assert.deepEqual(looked.outgoing, ["notes.md"], JSON.stringify(looked));
  assert.deepEqual(looked.differing, ["intro.qmd"], JSON.stringify(looked));
  assert.equal(await fx.link(), null, "comparing records nothing");

  const result = await limit("connect", connect(fx.ctx, fx.draft, "remote"));
  assert.equal(result.status, "pulled", JSON.stringify(result));
  await wait("the repository's files", () => textAt(fx.client, "intro.qmd") === INTRO_REMOTE && textAt(fx.client, "refs.bib") === BIB);
  assert.equal(textAt(fx.client, "notes.md"), NOTES, "the file only LibrePaper has stays");
  const link = await fx.link();
  assert.equal(link.checkpoint, head);
  assert.deepEqual(sorted(link.synced_paths), ["intro.qmd", "refs.bib"]);
  assert.equal(link.folder, "paper");
  assert.equal(fx.repo.headOf(), head, "connecting in this direction pushes nothing");
}));

test("1b. first connection keeping LibrePaper's copies", TEST, () => withFixture(firstConnection(), async (fx) => {
  const head = fx.repo.headOf();
  const result = await limit("connect", connect(fx.ctx, fx.draft, "local"));
  assert.equal(result.status, "pushed", JSON.stringify(result));
  await wait("the repository's bibliography", () => textAt(fx.client, "refs.bib") === BIB);
  assert.equal(textAt(fx.client, "intro.qmd"), INTRO_LOCAL, "LibrePaper's text is kept");

  const pushed = fx.repo.headOf();
  assert.notEqual(pushed, head);
  assert.deepEqual(fx.repo.parents(pushed), [head], "the push sits on the original head");
  assert.equal(fx.repo.read("paper/intro.qmd"), INTRO_LOCAL);
  assert.equal(fx.repo.read("paper/notes.md"), NOTES);
  assert.equal(fx.repo.read("paper/refs.bib"), BIB);
  assert.equal(fx.repo.read("README.md"), "# Repository\n", "files outside the folder are untouched");
  assert.equal((await fx.link()).checkpoint, pushed);
}));

test("1c. first connection to an empty branch pushes the project as the first commit", TEST, () => withFixture({
  project: { "intro.qmd": INTRO_LOCAL, "notes.md": NOTES },
  folder: "paper",
}, async (fx) => {
  assert.equal(fx.repo.headOf(), null, "the branch has no commits");
  const result = await limit("connect", connect(fx.ctx, fx.draft, "local"));
  assert.equal(result.status, "pushed", JSON.stringify(result));
  const first = fx.repo.headOf();
  assert.ok(first, "the branch now has a commit");
  assert.deepEqual(fx.repo.parents(first), [], "it is the first commit");
  assert.equal(fx.repo.read("paper/intro.qmd"), INTRO_LOCAL);
  assert.equal(fx.repo.read("paper/notes.md"), NOTES);
  const link = await fx.link();
  assert.equal(link.checkpoint, first);
  assert.deepEqual(sorted(link.synced_paths), ["intro.qmd", "notes.md"]);
}));

// ---------------------------------------------------------------- 2 and 3. merging

const TWO_PARAGRAPHS = "# Intro\n\nFirst paragraph.\n\nSecond paragraph.\n";

test("2. independent edits merge, and reach the repository only on a push", TEST, () => withFixture({
  remote: { "intro.qmd": TWO_PARAGRAPHS },
  project: { "intro.qmd": TWO_PARAGRAPHS },
}, async (fx) => {
  await linked(fx);
  await replaceIn(fx.client, "intro.qmd", "First paragraph.", "First paragraph, edited here.");
  const remoteEdit = "# Intro\n\nFirst paragraph.\n\nSecond paragraph, edited there.\n";
  fx.repo.commit({ "intro.qmd": remoteEdit });

  const pulled = await limit("pull", pull(fx.ctx));
  assert.equal(pulled.status, "pulled", JSON.stringify(pulled));
  const both = "# Intro\n\nFirst paragraph, edited here.\n\nSecond paragraph, edited there.\n";
  await wait("both edits", () => textAt(fx.client, "intro.qmd") === both);
  assert.equal(fx.repo.read("intro.qmd"), remoteEdit, "the repository has only its own edit until a push");
  assert.equal((await fx.link()).checkpoint, fx.repo.headOf());

  const pushed = await limit("push", push(fx.ctx, "Both edits"));
  assert.equal(pushed.status, "pushed", JSON.stringify(pushed));
  assert.equal(fx.repo.read("intro.qmd"), both);
  assert.equal(textAt(fx.client, "intro.qmd"), both);
}));

test("3. overlapping edits stop for a resolution and the resolved text is pushed", TEST, () => withFixture({
  remote: { "intro.qmd": "# Intro\n\nThe shared line.\n" },
  project: { "intro.qmd": "# Intro\n\nThe shared line.\n" },
}, async (fx) => {
  await linked(fx);
  const mine = "# Intro\n\nThe shared line, local.\n";
  await replaceIn(fx.client, "intro.qmd", "The shared line.", "The shared line, local.");
  fx.repo.commit({ "intro.qmd": "# Intro\n\nThe shared line, remote.\n" });

  const conflicted = await limit("pull", pull(fx.ctx));
  assert.equal(conflicted.status, "conflict", JSON.stringify(conflicted));
  assert.deepEqual(conflicted.conflicts.map((item) => item.path), ["intro.qmd"]);
  assert.equal(textAt(fx.client, "intro.qmd"), mine, "a conflicting pull changes nothing");

  const resolved = "# Intro\n\nThe shared line, both.\n";
  const applied = await limit("apply", applyResolution(fx.ctx, conflicted.session, { "intro.qmd": { text: resolved } }));
  assert.equal(applied.status, "pulled", JSON.stringify(applied));
  await wait("the resolved text", () => textAt(fx.client, "intro.qmd") === resolved);
  assert.equal((await fx.link()).checkpoint, fx.repo.headOf());

  const pushed = await limit("push", push(fx.ctx, "Resolved"));
  assert.equal(pushed.status, "pushed", JSON.stringify(pushed));
  assert.equal(fx.repo.read("intro.qmd"), resolved);
}));

// ---------------------------------------------------------------- 4. typing during a pull

test("4. typing in a touched file during a pull is refused once and kept", TEST, () => withFixture({
  remote: { "intro.qmd": "# Intro\n\nOne.\n\nTwo.\n" },
  project: { "intro.qmd": "# Intro\n\nOne.\n\nTwo.\n" },
}, async (fx) => {
  await linked(fx);
  fx.repo.commit({ "intro.qmd": "# Intro\n\nOne.\n\nTwo, remote.\n" });

  // The word is typed after the pull planned its patches and before the
  // request is sent, then the request goes through unchanged.
  const real = fx.ctx.api.sourceChange;
  const seen = [];
  const ctx = {
    ...fx.ctx,
    api: {
      ...fx.ctx.api,
      async sourceChange(body) {
        if (!seen.length) await replaceIn(fx.client, "intro.qmd", "One.", "One typed.");
        try {
          const answer = await real(body);
          seen.push({ status: 200 });
          return answer;
        } catch (error) {
          seen.push({ status: error.status, reason: error.body?.reason, files: error.body?.files });
          throw error;
        }
      },
    },
  };
  const pulled = await limit("pull", pull(ctx));
  assert.equal(pulled.status, "pulled", JSON.stringify(pulled));
  assert.ok(seen.length >= 2, `the pull retried: ${JSON.stringify(seen)}`);
  assert.equal(seen[0].status, 409, "the first request is refused");
  assert.equal(seen[0].reason, "changed", JSON.stringify(seen[0]));
  assert.equal(seen.at(-1).status, 200);
  const expected = "# Intro\n\nOne typed.\n\nTwo, remote.\n";
  await wait("the typed word and the pulled edit", () => textAt(fx.client, "intro.qmd") === expected);
}));

// ---------------------------------------------------------------- 5. rules change

test("5. a path the rules newly ignore stays where it is and is not pushed", TEST, () => withFixture({
  remote: { "intro.qmd": "# Intro\n\nHello.\n", "data/a.csv": "x,y\n1,2\n" },
  project: { "intro.qmd": "# Intro\n\nHello.\n", "data/a.csv": "x,y\n1,2\n" },
}, async (fx) => {
  const first = await linked(fx);
  assert.ok(first.synced_paths.includes("data/a.csv"), "the csv is synced before the rules change");

  fx.repo.commit({ ".librepaperignore": "data/\n" });
  const pulled = await limit("pull", pull(fx.ctx));
  assert.ok(["pulled", "nothing"].includes(pulled.status), JSON.stringify(pulled));
  assert.equal(textAt(fx.client, "data/a.csv"), "x,y\n1,2\n", "the project keeps the file");
  assert.equal(fx.repo.read("data/a.csv"), "x,y\n1,2\n", "the repository keeps the file");
  const link = await fx.link();
  assert.ok(!link.synced_paths.includes("data/a.csv"), "it leaves the synced paths");
  assert.ok(link.synced_paths.includes("intro.qmd"));

  await replaceIn(fx.client, "data/a.csv", "1,2", "1,2\n3,4");
  await replaceIn(fx.client, "intro.qmd", "Hello.", "Hello again.");
  const pushed = await limit("push", push(fx.ctx, "Edit the intro"));
  assert.equal(pushed.status, "pushed", JSON.stringify(pushed));
  assert.deepEqual(pushed.files.map((file) => file.path), ["intro.qmd"]);
  assert.equal(fx.repo.read("intro.qmd"), "# Intro\n\nHello again.\n");
  assert.equal(fx.repo.read("data/a.csv"), "x,y\n1,2\n", "the repository's copy is untouched");
  assert.equal(textAt(fx.client, "data/a.csv"), "x,y\n1,2\n3,4\n", "LibrePaper's edit stays in the project");
}));

// ---------------------------------------------------------------- 6. CRLF

test("6. a CRLF file keeps its endings until LibrePaper edits it", TEST, () => {
  const crlf = Buffer.from("@book{a,\r\n  title={A}\r\n}\r\n");
  return withFixture({
    remote: { "intro.qmd": "# Intro\n\nHello.\n", "refs.bib": crlf },
    project: { "intro.qmd": "# Intro\n\nHello.\n" },
  }, async (fx) => {
    const result = await limit("connect", connect(fx.ctx, fx.draft, "remote"));
    assert.equal(result.status, "pulled", JSON.stringify(result));
    await wait("the bibliography", () => textAt(fx.client, "refs.bib") === "@book{a,\n  title={A}\n}\n");

    await replaceIn(fx.client, "intro.qmd", "Hello.", "Hello there.");
    const first = await limit("push", push(fx.ctx, "Edit the intro"));
    assert.equal(first.status, "pushed", JSON.stringify(first));
    assert.deepEqual(first.files.map((file) => file.path), ["intro.qmd"]);
    assert.equal(fx.repo.read("intro.qmd"), "# Intro\n\nHello there.\n");
    assert.ok(fx.repo.show("refs.bib").equals(crlf), "the untouched file keeps its CRLF bytes");

    await replaceIn(fx.client, "refs.bib", "title={A}", "title={B}");
    const second = await limit("push", push(fx.ctx, "Edit the bibliography"));
    assert.equal(second.status, "pushed", JSON.stringify(second));
    const written = fx.repo.show("refs.bib");
    assert.ok(!written.includes(13), "an edited file is written with LF");
    assert.equal(written.toString("utf8"), "@book{a,\n  title={B}\n}\n");
  });
});

// ---------------------------------------------------------------- 7. rewritten history

test("7. rewritten history without the checkpoint asks to connect again", TEST, () => withFixture({
  remote: { "intro.qmd": "# Intro\n\nHello.\n" },
  project: { "intro.qmd": "# Intro\n\nHello.\n" },
}, async (fx) => {
  const link = await linked(fx);

  // A force push of an unrelated root commit, then the old objects are gone.
  const { clone, bare } = fx.repo;
  git(clone, "checkout", "--quiet", "--orphan", "rewritten");
  git(clone, "rm", "-rf", "--quiet", ".");
  writeFileSync(join(clone, "intro.qmd"), "# Intro\n\nA different history.\n");
  git(clone, "add", "--all");
  git(clone, "commit", "--quiet", "-m", "Rewritten");
  git(clone, "push", "--quiet", "--force", "origin", "rewritten:main");
  git(bare, "reflog", "expire", "--expire=now", "--all");
  git(bare, "gc", "--quiet", "--prune=now");
  assert.notEqual(fx.repo.headOf(), link.checkpoint);

  // This desktop loses its copy of the checkpoint.
  rmSync(privateRepository(link.connection_id), { recursive: true, force: true });
  await fx.git.connect({ connection_id: link.connection_id, remote: fx.draft.remote, branch: "main" });

  const refused = await limit("pull", pull(fx.ctx));
  assert.equal(refused.status, "refused", JSON.stringify(refused));
  assert.equal(refused.rebaseline, true, JSON.stringify(refused));
  assert.equal(textAt(fx.client, "intro.qmd"), "# Intro\n\nHello.\n", "nothing was guessed");
  assert.equal((await fx.link()).checkpoint, link.checkpoint);
}));

// ---------------------------------------------------------------- 8. interrupted push

test("8. a push whose record was never written is adopted by the next pull", TEST, () => withFixture({
  remote: { "intro.qmd": "# Intro\n\nHello.\n" },
  project: { "intro.qmd": "# Intro\n\nHello.\n" },
}, async (fx) => {
  const link = await linked(fx);
  const mine = "# Intro\n\nPushed by a desktop that died.\n";
  await replaceIn(fx.client, "intro.qmd", "Hello.", "Pushed by a desktop that died.");

  // The Git half of a push, by hand, and no record: the desktop died between
  // moving the branch and writing the link.
  const planned = await limit("outgoing", outgoing(fx.ctx));
  assert.equal(planned.status, "outgoing", JSON.stringify(planned));
  const plan = planned.outgoing;
  const tree = await fx.git.treeEdit(plan.remoteTree, plan.edits);
  const commit = await fx.git.commitTree(tree, [plan.remote], "Dead desktop", { name: "Tester", email: "tester@example.invalid" });
  await fx.git.push(commit);
  assert.equal(fx.repo.headOf(), commit);
  assert.equal((await fx.link()).checkpoint, link.checkpoint, "the server never heard of the push");

  const second = await fx.another();
  assert.equal(textAt(second.client, "intro.qmd"), mine, "the second page sees the live text");
  const adopted = await limit("pull", pull(second.ctx));
  assert.ok(["pulled", "nothing"].includes(adopted.status), JSON.stringify(adopted));
  assert.equal((adopted.files || []).length, 0, "nothing in the project changes");
  assert.equal((await fx.link()).checkpoint, commit);
  assert.equal(textAt(fx.client, "intro.qmd"), mine);
  assert.equal(textAt(second.client, "intro.qmd"), mine);
}));

// ---------------------------------------------------------------- 9. push moved

test("9. a branch that moves after the review asks again, then both land", TEST, () => withFixture({
  remote: { "intro.qmd": TWO_PARAGRAPHS },
  project: { "intro.qmd": TWO_PARAGRAPHS },
}, async (fx) => {
  await linked(fx);
  await replaceIn(fx.client, "intro.qmd", "First paragraph.", "First paragraph, edited here.");
  const reviewed = await limit("outgoing", outgoing(fx.ctx));
  assert.equal(reviewed.status, "outgoing", JSON.stringify(reviewed));

  const theirs = fx.repo.commit({ "intro.qmd": "# Intro\n\nFirst paragraph.\n\nSecond paragraph, edited there.\n" });
  const moved = await limit("push", push(fx.ctx, "Reviewed", { confirmed: reviewed.outgoing }));
  assert.equal(moved.status, "moved", JSON.stringify(moved));
  assert.equal(fx.repo.headOf(), theirs, "nothing was pushed");

  const both = "# Intro\n\nFirst paragraph, edited here.\n\nSecond paragraph, edited there.\n";
  await wait("the pulled edit", () => textAt(fx.client, "intro.qmd") === both);
  const landed = await limit("push", push(fx.ctx, "Both", { confirmed: moved.outgoing }));
  assert.equal(landed.status, "pushed", JSON.stringify(landed));
  assert.equal(fx.repo.read("intro.qmd"), both);
  assert.deepEqual(fx.repo.parents(fx.repo.headOf()), [theirs]);
}));

// ---------------------------------------------------------------- 10. comment anchors

// Anchoring a comment on a passage needs the editor's anchor records in the
// document, which only the browser's comment machinery writes. The HTTP route
// takes a place the page computed, so there is no honest way to make one here.
test("10. a comment anchor on text the pull leaves alone is kept", { skip: "creating an anchored comment needs the browser's anchor machinery" }, () => {});
