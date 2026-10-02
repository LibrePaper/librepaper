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
import { mkdtemp, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
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
const backupDestination = join(temporary, "backup-destination");
const env = {
  ...process.env,
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
const api = (path, body, token = pairingToken, method = "POST") => fetch(`${localBase}${path}`, {
  method,
  headers: {
    Origin: origin,
    Authorization: `Bearer ${token}`,
    "Content-Type": "application/json",
  },
  body: JSON.stringify(body),
});

let companionStarted = false;
try {
  await deployment.publish({
    title: "Authorization Backup Project",
    source: "# Authorization Backup Project\n\nThis project must appear in the real backup archive.\n",
    source_format: "markdown",
  });

  // Pairing state mirrors the companion's persisted format. It contains only
  // a hash of this test token, just like a consented browser pairing.
  const localState = join(stateHome, "librepaper", "local");
  await mkdir(localState, { recursive: true });
  await mkdir(backupDestination, { recursive: true });
  await writeFile(join(localState, "pairings.json"), JSON.stringify({
    [origin]: {
      token_sha256: createHash("sha256").update(pairingToken).digest("hex"),
      origin,
      created: Math.floor(Date.now() / 1000),
      expires: Math.floor(Date.now() / 1000) + 3600,
      label: "backup authorization integration",
    },
  }), { mode: 0o600 });
  // The native folder chooser cannot be driven in this headless test. Seed
  // the same persisted setting the chooser creates, disabled until the new
  // device credential has been approved.
  await writeFile(join(localState, "backups.json"), JSON.stringify([{
    origin,
    account_id: accountId,
    enabled: false,
    frequency_minutes: 5,
    destination: backupDestination,
    last_attempt: null,
    last_success: null,
    error: null,
    projects: 0,
    updated: 0,
    run_generation: 0,
    revision: 0,
  }]), { mode: 0o600 });

  await cli("start", "--port", String(port));
  companionStarted = true;
  const health = await fetch(`${localBase}/health`);
  assert.equal(health.status, 200, "the isolated companion is running");
  assert.equal(existsSync(join(stateHome, "librepaper", "tokens.json")), false,
    "the companion starts without a cached CLI token");

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
    headers: {
      Cookie: deployment.cookie,
      Origin: origin,
      "Content-Type": "application/json",
      "X-LibrePaper-Client": "shell",
    },
    body: JSON.stringify({ user_code: request.user_code }),
  });
  assert.equal(approved.status, 200, `signed-in approval: ${await approved.clone().text()}`);
  assert.deepEqual(await approved.json(), { approved: true });

  const otherAccountId = "00000000-0000-7000-8000-000000000001";
  const mismatched = await api("/backups/authorize/complete", {
    account_id: otherAccountId,
    authorization_id: request.authorization_id,
  });
  assert.equal(mismatched.status, 400, "an authorization cannot be completed for a different account");

  const completed = await api("/backups/authorize/complete", {
    account_id: accountId,
    authorization_id: request.authorization_id,
  });
  assert.equal(completed.status, 200, `authorization completion: ${await completed.clone().text()}`);
  const status = await completed.json();
  assert.equal(status.needs_login, false, "the approved device grant is stored for this account");
  assert.ok(!JSON.stringify(status).includes("token"), "completion returns status, never the device token");
  const tokenPath = join(stateHome, "librepaper", "tokens.json");
  const tokens = JSON.parse(await readFile(tokenPath, "utf8"));
  assert.equal(typeof tokens[origin], "string", "the device token is cached under this server origin");
  assert.ok(tokens[origin].length > 0);
  assert.ok(!JSON.stringify(request).includes(tokens[origin]), "the device token was never returned to the browser");
  assert.ok(!JSON.stringify(status).includes(tokens[origin]), "the completion response does not expose the cached token");

  const replay = await api("/backups/authorize/complete", {
    account_id: accountId,
    authorization_id: request.authorization_id,
  });
  assert.equal(replay.status, 400, "a completed authorization cannot be replayed");

  // Restart the actual process so persistence is exercised across a fresh
  // service instance. The token came from browser approval, not a CLI login.
  await cli("stop");
  companionStarted = false;
  await cli("start", "--port", String(port));
  companionStarted = true;
  const statusAfterRestart = await fetch(`${localBase}/backups?account=${encodeURIComponent(accountId)}`, {
    headers: { Origin: origin, Authorization: `Bearer ${pairingToken}` },
  });
  assert.equal(statusAfterRestart.status, 200, `backup status after restart: ${await statusAfterRestart.clone().text()}`);
  assert.equal((await statusAfterRestart.json()).needs_login, false, "the backup remains authorized after restart");
  assert.deepEqual(JSON.parse(await readFile(tokenPath, "utf8")), tokens,
    "the approved token remains cached under the same server origin after restart");

  const enabled = await api("/backups", {
    account_id: accountId,
    enabled: true,
    frequency_minutes: 5,
  }, pairingToken, "PUT");
  assert.equal(enabled.status, 200, `enable scheduled backup: ${await enabled.clone().text()}`);

  const deadline = Date.now() + 30000;
  let finished;
  while (Date.now() < deadline) {
    const response = await fetch(`${localBase}/backups?account=${encodeURIComponent(accountId)}`, {
      headers: { Origin: origin, Authorization: `Bearer ${pairingToken}` },
    });
    assert.equal(response.status, 200, `scheduled backup status: ${await response.clone().text()}`);
    finished = await response.json();
    if (finished.last_success && finished.projects === 1 && !finished.running) break;
    if (finished.error) throw new Error(`scheduled backup failed: ${finished.error}`);
    await new Promise((done) => setTimeout(done, 100));
  }
  assert.ok(finished?.last_success && finished.projects === 1 && !finished.running,
    "the scheduler writes a real project backup archive");

  const normalizedOrigin = `${new URL(origin).protocol}//${new URL(origin).hostname.toLowerCase()}:${new URL(origin).port}`;
  const namespace = createHash("sha256").update(`${normalizedOrigin}\0${accountId}`).digest("hex");
  const archiveDirectory = join(backupDestination, "librepaper-backups", namespace);
  const archiveNames = await readdir(archiveDirectory);
  const archiveName = archiveNames.find((name) => name.endsWith(".zip"));
  assert.ok(archiveName, "a ZIP archive is present in the scoped backup directory");
  const archive = await exec("unzip", ["-p", join(archiveDirectory, archiveName), "main.md"]);
  assert.match(archive.stdout, /This project must appear in the real backup archive\./,
    "the ZIP contains the deployed project's source");

  console.log("backups-authorization: signed-in device approval, account and replay rejection, credential isolation, restart persistence, and real ZIP backup passed");
} finally {
  if (companionStarted) await cli("stop").catch(() => {});
  await deployment.stop();
  await rm(temporary, { recursive: true, force: true });
}
