// Real detached local service and isolated state, driven the way the desktop
// app drives it: `local endpoint`, `local stop`, and the credential and origin
// headers its shell adds. Never modifies the user's installation, tools, or
// documents.
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { existsSync } from "node:fs";
import { mkdtemp, mkdir, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";
import { createServer } from "node:net";
const exec = promisify(execFile);
const root = await mkdtemp(join(tmpdir(), "librepaper-lifecycle-"));
const repository = resolve(import.meta.dirname, "../../..");
const binary = resolve(process.env.LIBREPAPER_TEST_BINARY || join(repository, "target/debug/librepaper"));
if (!existsSync(binary)) {
  console.log("local-service: no librepaper binary; skipping (run `cargo build`)");
  process.exit(0);
}
const tools = join(root, "bin");
await mkdir(tools);
const env = { ...process.env, HOME: root, XDG_STATE_HOME: join(root, "state"), XDG_CACHE_HOME: join(root, "cache"), XDG_CONFIG_HOME: join(root, "config"), PATH: `${tools}:${process.env.PATH}`, LIBREPAPER_SERVER: "https://app.example/" };
delete env.DISPLAY;
delete env.WAYLAND_DISPLAY;
const cli = (...args) => exec(binary, args, { env, timeout: 20000 });
const local = (...args) => exec(binary, ["local", ...args], { env, timeout: 20000 });
const listener = createServer();
await new Promise((resolve) => listener.listen(0, "127.0.0.1", resolve));
const port = listener.address().port;
await new Promise((resolve) => listener.close(resolve));
env.LIBREPAPER_LOCAL_PORT = String(port);
const endpoint = async () => JSON.parse((await local("endpoint")).stdout.trim().split("\n").at(-1));
const site = "https://papers.example";
const state = async () => JSON.parse(await readFile(join(env.XDG_STATE_HOME, "librepaper/local/service.json"), "utf8"));
try {
  // Stopping with nothing running is a success, because the desktop app
  // stops the service on every exit.
  await local("stop");
  await assert.rejects(local("status"), "status reports that nothing is running");

  // Two cold starts that race share one service.
  const [raceA, raceB] = await Promise.all([endpoint(), endpoint()]);
  assert.equal(raceA.instance, raceB.instance, "racing endpoint calls reuse one service");
  const first = await state();
  assert.equal(raceA.instance, first.instance);
  assert.equal((await state()).pid, first.pid);
  await local("start");
  assert.equal((await state()).pid, first.pid, "an explicit start reuses the running service");
  await local("status");

  for (const removed of [["start"], ["stop"], ["status"], ["install-desktop"], ["--at-login"], ["local", "approve", "123456"], ["local", "tray"], ["local", "open", "librepaper://open?url=https%3A%2F%2Fapp.example%2F"]]) {
    await assert.rejects(cli(...removed), `${removed.join(" ")} is gone`);
  }
  await assert.rejects(cli(), "a bare invocation prints help and launches nothing");
  assert.equal((await state()).pid, first.pid);

  // The endpoint command reports the running service and its credential.
  const found = await endpoint();
  assert.equal(found.address, `http://127.0.0.1:${port}/`);
  assert.equal(found.instance, first.instance);
  assert.ok(found.token, "the endpoint carries the local credential");
  assert.deepEqual(found.protocol, [2]);
  const base = `${found.address}librepaper/local`;
  const manage = `${found.address}companion/api`;
  const shell = { Authorization: `Bearer ${found.token}`, "X-LibrePaper-Origin": site };

  assert.equal((await fetch(`${base}/health`)).status, 200, "health needs no credential");
  assert.equal((await fetch(`${base}/capabilities`, { headers: { "X-LibrePaper-Origin": site } })).status, 401, "a feature route needs the credential");
  assert.equal((await fetch(`${base}/capabilities`, { headers: { Authorization: shell.Authorization } })).status, 403, "a feature route needs the page origin");
  assert.equal((await fetch(`${base}/capabilities`, { headers: { ...shell, Authorization: "Bearer wrong" } })).status, 401);
  assert.equal((await fetch(`${base}/capabilities`, { headers: shell })).status, 200, "the shell's headers are accepted");
  assert.equal((await fetch(`${base}/capabilities`, { headers: { ...shell, Origin: site } })).status, 403, "a request with a browser Origin is refused");
  assert.equal((await fetch(`${base}/capabilities`, { headers: { ...shell, "Sec-Fetch-Site": "cross-site" } })).status, 403, "a request with Sec-Fetch-Site is refused");
  assert.equal((await fetch(`${manage}/state`, { headers: shell })).status, 403, "management is refused for a site other than the configured server");
  assert.equal((await fetch(`${manage}/state`, { headers: { ...shell, "X-LibrePaper-Origin": "https://app.example" } })).status, 200, "management is allowed for the configured server");
  for (const gone of ["pair/request", "connect/claim", "quit", "startup", "approve"]) {
    assert.equal((await fetch(`${base}/${gone}`, { method: "POST", headers: shell })).status, 404, `${gone} is gone`);
  }
  const management = { ...shell, "X-LibrePaper-Origin": "https://app.example" };
  for (const gone of ["quit", "approvals/abc"]) {
    assert.equal((await fetch(`${manage}/${gone}`, { method: "POST", headers: management, body: "{}" })).status, 404, `management ${gone} is gone`);
  }
  const listed = await (await fetch(`${manage}/state`, { headers: management })).json();
  assert.equal(listed.approvals, undefined, "the state lists no browser approvals");
  assert.equal(listed.standalone, undefined, "the state does not report standalone");

  const bindings = await fetch(`${base}/bindings?project=paper`, { headers: shell });
  assert.equal((await bindings.json()).bindings.length, 0, "no folder chooser has created a binding");

  // `stop` then `endpoint` is the restart sequence. Each start mints a new
  // credential, which is why the shell reads the endpoint again after a 401.
  await local("stop");
  await assert.rejects(local("status"), "status reports the stopped service");
  await assert.rejects(fetch(`${base}/health`));
  const restarted = await endpoint();
  assert.notEqual((await state()).instance, first.instance);
  assert.equal((await fetch(`${base}/capabilities`, { headers: shell })).status, 401, "the old credential is refused after a restart");
  assert.equal((await fetch(`${base}/capabilities`, { headers: { ...shell, Authorization: `Bearer ${restarted.token}` } })).status, 200);
  await local("stop");
  await assert.rejects(fetch(`${base}/health`));
  console.log("local-service: endpoint, racing starts, stop, credential and origin rules, management origin, removed routes and restart passed");
} finally {
  await local("stop").catch(() => {});
  await rm(root, { recursive: true, force: true });
}
