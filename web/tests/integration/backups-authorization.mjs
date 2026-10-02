// A real deployment approves a device grant requested by a real, isolated
// companion. The browser session can approve a user code, but never receives
// the device credential that the companion stores for scheduled backups.
//
// Requires a built librepaper binary and the optional PostgreSQL test URL.
// Only those established missing prerequisites are skipped.
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createServer } from "node:net";
import { deploymentBinary, startDeployment } from "../helpers/deployment.mjs";

const exec = promisify(execFile);
const binary = deploymentBinary();
if (!existsSync(binary)) {
  console.log("backups-authorization: no companion binary; skipping (run `cargo build`)");
  process.exit(0);
}

const deployment = await startDeployment({ label: "backups_authorization" });
if (!deployment || deployment.unavailable) {
  console.log(`backups-authorization: ${deployment?.unavailable || "no librepaper binary; run cargo build"}; skipping`);
  process.exit(0);
}

const temporary = await mkdtemp(join(tmpdir(), "librepaper-backups-authorization-"));
const stateHome = join(temporary, "state");
const origin = deployment.base;
const pairingToken = "integration-browser-pairing-token";
const accountId = deployment.accountId;
const env = {
  ...process.env,
  HOME: temporary,
  XDG_STATE_HOME: stateHome,
  XDG_CACHE_HOME: join(temporary, "cache"),
  XDG_CONFIG_HOME: join(temporary, "config"),
};
delete env.LIBREPAPER_LOCAL_PORT;
delete env.LIBREPAPER_LOCAL_CODE;

const listener = createServer();
await new Promise((done, fail) => {
  listener.once("error", fail);
  listener.listen(0, "127.0.0.1", done);
});
const port = listener.address().port;
await new Promise((done, fail) => listener.close((error) => error ? fail(error) : done()));

const cli = (...args) => exec(binary, ["local", ...args], { env, timeout: 20000 });
const localBase = `http://127.0.0.1:${port}/librepaper/local`;
const api = (path, body, token = pairingToken) => fetch(`${localBase}${path}`, {
  method: "POST",
  headers: {
    Origin: origin,
    Authorization: `Bearer ${token}`,
    "Content-Type": "application/json",
  },
  body: JSON.stringify(body),
});

let companionStarted = false;
try {
  // Pairing state mirrors the companion's persisted format. It contains only
  // a hash of this test token, just like a consented browser pairing.
  const localState = join(stateHome, "librepaper", "local");
  await mkdir(localState, { recursive: true });
  await writeFile(join(localState, "pairings.json"), JSON.stringify({
    [origin]: {
      token_sha256: createHash("sha256").update(pairingToken).digest("hex"),
      origin,
      created: Math.floor(Date.now() / 1000),
      expires: Math.floor(Date.now() / 1000) + 3600,
      label: "backup authorization integration",
    },
  }), { mode: 0o600 });

  await cli("start", "--port", String(port));
  companionStarted = true;
  const health = await fetch(`${localBase}/health`);
  assert.equal(health.status, 200, "the isolated companion is running");

  const unauthenticated = await api("/backups/authorize", { account_id: accountId }, "");
  assert.equal(unauthenticated.status, 401, "starting authorization requires the paired browser token");
  const invalidAccount = await api("/backups/authorize", { account_id: "bad account id" });
  assert.equal(invalidAccount.status, 400, "invalid account identifiers cannot start device grants");

  const requested = await api("/backups/authorize", { account_id: accountId });
  assert.equal(requested.status, 200, `authorization request: ${await requested.clone().text()}`);
  const request = await requested.json();
  assert.match(request.user_code, /^[A-Z0-9-]+$/i, "the browser receives a human-approvable code");
  assert.equal(typeof request.authorization_id, "string");
  assert.ok(request.authorization_id.length > 0);
  assert.ok(!JSON.stringify(request).includes("device_code"), "the device secret never reaches the browser");
  assert.ok(!JSON.stringify(request).includes("token"), "no device token reaches the browser");

  const approved = await fetch(`${origin}/api/auth/device/approve`, {
    method: "POST",
    headers: { Cookie: deployment.cookie, Origin: origin, "Content-Type": "application/json" },
    body: JSON.stringify({ user_code: request.user_code }),
  });
  assert.equal(approved.status, 200, `signed-in approval: ${await approved.clone().text()}`);
  assert.deepEqual(await approved.json(), { approved: true });

  const completed = await api("/backups/authorize/complete", {
    account_id: accountId,
    authorization_id: request.authorization_id,
  });
  assert.equal(completed.status, 200, `authorization completion: ${await completed.clone().text()}`);
  const status = await completed.json();
  assert.equal(status.needs_login, false, "the approved device grant is stored for this account");

  // Restart the actual process so persistence is exercised across a fresh
  // service instance, with no CLI token cache in this XDG state directory.
  await cli("stop");
  companionStarted = false;
  await cli("start", "--port", String(port));
  companionStarted = true;
  const persisted = await fetch(`${localBase}/backups?account=${encodeURIComponent(accountId)}`, {
    headers: { Origin: origin, Authorization: `Bearer ${pairingToken}` },
  });
  assert.equal(persisted.status, 200, `backup status after restart: ${await persisted.clone().text()}`);
  assert.equal((await persisted.json()).needs_login, false, "the backup remains authorized after restart");
  assert.equal(await readFile(join(stateHome, "librepaper", "tokens.json"), "utf8").then(() => true, () => false), false,
    "device authorization is not stored as a CLI login token");

  console.log("backups-authorization: real signed-in approval, companion credential isolation, and restart persistence passed");
} finally {
  if (companionStarted) await cli("stop").catch(() => {});
  await deployment.stop();
  await rm(temporary, { recursive: true, force: true });
}
