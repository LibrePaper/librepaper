// The shared harness of the Git sync integration tests (git-sync.mjs and
// git-sync-fuzz.mjs): a real `librepaper admin serve` through deployment.mjs, a
// real detached local service with isolated state, the project session fed by
// the room's frames, and the other side as an ordinary clone driven with plain
// `git`.
//
// Importing this module starts the deployment. When the deployment or git is
// unavailable it exits the process with success, so a test file that imports
// it skips the same way. The label names the throwaway database and temporary
// directory. LIBREPAPER_GIT_HARNESS_LABEL overrides the default "git_sync", and
// each run adds a random suffix, so test files that share the default are safe.
//
// Local remotes need a grant, which the app gets from a native folder chooser.
// The chooser cannot run headless, so the harness writes the same grant record
// the chooser creates into the service's isolated state, and the service reads
// it again on every request.
import assert from "node:assert/strict";
import { execFile, execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { after } from "node:test";
import { promisify } from "node:util";
import { createProjectSession } from "../../src/lib/project-session.js";
import { createBridge } from "../../src/lib/gitsync/bridge.js";
import { connect, pull } from "../../src/lib/gitsync/sync.js";
import { deploymentBinary, startDeployment } from "./deployment.mjs";

const run = promisify(execFile);

// Every scenario and every seed of the random walk publishes a document and
// uploads figures, more than the default hourly limit allows in one run.
const deployment = await startDeployment({
  label: process.env.LIBREPAPER_GIT_HARNESS_LABEL || "git_sync",
  advanced: "[limits]\npublisher_uploads_per_hour = 100000\n",
});
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
  // The root also holds files, such as the empty attributes file merges use.
  const hashes = readdirSync(gitRoot, { withFileTypes: true }).filter((entry) => entry.isDirectory()).map((entry) => entry.name);
  for (const hash of hashes) {
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
    async hasAsset(sha) {
      const response = await fetch(`${root}/assets/${sha}`, { method: "HEAD", headers: headers() });
      if (response.ok) return true;
      if (response.status === 404) return false;
      await parse(response);
      return false;
    },
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

/// Connects a fixture whose project already matches the repository, runs the
/// first pull and checks that nothing changed.
async function linked(fx) {
  const connected = await limit("connect", connect(fx.ctx, fx.draft));
  assert.equal(connected.status, "connected", JSON.stringify(connected));
  const result = await limit("pull", pull(fx.ctx));
  assert.ok(["pulled", "nothing"].includes(result.status), JSON.stringify(result));
  const link = await fx.link();
  assert.equal(link.checkpoint, fx.repo.headOf(), "the link records the head it synced with");
  return link;
}

const TEST = { timeout: 240000 };

export { TEST, git, limit, linked, paths, privateRepository, replaceIn, settle, sorted, textAt, wait, withFixture };
