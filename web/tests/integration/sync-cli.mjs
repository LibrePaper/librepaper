// `librepaper push` and `librepaper pull` (SPEC-sync.md), end to end: the real
// binary, run in real working directories, against a real `librepaper admin
// serve` on a throwaway PostgreSQL database. Edits on the web are made through
// the room's socket the way a browser makes them. Every outcome is read back
// from the server's own routes (project, history, one version, assets) and
// from git itself.
//
// One project lives through the whole file and the scenarios build on each
// other, in the order the spec tells the story:
//   1. publish a directory            6. figures
//   2. pull into an empty repository  7. a history trim
//   3. a commit per named version     8. a push whose reply is cut
//   4. a refused push, then a push    9. a folder without git
//      beside an untouched web edit
//   5. a pending suggestion
//
// Requires a built librepaper binary (LIBREPAPER_TEST_BINARY, by default
// target/debug/librepaper), git 2.38 or newer and LIBREPAPER_TEST_POSTGRES_URL,
// plus LIBREPAPER_TEST_PSQL and LIBREPAPER_TEST_PSQL_URL when psql lives in a
// container. `tools/dev/db sync-test` provides all of it. When something is
// missing the file says so and skips.
import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { createHash, randomBytes, randomUUID } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import http from "node:http";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { deflateSync } from "node:zlib";
import { createProjectSession } from "../../src/lib/project-session.js";
import { listChanges, reject, trackedSplice } from "../../src/lib/track-changes.js";
import { deploymentBinary, startDeployment, until } from "../helpers/deployment.mjs";

// ---------------------------------------------------------------- skipping

function gitProblem() {
  try {
    const found = execFileSync("git", ["--version"], { encoding: "utf8" }).match(/(\d+)\.(\d+)/);
    if (!found || Number(found[1]) < 2 || (Number(found[1]) === 2 && Number(found[2]) < 38)) {
      return `git ${found?.[0] ?? "of unknown version"} is older than 2.38`;
    }
    return null;
  } catch {
    return "git is not installed";
  }
}
const missingGit = gitProblem();
if (missingGit) {
  console.log(`sync-cli: ${missingGit}; skipping`);
  process.exit(0);
}

const HANDLE = "tester";
// Every scenario publishes or uploads, more than the default hourly limit
// allows across repeated runs against one account.
const deployment = await startDeployment({
  label: "sync_cli",
  handle: HANDLE,
  advanced: "[limits]\npublisher_uploads_per_hour = 100000\n",
});
if (!deployment || deployment.unavailable) {
  console.log(`sync-cli: ${deployment?.unavailable || "no librepaper binary; run cargo build"}; skipping`);
  process.exit(0);
}

const base = deployment.base;
const binary = deploymentBinary();
const config = await (await fetch(`${base}/api/config`)).json();
const owner = { "x-librepaper-client": "1", cookie: deployment.cookie };

const root = await mkdtemp(join(tmpdir(), "librepaper-sync-cli-")).catch(async (error) => {
  await deployment.stop();
  throw error;
});

// ---------------------------------------------------------------- the files under test

const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

const crcTable = Array.from({ length: 256 }, (_, n) => {
  let value = n;
  for (let bit = 0; bit < 8; bit += 1) value = value & 1 ? 0xedb88320 ^ (value >>> 1) : value >>> 1;
  return value >>> 0;
});
const crc32 = (bytes) => {
  let value = 0xffffffff;
  for (const byte of bytes) value = crcTable[(value ^ byte) & 0xff] ^ (value >>> 8);
  return (value ^ 0xffffffff) >>> 0;
};
function pngChunk(type, data) {
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const checksum = Buffer.alloc(4);
  checksum.writeUInt32BE(crc32(body));
  return Buffer.concat([length, body, checksum]);
}
/// A valid PNG of one pixel, so two colours are two different figures.
function png(red, green, blue) {
  const header = Buffer.alloc(13);
  header.writeUInt32BE(1, 0);
  header.writeUInt32BE(1, 4);
  header.set([8, 2, 0, 0, 0], 8);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    pngChunk("IHDR", header),
    pngChunk("IDAT", deflateSync(Buffer.from([0, red, green, blue]))),
    pngChunk("IEND", Buffer.alloc(0)),
  ]);
}

const mainTex = (greeting) => `\\documentclass{article}\n\\begin{document}\n${greeting}\n\\input{chapters/one}\n\\end{document}\n`;
const ONE = "\\section{One}\nThe first chapter.\n";
const ONE_WEB = "\\section{One}\nThe first chapter, edited on the web.\n";
const ONE_LOCAL = "\\section{One}\nThe first chapter, edited here.\n";
const APPENDIX = "\\section{Appendix}\nNotes.\n";
const RED = png(200, 10, 10);
const GREEN = png(10, 200, 30);

// ---------------------------------------------------------------- git and the CLI

// Nothing here reads the user's own git configuration, state or credentials.
const home = join(root, "home");
for (const name of ["config", "state", "cache", "data"]) mkdirSync(join(home, name), { recursive: true });
const gitConfig = join(root, "gitconfig");
writeFileSync(gitConfig, [
  "[init]", "\tdefaultBranch = main",
  "[user]", "\tname = Test Committer", "\temail = committer@example.invalid",
  "",
].join("\n"));
const baseEnv = {
  PATH: process.env.PATH,
  HOME: home,
  XDG_CONFIG_HOME: join(home, "config"),
  XDG_STATE_HOME: join(home, "state"),
  XDG_CACHE_HOME: join(home, "cache"),
  XDG_DATA_HOME: join(home, "data"),
  GIT_CONFIG_GLOBAL: gitConfig,
  GIT_CONFIG_NOSYSTEM: "1",
  GIT_TERMINAL_PROMPT: "0",
};

/// What git printed, as bytes.
function git(cwd, ...args) {
  return execFileSync("git", args, { cwd, env: baseEnv, stdio: ["ignore", "pipe", "pipe"] });
}
const gitText = (cwd, ...args) => git(cwd, ...args).toString("utf8").trim();

/// The commit a ref names, or null when it names nothing.
function refOf(dir, ref) {
  try {
    return gitText(dir, "rev-parse", "--verify", "--quiet", ref);
  } catch {
    return null;
  }
}

function newRepository(name, author = "Alice Author") {
  const dir = join(root, name);
  mkdirSync(dir, { recursive: true });
  git(dir, "init", "--quiet", "-b", "main");
  git(dir, "config", "user.name", author);
  git(dir, "config", "user.email", "alice@example.invalid");
  git(dir, "config", "core.autocrlf", "false");
  return dir;
}

function writeFiles(dir, files) {
  for (const [path, content] of Object.entries(files)) {
    const target = join(dir, path);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, content);
  }
}

function commitAll(dir, message) {
  git(dir, "add", "--all");
  git(dir, "commit", "--quiet", "-m", message);
  return refOf(dir, "HEAD");
}

/// The commits reachable from a ref, oldest first, with the author, the
/// subject and the value of the LibrePaper-Version trailer.
function commitsOn(dir, ref) {
  const format = "--format=%H%x1f%an%x1f%ae%x1f%at%x1f%s%x1f%(trailers:key=LibrePaper-Version,valueonly)%x1e";
  return git(dir, "log", "--reverse", format, ref).toString("utf8")
    .split("\x1e")
    .map((record) => record.replace(/^\n/, ""))
    .filter((record) => record.length > 0)
    .map((record) => {
      const [hash, name, email, time, subject, version] = record.split("\x1f").map((field) => field.trim());
      return { hash, name, email, time: Number(time), subject, version };
    });
}

/// Every file of a commit, as a map from path to bytes.
function treeOf(dir, ref) {
  const names = git(dir, "ls-tree", "-r", "-z", "--name-only", ref).toString("utf8").split("\0").filter(Boolean);
  return Object.fromEntries(names.map((name) => [name, git(dir, "cat-file", "blob", `${ref}:${name}`)]));
}

/// The record the CLI keeps in a repository, a blob at refs/librepaper/state.
const stateOf = (dir) => JSON.parse(gitText(dir, "cat-file", "-p", "refs/librepaper/state"));

let accountToken = null;
let editToken = null;

/// Runs the librepaper binary in `cwd` and answers how it ended. The token is
/// the only credential it has: the environment holds nothing else.
function librepaper(cwd, args, { token = editToken, env = {} } = {}) {
  return new Promise((done, fail) => {
    const child = spawn(binary, args, {
      cwd,
      env: { ...baseEnv, LIBREPAPER_TOKEN: token, ...env },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (bytes) => { stdout += bytes; });
    child.stderr.on("data", (bytes) => { stderr += bytes; });
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      fail(new Error(`librepaper ${args.join(" ")} did not finish within a minute\n${stdout}${stderr}`));
    }, 60000);
    child.once("error", (error) => { clearTimeout(timer); fail(error); });
    child.once("close", (code) => {
      clearTimeout(timer);
      done({ code, stdout, stderr, output: stdout + stderr });
    });
  });
}

function succeeds(result, what) {
  assert.equal(result.code, 0, `${what} should succeed but exited ${result.code}:\n${result.output}`);
  return result;
}

// ---------------------------------------------------------------- the server side

let slug = null;
const siteUrl = () => `${base}/docs/${slug}`;

function api(path, { method = "GET", body } = {}) {
  return fetch(`${base}/api/documents/${slug}${path}`, {
    method,
    headers: body === undefined ? owner : { ...owner, "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
}

async function getJson(path) {
  const response = await api(path);
  const text = await response.text();
  assert.equal(response.status, 200, `GET ${path} answered ${response.status}: ${text}`);
  return JSON.parse(text);
}

/// The project as the web holds it: its title, its files and its texts.
async function projectOf() {
  const body = await getJson("/project");
  return { title: body.title, files: body.tree.files, texts: body.texts };
}

/// Named versions, newest first. A push is one too.
const namedVersions = async () => (await getJson("/history?named=1&limit=200")).labels;

/// One version with its files and texts, pending suggestions undone.
const versionOf = (sha) => getJson(`/history/${sha}?unsuggested=1`);

async function nameVersion(label) {
  const response = await api("/history/current", { method: "PATCH", body: { label, request_id: randomUUID() } });
  assert.equal(response.status, 200, `naming a version ${label} answered ${response.status}: ${await response.text()}`);
}

/// Keeps the current version and deletes everything before it. The server
/// refuses while another editor is connected, and a page closed a moment ago
/// may not be forgotten yet, so a refusal is tried again for a while.
async function trimHistory() {
  const attempt = await until("the trim to be accepted", async () => {
    const response = await api("/history/trim", { method: "POST", body: {} });
    return response.status === 409 ? false : response;
  });
  assert.equal(attempt.status, 200, `the trim answered ${attempt.status}: ${await attempt.text()}`);
}

/// A token for the CLI. A project token cannot create a project, so the
/// publish uses the account's own sign-in token, minted the way
/// `librepaper login` mints it: ask for a code, approve it with the session,
/// then claim the token.
async function mintAccountToken() {
  const plain = { "content-type": "application/json" };
  const asked = await (await fetch(`${base}/api/auth/device`, { method: "POST", headers: plain, body: "{}" })).json();
  const approved = await fetch(`${base}/api/auth/device/approve`, {
    method: "POST", headers: { ...owner, ...plain }, body: JSON.stringify({ user_code: asked.user_code }),
  });
  assert.equal(approved.status, 200, `approving the sign-in answered ${approved.status}: ${await approved.text()}`);
  const claimed = await (await fetch(`${base}/api/auth/device/token`, {
    method: "POST", headers: plain, body: JSON.stringify({ device_code: asked.device_code }),
  })).json();
  assert.match(claimed.token || "", /^lp_/, `no sign-in token: ${JSON.stringify(claimed)}`);
  return claimed.token;
}

async function mintEditToken() {
  const response = await api("/share", {
    method: "POST",
    body: { create: { kind: "token", role: "edit", name: "Sync test", until: "30d" } },
  });
  const payload = await response.json();
  assert.equal(response.status, 200, `minting a token answered ${response.status}: ${JSON.stringify(payload)}`);
  assert.match(payload.created?.token || "", /^lpt_/, `no project token: ${JSON.stringify(payload)}`);
  return payload.created.token;
}

// ---------------------------------------------------------------- the web side

/// A page the way the browser holds one: a socket with the session cookie and
/// a project session fed by the room's frames. `settle` waits until the server
/// has made every edit the page sent durable.
async function openPage() {
  const socket = new WebSocket(`${base.replace("http", "ws")}/ws/${slug}`, { headers: { cookie: deployment.cookie } });
  await new Promise((done, fail) => {
    socket.addEventListener("open", done, { once: true });
    socket.addEventListener("error", () => fail(new Error("the socket could not be opened")), { once: true });
  });
  const state = { pending: 0 };
  const session = createProjectSession({
    send: (message) => socket.send(JSON.stringify(message)),
    presenceId: () => `sync-test-${randomBytes(4).toString("hex")}`,
    name: "Tester",
    onState: (next) => { state.pending = next.pending; },
    fetchReference: async (reference) => {
      const response = await fetch(new URL(reference, base), { headers: owner });
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
  await until("the page to join", () => {
    if (failures.length) throw Object.assign(failures[0], { fatal: true });
    return session.joined;
  });
  return {
    session,
    async settle() {
      await new Promise((done) => setTimeout(done, 100));
      await until("the edit to be saved", () => state.pending === 0);
    },
    close() {
      session.leave();
      socket.close();
    },
  };
}

async function withPage(action) {
  const page = await openPage();
  try {
    return await action(page);
  } finally {
    page.close();
  }
}

const fileOf = (page, path) => {
  const file = page.session.list().find((item) => item.path === path);
  assert.ok(file, `${path} is in the project`);
  return file;
};

/// An edit made in the browser: `from` becomes `to` inside one file.
async function editOnWeb(path, from, to) {
  await withPage(async (page) => {
    const body = page.session.textOf(fileOf(page, path).id);
    const at = body.toString().indexOf(from);
    assert.ok(at >= 0, `${path} contains ${JSON.stringify(from)}`);
    body.delete(at, from.length);
    body.insert(at, to);
    page.session.doc.commit();
    await page.settle();
  });
}

/// A suggestion made in the browser: `from` becomes `to` as a tracked change
/// that nobody has decided. Answers the change's id.
function suggestOnWeb(path, from, to) {
  return withPage(async (page) => {
    const file = fileOf(page, path);
    const at = page.session.textOf(file.id).toString().indexOf(from);
    assert.ok(at >= 0, `${path} contains ${JSON.stringify(from)}`);
    const ids = trackedSplice(page.session.doc, file.id, at, at + from.length, to, { author: "Ada", who: "acct-ada" }, { run: null });
    assert.equal(ids.length, 1, "one replacement is one suggestion");
    page.session.doc.commit();
    await page.settle();
    return ids[0];
  });
}

async function rejectOnWeb(id) {
  await withPage(async (page) => {
    await until("the suggestion to arrive", () => listChanges(page.session.doc).some((change) => change.id === id));
    assert.equal(reject(page.session.doc, id), true, "the suggestion is rejected");
    await page.settle();
  });
}

// ---------------------------------------------------------------- comparisons

/// What a working directory holds at the project's paths, as bytes.
function diskFiles(dir, project) {
  const held = {};
  for (const path of Object.keys(project.files)) {
    const file = join(dir, path);
    assert.ok(existsSync(file), `${path} is on disk in ${dir}`);
    held[path] = readFileSync(file);
  }
  return held;
}

/// `held` (a map from path to bytes) is exactly the files of `version`, which
/// is a project or one version of it: the same paths, the same texts and
/// figures with the same digests.
function assertHolds(held, version, what) {
  assert.deepEqual(Object.keys(held).sort(), Object.keys(version.files).sort(), `${what} holds the same paths`);
  for (const [path, entry] of Object.entries(version.files)) {
    if (entry.kind === "text") assert.equal(held[path].toString("utf8"), version.texts[path], `${what}: ${path}`);
    else assert.equal(sha256(held[path]), entry.digest, `${what}: ${path}`);
  }
}

// ---------------------------------------------------------------- a proxy that cuts one reply

/// A forwarding proxy in front of the deployment. Once armed, the next
/// source-change request goes through and the deployment applies it, but the
/// answer never reaches the client: the connection is dropped instead.
async function startCuttingProxy() {
  const target = new URL(base);
  const seen = { sourceChanges: 0, cut: 0 };
  let armed = false;
  const isSourceChange = (request) =>
    request.method === "POST" && new URL(request.url, "http://proxy").pathname.endsWith("/source-change");
  const server = http.createServer((request, response) => {
    const cutting = isSourceChange(request) && armed;
    if (isSourceChange(request)) seen.sourceChanges += 1;
    if (cutting) armed = false;
    const upstream = http.request({
      hostname: target.hostname, port: target.port, path: request.url, method: request.method,
      headers: { ...request.headers, host: target.host },
    }, (reply) => {
      if (cutting) {
        seen.cut += 1;
        reply.resume();
        reply.once("end", () => request.socket.destroy());
        return;
      }
      response.writeHead(reply.statusCode, reply.headers);
      reply.pipe(response);
    });
    upstream.on("error", () => {
      if (!response.headersSent) response.writeHead(502);
      response.end();
    });
    request.pipe(upstream);
  });
  await new Promise((done, fail) => { server.once("error", fail); server.listen(0, "127.0.0.1", done); });
  return {
    url: `http://127.0.0.1:${server.address().port}`,
    seen,
    cutNextReply() { armed = true; },
    async close() {
      server.closeAllConnections();
      await new Promise((done) => server.close(done));
    },
  };
}

// ---------------------------------------------------------------- the scenarios

async function scenario(name, run) {
  try {
    await run();
  } catch (error) {
    error.message = `${name}: ${error.message}`;
    throw error;
  }
  console.log(`sync-cli: ${name}`);
}

let proxy = null;
let dirTwo = null;

try {
  accountToken = await mintAccountToken();

  await scenario("1. push --title publishes a directory and records its state", async () => {
    const dir = newRepository("one");
    writeFiles(dir, {
      "main.tex": mainTex("Hello, world."),
      "chapters/one.tex": ONE,
      "fig/a.png": RED,
      ".gitignore": "*.pdf\n",
      "main.pdf": "%PDF-1.4 built here, never synced\n",
    });
    const head = commitAll(dir, "Start the paper");
    succeeds(await librepaper(dir, ["push", "--title", "Sync test"], { token: accountToken, env: { LIBREPAPER_SERVER: base } }), "the first push");

    const state = stateOf(dir);
    assert.ok(state.slug, `the state names the project: ${JSON.stringify(state)}`);
    slug = state.slug;
    assert.equal(new URL(state.site).origin, new URL(base).origin, "the state names the site");
    assert.equal(state.head, head, "the state remembers the commit that was published");

    const project = await projectOf();
    assert.equal(project.title, "Sync test");
    assert.deepEqual(Object.keys(project.files).sort(), ["chapters/one.tex", "fig/a.png", "main.tex"], "the ignored PDF and the dotfile stay behind");
    assert.equal(project.files["fig/a.png"].kind, "asset");
    assertHolds(diskFiles(dir, project), project, "the project");
    assert.deepEqual(Object.keys(state.files).sort(), Object.keys(project.files).sort(), "the state records every synced path");
    for (const [path, entry] of Object.entries(project.files)) {
      assert.equal(state.files[path].sha, entry.digest, `the state hash of ${path} is the server's digest`);
      assert.equal(state.files[path].kind, entry.kind, `the state kind of ${path}`);
    }

    editToken = await mintEditToken();
  });

  await scenario("2. pull into an empty repository", async () => {
    dirTwo = newRepository("two");
    succeeds(await librepaper(dirTwo, ["pull", siteUrl()]), "the first pull");

    const project = await projectOf();
    assert.equal(commitsOn(dirTwo, "refs/librepaper/main").length, 1, "the web project arrives as one commit");
    assertHolds(treeOf(dirTwo, "refs/librepaper/main"), project, "the pulled commit");
    assert.equal(refOf(dirTwo, "main"), refOf(dirTwo, "refs/librepaper/main"), "main was fast-forwarded to the pulled commit");
    assertHolds(diskFiles(dirTwo, project), project, "the working tree");
    assert.equal(gitText(dirTwo, "status", "--porcelain"), "", "the working tree is clean");
    const state = stateOf(dirTwo);
    assert.equal(state.slug, slug);
    assert.equal(state.head, refOf(dirTwo, "refs/librepaper/main"), "the state remembers the pulled commit");
  });

  await scenario("3. a pull makes one commit per named version", async () => {
    await editOnWeb("main.tex", "Hello, world.", "Hello from the web.");
    await nameVersion("Intro");
    await editOnWeb("main.tex", "Hello from the web.", "Hello again from the web.");
    await nameVersion("Intro 2");

    succeeds(await librepaper(dirTwo, ["pull"]), "the pull");

    const chain = commitsOn(dirTwo, "refs/librepaper/main");
    assert.equal(chain.length, 3, `the first pull's commit and one per named version: ${JSON.stringify(chain.map((commit) => commit.subject))}`);
    assert.deepEqual(chain.slice(-2).map((commit) => commit.subject), ["Intro", "Intro 2"], "the commits carry the version names, oldest first");
    const versions = await namedVersions();
    for (const name of ["Intro", "Intro 2"]) {
      const commit = chain.find((item) => item.subject === name);
      const version = versions.find((item) => item.label === name);
      assert.equal(commit.version, version.sha, `${name} carries the LibrePaper-Version trailer`);
      assert.equal(commit.name, version.by, `${name} is authored by the account's name`);
      assert.ok(commit.email.startsWith(`${HANDLE}@`), `${name} is authored from the account's handle: ${commit.email}`);
      assert.ok(Math.abs(commit.time - Date.parse(version.at) / 1000) <= 1, `${name} is dated when the version was made`);
      assertHolds(treeOf(dirTwo, commit.hash), await versionOf(version.sha), `the commit for ${name}`);
    }
    assert.equal(refOf(dirTwo, "main"), chain.at(-1).hash, "main was fast-forwarded");
    assert.equal(gitText(dirTwo, "rev-list", "--merges", "--count", "main"), "0", "no merge commit was needed");
    assert.equal(readFileSync(join(dirTwo, "main.tex"), "utf8"), mainTex("Hello again from the web."), "git wrote the files");
    const state = stateOf(dirTwo);
    assert.equal(state.pulled, versions[0].sequence, "the state moved to the newest version taken in");
    assert.equal(state.head, chain.at(-1).hash);
  });

  await scenario("4. a push over a file the web changed is refused, a push beside it succeeds", async () => {
    writeFiles(dirTwo, { "chapters/one.tex": ONE_LOCAL });
    commitAll(dirTwo, "Edit the chapter here");
    await editOnWeb("chapters/one.tex", "The first chapter.", "The first chapter, edited on the web.");

    const refused = await librepaper(dirTwo, ["push"]);
    assert.equal(refused.code, 1, `a push over a changed file exits 1:\n${refused.output}`);
    assert.match(refused.output, /chapters\/one\.tex/, "the refusal names the path");
    assert.match(refused.output, /Pull first\./);
    const unchanged = await projectOf();
    assert.equal(unchanged.texts["chapters/one.tex"], ONE_WEB, "the web text is as the web left it");
    assert.equal(unchanged.texts["main.tex"], mainTex("Hello again from the web."), "nothing else was written");
    assert.ok(!(await namedVersions()).some((version) => version.label === "Edit the chapter here"), "no version was made");

    // Drop the refused commit, so the only local change is in a file the web did not touch.
    git(dirTwo, "reset", "--quiet", "--hard", "HEAD~1");
    writeFiles(dirTwo, { "main.tex": mainTex("Hello again, from Alice.") });
    const subject = "Reword the greeting";
    const commit = commitAll(dirTwo, subject);
    succeeds(await librepaper(dirTwo, ["push"]), "the push beside an untouched web edit");

    const project = await projectOf();
    assert.equal(project.texts["main.tex"], mainTex("Hello again, from Alice."), "the web has the pushed text");
    assert.equal(project.texts["chapters/one.tex"], ONE_WEB, "the web edit of the other file is kept");
    const [newest] = await namedVersions();
    assert.equal(newest.reason, "push");
    assert.equal(newest.label, subject, "the version is named by the commit subject");
    assert.equal(newest.by, "Alice Author", "the version shows the git author");
    const state = stateOf(dirTwo);
    assert.equal(state.head, commit, "the state remembers the pushed commit");
    assert.equal(state.files["main.tex"].sha, sha256(mainTex("Hello again, from Alice.")));

    // The version this directory pushed stands on its own commit. The web had
    // edited the other file before the push, so the version holds that edit
    // too, as one commit on top of the pushed one rather than a copy of it.
    succeeds(await librepaper(dirTwo, ["pull"]), "the pull after the push");
    const tip = refOf(dirTwo, "refs/librepaper/main");
    const parents = gitText(dirTwo, "rev-list", "--parents", "-1", tip).split(" ").slice(1);
    assert.ok(tip === commit || parents.includes(commit), `the pushed commit is the version's commit or its parent: ${tip} ${parents}`);
    const chain = commitsOn(dirTwo, "refs/librepaper/main");
    assert.equal(chain.filter((entry) => entry.hash === commit).length, 1, "the pushed commit is in the chain once");
    assert.equal(chain.length, tip === commit ? 4 : 5, "nothing is committed twice");
    const held = treeOf(dirTwo, tip);
    assert.equal(held["main.tex"].toString("utf8"), mainTex("Hello again, from Alice."), "the chain holds the pushed text");
    assert.equal(held["chapters/one.tex"].toString("utf8"), ONE_WEB, "and the web's edit of the other file");
    assert.equal(gitText(dirTwo, "status", "--porcelain"), "");
  });

  await scenario("5. a push over a pending suggestion is refused", async () => {
    const suggestion = await suggestOnWeb("main.tex", "Alice", "Carol");
    const subject = "Rename the author";
    writeFiles(dirTwo, { "main.tex": mainTex("Hello again, from Dana.") });
    commitAll(dirTwo, subject);

    const refused = await librepaper(dirTwo, ["push"]);
    assert.equal(refused.code, 1, `a push over a suggestion exits 1:\n${refused.output}`);
    assert.match(refused.output, /suggest/i, "the refusal talks about the suggestion");
    assert.match(refused.output, /main\.tex/, "and names the path");
    assert.doesNotMatch(refused.output, /Pull first/, "it does not ask for a pull the pull could not settle");
    assert.ok(!(await namedVersions()).some((version) => version.label === subject), "no version was made");

    git(dirTwo, "reset", "--quiet", "--hard", "HEAD~1");
    await rejectOnWeb(suggestion);
    await until("the rejection to be saved", async () => (await projectOf()).texts["main.tex"] === mainTex("Hello again, from Alice."));
  });

  await scenario("6. a figure is replaced and then deleted", async () => {
    writeFiles(dirTwo, { "fig/a.png": GREEN });
    commitAll(dirTwo, "Recolour the figure");
    succeeds(await librepaper(dirTwo, ["push"]), "the push that replaces a figure");
    const digest = sha256(GREEN);
    assert.equal((await projectOf()).files["fig/a.png"].digest, digest, "the project lists the new digest");
    const served = await fetch(`${base}/api/documents/${slug}/assets/${digest}`, { headers: owner });
    assert.equal(served.status, 200, "the new figure can be fetched");
    assert.ok(Buffer.from(await served.arrayBuffer()).equals(GREEN), "and it is the bytes that were pushed");

    git(dirTwo, "rm", "--quiet", "fig/a.png");
    commitAll(dirTwo, "Drop the figure");
    succeeds(await librepaper(dirTwo, ["push"]), "the push that deletes a figure");
    const project = await projectOf();
    assert.ok(!("fig/a.png" in project.files), "the figure is gone from the project");
    assert.ok("main.tex" in project.files && "chapters/one.tex" in project.files, "the texts stay");
  });

  await scenario("7. a pull after a history trim", async () => {
    const before = refOf(dirTwo, "HEAD");
    await trimHistory();

    const pulled = succeeds(await librepaper(dirTwo, ["pull"]), "the pull after a trim");
    assert.match(pulled.output, /trim/i, "the pull says history was trimmed");
    const project = await projectOf();
    assertHolds(treeOf(dirTwo, "refs/librepaper/main"), project, "the commit made for the trimmed history");
    assertHolds(diskFiles(dirTwo, project), project, "the working tree");
    assert.equal(gitText(dirTwo, "status", "--porcelain"), "", "the working tree is clean");
    assert.doesNotThrow(() => git(dirTwo, "merge-base", "--is-ancestor", before, "main"), "the local history is kept");
  });

  await scenario("8. a push whose reply is cut is retried and applied once", async () => {
    proxy = await startCuttingProxy();
    const dir = newRepository("four");
    const url = `${proxy.url}/docs/${slug}`;
    succeeds(await librepaper(dir, ["pull", url], { token: accountToken }), "the pull through the proxy");

    const subject = "Edit the chapter over a bad connection";
    const edited = `${ONE_WEB}A second paragraph.\n`;
    writeFiles(dir, { "chapters/one.tex": edited });
    commitAll(dir, subject);
    proxy.cutNextReply();
    succeeds(await librepaper(dir, ["push"], { token: accountToken }), "the push whose first reply is cut");
    assert.equal(proxy.seen.cut, 1, "the proxy cut one reply after the server applied the change");
    assert.equal(proxy.seen.sourceChanges, 2, "the CLI sent the change again");
    assert.equal((await namedVersions()).filter((version) => version.label === subject).length, 1, "the retry replayed the first receipt");
    assert.equal((await projectOf()).texts["chapters/one.tex"], edited);

    // The same guarantee on the wire: one operation key sent twice is applied once.
    const operation = `v2.${Date.now()}.${randomBytes(16).toString("hex")}`;
    const message = "Replay the receipt";
    const send = async () => {
      const response = await fetch(`${base}/api/documents/${slug}/source-change`, {
        method: "POST",
        headers: { authorization: `Bearer ${editToken}`, "content-type": "application/json" },
        body: JSON.stringify({
          operation,
          change: { create: [{ path: "appendix.tex", text: APPENDIX }] },
          sync: { provider: "cli", message },
        }),
      });
      const body = await response.json();
      assert.equal(response.status, 200, `the change answered ${response.status}: ${JSON.stringify(body)}`);
      return body;
    };
    const first = await send();
    const second = await send();
    const withoutReplayFlag = (body) => {
      const copy = structuredClone(body);
      if (copy.receipt) delete copy.receipt.replay;
      return copy;
    };
    assert.ok(first.label?.sha, `the reply names the version it wrote: ${JSON.stringify(first)}`);
    assert.deepEqual(withoutReplayFlag(second), withoutReplayFlag(first), "the second reply is the first receipt again");
    assert.equal((await namedVersions()).filter((version) => version.label === message).length, 1, "the change was applied once");
    assert.equal((await projectOf()).texts["appendix.tex"], APPENDIX);
  });

  await scenario("9. a folder without git lists conflicts and settles them per path", async () => {
    const dir = join(root, "three");
    mkdirSync(dir);
    succeeds(await librepaper(dir, ["pull", siteUrl()]), "the first pull into a folder");
    assert.ok(!existsSync(join(dir, ".git")), "no repository was made");
    const written = await projectOf();
    assertHolds(diskFiles(dir, written), written, "the folder");

    const mine = mainTex("Hello again, from Eve.");
    writeFiles(dir, { "main.tex": mine });
    await editOnWeb("main.tex", "Alice", "Frank");
    await nameVersion("Frank greets");

    const conflicted = await librepaper(dir, ["pull"]);
    assert.match(conflicted.output, /main\.tex/, "the pull names the file");
    assert.match(conflicted.output, /conflict/i, "and calls it a conflict");
    assert.equal(readFileSync(join(dir, "main.tex"), "utf8"), mine, "the local edit is left alone");

    succeeds(await librepaper(dir, ["pull", "--theirs", "main.tex"]), "the pull that takes the web version");
    assert.equal(readFileSync(join(dir, "main.tex"), "utf8"), mainTex("Hello again, from Frank."), "the web version is written");
    const project = await projectOf();
    assertHolds(diskFiles(dir, project), project, "the folder after settling the conflict");
  });

  console.log("sync-cli: publish, pull, refusals, figures, trim, retry and a folder without git verified");
} catch (error) {
  console.error(deployment.log.slice(-6000));
  throw error;
} finally {
  await proxy?.close();
  await deployment.stop();
  await rm(root, { recursive: true, force: true });
}
