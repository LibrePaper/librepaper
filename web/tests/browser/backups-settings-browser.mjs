// Account-wide backup controls run in the desktop app and must stay scoped
// to the signed-in account even when an earlier status response arrives late.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-backups-settings-"));
const output = join(temporary, "build");
const entry = join(temporary, "entry.js");
const harness = join(temporary, "Harness.svelte");
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
  import { backups } from ${JSON.stringify(join(root, "web/src/lib/desktop/backups.svelte.js"))};
  let account = $state({});
  window.setAccount = (next) => { account = next; };
  window.setBrowserAccount = (next) => { window.browserAccount = next; };
  window.browserAccount = { id: "account-a", provider: "github" };
  window.refreshBackupStatus = () => backups.refresh();
  window.resetBackupStatus = () => backups.reset();
</script>
<BackupsSettings {account} />
`);

writeFileSync(clientMock, `
export const INSTALL_URL = "https://librepaper.org/desktop-app.html";
export const DESKTOP_ONLY_MESSAGE = "This needs the LibrePaper desktop app.";
export const desktop = () => window.__LIBREPAPER_DESKTOP__ ?? null;
window.backupCalls = [];
window.backupRequests = [];
window.backupStatusRequest = (accountId) => new Promise((resolve, reject) => {
  window.backupCalls.push({ method: "GET", account_id: accountId });
  window.backupRequests.push({ resolve, reject, account_id: accountId });
});
export function backups(accountId) { return window.backupStatusRequest(accountId); }
export async function authorizeBackups(accountId, approveCode, isCurrent) {
  window.backupCalls.push({ method: "POST authorize", account_id: accountId });
  if (!isCurrent()) throw new Error("The backup authorization scope changed.");
  await approveCode("ABCD-EFGH", isCurrent);
  if (!isCurrent()) throw new Error("The backup authorization scope changed.");
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
  return window.browserAccount;
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
    if (source.endsWith("/lib/desktop/client.js") || (source === "./client.js" && importer?.endsWith("/lib/desktop/backups.svelte.js"))) return clientMock;
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
// Inside the desktop app the page global exists; the plain page is a browser.
const desktopPage = page.replace("<body>", "<body><script>window.__LIBREPAPER_DESKTOP__ = { base: 'x', version: 'test' }</script>");
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
  response.end(request.url.startsWith("/plain") ? page : desktopPage);
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
  const disabledControls = "document.querySelector('#backup-enable [role=switch]').disabled && document.querySelector('#backup-destination input').disabled && document.querySelector('#backup-destination button').disabled && document.querySelector('#backup-frequency select').disabled && document.querySelector('.backup-status button').disabled";
  await b.navigate(`http://127.0.0.1:${port}/plain`);
  await until("browser notice shown", () => b.evaluate("Boolean(document.querySelector('.desktop-only'))"), 10000);
  assert.equal(await b.evaluate("document.querySelector('.desktop-only-message').textContent.trim()"), "This needs the LibrePaper desktop app.", "a browser is told backups need the desktop app");
  assert.equal(await b.evaluate("Boolean(document.querySelector('#backup-enable'))"), false, "a browser shows no backup controls");
  assert.equal(await b.evaluate("window.backupCalls.length"), 0, "a browser is not queried for backup status");

  await b.navigate(`http://127.0.0.1:${port}/`);
  await until("backup settings mounted", () => b.evaluate("Boolean(document.querySelector('#backup-enable'))"), 10000);
  const visibleControls = "Boolean(document.querySelector('#backup-enable [role=switch]') && document.querySelector('#backup-destination input') && document.querySelector('#backup-destination button') && document.querySelector('#backup-frequency select') && document.querySelector('.backup-status button'))";
  assert.equal(await b.evaluate(visibleControls), true, "backup settings are visible inside the desktop app");
  assert.equal(await b.evaluate("window.backupCalls.length"), 0, "a signed-out page is not queried for backup status");
  assert.equal(await b.evaluate("document.querySelector('#backup-enable').innerText.includes('Unknown') && document.querySelector('#backup-frequency').innerText.includes('Unknown')"), true, "unloaded switches and frequency are visibly unknown");
  assert.equal(await b.evaluate("document.body.innerText.includes('Sign in')"), true, "signed-out prompt shown");
  assert.equal(await b.evaluate(disabledControls), true, "all account-scoped controls are disabled when signed out");
  await b.evaluate("window.setAccount({ id: 'account-a', provider: 'github' })");
  await until("signed-in status request", () => b.evaluate("window.backupRequests.length === 1"), 5000);
  assert.equal(await b.evaluate(visibleControls), true, "controls stay mounted while the first status request is pending");
  assert.equal(await b.evaluate("document.querySelector('#backup-enable [role=switch]').disabled && document.querySelector('#backup-destination input').disabled && document.querySelector('#backup-destination button').disabled && document.querySelector('#backup-frequency select').disabled && document.querySelector('.backup-status button').disabled"), true, "controls stay disabled until a valid status arrives");
  assert.match(await b.evaluate("document.body.innerText"), /Loading/, "pending status is described as loading");
  await b.evaluate(resolveStatus(0, { enabled: false, frequency_minutes: 5, destination_set: true, destination: "papers", running: false, last_success: null, error: null, projects: 2, needs_login: false }));
  await until("valid backup status applied", () => b.evaluate("Boolean(document.querySelector('#backup-frequency') && !document.querySelector('#backup-enable [role=switch]').disabled && document.querySelector('#backup-destination input').value === 'papers')"), 5000);
  assert.equal(await b.evaluate("document.querySelector('#backup-enable [role=switch]').disabled"), false, "an empty folder label does not disable the automatic backup control");
  assert.equal(await b.evaluate("document.querySelector('.backup-status button').disabled"), true, "manual runs stay disabled when automatic backups are off");

  await b.evaluate("void window.refreshBackupStatus()");
  await until("empty destination label refresh", () => b.evaluate("window.backupRequests.length === 2"), 5000);
  await b.evaluate(resolveStatus(1, { enabled: true, frequency_minutes: 5, destination_set: true, destination: "", running: false, last_success: 1790800000, error: null, projects: 2, needs_login: false }));
  await until("enabled backups with empty folder label", () => b.evaluate("document.querySelector('#backup-destination input').readOnly && document.querySelector('#backup-destination input').value === ''"), 5000);
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
  await until("frequency save confirmed", () => b.evaluate("document.querySelector('.setting-feedback')?.textContent.trim() === 'Saved'"), 5000);

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
  assert.equal(await b.evaluate("document.querySelector('#backup-destination input').value === 'stale-folder'"), false, "a late response cannot show another account's destination");
  await b.evaluate(resolveStatus(9, { enabled: false, frequency_minutes: 5, destination_set: true, destination: "account-b-folder", running: false, projects: 1, needs_login: false }));
  await until("queued account B refresh", () => b.evaluate("window.backupRequests.length === 11"), 5000);
  await until("account B settings shown", () => b.evaluate("document.querySelector('#backup-destination input').value === 'account-b-folder'"), 5000);
  assert.equal(await b.evaluate("document.body.innerText.includes('Backup status unavailable')"), false);

  // Turning backups on is the authorization. A status that reports lost access
  // while backups are on re-authorizes by itself, and the folder stays visible
  // as a read-only path.
  await b.evaluate("window.setBrowserAccount({ id: 'account-b', provider: 'google' })");
  await b.evaluate(resolveStatus(10, { enabled: true, frequency_minutes: 30, destination_set: true, destination: "account-b-folder", running: false, projects: 1, needs_login: true }));
  await until("automatic reauthorization requested", () => b.evaluate("window.backupRequests.length === 12"), 5000);
  await until("automatic reauthorization completed", () => b.evaluate("window.backupCalls.some((call) => call.method === 'POST authorize complete' && call.account_id === 'account-b')"), 5000);
  assert.equal(await b.evaluate("document.body.innerText.includes('Authorize backups')"), false, "no authorization prompt is shown");
  assert.equal(await b.evaluate("document.querySelector('#backup-destination input').value"), "account-b-folder");
  assert.equal(await b.evaluate("document.querySelector('#backup-destination input').readOnly"), true);
  assert.equal(await b.evaluate("document.querySelector('#backup-enable [role=switch]').getAttribute('aria-checked')"), "true", "an enabled schedule stays on while it re-authorizes");
  await b.evaluate(resolveStatus(11, { enabled: true, frequency_minutes: 30, destination_set: true, destination: "account-b-folder", running: false, projects: 1, needs_login: false }));
  await until("reauthorized status applied", () => b.evaluate("!document.querySelector('#backup-enable [role=switch]').disabled"), 5000);

  await b.evaluate(click("#backup-enable [role='switch']"));
  await until("schedule disabled after reauthorization", () => b.evaluate("window.backupCalls.some((call) => call.method === 'PUT' && call.account_id === 'account-b' && call.enabled === false)"), 5000);
  await until("disable refresh requested", () => b.evaluate("window.backupRequests.length === 13"), 5000);
  await b.evaluate(resolveStatus(12, { enabled: false, frequency_minutes: 30, destination_set: true, destination: "account-b-folder", running: false, projects: 1, needs_login: true }));
  await until("disabled schedule shown with needs-login status", () => b.evaluate("document.querySelector('#backup-enable [role=switch]').getAttribute('aria-checked') === 'false' && !document.querySelector('#backup-enable [role=switch]').disabled"), 5000);
  assert.equal(await b.evaluate("document.body.innerText.includes('Authorize backups')"), false, "no authorization prompt is shown when the schedule is off");
  assert.equal(await b.evaluate("document.querySelector('#backup-enable [role=switch]').disabled"), false, "a needs-login status does not block turning the schedule on");
  assert.equal(await b.evaluate("document.querySelector('#backup-frequency select').disabled"), true, "a needs-login status prevents changing frequency");
  assert.equal(await b.evaluate("document.querySelector('#backup-destination button').disabled"), false, "folder selection remains available while browser authorization is needed");

  // A changed browser account must not approve, and must not enable backups.
  await b.evaluate("window.setBrowserAccount({ id: 'account-a', provider: 'github' })");
  const postsBeforeIdentity = await b.evaluate("window.apiPosts?.length || 0");
  await b.evaluate(click("#backup-enable [role='switch']"));
  await until("changed browser identity rejected", () => b.evaluate("document.body.innerText.includes('signed-in browser account changed')"), 5000);
  assert.equal(await b.evaluate("window.apiPosts?.length || 0"), postsBeforeIdentity, "a changed browser identity is not approved");
  assert.equal(await b.evaluate("window.backupCalls.some((call) => call.method === 'PUT' && call.account_id === 'account-b' && call.enabled === true)"), false, "a changed browser identity cannot turn the schedule on");
  await b.evaluate("window.setBrowserAccount({ id: 'account-b', provider: 'google' })");

  // A refused browser approval must not authorize the desktop app or enable backups.
  const completesBeforeFailure = await b.evaluate("window.backupCalls.filter((call) => call.method === 'POST authorize complete').length");
  await b.evaluate("window.apiPostError = 'approval refused'");
  await b.evaluate(click("#backup-enable [role='switch']"));
  await until("browser approval failure shown", () => b.evaluate("document.body.innerText.includes('approval refused')"), 5000);
  assert.equal(await b.evaluate("window.backupCalls.filter((call) => call.method === 'POST authorize complete').length"), completesBeforeFailure, "failed browser approval cannot complete desktop app authorization");
  assert.equal(await b.evaluate("window.backupCalls.some((call) => call.method === 'PUT' && call.account_id === 'account-b' && call.enabled === true)"), false, "failed browser approval cannot turn the schedule on");
  await b.evaluate("window.apiPostError = ''");

  await b.evaluate(click("#backup-enable [role='switch']"));
  await until("browser device approval sent", () => b.evaluate("window.apiPosts?.some((call) => call.path === '/api/auth/device/approve')"), 5000);
  assert.equal(await b.evaluate("window.apiPosts.find((call) => call.path === '/api/auth/device/approve').user_code"), "ABCD-EFGH");
  await until("desktop app authorization completed", () => b.evaluate("window.backupCalls.filter((call) => call.method === 'POST authorize complete' && call.account_id === 'account-b').length === 2"), 5000);
  await until("authorization status refresh", () => b.evaluate("window.backupRequests.length === 14"), 5000);
  await b.evaluate(resolveStatus(13, { enabled: false, frequency_minutes: 30, destination_set: true, destination: "account-b-folder", running: false, projects: 1, needs_login: false }));
  await until("schedule enable after authorization", () => b.evaluate("window.backupCalls.some((call) => call.method === 'PUT' && call.account_id === 'account-b' && call.enabled === true)"), 5000);
  await until("enable refresh completes", () => b.evaluate("window.backupRequests.length === 15"), 5000);
  await b.evaluate(resolveStatus(14, { enabled: true, frequency_minutes: 30, destination_set: true, destination: "account-b-folder", running: false, projects: 1, needs_login: false }));
  await until("enabled switch unlocked before next action", () => b.evaluate("document.querySelector('#backup-enable [role=switch]').getAttribute('aria-checked') === 'true' && !document.querySelector('#backup-enable [role=switch]').disabled"), 5000);

  await b.evaluate(click("#backup-enable [role='switch']"));
  await until("schedule disabled after refresh settled", () => b.evaluate("window.backupCalls.filter((call) => call.method === 'PUT' && call.account_id === 'account-b' && call.enabled === false).length === 2"), 5000);
  await until("disable refresh completes", () => b.evaluate("window.backupRequests.length === 16"), 5000);
  await b.evaluate(resolveStatus(15, { enabled: false, frequency_minutes: 30, destination_set: true, destination: "account-b-folder", running: false, projects: 1, needs_login: false }));

  await b.evaluate("void window.refreshBackupStatus()");
  await until("status error refresh requested", () => b.evaluate("window.backupRequests.length === 17"), 5000);
  await b.evaluate("window.backupRequests[16].reject(new Error('service offline'))");
  await until("status error shown", () => b.evaluate("document.body.innerText.includes('service offline')"), 5000);
  assert.match(await b.evaluate("document.body.innerText"), /Backup status unavailable/);
  assert.equal(await b.evaluate(visibleControls), true, "backup controls remain visible when status loading fails");
  assert.equal(await b.evaluate("document.querySelector('#backup-enable [role=switch]').disabled && document.querySelector('#backup-destination input').disabled && document.querySelector('#backup-destination button').disabled && document.querySelector('#backup-frequency select').disabled && document.querySelector('.backup-status button').disabled"), true, "status failure leaves all backup actions disabled");
  assert.doesNotMatch(await b.evaluate("document.body.innerText"), /Backups are off/i, "unknown status is not described as backups being off");

  await b.evaluate("void window.refreshBackupStatus()");
  await until("valid status recovery requested", () => b.evaluate("window.backupRequests.length === 18"), 5000);
  await b.evaluate(resolveStatus(17, { enabled: true, frequency_minutes: 30, destination_set: true, destination: "recovered-folder", running: false, projects: 1, needs_login: false }));
  await until("valid status recovered", () => b.evaluate("document.querySelector('#backup-destination input').value === 'recovered-folder'"), 5000);

  console.log("backups-settings-browser: persistent backup settings, disabled signed-out/loading/error states, automatic reauthorization, enable-to-authorize, and account-scoped stale replies passed");
} finally {
  if (b) await b.close();
  server.close();
  removeTemporary(temporary);
}
