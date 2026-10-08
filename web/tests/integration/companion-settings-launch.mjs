// Pairing approvals should open the trusted app's Companion Settings page,
// without launching a browser in this regression. All companion state, the
// fake desktop opener, and XDG directories are isolated under one temp root.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer } from "node:net";
import { createHash, randomBytes } from "node:crypto";

if (process.platform !== "linux") {
  console.log("companion-settings-launch: this fixture intercepts the Linux desktop opener; skipping");
  process.exit(0);
}

const here = dirname(fileURLToPath(import.meta.url));
const repository = resolve(here, "../../..");
const binary = resolve(process.env.LIBREPAPER_TEST_BINARY || join(repository, "target/debug/librepaper"));
if (!existsSync(binary)) {
  console.log("companion-settings-launch: no companion binary; skipping (run `cargo build`)");
  process.exit(0);
}

const root = await mkdtemp(join(tmpdir(), "librepaper-settings-launch-"));
const binDir = join(root, "bin");
const stateHome = join(root, "state");
const cacheHome = join(root, "cache");
const configHome = join(root, "config");
const dataHome = join(root, "data");
const runtimeHome = join(root, "runtime");
const openedFile = join(root, "opened-url");
await Promise.all([binDir, stateHome, cacheHome, configHome, dataHome, runtimeHome].map((path) => mkdir(path)));
await writeFile(
  join(binDir, "xdg-open"),
  '#!/bin/sh\nprintf "%s" "$1" > "$LIBREPAPER_TEST_OPENED"\n',
  { mode: 0o755 },
);

async function freePort() {
  const server = createServer();
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  const port = server.address().port;
  await new Promise((resolve) => server.close(resolve));
  return port;
}

const appPort = await freePort();
let companionPort = await freePort();
while (companionPort === appPort) companionPort = await freePort();
const appOrigin = `http://localhost:${appPort}`;
const base = `http://127.0.0.1:${companionPort}/librepaper/local`;
const env = {
  ...process.env,
  HOME: root,
  XDG_STATE_HOME: stateHome,
  XDG_CACHE_HOME: cacheHome,
  XDG_CONFIG_HOME: configHome,
  XDG_DATA_HOME: dataHome,
  XDG_RUNTIME_DIR: runtimeHome,
  PATH: `${binDir}:${process.env.PATH || ""}`,
  LIBREPAPER_LOCAL_PORT: String(companionPort),
  LIBREPAPER_SERVER: `${appOrigin}/`,
  LIBREPAPER_TEST_OPENED: openedFile,
  DISPLAY: ":librepaper-settings-test",
  DBUS_SESSION_BUS_ADDRESS: `unix:path=${join(root, "unavailable-session-bus")}`,
};
delete env.WAYLAND_DISPLAY;
delete env.LIBREPAPER_LOCAL_CODE;

const child = spawn(binary, ["start", "--foreground", "--port", String(companionPort)], {
  env,
  stdio: ["ignore", "ignore", "ignore"],
});
const childExit = new Promise((resolve) => child.once("exit", resolve));
const statePath = join(stateHome, "librepaper", "local", "control-token.json");

async function waitFor(label, check, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const value = await check();
      if (value) return value;
    } catch (error) { lastError = error; }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`${label} did not become ready${lastError ? `: ${lastError.message}` : ""}`);
}

try {
  await waitFor("companion health", async () => {
    const response = await fetch(`${base}/health`);
    return response.ok;
  });
  const credential = await waitFor("control credential", async () => {
    try { return JSON.parse(await readFile(statePath, "utf8")); } catch { return null; }
  });

  const challenge = createHash("sha256").update(randomBytes(32)).digest("hex");
  const requestId = randomBytes(24).toString("base64url");
  const returnTo = `${appOrigin}/`;
  const pairing = await fetch(`${base}/pair/request`, {
    method: "POST",
    headers: { Origin: appOrigin, "Content-Type": "application/json" },
    body: JSON.stringify({ origin: appOrigin, request: requestId, challenge, return: returnTo }),
  });
  assert.equal(pairing.status, 202, "the local app's pair request should enter the approval queue");

  // The configured server pairs without consent, and that pairing manages the companion.
  const session = await fetch(`http://127.0.0.1:${companionPort}/companion/api/session`, {
    method: "POST",
    headers: { Origin: appOrigin, "Content-Type": "application/json" },
    body: "{}",
  });
  assert.equal(session.status, 200, "the configured server gets its pairing from the session route");
  const { token: sessionToken } = await session.json();
  assert.notEqual(sessionToken, credential.token, "the session pairing is not the CLI file credential");
  const apiHeaders = { Origin: appOrigin, Authorization: `Bearer ${sessionToken}` };
  const approval = await waitFor("pending approval and Settings launch", async () => {
    let opened;
    try { opened = await readFile(openedFile, "utf8"); } catch { return null; }
    const response = await fetch("http://127.0.0.1:" + companionPort + "/companion/api/state", { headers: apiHeaders });
    if (!response.ok) return null;
    const state = await response.json();
    const request = (state.approvals || []).find((item) => item.title === "Connect LibrePaper Companion");
    return request ? { opened, request } : null;
  });

  const settingsUrl = new URL(approval.opened);
  assert.equal(settingsUrl.origin, appOrigin, "the opener targets the configured local app origin");
  assert.equal(settingsUrl.pathname, "/", "the opener targets the app root");
  const fragment = new URLSearchParams(settingsUrl.hash.slice(1));
  assert.ok(fragment.get("settings") === "local", "the opener targets Companion Settings");
  assert.equal(fragment.get("companion_control"), null, "the Settings link carries no credential");
  assert.ok(!approval.opened.includes(credential.token), "the CLI file credential never reaches the browser");
  assert.ok(fragment.get("companion_address") === `http://127.0.0.1:${companionPort}/`, "Settings receives the running companion address");
  assert.equal(credential.server, `${appOrigin}/`, "the saved settings target is the configured local app");

  const denied = await fetch(`http://127.0.0.1:${companionPort}/companion/api/approvals/${encodeURIComponent(approval.request.id)}`, {
    method: "POST",
    headers: { ...apiHeaders, "Content-Type": "application/json" },
    body: JSON.stringify({ decision: "deny" }),
  });
  assert.equal(denied.status, 200, "the test closes its pending request through the trusted Settings API");
  const deniedStatus = await waitFor("pair request denial", async () => {
    const response = await fetch(`${base}/pair/status?request=${encodeURIComponent(requestId)}`);
    return response.status === 403 ? response.status : null;
  });
  assert.equal(deniedStatus, 403, "the denied request is cleanly resolved");

  console.log("companion-settings-launch: local app Origin opens Settings with matching companion credentials; approval cleanup passed");
} finally {
  if (child.exitCode === null && child.signalCode === null) child.kill("SIGTERM");
  let timer;
  await Promise.race([childExit, new Promise((resolve) => { timer = setTimeout(resolve, 5000); })]);
  clearTimeout(timer);
  if (child.exitCode === null && child.signalCode === null) {
    child.kill("SIGKILL");
    await childExit;
  }
  await rm(root, { recursive: true, force: true });
}
