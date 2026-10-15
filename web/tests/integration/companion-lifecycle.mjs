// Real detached companion, isolated state and a fake desktop opener, driven
// the way the desktop shell drives it. Never
// modifies the user's installation, startup entries, tools, or documents.
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { existsSync } from "node:fs";
import { mkdtemp, mkdir, writeFile, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";
import { createServer } from "node:net";
const exec = promisify(execFile);
const root = await mkdtemp(join(tmpdir(), "librepaper-lifecycle-"));
const repository = resolve(import.meta.dirname, "../../..");
const binary = resolve(process.env.LIBREPAPER_TEST_BINARY || join(repository, "target/debug/librepaper"));
if (!existsSync(binary)) {
  console.log("companion-lifecycle: no companion binary; skipping (run `cargo build`)");
  process.exit(0);
}
const tools = join(root, "bin");
await mkdir(tools);
const opened = join(root, "opened-url");
await writeFile(join(tools, "xdg-open"), '#!/bin/sh\nprintf "%s" "$1" > "$LIBREPAPER_TEST_OPENED"\n', { mode: 0o755 });
const env = { ...process.env, HOME: root, XDG_STATE_HOME: join(root, "state"), XDG_CACHE_HOME: join(root, "cache"), XDG_CONFIG_HOME: join(root, "config"), PATH: `${tools}:${process.env.PATH}`, LIBREPAPER_TEST_OPENED: opened, LIBREPAPER_SERVER: "https://app.example/" };
delete env.DISPLAY;
delete env.WAYLAND_DISPLAY;
delete env.LIBREPAPER_LOCAL_CODE;
const cli = (...args) => exec(binary, args, { env, timeout: 20000 });
const legacy = (...args) => exec(binary, ["local", ...args], { env, timeout: 20000 });
const listener = createServer();
await new Promise((resolve) => listener.listen(0, "127.0.0.1", resolve));
const port = listener.address().port;
await new Promise((resolve) => listener.close(resolve));
env.LIBREPAPER_LOCAL_PORT = String(port);
const endpoint = async () => JSON.parse((await legacy("endpoint")).stdout.trim().split("\n").at(-1));
const site = "https://papers.example";
const state = async () => JSON.parse(await readFile(join(env.XDG_STATE_HOME, "librepaper/local/service.json"), "utf8"));
try {
  await cli();
  const first = await state();
  await cli();
  assert.equal((await state()).pid, first.pid, "repeated bare invocation reuses the running companion");
  await cli("start");
  assert.equal((await state()).pid, first.pid, "explicit start reuses the running companion");
  await assert.rejects(cli("desktop"), "the removed desktop subcommand is rejected");
  await cli("--at-login");
  const desktopEntry = await readFile(join(env.XDG_CONFIG_HOME, "autostart/librepaper-local.desktop"), "utf8");
  assert.match(desktopEntry, /^Exec=.* start$/m);
  assert.doesNotMatch(desktopEntry, /local start/);
  await cli("status");

  // The endpoint command reports the running companion and its credential.
  const found = await endpoint();
  assert.equal(found.address, `http://127.0.0.1:${port}/`);
  assert.equal(found.instance, first.instance);
  assert.ok(found.token, "the endpoint carries the local credential");
  assert.deepEqual(found.protocol, [2]);
  const base = `${found.address}librepaper/local`;
  const manage = `${found.address}companion/api`;
  const shell = { Authorization: `Bearer ${found.token}`, "X-LibrePaper-Origin": site };

  assert.equal((await fetch(`${base}/health`)).status, 200, "health needs no credential");
  assert.equal((await fetch(`${base}/capabilities`)).status, 401, "a feature route needs the credential");
  assert.equal((await fetch(`${base}/capabilities`, { headers: { ...shell, Authorization: "Bearer wrong" } })).status, 401);
  assert.equal((await fetch(`${base}/capabilities`, { headers: shell })).status, 200, "the shell's headers are accepted");
  assert.equal((await fetch(`${base}/capabilities`, { headers: { ...shell, Origin: site } })).status, 403, "a request with a browser Origin is refused");
  assert.equal((await fetch(`${base}/capabilities`, { headers: { ...shell, "Sec-Fetch-Site": "cross-site" } })).status, 403, "a request with Sec-Fetch-Site is refused");
  assert.equal((await fetch(`${base}/quit`, { method: "POST", headers: { "X-LibrePaper-Origin": site } })).status, 401, "quitting needs the credential");
  assert.equal((await fetch(`${manage}/state`, { headers: shell })).status, 403, "management is refused for a site other than the configured server");
  assert.equal((await fetch(`${manage}/state`, { headers: { ...shell, "X-LibrePaper-Origin": "https://app.example" } })).status, 200, "management is allowed for the configured server");
  for (const gone of ["pair/request", "connect/claim"]) {
    assert.equal((await fetch(`${base}/${gone}`, { method: "POST", headers: shell })).status, 404, `${gone} is gone`);
  }

  const bindings = await fetch(`${base}/bindings?project=paper`, { headers: shell });
  assert.equal((await bindings.json()).bindings.length, 0, "no folder chooser has created a binding");

  // `stop` then `start` is the restart sequence; the credential stays valid.
  await cli("stop");
  await assert.rejects(cli("status"), "root status reports the stopped companion");
  await cli("start");
  assert.notEqual((await state()).instance, first.instance);
  const restarted = await endpoint();
  assert.equal((await fetch(`${base}/capabilities`, { headers: { ...shell, Authorization: `Bearer ${restarted.token}` } })).status, 200);
  await cli("stop");
  await assert.rejects(fetch(`${base}/health`));
  await assert.rejects(legacy("open", "https://evil.example/"));
  await assert.rejects(legacy("open", "librepaper://connect?origin=https%3A%2F%2Fpapers.example&request=r&challenge=c&return=x"), "pairing links are refused");
  await assert.rejects(fetch(`${base}/health`), "an invalid link does not start the companion");
  console.log("companion-lifecycle: start, at-login, endpoint, credential and origin rules, management origin, removed pairing routes and restart passed");
} finally {
  await cli("stop").catch(() => {});
  await rm(root, { recursive: true, force: true });
}
