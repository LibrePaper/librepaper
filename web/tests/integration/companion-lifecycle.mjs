// Real detached companion, isolated state, and a fake desktop opener. Never
// modifies the user's installation, startup entries, tools, or documents.
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { mkdtemp, mkdir, writeFile, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";
import { createServer } from "node:net";
import { createHash, randomBytes } from "node:crypto";
const exec = promisify(execFile);
const root = await mkdtemp(join(tmpdir(), "librepaper-lifecycle-"));
const binary = resolve(process.env.LIBREPAPER_TEST_BINARY || "target/debug/librepaper");
const tools = join(root, "bin");
await mkdir(tools);
const opened = join(root, "opened-url");
const projectFolder = join(root, "private project");
await mkdir(projectFolder);
await writeFile(join(projectFolder, "main.qmd"), "# Local input\n");
await writeFile(join(tools, "zenity"), '#!/bin/sh\nprintf "%s\\n" "$LIBREPAPER_TEST_FOLDER"\n', { mode: 0o755 });
await writeFile(join(tools, "xdg-open"), '#!/bin/sh\nprintf "%s" "$1" > "$LIBREPAPER_TEST_OPENED"\n', { mode: 0o755 });
const env = { ...process.env, HOME: root, XDG_STATE_HOME: join(root, "state"), XDG_CACHE_HOME: join(root, "cache"), XDG_CONFIG_HOME: join(root, "config"), PATH: `${tools}:${process.env.PATH}`, LIBREPAPER_TEST_OPENED: opened, LIBREPAPER_TEST_FOLDER: projectFolder };
delete env.LIBREPAPER_LOCAL_PORT; delete env.LIBREPAPER_LOCAL_CODE;
const cli = (...args) => exec(binary, ["local", ...args], { env, timeout: 20000 });
const listener = createServer();
await new Promise((resolve) => listener.listen(0, "127.0.0.1", resolve));
const port = listener.address().port;
await new Promise((resolve) => listener.close(resolve));
const base = `http://127.0.0.1:${port}/librepaper/local/v1`;
const site = "https://papers.example";
const state = async () => JSON.parse(await readFile(join(env.XDG_STATE_HOME, "librepaper/local/service.json"), "utf8"));
try {
  await cli("launch", "--port", String(port));
  const first = await state();
  await cli("launch", "--port", String(port));
  assert.equal((await state()).pid, first.pid, "launch is idempotent");
  await cli("startup", "enable");
  assert.match(await readFile(join(env.XDG_CONFIG_HOME, "autostart/librepaper-local.desktop"), "utf8"), /local launch/);
  await cli("startup", "disable");

  const request = randomBytes(24).toString("base64url");
  const verifier = randomBytes(32).toString("base64url");
  const challenge = createHash("sha256").update(verifier).digest("hex");
  await cli("open", `librepaper://connect?${new URLSearchParams({ origin: site, project: "paper", request, challenge })}`);
  let target;
  for (let attempt = 0; attempt < 30; attempt++) {
    try { target = await readFile(opened, "utf8"); if (target) break; } catch {}
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  assert.equal(new URL(target).port, String(port), "handler uses the running companion port");
  assert.ok(!target.includes(verifier));
  const registration = await fetch(target);
  assert.equal(registration.status, 200);
  assert.match(await registration.text(), /paper/);
  const form = new URLSearchParams({ origin: site, project: "paper", request, challenge });
  const consent = await fetch(`${base}/pair`, { method: "POST", headers: { Origin: `http://127.0.0.1:${port}`, "Sec-Fetch-Site": "same-origin" }, body: form });
  assert.equal(consent.status, 200, await consent.text());
  const claim = () => fetch(`${base}/connect/claim`, { method: "POST", headers: { Origin: site, "Content-Type": "application/json" }, body: JSON.stringify({ origin: site, project: "paper", request, verifier }) });
  const accepted = await claim();
  assert.equal(accepted.status, 200);
  const { token } = await accepted.json();
  assert.equal((await claim()).status, 404);
  const caps = () => fetch(`${base}/capabilities`, { headers: { Origin: site, Authorization: `Bearer ${token}` } });
  assert.equal((await caps()).status, 200);

  const folder = await fetch(`${base}/bindings/folder`, { method: "POST", headers: { Origin: site, Authorization: `Bearer ${token}`, "Content-Type": "application/json" }, body: JSON.stringify({ project: "paper", entrypoint: "main.qmd" }) });
  assert.equal(folder.status, 200, await folder.clone().text());
  const binding = await folder.json();
  assert.ok(binding.id);
  assert.ok(!JSON.stringify(binding).includes(root), "folder paths stay local");
  const unauthorizedFolder = await fetch(`${base}/bindings/folder`, { method: "POST", headers: { Origin: site, Authorization: `Bearer ${token}`, "Content-Type": "application/json" }, body: JSON.stringify({ project: "other", entrypoint: "main.qmd" }) });
  assert.equal(unauthorizedFolder.status, 403);
  const bindings = await fetch(`${base}/bindings`, { headers: { Origin: site, Authorization: `Bearer ${token}` } });
  assert.equal((await bindings.json()).bindings[0].id, binding.id);
  const revoke = await fetch(`${base}/bindings/${binding.id}`, { method: "DELETE", headers: { Origin: site, Authorization: `Bearer ${token}` } });
  assert.equal((await revoke.json()).revoked, true);

  const manage = await fetch(`${base}/manage`, { headers: { Origin: site } });
  assert.equal(manage.headers.get("access-control-allow-origin"), null);
  const html = await manage.text();
  assert.match(html, /Quit companion/);
  const nonce = html.match(/name="nonce" value="([^"]+)"/)[1];
  const forbidden = await fetch(`${base}/manage`, { method: "POST", headers: { Origin: site }, body: new URLSearchParams({ nonce, action: "quit" }) });
  assert.equal(forbidden.status, 403);

  await cli("restart");
  assert.notEqual((await state()).instance, first.instance);
  assert.equal((await caps()).status, 200, "permission survives restart");
  await cli("stop");
  await assert.rejects(fetch(`${base}/health`));
  await assert.rejects(cli("open", "https://evil.example/"));
  await assert.rejects(fetch(`${base}/health`), "invalid link does not launch companion");
  console.log("companion-lifecycle: background launch, startup, deep-link consent, management isolation, restart and stop passed");
} finally {
  await cli("stop").catch(() => {});
  await rm(root, { recursive: true, force: true });
}
