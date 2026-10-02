// Real detached companion, isolated state, and a fake desktop opener. Never
// modifies the user's installation, startup entries, tools, or documents.
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { existsSync } from "node:fs";
import { mkdtemp, mkdir, writeFile, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";
import { createServer } from "node:net";
import { createHash, randomBytes } from "node:crypto";
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
const env = { ...process.env, HOME: root, XDG_STATE_HOME: join(root, "state"), XDG_CACHE_HOME: join(root, "cache"), XDG_CONFIG_HOME: join(root, "config"), PATH: `${tools}:${process.env.PATH}`, LIBREPAPER_TEST_OPENED: opened };
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
const base = `http://127.0.0.1:${port}/librepaper/local`;
const site = "https://papers.example";
const returnUrl = `${site}/docs/paper`;
const state = async () => JSON.parse(await readFile(join(env.XDG_STATE_HOME, "librepaper/local/service.json"), "utf8"));
try {
  await cli();
  const first = await state();
  await cli();
  assert.equal((await state()).pid, first.pid, "repeated bare invocation reuses the running companion");
  await cli("start");
  assert.equal((await state()).pid, first.pid, "explicit start reuses the running companion");
  await cli("--at-login");
  const desktopEntry = await readFile(join(env.XDG_CONFIG_HOME, "autostart/librepaper-local.desktop"), "utf8");
  assert.match(desktopEntry, /^Exec=.* start$/m);
  assert.doesNotMatch(desktopEntry, /local start/);
  await cli("status");

  const request = randomBytes(24).toString("base64url");
  const verifier = randomBytes(32).toString("base64url");
  const challenge = createHash("sha256").update(verifier).digest("hex");
  // `local open` sends the pairing request to the companion. Depending on
  // whether the test environment has a display, approval either opens the
  // browser handoff directly or prints a one-time code for `local approve`.
  const opening = legacy("open", `librepaper://connect?${new URLSearchParams({ origin: site, request, challenge, return: returnUrl })}`).then(() => null, (error) => error);
  const log = join(env.XDG_STATE_HOME, "librepaper/local/companion.log");
  let approvalCode;
  let target;
  for (let attempt = 0; attempt < 100; attempt++) {
    try { target = await readFile(opened, "utf8"); if (target) break; } catch {}
    try {
      const output = await readFile(log, "utf8");
      approvalCode = output.match(/librepaper local approve (\d{6})/)?.[1];
      if (approvalCode) break;
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  if (approvalCode) {
    await legacy("approve", approvalCode);
    for (let attempt = 0; attempt < 30; attempt++) {
      try { target = await readFile(opened, "utf8"); if (target) break; } catch {}
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
  }
  assert.ok(target, "approval produces the browser handoff");
  const handoff = new URL(target);
  assert.equal(`${handoff.origin}${handoff.pathname}`, returnUrl, "approval returns to the requesting document");
  const fragment = new URLSearchParams(handoff.hash.slice(1));
  assert.equal(fragment.get("librepaper-local"), `http://127.0.0.1:${port}/`, "handoff carries the running companion address");
  assert.equal(fragment.get("librepaper-request"), request, "handoff is tied to this pairing request");
  assert.ok(!target.includes(verifier), "the browser verifier stays out of the handoff");
  const openError = await opening;
  assert.ifError(openError);
  const claim = () => fetch(`${base}/connect/claim`, { method: "POST", headers: { Origin: site, "Content-Type": "application/json" }, body: JSON.stringify({ origin: site, request, verifier }) });
  const accepted = await claim();
  assert.equal(accepted.status, 200);
  const { token } = await accepted.json();
  assert.equal((await claim()).status, 404);
  const caps = () => fetch(`${base}/capabilities`, { headers: { Origin: site, Authorization: `Bearer ${token}` } });
  assert.equal((await caps()).status, 200);

  const unauthenticatedFolder = await fetch(`${base}/bindings/folder`, { method: "POST", headers: { Origin: site, "Content-Type": "application/json" }, body: JSON.stringify({ project: "other", entrypoint: "main.qmd" }) });
  assert.equal(unauthenticatedFolder.status, 401, "folder selection requires a bearer token");
  const bindings = await fetch(`${base}/bindings?project=paper`, { headers: { Origin: site, Authorization: `Bearer ${token}` } });
  const listed = (await bindings.json()).bindings;
  assert.equal(listed.length, 0, "no folder chooser has created a binding");

  const unauthenticatedQuit = await fetch(`${base}/quit`, { method: "POST", headers: { Origin: site } });
  assert.equal(unauthenticatedQuit.status, 401, "quitting requires a bearer token");

  // `librepaper local restart` was removed in the CLI cull; `stop` then
  // `start` is the replacement sequence and exercises the same instance
  // change and permission persistence.
  await cli("stop");
  await assert.rejects(cli("status"), "root status reports the stopped companion");
  await cli("start");
  assert.notEqual((await state()).instance, first.instance);
  assert.equal((await caps()).status, 200, "permission survives restart");
  await cli("stop");
  await assert.rejects(fetch(`${base}/health`));
  await assert.rejects(legacy("open", "https://evil.example/"));
  await assert.rejects(fetch(`${base}/health`), "invalid link does not start companion");
  await legacy("start");
  await legacy("status");
  await legacy("stop");
  console.log("companion-lifecycle: bare and explicit start, at-login, deep-link consent, quit authorization, root status/stop, legacy aliases and restart passed");
} finally {
  await cli("stop").catch(() => {});
  await rm(root, { recursive: true, force: true });
}
