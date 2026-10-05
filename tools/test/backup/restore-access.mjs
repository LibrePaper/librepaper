#!/usr/bin/env node
// Start a restored deployment with its recovered signing key and verify that
// a cookie issued by the source deployment still authenticates to its account.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { spawn } from "node:child_process";
import { createServer } from "node:net";

const [binary, configPath, accountId, cookiePath] = process.argv.slice(2);
if (!binary || !configPath || !accountId || !cookiePath) {
	console.error("usage: restore-access.mjs <binary> <config> <account-id> <source-cookie>");
  process.exit(2);
}

async function freePort() {
  const listener = createServer();
  await new Promise((resolve, reject) => {
    listener.once("error", reject);
    listener.listen(0, "127.0.0.1", resolve);
  });
  const { port } = listener.address();
  await new Promise((resolve, reject) => listener.close((error) => error ? reject(error) : resolve()));
  return port;
}

const port = await freePort();
const base = `http://127.0.0.1:${port}`;
const child = spawn(binary, ["admin", "serve", "--config", configPath], {
  stdio: ["ignore", "pipe", "pipe"],
  env: { ...process.env, TEST_RESTORE_ADDRESS: `127.0.0.1:${port}` },
});
let log = "";
child.stdout.on("data", (bytes) => { log += bytes; });
child.stderr.on("data", (bytes) => { log += bytes; });

async function stop() {
  if (child.exitCode !== null || child.signalCode !== null) return;
  child.kill("SIGTERM");
  const timer = setTimeout(() => child.kill("SIGKILL"), 5000);
  await new Promise((resolve) => child.once("close", resolve));
  clearTimeout(timer);
}

try {
  const deadline = Date.now() + 30000;
  let ready = false;
  while (Date.now() < deadline) {
    if (child.exitCode !== null || child.signalCode !== null) {
      throw new Error(`restored server exited during startup\n${log}`);
    }
    try {
      const response = await fetch(`${base}/api/config`, { signal: AbortSignal.timeout(1000) });
      if (response.ok) {
        ready = true;
        break;
      }
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  assert.ok(ready, `restored server did not become ready\n${log}`);

  const cookie = readFileSync(cookiePath, "utf8").trim();
  const response = await fetch(`${base}/api/me`, {
    headers: { cookie, "x-librepaper-client": "1" },
    signal: AbortSignal.timeout(5000),
  });
  if (!response.ok) {
    throw new Error(`/api/me returned ${response.status}: ${await response.text()}`);
  }
  const identity = await response.json();
  assert.equal(identity.id, accountId, "the recovered signing key authenticates the original account");
  console.log("restored app accepted the original session cookie for the recovered account");
} finally {
  await stop();
}
