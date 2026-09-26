// Real queue overflow, production automatic reconnect and IndexedDB recovery.
// Requires Chromium, a built librepaper binary and LIBREPAPER_TEST_POSTGRES_URL.
// Only absent optional server/database configuration is skipped; setup errors fail.
import assert from "node:assert/strict";
import { build } from "vite";
import http from "node:http";
import net from "node:net";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { browser } from "../../tools/browser-driver.mjs";
import { startDeployment, until } from "../helpers/deployment.mjs";

const PEER_QUEUE = 4;
const noise = process.env.LIBREPAPER_TEST_VERBOSE ? console.error : () => {};
const deployment = await startDeployment({
  label: "slow_subscriber",
  advanced: `session_peer_queue: ${PEER_QUEUE}\n`,
});
if (!deployment || deployment.unavailable) {
  console.log(`slow-subscriber-recovery: ${deployment?.unavailable || "no librepaper binary; run cargo build"}; skipping`);
  process.exit(0);
}

// HTTP stays available while WebSocket reads stall or new connections are
// refused. Thus an offline reload can fetch its shell and metadata, but its
// document text can only come from the browser's real IndexedDB database.
async function relay(targetPort, fixtureRoot) {
  const sockets = new Set();
  let stalled = false;
  let blocked = false;
  const server = http.createServer(async (request, response) => {
    const path = new URL(request.url, "http://relay").pathname;
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
    if (blocked) { client.end("HTTP/1.1 503 Service Unavailable\r\nConnection: close\r\n\r\n"); return; }
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
      upstream.on("data", (chunk) => { if (!client.write(chunk)) upstream.pause(); });
      client.on("drain", () => { if (!stalled) upstream.resume(); });
      if (stalled) upstream.pause();
    });
  });
  await new Promise((done, fail) => { server.once("error", fail); server.listen(0, "127.0.0.1", done); });
  return {
    origin: `http://127.0.0.1:${server.address().port}`,
    stall() { stalled = true; for (const { upstream } of sockets) upstream.pause(); },
    resume() { stalled = false; for (const { upstream } of sockets) upstream.resume(); },
    block() { blocked = true; },
    allow() { blocked = false; },
    disconnect() { for (const { client, upstream } of sockets) { client.destroy(); upstream.destroy(); } },
    async close() {
      for (const { client, upstream } of sockets) { client.destroy(); upstream.destroy(); }
      server.closeAllConnections();
      await new Promise((done) => server.close(done));
    },
  };
}

async function freePort() {
  const socket = net.createServer();
  await new Promise((done, fail) => { socket.once("error", fail); socket.listen(0, "127.0.0.1", done); });
  const port = socket.address().port;
  await new Promise((done) => socket.close(done));
  return port;
}

let directory;
const relays = [];
const clients = [];
async function editor(origin, slug, name) {
  const profile = await mkdtemp(join(directory, `${name}-`));
  const tab = await browser("chromium", profile, await freePort());
  clients.push(tab);
  const separator = deployment.cookie.indexOf("=");
  await tab.setCookie(deployment.cookie.slice(0, separator), deployment.cookie.slice(separator + 1), origin);
  const url = `${origin}/recovery?slug=${encodeURIComponent(slug)}`;
  const snapshot = async () => {
    const result = await tab.evaluate("window.recovery?.snapshot()");
    if (result) assert.deepEqual(result.errors, [], `${name} browser errors`);
    return result;
  };
  await tab.navigate(url);
  return {
    tab, snapshot,
    type: (text) => tab.evaluate(`window.recovery.type(${JSON.stringify(text)})`),
    reload: () => tab.navigate(url),
    joined: () => until(`${name} to join automatically`, async () => (await snapshot())?.joined),
    disconnected: () => until(`${name} to notice disconnection`, async () => !(await snapshot())?.joined),
    close: async () => { await tab.evaluate("window.recovery.close()"); await tab.close(); clients.splice(clients.indexOf(tab), 1); },
  };
}
const status = async () => (await (await fetch(`${deployment.base}/api/status`, { signal: AbortSignal.timeout(5000) })).json()).rooms;

try {
  directory = await mkdtemp(join(tmpdir(), "librepaper-recovery-"));
  const fixtureRoot = join(directory, "build");
  await build({
    configFile: false,
    root: resolve(import.meta.dirname, "../.."),
    build: { outDir: fixtureRoot, emptyOutDir: true,
      lib: { entry: resolve(import.meta.dirname, "../fixtures/recovery-browser-entry.js"), formats: ["es"], fileName: () => "recovery.js" } },
    logLevel: "error",
  });
  const targetPort = Number(new URL(deployment.base).port);
  const aliceRelay = await relay(targetPort, fixtureRoot);
  relays.push(aliceRelay);
  const bobRelay = await relay(targetPort, fixtureRoot);
  relays.push(bobRelay);
  const { slug } = await deployment.publish({ title: "Recovery", source: "# Recovery\n\nThe first paragraph.\n", source_format: "markdown" });
  const alice = await editor(aliceRelay.origin, slug, "Alice");
  await alice.joined();
  const bob = await editor(bobRelay.origin, slug, "Bob");
  await bob.joined();
  await until("both editors to subscribe", async () => (await status()).subscribers === 2);
  assert.match((await bob.snapshot()).text, /first paragraph/);

  const frameBytes = await alice.tab.evaluate("window.recovery.prepareFlood()");
  assert.ok(frameBytes < PEER_QUEUE * 64 * 1024, "each presence frame fits an empty outbound queue");
  // Control: a healthy reader must receive these same frames without closing.
  for (let i = 0; i < 64; i++) {
    const before = (await bob.snapshot()).presenceFrames;
    assert.equal(await alice.tab.evaluate("window.recovery.flood()"), true);
    await until("healthy Bob to receive the flood frame", async () => (await bob.snapshot()).presenceFrames > before);
  }
  assert.deepEqual((await bob.snapshot()).closes, [], "admissible frames do not close a healthy reader");
  assert.equal((await status()).subscribers, 2);
  noise(`healthy control received 64 frames of ${frameBytes} bytes`);

  bobRelay.stall();
  await alice.type("\nAlice wrote this into the stall.\n");
  let frames = 0;
  await until("backpressure to drop Bob's subscriber", async () => {
    if ((await status()).subscribers === 1) return true;
    assert.equal(await alice.tab.evaluate("window.recovery.flood()"), true);
    frames++;
    return (await status()).subscribers === 1;
  }, 20000);
  assert.ok(frames > 1, "overflow requires accumulated backpressure");
  noise(`stalled reader dropped after ${frames} frames`);
  // Deliver the queued close, but keep normal reconnect attempts offline
  // long enough to observe unsaved editing; no reconnect callback is invoked.
  bobRelay.block();
  bobRelay.resume();
  await until("the server's slow-subscriber close", async () =>
    (await bob.snapshot()).closes.some(({ reason }) => reason === "subscriber too slow; reconnect"));
  await bob.disconnected();
  await bob.type("\nBob typed this while disconnected.\n");
  assert.equal((await bob.snapshot()).pending, 1);
  assert.equal((await bob.snapshot()).local, true);
  await alice.type("\nAlice wrote this while Bob was away.\n");
  bobRelay.allow();
  await bob.joined();
  await until("both editors to converge", async () => {
    const a = await alice.snapshot();
    const b = await bob.snapshot();
    return a.text === b.text && b.text.includes("while disconnected") && b.text.includes("while Bob was away");
  });
  assert.match((await bob.snapshot()).text, /into the stall/);

  await until("previous work to become durable", async () => (await bob.snapshot()).pending === 0);
  await bob.type("\nAnd this, once he was back.\n");
  await until("new work to enter the server buffer", async () => (await status()).buffered_batches > 0);
  assert.equal((await bob.snapshot()).pending, 1, "buffered work is not saved");
  await until("new work to become durable", async () => (await bob.snapshot()).pending === 0);
  assert.equal((await status()).buffered_batches, 0);

  // An additional unsent edit must survive a real page reload. Blocking all
  // new room connections makes a broken IndexedDB adapter unable to recover
  // this text from the server and falsely pass the test.
  bobRelay.block();
  bobRelay.disconnect();
  await bob.disconnected();
  const offlineReload = "\nOnly IndexedDB can restore this unsent edit.\n";
  await bob.type(offlineReload);
  assert.equal((await bob.snapshot()).pending, 1);
  assert.ok(!(await alice.snapshot()).text.includes(offlineReload));
  await bob.reload();
  await until("IndexedDB to restore the unsent edit before reconnection", async () => (await bob.snapshot())?.text.includes(offlineReload));
  assert.equal((await bob.snapshot()).joined, false);
  assert.equal((await bob.snapshot()).pending, 1);
  bobRelay.allow();
  await bob.joined();
  await until("reloaded edits to converge and become durable", async () => {
    const a = await alice.snapshot();
    const b = await bob.snapshot();
    return a.text === b.text && b.pending === 0;
  });
  const converged = (await bob.snapshot()).text;
  await alice.close();
  await bob.close();
  await deployment.restart();
  const fresh = await editor(aliceRelay.origin, slug, "Fresh");
  await fresh.joined();
  assert.equal((await fresh.snapshot()).text, converged, "a fresh browser reads the durable document after a server restart");
  noise("automatic reconnect, unsent IndexedDB reload and server restart passed");
} catch (error) {
  if (process.env.LIBREPAPER_TEST_VERBOSE) console.error(deployment.log);
  throw error;
} finally {
  await Promise.allSettled(clients.map((tab) => tab.close()));
  await Promise.allSettled(relays.map((one) => one.close()));
  await deployment.stop();
  if (directory) await rm(directory, { recursive: true, force: true });
}
console.log("slow-subscriber-recovery: ok");
