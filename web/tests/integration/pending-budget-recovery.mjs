// SPEC-frugal §2, end to end: several documents, a stalled write path, the
// deployment-wide pending-source bound reached, and every edit eventually
// durable without anybody typing again.
//
// Requires Chromium, a built librepaper binary and LIBREPAPER_TEST_POSTGRES_URL.
// Only absent optional server/database configuration is skipped; setup errors
// fail.
//
// **How the slowdown is produced.** Not by stalling the TCP link to
// PostgreSQL, which was the first idea and is the wrong one: every
// `doc-update` frame calls `verify_writer` on the lease connection before it
// reaches the sequencer, so a link-level stall blocks the typing path itself
// and the buffer never grows. What this holds instead is
// `LOCK TABLE document_updates IN EXCLUSIVE MODE` in a session of its own,
// which blocks the INSERT a flush makes and leaves every read, the lease
// check and the relay path running. That is the shape the bound is actually
// for: writes are slow, typing is not, and the unsaved bytes pile up.
//
// **What this asserts that the Rust tests cannot.** The Rust cases assert the
// accounting invariants directly against a fake catalogue. This one asserts
// that a real browser, holding a real refused edit, gets that edit saved
// without another keystroke -- the half of the contract that lives in
// `project-session.js` and in the protocol between them.
import assert from "node:assert/strict";
import { build } from "vite";
import { spawn } from "node:child_process";
import http from "node:http";
import net from "node:net";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { browser } from "../../tools/browser-driver.mjs";
import { psqlCommand } from "../../tools/postgres-test.mjs";
import { startDeployment, until } from "../helpers/deployment.mjs";

// The smallest pending ceilings the server will accept. `pending_mb` may not
// go below one document's own buffer allowance (4 MiB plus its framing), and
// `pending_scratch_mb` may not go below what one maximum-size row costs to
// write -- a deployment that could not write its largest row would admit work
// it could never persist, which is the thing the two-pool design exists to
// prevent. So this is pressure reached as cheaply as the server permits.
const PENDING_MB = 5;
const SCRATCH_MB = 64;
// One typed chunk. Random text rather than a repeated character: Loro's
// run-length encoding would collapse the latter and the buffer would barely
// grow.
const CHUNK = 96 * 1024;
const DOCUMENTS = 3;

const noise = process.env.LIBREPAPER_TEST_VERBOSE ? console.error : () => {};
const deployment = await startDeployment({
  label: "pending_budget",
  advanced: `pending_mb: ${PENDING_MB}\npending_scratch_mb: ${SCRATCH_MB}\n`,
});
if (!deployment || deployment.unavailable) {
  console.log(`pending-budget-recovery: ${deployment?.unavailable || "no librepaper binary; run cargo build"}; skipping`);
  process.exit(0);
}

/// The name the locking session announces itself under, so it can be found
/// and ended from another session.
const STALL = "librepaper_pending_stall";

function psql(sql, { collect = false } = {}) {
  const { program, prefix } = psqlCommand();
  const url = new URL(deployment.postgres.psqlUrl);
  url.searchParams.set("application_name", STALL);
  return spawn(program, [
    ...prefix, url.toString(), "-v", "ON_ERROR_STOP=1", "-tA", "-c", sql,
  ], { stdio: ["ignore", collect ? "pipe" : "ignore", "pipe"] });
}

async function psqlOnce(sql) {
  const child = psql(sql, { collect: true });
  let out = "";
  child.stdout.on("data", (bytes) => { out += bytes; });
  const code = await new Promise((done) => child.once("close", done));
  assert.equal(code, 0, `psql failed: ${sql}`);
  return out.trim();
}

/// A session holding `document_updates` locked against writes, until it is
/// released.
///
/// `EXCLUSIVE` and not `ACCESS EXCLUSIVE`: it blocks the `INSERT` a flush
/// makes and leaves `SELECT` alone, so reads, joins and the writer-lease
/// check all keep working. That is the shape of a database slowdown this
/// bound is for. `pg_sleep` rather than reading stdin, so the transaction is
/// alive the moment the command returns.
function stallWrites() {
  const held = psql("BEGIN; LOCK TABLE document_updates IN EXCLUSIVE MODE; SELECT pg_sleep(600);");
  let complaint = "";
  held.stderr.on("data", (bytes) => { complaint += bytes; });
  return {
    async release() {
      // Killing the local client is not enough to end the session: when psql
      // runs inside a container, the process here is `docker exec` and the
      // backend outlives it holding the lock. So the backend is ended by
      // name, from a session of its own, and the caller waits for it to be
      // gone rather than for the client process to exit.
      await psqlOnce(
        `SELECT pg_terminate_backend(pid) FROM pg_stat_activity
         WHERE application_name = '${STALL}' AND pid <> pg_backend_pid()`,
      );
      held.kill("SIGKILL");
      await until("the stalling session to end", async () =>
        (await psqlOnce(
          `SELECT count(*) FROM pg_stat_activity
           WHERE application_name = '${STALL}' AND pid <> pg_backend_pid()`,
        )) === "0");
      if (complaint.trim()) noise(`lock session said: ${complaint.trim()}`);
    },
  };
}

const status = async () =>
  (await (await fetch(`${deployment.base}/api/status`, { signal: AbortSignal.timeout(5000) })).json());

async function freePort() {
  const socket = net.createServer();
  await new Promise((done, fail) => { socket.once("error", fail); socket.listen(0, "127.0.0.1", done); });
  const port = socket.address().port;
  await new Promise((done) => socket.close(done));
  return port;
}

/// The deployment, plus the fixture page, on one origin.
///
/// The fixture is a page the server does not serve, and it has to be
/// same-origin with the API it fetches and the socket it opens or none of the
/// session cookie, the `fetch` or the WebSocket would work. So everything
/// but the two fixture paths is passed straight through, upgrades included.
/// Nothing here interferes with the traffic: the slowdown this test is about
/// is on the database side, not the transport.
async function origin(targetPort, fixtureRoot) {
  const sockets = new Set();
  const server = http.createServer(async (request, response) => {
    const path = new URL(request.url, "http://origin").pathname;
    if (path === "/recovery") {
      response.setHeader("content-type", "text/html");
      response.end('<!doctype html><script type="module" src="/recovery.js"></script>');
      return;
    }
    if (path === "/recovery.js") {
      response.setHeader("content-type", "text/javascript");
      try { response.end(await readFile(join(fixtureRoot, "recovery.js"))); }
      catch { response.writeHead(500); response.end(); }
      return;
    }
    const upstream = http.request({ hostname: "127.0.0.1", port: targetPort,
      path: request.url, method: request.method, headers: request.headers }, (reply) => {
      response.writeHead(reply.statusCode, reply.headers);
      reply.pipe(response);
    });
    upstream.on("error", () => { response.writeHead(502); response.end(); });
    request.pipe(upstream);
  });
  server.on("upgrade", (request, client, head) => {
    const upstream = net.connect(targetPort, "127.0.0.1");
    const pair = { client, upstream };
    sockets.add(pair);
    const close = () => { sockets.delete(pair); client.destroy(); upstream.destroy(); };
    client.on("error", close).on("close", close);
    upstream.on("error", close).on("close", close);
    upstream.once("connect", () => {
      const headers = request.rawHeaders.reduce((lines, value, index, all) =>
        index % 2 ? lines : `${lines}${value}: ${all[index + 1]}\r\n`, "");
      upstream.write(`${request.method} ${request.url} HTTP/${request.httpVersion}\r\n${headers}\r\n`);
      if (head.length) upstream.write(head);
      client.pipe(upstream);
      upstream.pipe(client);
    });
  });
  await new Promise((done, fail) => { server.once("error", fail); server.listen(0, "127.0.0.1", done); });
  return {
    url: `http://127.0.0.1:${server.address().port}`,
    async close() {
      for (const { client, upstream } of sockets) { client.destroy(); upstream.destroy(); }
      server.closeAllConnections();
      await new Promise((done) => server.close(done));
    },
  };
}

let directory;
let pages;
const clients = [];
async function editor(slug, name) {
  const profile = await mkdtemp(join(directory, `${name}-`));
  const tab = await browser("chromium", profile, await freePort());
  clients.push(tab);
  const separator = deployment.cookie.indexOf("=");
  await tab.setCookie(deployment.cookie.slice(0, separator), deployment.cookie.slice(separator + 1), pages.url);
  const url = `${pages.url}/recovery?slug=${encodeURIComponent(slug)}`;
  const snapshot = async () => {
    const result = await tab.evaluate("window.recovery?.snapshot()");
    if (result) assert.deepEqual(result.errors, [], `${name} browser errors`);
    return result;
  };
  await tab.navigate(url);
  return {
    name, tab, snapshot,
    type: (text) => tab.evaluate(`window.recovery.type(${JSON.stringify(text)})`),
    joined: () => until(`${name} to join automatically`, async () => (await snapshot())?.joined),
    close: async () => { await tab.evaluate("window.recovery.close()"); await tab.close(); clients.splice(clients.indexOf(tab), 1); },
  };
}

/// A chunk of text no compressor will collapse, so typing it really does cost
/// the buffer what it appears to.
function filler(bytes) {
  const alphabet = "abcdefghijklmnopqrstuvwxyz0123456789 ";
  let out = "";
  while (out.length < bytes) out += alphabet[Math.floor(Math.random() * alphabet.length)];
  return `${out}\n`;
}

let stall;
try {
  directory = await mkdtemp(join(tmpdir(), "librepaper-pending-"));
  const fixtureRoot = join(directory, "build");
  await build({
    configFile: false,
    root: resolve(import.meta.dirname, "../.."),
    build: { outDir: fixtureRoot, emptyOutDir: true,
      lib: { entry: resolve(import.meta.dirname, "../fixtures/recovery-browser-entry.js"), formats: ["es"], fileName: () => "recovery.js" } },
    logLevel: "error",
  });
  pages = await origin(Number(new URL(deployment.base).port), fixtureRoot);

  // 0. Three documents, three editors.
  const slugs = [];
  for (let index = 0; index < DOCUMENTS; index++) {
    const { slug } = await deployment.publish({
      title: `Pending ${index}`,
      source: `# Pending ${index}\n\nThe first paragraph.\n`,
      source_format: "markdown",
    });
    slugs.push(slug);
  }
  const editors = [];
  for (const [index, slug] of slugs.entries()) {
    const one = await editor(slug, `Editor${index}`);
    await one.joined();
    editors.push(one);
  }

  const opening = await status();
  assert.equal(opening.pending_budget.retained_limit_bytes, PENDING_MB * 1024 * 1024,
    "the deployment is not running the ceilings this test configured");
  assert.equal(opening.pending_budget.scratch_limit_bytes, SCRATCH_MB * 1024 * 1024);
  assert.equal(opening.pending_budget.refused_retained, 0, "nothing has been refused yet");

  // 1. The write path stops answering. Typing does not.
  stall = stallWrites();
  // The lock is taken asynchronously by another process; wait until a flush
  // actually blocks on it, which is visible as unsaved work that stops
  // draining.
  await editors[0].type(filler(CHUNK));
  await until("the write path to stall", async () => {
    const { pending_budget } = await status();
    return pending_budget.retained_reserved_bytes > 0;
  });
  noise("the write path is stalled");

  // 2. Everyone types until the shared pool refuses somebody. Each document
  //    stays well inside its own 4 MiB allowance, so a refusal here can only
  //    be the deployment's.
  const typed = new Map(editors.map((one) => [one.name, ""]));
  let refusedBy = null;
  await until("the deployment pending pool to refuse an update", async () => {
    for (const one of editors) {
      const chunk = filler(CHUNK);
      typed.set(one.name, typed.get(one.name) + chunk);
      await one.type(chunk);
      const refusals = (await one.snapshot()).refusals;
      if (refusals.length) {
        refusedBy = one;
        return true;
      }
    }
    return false;
  }, 120000);

  const pressed = (await status()).pending_budget;
  assert.ok(pressed.refused_retained > 0, "the accounting never reached its admission boundary");
  assert.ok(pressed.retained_reserved_bytes <= pressed.retained_limit_bytes,
    `${pressed.retained_reserved_bytes} bytes reserved against a limit of ${pressed.retained_limit_bytes}`);
  const refusals = (await refusedBy.snapshot()).refusals;
  assert.ok(refusals.includes("pending_budget"),
    `the refusal should name the deployment bound, not ${JSON.stringify(refusals)}`);
  noise(`refused after ${pressed.retained_reserved_bytes} unsaved bytes across ${DOCUMENTS} documents`);

  // 3. Every client keeps its work and reads as unsaved.
  for (const one of editors) {
    const snapshot = await one.snapshot();
    assert.equal(snapshot.pending, 1, `${one.name} should still be waiting on durability`);
    assert.ok(snapshot.text.includes(typed.get(one.name).trim().slice(-64)),
      `${one.name} lost the end of what it typed`);
    assert.ok(snapshot.joined, `${one.name} was disconnected, which back pressure must not do`);
  }

  // 4. Nobody types again from here. This is the whole point: recovery must
  //    not need another keystroke.
  await stall.release();
  stall = null;

  // 5. Everything converges and becomes durable.
  await until("every editor's work to become durable with nobody typing", async () => {
    const waiting = [];
    for (const one of editors) {
      const snapshot = await one.snapshot();
      if (snapshot.pending !== 0) waiting.push(`${one.name}(joined=${snapshot.joined},refusals=${snapshot.refusals.length})`);
    }
    if (!waiting.length) return true;
    const { pending_budget } = await status();
    noise(`waiting on ${waiting.join(" ")} retained=${pending_budget.retained_reserved_bytes} scratch=${pending_budget.scratch_reserved_bytes} refused=${pending_budget.refused_retained}/${pending_budget.refused_scratch}`);
    return false;
  }, 180000);

  const drained = (await status()).pending_budget;
  assert.equal(drained.retained_reserved_bytes, 0, "the pool was not given back as the rows were written");
  assert.equal(drained.scratch_reserved_bytes, 0, "a write left its scratch behind");
  for (const one of editors) {
    const snapshot = await one.snapshot();
    assert.ok(snapshot.text.includes(typed.get(one.name).trim().slice(-64)),
      `${one.name}'s work did not survive to durability`);
  }
  noise("every document drained after the pressure cleared");

  const expected = new Map();
  for (const one of editors) expected.set(one.name, (await one.snapshot()).text);
  for (const one of editors) await one.close();

  // 6. And a fresh client, after a restart, reads what was persisted --
  //    rather than what some surviving process happened to hold in memory.
  await deployment.restart();
  for (const [index, slug] of slugs.entries()) {
    const fresh = await editor(slug, `Fresh${index}`);
    await fresh.joined();
    await until(`Fresh${index} to receive the persisted document`, async () =>
      (await fresh.snapshot()).text === expected.get(`Editor${index}`));
    await fresh.close();
  }
  noise("a fresh browser after a restart reads the persisted result");
} catch (error) {
  if (process.env.LIBREPAPER_TEST_VERBOSE) console.error(deployment.log);
  throw error;
} finally {
  if (stall) await stall.release();
  await Promise.allSettled(clients.map((tab) => tab.close()));
  if (pages) await pages.close();
  await deployment.stop();
  if (directory) await rm(directory, { recursive: true, force: true });
}
console.log("pending-budget-recovery: ok");
