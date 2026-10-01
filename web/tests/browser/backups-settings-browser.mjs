// Account-wide backup controls need a paired companion and must stay scoped
// to the signed-in account even when an earlier status response arrives late.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until } from "../../tools/browser-driver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-backups-settings-"));
const output = join(temporary, "build");
const entry = join(temporary, "entry.js");
const harness = join(temporary, "Harness.svelte");
const statusMock = join(temporary, "status.svelte.js");
const clientMock = join(temporary, "client.js");

async function freePort() {
  const probe = createServer();
  await new Promise((resolve) => probe.listen(0, "127.0.0.1", resolve));
  const port = probe.address().port;
  await new Promise((resolve) => probe.close(resolve));
  return port;
}

writeFileSync(harness, `
<script>
  import BackupsSettings from ${JSON.stringify(join(root, "web/src/components/settings/BackupsSettings.svelte"))};
  import { backups } from ${JSON.stringify(join(root, "web/src/lib/companion/backups.svelte.js"))};
  let account = $state({ id: "account-a", provider: "github" });
  window.setAccount = (next) => { account = next; };
  window.refreshBackupStatus = () => backups.refresh();
  window.resetBackupStatus = () => backups.reset();
</script>
<BackupsSettings {account} />
`);

writeFileSync(statusMock, `
let current = $state.raw({ state: "unreachable" });
export const companion = { get status() { return current; }, watch() { return () => {}; } };
window.setPairing = (paired) => { current = { state: paired ? "connected" : "unreachable" }; };
`);

writeFileSync(clientMock, `
window.backupCalls = [];
window.backupRequests = [];
window.backupStatusRequest = (accountId) => new Promise((resolve, reject) => {
  window.backupCalls.push({ method: "GET", account_id: accountId });
  window.backupRequests.push({ resolve, reject, account_id: accountId });
});
export function backups(accountId) { return window.backupStatusRequest(accountId); }
export async function connectApp() { window.connectCalls = (window.connectCalls || 0) + 1; }
export async function chooseBackupFolder(accountId) {
  window.backupCalls.push({ method: "POST folder", account_id: accountId });
}
export async function updateBackups(accountId, settings) {
  window.backupCalls.push({ method: "PUT", account_id: accountId, ...settings });
}
export async function runBackups(accountId) {
  window.backupCalls.push({ method: "POST run", account_id: accountId });
}
`);

writeFileSync(entry, `
import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
import { mount } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import Harness from ${JSON.stringify(harness)};
mount(Harness, { target: document.body });
`);

const mockModules = {
  name: "backups-settings-test-mocks",
  enforce: "pre",
  resolveId(source, importer) {
    if (source.endsWith("/lib/companion/status.svelte.js")) return statusMock;
    if (source.endsWith("/lib/companion/client.js") || (source === "./client.js" && importer?.endsWith("/lib/companion/backups.svelte.js"))) return clientMock;
    return null;
  },
};

await build({
  configFile: false,
  root: join(root, "web"),
  logLevel: "error",
  plugins: [mockModules, svelte(), tailwindcss()],
  build: { outDir: output, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "check.js", cssFileName: "check" } },
});

const page = `<!doctype html><html><head><meta charset="utf-8"><link rel="stylesheet" href="/check.css"><title>backup settings check</title></head><body><script type="module" src="/check.js"></script></body></html>`;
const server = createServer((request, response) => {
  if (request.url === "/check.js") {
    response.setHeader("content-type", "text/javascript");
    response.end(readFileSync(join(output, "check.js")));
    return;
  }
  if (request.url === "/check.css") {
    response.setHeader("content-type", "text/css");
    response.end(readFileSync(join(output, "check.css")));
    return;
  }
  response.setHeader("content-type", "text/html");
  response.end(page);
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const port = server.address().port;
const debugPort = await freePort();
let b;
const settle = () => new Promise((resolve) => setTimeout(resolve, 80));
const click = (selector) => `(() => { const node = document.querySelector(${JSON.stringify(selector)}); if (!node) throw new Error('Could not find control: ' + ${JSON.stringify(selector)}); node.click(); })()`;
const resolveStatus = (index, value) => `window.backupRequests[${index}].resolve(${JSON.stringify(value)})`;

try {
  b = await browser("chromium", join(temporary, "chrome"), debugPort);
  await b.navigate(`http://127.0.0.1:${port}/`);
  await until("backup settings mounted", () => b.evaluate("document.body.innerText.includes('Connect LibrePaper Companion')"), 10000);
  assert.equal(await b.evaluate("window.backupCalls.length"), 0, "an unpaired companion is not queried for backup status");
  assert.equal(await b.evaluate("window.connectCalls || 0"), 0, "opening settings does not start a companion connection");

  await b.evaluate(click("button.lp-control-brand"));
  await until("existing companion connection flow invoked", () => b.evaluate("window.connectCalls === 1"), 5000);
  await b.evaluate("window.setPairing(true)");
  await until("paired status request", () => b.evaluate("window.backupRequests.length === 1"), 5000);
  await b.evaluate(resolveStatus(0, { enabled: false, frequency_minutes: 5, destination: "papers", running: false, last_success: null, error: null, projects: 2, needs_login: false }));
  await until("backup controls shown", () => b.evaluate("Boolean(document.querySelector('#backup-frequency'))"), 5000);
  assert.equal(await b.evaluate("document.querySelector('.backup-status button').disabled"), true, "manual runs stay disabled when automatic backups are off");

  await b.evaluate(click("#backup-destination button"));
  await until("native folder request sent", () => b.evaluate("window.backupCalls.some((call) => call.method === 'POST folder')"), 5000);
  assert.equal(await b.evaluate("window.backupCalls.find((call) => call.method === 'POST folder').account_id"), "account-a");
  await until("folder selection refresh requested", () => b.evaluate("window.backupRequests.length === 2"), 5000);
  await b.evaluate(resolveStatus(1, { enabled: false, frequency_minutes: 5, destination: "papers", running: false, projects: 2, needs_login: false }));

  await b.evaluate(`(() => { const input = document.querySelector('#backup-frequency select'); input.value = '15'; input.dispatchEvent(new Event('change', { bubbles: true })); })()`);
  await until("frequency update sent", () => b.evaluate("window.backupCalls.some((call) => call.method === 'PUT' && call.frequency_minutes === 15)"), 5000);
  assert.equal(await b.evaluate("window.backupCalls.find((call) => call.method === 'PUT').enabled"), false);
  assert.equal(await b.evaluate("window.backupCalls.find((call) => call.method === 'PUT').account_id"), "account-a");
  await until("frequency refresh requested", () => b.evaluate("window.backupRequests.length === 3"), 5000);
  await b.evaluate(resolveStatus(2, { enabled: false, frequency_minutes: 15, destination: "papers", running: false, projects: 2, needs_login: false }));

  await b.evaluate(click("#backup-enable [role='switch']"));
  await until("automatic backups enabled", () => b.evaluate("window.backupCalls.some((call) => call.method === 'PUT' && call.enabled === true)"), 5000);
  assert.equal(await b.evaluate("window.backupCalls.find((call) => call.method === 'PUT' && call.enabled).frequency_minutes"), 15);
  await until("enable refresh requested", () => b.evaluate("window.backupRequests.length === 4"), 5000);
  await b.evaluate(resolveStatus(3, { enabled: true, frequency_minutes: 15, destination: "papers", running: false, last_success: 1790800000, error: null, projects: 2, needs_login: false }));
  await until("healthy state shown", () => b.evaluate("document.body.innerText.includes('Last backup')"), 5000);
  await b.evaluate("void window.refreshBackupStatus()");
  await until("running status poll", () => b.evaluate("window.backupRequests.length === 5"), 5000);
  await b.evaluate(resolveStatus(4, { enabled: true, frequency_minutes: 15, destination: "papers", running: true, last_success: 1790800000, error: null, projects: 2, needs_login: false }));
  await until("running status shown", () => b.evaluate("document.body.innerText.includes('Backing up 2 projects')"), 5000);
  assert.equal(await b.evaluate("document.querySelector('.backup-status button').disabled"), true, "manual runs stay disabled while a run is active");

  // A response for account A may finish after the signed-in account changes.
  await b.evaluate("void window.refreshBackupStatus()");
  await until("in-flight account A poll", () => b.evaluate("window.backupRequests.length === 6"), 5000);
  await b.evaluate("window.setAccount({ id: 'account-b', provider: 'google' })");
  await until("account B status request", () => b.evaluate("window.backupRequests.length === 7"), 5000);
  assert.equal(await b.evaluate("window.backupRequests[6].account_id"), "account-b", "status requests follow the signed-in account");
  await b.evaluate("void window.refreshBackupStatus()");
  await b.evaluate(resolveStatus(5, { enabled: true, frequency_minutes: 60, destination: "stale-folder", running: false, projects: 99, needs_login: false }));
  await settle();
  assert.equal(await b.evaluate("document.body.innerText.includes('stale-folder')"), false, "a late response cannot show another account's destination");
  await b.evaluate(resolveStatus(6, { enabled: false, frequency_minutes: 5, destination: "account-b-folder", running: false, projects: 1, needs_login: false }));
  await until("queued account B refresh", () => b.evaluate("window.backupRequests.length === 8"), 5000);
  await until("account B settings shown", () => b.evaluate("document.body.innerText.includes('account-b-folder')"), 5000);
  assert.equal(await b.evaluate("document.body.innerText.includes('Backup status unavailable')"), false);

  // Companion-reported login errors show the server-specific CLI guidance.
  await b.evaluate(resolveStatus(7, { enabled: false, frequency_minutes: 5, destination: null, running: false, projects: 0, needs_login: true }));
  await until("CLI login guidance shown", () => b.evaluate("document.body.innerText.includes('librepaper login --server http://127.0.0.1')"), 5000);

  await b.evaluate(click(".setting-status button"));
  await until("login retry status refresh", () => b.evaluate("window.backupRequests.length === 9"), 5000);
  await b.evaluate("window.backupRequests[8].reject(new Error('companion offline'))");
  await until("status error shown", () => b.evaluate("document.body.innerText.includes('companion offline')"), 5000);
  assert.match(await b.evaluate("document.body.innerText"), /Backup status unavailable/);

  console.log("backups-settings-browser: paired-only status, native folder, schedule controls, status errors, and account-scoped stale replies passed");
} finally {
  if (b) await b.close();
  server.close();
  rmSync(temporary, { recursive: true, force: true });
}
