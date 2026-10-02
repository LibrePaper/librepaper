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
const apiMock = join(temporary, "api.js");

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
  window.setBrowserAccount = (next) => { window.browserAccount = next; };
  window.browserAccount = { id: "account-a", provider: "github" };
  window.refreshBackupStatus = () => backups.refresh();
  window.resetBackupStatus = () => backups.reset();
</script>
<BackupsSettings {account} />
`);

writeFileSync(statusMock, `
let current = $state.raw({ state: "unreachable" });
export const companion = { get status() { return current; }, watch() { return () => {}; } };
window.setPairing = (paired) => { window.pairingVersion = (window.pairingVersion || 0) + 1; current = { state: paired ? "connected" : "unreachable", address: "http://127.0.0.1:8763/", instance: "companion-a" }; };
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
export async function authorizeBackups(accountId, approveCode, isCurrent) {
  window.backupCalls.push({ method: "POST authorize", account_id: accountId });
  const pairing = window.pairingVersion || 0;
  const pairingCurrent = () => pairing === (window.pairingVersion || 0);
  if (!pairingCurrent() || !isCurrent()) throw new Error("The backup authorization scope changed.");
  await approveCode("ABCD-EFGH", pairingCurrent);
  if (pairing !== (window.pairingVersion || 0) || !isCurrent()) throw new Error("The backup authorization scope changed.");
  window.backupCalls.push({ method: "POST authorize complete", account_id: accountId });
  return { status: "authorized" };
}
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

writeFileSync(apiMock, `
export async function me() {
  if (!window.holdMe) return window.browserAccount;
  window.meStarted = true;
  return new Promise((resolve) => { window.releaseMe = () => resolve(window.browserAccount); });
}
export async function post(path, body) {
  window.apiPosts = window.apiPosts || [];
  window.apiPosts.push({ path, ...body });
  if (window.apiPostError) throw new Error(window.apiPostError);
  return {};
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
    if (source.endsWith("/lib/api.js")) return apiMock;
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
  const visibleControls = "Boolean(document.querySelector('#backup-enable [role=switch]') && document.querySelector('#backup-destination input') && document.querySelector('#backup-destination button') && document.querySelector('#backup-frequency select') && document.querySelector('.backup-status button'))";
  assert.equal(await b.evaluate(visibleControls), true, "backup settings remain visible before a companion is connected");
  assert.equal(await b.evaluate("document.querySelector('#backup-enable [role=switch]').disabled && document.querySelector('#backup-destination input').disabled && document.querySelector('#backup-destination button').disabled && document.querySelector('#backup-frequency select').disabled && document.querySelector('.backup-status button').disabled"), true, "controls are disabled while the companion is disconnected");
  assert.equal(await b.evaluate("window.backupCalls.length"), 0, "an unpaired companion is not queried for backup status");
  assert.equal(await b.evaluate("window.connectCalls || 0"), 0, "opening settings does not start a companion connection");

  await b.evaluate("window.setAccount({})");
  await until("signed-out prompt shown", () => b.evaluate("document.body.innerText.includes('Sign in')"), 5000);
  assert.equal(await b.evaluate(visibleControls), true, "backup settings remain visible when signed out");
  assert.equal(await b.evaluate("document.querySelector('#backup-enable [role=switch]').disabled && document.querySelector('#backup-destination input').disabled && document.querySelector('#backup-destination button').disabled && document.querySelector('#backup-frequency select').disabled && document.querySelector('.backup-status button').disabled"), true, "all account-scoped controls are disabled when signed out");
  await b.evaluate("window.setAccount({ id: 'account-a', provider: 'github' })");
  await until("connect prompt restored", () => b.evaluate("document.body.innerText.includes('Connect LibrePaper Companion')"), 5000);

  await b.evaluate(click("button.lp-control-brand"));
  await until("existing companion connection flow invoked", () => b.evaluate("window.connectCalls === 1"), 5000);
  await b.evaluate("window.setPairing(true)");
  await until("paired status request", () => b.evaluate("window.backupRequests.length === 1"), 5000);
  assert.equal(await b.evaluate(visibleControls), true, "controls stay mounted while the first status request is pending");
  assert.equal(await b.evaluate("document.querySelector('#backup-enable [role=switch]').disabled && document.querySelector('#backup-destination input').disabled && document.querySelector('#backup-destination button').disabled && document.querySelector('#backup-frequency select').disabled && document.querySelector('.backup-status button').disabled"), true, "controls stay disabled until a valid status arrives");
  assert.match(await b.evaluate("document.body.innerText"), /status (unknown|unavailable)|Loading backup status/i, "pending status is described as unknown or loading");
  await b.evaluate(resolveStatus(0, { enabled: false, frequency_minutes: 5, destination_set: true, destination: "papers", running: false, last_success: null, error: null, projects: 2, needs_login: false }));
  await until("valid backup status applied", () => b.evaluate("Boolean(document.querySelector('#backup-frequency') && !document.querySelector('#backup-enable [role=switch]').disabled && document.querySelector('#backup-destination input').value === 'papers')"), 5000);
  assert.equal(await b.evaluate("document.querySelector('#backup-enable [role=switch]').disabled"), false, "an empty folder label does not disable the automatic backup control");
  assert.equal(await b.evaluate("document.querySelector('.backup-status button').disabled"), true, "manual runs stay disabled when automatic backups are off");

  await b.evaluate("void window.refreshBackupStatus()");
  await until("empty destination label refresh", () => b.evaluate("window.backupRequests.length === 2"), 5000);
  await b.evaluate(resolveStatus(1, { enabled: true, frequency_minutes: 5, destination_set: true, destination: "", running: false, last_success: 1790800000, error: null, projects: 2, needs_login: false }));
  await until("enabled backups with empty folder label", () => b.evaluate("document.body.innerText.includes('Selected folder on this computer.')"), 5000);
  assert.equal(await b.evaluate("document.querySelector('#backup-enable [role=switch]').disabled"), false, "empty display labels do not disable the backup switch");
  assert.equal(await b.evaluate("document.querySelector('.backup-status button').disabled"), false, "empty display labels do not disable Back up now");

  await b.evaluate(click("#backup-destination button"));
  await until("native folder request sent", () => b.evaluate("window.backupCalls.some((call) => call.method === 'POST folder')"), 5000);
  assert.equal(await b.evaluate("window.backupCalls.find((call) => call.method === 'POST folder').account_id"), "account-a");
  await until("folder selection refresh requested", () => b.evaluate("window.backupRequests.length === 3"), 5000);
  await b.evaluate(resolveStatus(2, { enabled: false, frequency_minutes: 5, destination_set: true, destination: "papers", running: false, projects: 2, needs_login: false }));

  await b.evaluate(`(() => { const input = document.querySelector('#backup-frequency select'); input.value = '15'; input.dispatchEvent(new Event('change', { bubbles: true })); })()`);
  await until("frequency update sent", () => b.evaluate("window.backupCalls.some((call) => call.method === 'PUT' && call.frequency_minutes === 15)"), 5000);
  assert.equal(await b.evaluate("window.backupCalls.find((call) => call.method === 'PUT').enabled"), false);
  assert.equal(await b.evaluate("window.backupCalls.find((call) => call.method === 'PUT').account_id"), "account-a");
  await until("frequency refresh requested", () => b.evaluate("window.backupRequests.length === 4"), 5000);
  await b.evaluate(resolveStatus(3, { enabled: false, frequency_minutes: 15, destination_set: true, destination: "papers", running: false, projects: 2, needs_login: false }));

  await b.evaluate(click("#backup-enable [role='switch']"));
  await until("automatic backups enabled", () => b.evaluate("window.backupCalls.some((call) => call.method === 'PUT' && call.enabled === true)"), 5000);
  assert.equal(await b.evaluate("window.backupCalls.find((call) => call.method === 'PUT' && call.enabled).frequency_minutes"), 15);
  await until("enable refresh requested", () => b.evaluate("window.backupRequests.length === 5"), 5000);
  await b.evaluate(resolveStatus(4, { enabled: true, frequency_minutes: 15, destination_set: true, destination: "papers", running: false, last_success: 1790800000, error: null, projects: 2, needs_login: false }));
  await until("healthy state shown", () => b.evaluate("document.body.innerText.includes('Last backup')"), 5000);

  // A frequency mutation can overlap a status poll. Its follow-up request
  // must finish before Settings releases the mutation lock or stale state can
  // switch automatic backups back off.
  await b.evaluate("void window.refreshBackupStatus()");
  await until("active status poll", () => b.evaluate("window.backupRequests.length === 6"), 5000);
  await b.evaluate(`(() => { const input = document.querySelector('#backup-frequency select'); input.value = '30'; input.dispatchEvent(new Event('change', { bubbles: true })); })()`);
  await until("overlapping frequency mutation", () => b.evaluate("window.backupCalls.some((call) => call.method === 'PUT' && call.frequency_minutes === 30)"), 5000);
  await b.evaluate(resolveStatus(5, { enabled: false, frequency_minutes: 15, destination_set: true, destination: "papers", running: false, last_success: 1790800000, error: null, projects: 2, needs_login: false }));
  await until("post-mutation follow-up status request", () => b.evaluate("window.backupRequests.length === 7"), 5000);
  assert.equal(await b.evaluate("document.querySelector('#backup-enable [role=switch]').disabled"), true, "the schedule switch stays locked until the queued read completes");
  assert.equal(await b.evaluate("document.querySelector('#backup-frequency select').disabled"), true, "frequency stays locked until the queued read completes");
  await b.evaluate(resolveStatus(6, { enabled: true, frequency_minutes: 30, destination_set: true, destination: "papers", running: false, last_success: 1790800000, error: null, projects: 2, needs_login: false }));
  await until("post-mutation state applied", () => b.evaluate("document.querySelector('#backup-enable [role=switch]').getAttribute('aria-checked') === 'true' && !document.querySelector('#backup-frequency select').disabled"), 5000);
  assert.equal(await b.evaluate("document.querySelector('#backup-frequency select').value"), "30");

  await b.evaluate("void window.refreshBackupStatus()");
  await until("running status poll", () => b.evaluate("window.backupRequests.length === 8"), 5000);
  await b.evaluate(resolveStatus(7, { enabled: true, frequency_minutes: 30, destination_set: true, destination: "papers", running: true, last_success: 1790800000, error: null, projects: 2, needs_login: false }));
  await until("running status shown", () => b.evaluate("document.body.innerText.includes('Backing up 2 projects')"), 5000);
  assert.equal(await b.evaluate("document.querySelector('.backup-status button').disabled"), true, "manual runs stay disabled while a run is active");

  // A response for account A may finish after the signed-in account changes.
  await b.evaluate("void window.refreshBackupStatus()");
  await until("in-flight account A poll", () => b.evaluate("window.backupRequests.length === 9"), 5000);
  await b.evaluate("window.setAccount({ id: 'account-b', provider: 'google' })");
  await until("account B status request", () => b.evaluate("window.backupRequests.length === 10"), 5000);
  assert.equal(await b.evaluate("window.backupRequests[9].account_id"), "account-b", "status requests follow the signed-in account");
  await b.evaluate("void window.refreshBackupStatus()");
  await b.evaluate(resolveStatus(8, { enabled: true, frequency_minutes: 60, destination_set: true, destination: "stale-folder", running: false, projects: 99, needs_login: false }));
  await settle();
  assert.equal(await b.evaluate("document.body.innerText.includes('stale-folder')"), false, "a late response cannot show another account's destination");
  await b.evaluate(resolveStatus(9, { enabled: false, frequency_minutes: 5, destination_set: true, destination: "account-b-folder", running: false, projects: 1, needs_login: false }));
  await until("queued account B refresh", () => b.evaluate("window.backupRequests.length === 11"), 5000);
  await until("account B settings shown", () => b.evaluate("document.body.innerText.includes('account-b-folder')"), 5000);
  assert.equal(await b.evaluate("document.body.innerText.includes('Backup status unavailable')"), false);

  // Companion-reported authorization errors offer browser approval and keep
  // the native-selected folder visible as a read-only path.
  await b.evaluate(resolveStatus(10, { enabled: true, frequency_minutes: 30, destination_set: true, destination: "account-b-folder", running: false, projects: 1, needs_login: true }));
  await until("browser authorization shown", () => b.evaluate("document.body.innerText.includes('Authorize backups')"), 5000);
  assert.equal(await b.evaluate("document.querySelector('#backup-destination input').value"), "account-b-folder");
  assert.equal(await b.evaluate("document.querySelector('#backup-destination input').readOnly"), true);
  assert.equal(await b.evaluate("document.querySelector('#backup-enable [role=switch]').getAttribute('aria-checked')"), "true", "an enabled schedule stays accessible when credentials expire");

  await b.evaluate(click("#backup-enable [role='switch']"));
  await until("disable allowed while browser authorization is needed", () => b.evaluate("window.backupCalls.some((call) => call.method === 'PUT' && call.account_id === 'account-b' && call.enabled === false)"), 5000);
  await until("expired-authorization disable refresh", () => b.evaluate("window.backupRequests.length === 12"), 5000);
  await b.evaluate(resolveStatus(11, { enabled: false, frequency_minutes: 30, destination_set: true, destination: "account-b-folder", running: false, projects: 1, needs_login: true }));
  await until("disabled schedule remains available for authorization", () => b.evaluate("document.body.innerText.includes('Authorize backups') && Boolean(document.querySelector('#backup-enable [role=switch]'))"), 5000);
  assert.equal(await b.evaluate("document.querySelector('#backup-enable [role=switch]').disabled"), true, "a needs-login status prevents enabling the schedule");
  assert.equal(await b.evaluate("document.querySelector('#backup-frequency select').disabled"), true, "a needs-login status prevents changing frequency");
  assert.equal(await b.evaluate("document.querySelector('#backup-destination button').disabled"), false, "folder selection remains available while browser authorization is needed");

  await b.evaluate("window.setBrowserAccount({ id: 'account-b', provider: 'google' }); window.holdMe = true");
  await b.evaluate(click(".setting-status button"));
  await until("browser account check pending", () => b.evaluate("window.meStarted === true"), 5000);
  await b.evaluate("window.setPairing(true); window.releaseMe(); window.holdMe = false");
  await until("rotated pairing rejected during browser account check", () => b.evaluate("document.body.innerText.includes('backup authorization scope changed')"), 5000);
  assert.equal(await b.evaluate("window.apiPosts?.length || 0"), 0, "a rotated pairing cannot approve the device code");

  await b.evaluate("window.setBrowserAccount({ id: 'account-a', provider: 'github' })");
  await b.evaluate(click(".setting-status button"));
  await until("changed browser identity rejected", () => b.evaluate("document.body.innerText.includes('signed-in browser account changed')"), 5000);
  assert.equal(await b.evaluate("window.apiPosts?.length || 0"), 0, "a changed browser identity is not approved");
  await b.evaluate("window.setBrowserAccount({ id: 'account-b', provider: 'google' })");

  await b.evaluate("window.apiPostError = 'approval refused'");
  await b.evaluate(click(".setting-status button"));
  await until("browser approval failure shown", () => b.evaluate("document.body.innerText.includes('approval refused')"), 5000);
  assert.equal(await b.evaluate("window.backupCalls.some((call) => call.method === 'POST authorize complete')"), false, "failed browser approval cannot complete companion authorization");
  await b.evaluate("window.apiPostError = ''");
  await b.evaluate(click(".setting-status button"));
  await until("browser device approval sent", () => b.evaluate("window.apiPosts?.some((call) => call.path === '/api/auth/device/approve')"), 5000);
  assert.equal(await b.evaluate("window.apiPosts.find((call) => call.path === '/api/auth/device/approve').user_code"), "ABCD-EFGH");
  await until("companion authorization completed", () => b.evaluate("window.backupCalls.some((call) => call.method === 'POST authorize complete' && call.account_id === 'account-b')"), 5000);
  await until("authorization status refresh", () => b.evaluate("window.backupRequests.length === 13"), 5000);
  await b.evaluate(resolveStatus(12, { enabled: false, frequency_minutes: 30, destination_set: true, destination: "account-b-folder", running: false, projects: 1, needs_login: false }));
  await until("controls unlocked after authorization refresh", () => b.evaluate("Boolean(document.querySelector('#backup-enable [role=switch]') && !document.querySelector('#backup-enable [role=switch]').disabled)"), 5000);

  await b.evaluate(click("#backup-enable [role='switch']"));
  await until("schedule enable after authorization", () => b.evaluate("window.backupCalls.some((call) => call.method === 'PUT' && call.account_id === 'account-b' && call.enabled === true)"), 5000);
  await until("enable refresh completes", () => b.evaluate("window.backupRequests.length === 14"), 5000);
  await b.evaluate(resolveStatus(13, { enabled: true, frequency_minutes: 30, destination_set: true, destination: "account-b-folder", running: false, projects: 1, needs_login: false }));
  await until("enabled switch unlocked before next action", () => b.evaluate("document.querySelector('#backup-enable [role=switch]').getAttribute('aria-checked') === 'true' && !document.querySelector('#backup-enable [role=switch]').disabled"), 5000);

  await b.evaluate(click("#backup-enable [role='switch']"));
  await until("schedule disabled after refresh settled", () => b.evaluate("window.backupCalls.filter((call) => call.method === 'PUT' && call.account_id === 'account-b' && call.enabled === false).length === 2"), 5000);
  await until("disable refresh completes", () => b.evaluate("window.backupRequests.length === 15"), 5000);
  await b.evaluate(resolveStatus(14, { enabled: false, frequency_minutes: 30, destination_set: true, destination: "account-b-folder", running: false, projects: 1, needs_login: false }));

  await b.evaluate("void window.refreshBackupStatus()");
  await until("status error refresh requested", () => b.evaluate("window.backupRequests.length === 16"), 5000);
  await b.evaluate("window.backupRequests[15].reject(new Error('companion offline'))");
  await until("status error shown", () => b.evaluate("document.body.innerText.includes('companion offline')"), 5000);
  assert.match(await b.evaluate("document.body.innerText"), /Backup status unavailable/);
  assert.equal(await b.evaluate(visibleControls), true, "backup controls remain visible when status loading fails");
  assert.equal(await b.evaluate("document.querySelector('#backup-enable [role=switch]').disabled && document.querySelector('#backup-destination input').disabled && document.querySelector('#backup-destination button').disabled && document.querySelector('#backup-frequency select').disabled && document.querySelector('.backup-status button').disabled"), true, "status failure leaves all backup actions disabled");
  assert.doesNotMatch(await b.evaluate("document.body.innerText"), /Backups are off/i, "unknown status is not described as backups being off");

  await b.evaluate("void window.refreshBackupStatus()");
  await until("valid status recovery requested", () => b.evaluate("window.backupRequests.length === 17"), 5000);
  await b.evaluate(resolveStatus(16, { enabled: true, frequency_minutes: 30, destination_set: true, destination: "recovered-folder", running: false, projects: 1, needs_login: false }));
  await until("valid status recovered", () => b.evaluate("document.querySelector('#backup-destination input').value === 'recovered-folder'"), 5000);

  // Disconnecting after a successful load clears the account's destination and
  // disables every action while keeping the settings page visible.
  await b.evaluate("window.setPairing(false)");
  await until("disconnected settings remain visible", () => b.evaluate("document.body.innerText.includes('Connect LibrePaper Companion') && Boolean(document.querySelector('#backup-enable [role=switch]'))"), 5000);
  assert.equal(await b.evaluate("document.querySelector('#backup-destination input').value"), "", "disconnecting does not expose the previously loaded folder");
  assert.equal(await b.evaluate("document.querySelector('#backup-enable [role=switch]').disabled && document.querySelector('#backup-destination input').disabled && document.querySelector('#backup-destination button').disabled && document.querySelector('#backup-frequency select').disabled && document.querySelector('.backup-status button').disabled"), true, "disconnecting disables all backup actions");

  console.log("backups-settings-browser: persistent backup settings, disabled disconnected/loading/error states, needs-login controls, browser authorization, and account-scoped stale replies passed");
} finally {
  if (b) await b.close();
  server.close();
  rmSync(temporary, { recursive: true, force: true });
}
