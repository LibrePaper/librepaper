// End-to-end check for the main Settings dialog's local companion controls.
// Uses a real companion process and browser with isolated HOME/state/cache.
// Run with LIBREPAPER_TEST_BINARY=dist/librepaper.

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until } from "../../tools/browser-driver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const binary = process.env.LIBREPAPER_TEST_BINARY || join(root, "dist", "librepaper");
if (!existsSync(binary)) {
  console.log(`companion-settings-browser: no binary at ${binary}; skipping (build it or set LIBREPAPER_TEST_BINARY)`);
  process.exit(0);
}

const temporary = mkdtempSync(join(tmpdir(), "librepaper-settings-check-"));
const output = join(temporary, "build");
const entry = join(temporary, "entry.js");
const harness = join(temporary, "Harness.svelte");
const statusMock = join(temporary, "status.svelte.js");
const clientMock = join(temporary, "client.js");
const stateHome = join(temporary, "state");
const cacheHome = join(temporary, "cache");
const appEnv = {
  ...process.env,
  HOME: temporary,
  XDG_STATE_HOME: stateHome,
  XDG_CACHE_HOME: cacheHome,
  XDG_CONFIG_HOME: join(temporary, "config"),
  XDG_DATA_HOME: join(temporary, "data"),
  DBUS_SESSION_BUS_ADDRESS: `unix:path=${join(temporary, "no-dbus", "bus")}`,
  LIBREPAPER_SERVER: "",
};
delete appEnv.DISPLAY;
delete appEnv.WAYLAND_DISPLAY;

async function freePort() {
  const probe = createServer();
  await new Promise((done) => probe.listen(0, "127.0.0.1", done));
  const port = probe.address().port;
  await new Promise((done) => probe.close(done));
  return port;
}

writeFileSync(harness, `
<script>
  import SettingsDialog from ${JSON.stringify(join(root, "web/src/components/settings/SettingsDialog.svelte"))};
  let open = $state(true);
  let category = $state("local");
  window.showSettingsCategory = (next) => category = next;
</script>
<SettingsDialog bind:open bind:category sourceFormat="quarto" mayEdit={true} userId="browser-check"
                remoteConnected={true} />
`);

writeFileSync(statusMock, `
let current = $state.raw({ state: "unreachable", address: "http://127.0.0.1:8763/", instance: null });
export const companion = { get status() { return current; }, watch() { return () => {}; } };
`);

writeFileSync(clientMock, `
export const DEFAULT_ADDRESS = "http://127.0.0.1:8763/";
export function address() { return DEFAULT_ADDRESS; }
export function setAddress() {}
export function probe() { return Promise.resolve(); }
export function retry() { return Promise.resolve(); }
export function disconnect() { return Promise.resolve(); }
export function connectApp() { return Promise.resolve(); }
export function capabilities() { return Promise.resolve({}); }
export function settings() { return Promise.resolve({ standalone: false, integrations: {} }); }
export function setStartup() { return Promise.resolve(); }
export function quit() { return Promise.resolve(); }
`);

writeFileSync(entry, `
import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
import { mount } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import * as control from ${JSON.stringify(join(root, "web/src/lib/companion/control.js"))};
import Harness from ${JSON.stringify(harness)};
await control.intake();
mount(Harness, { target: document.body });
`);

const mockModules = {
  name: "companion-settings-browser-test-mocks",
  enforce: "pre",
  resolveId(source) {
    if (source.endsWith("/lib/companion/status.svelte.js")) return statusMock;
    if (source.endsWith("/lib/companion/client.js")) return clientMock;
    return null;
  },
};

let app;
let appDone;
let b;
let server;
let appLog = "";

async function captureSettingsScreenshot() {
  if (!b || !process.env.LIBREPAPER_SETTINGS_SCREENSHOT) return;
  const shot = await b.command("Page.captureScreenshot", { format: "png", captureBeyondViewport: true });
  const screenshot = resolve(process.env.LIBREPAPER_SETTINGS_SCREENSHOT);
  mkdirSync(dirname(screenshot), { recursive: true });
  writeFileSync(screenshot, Buffer.from(shot.data, "base64"));
}

try {
  await build({
    configFile: false,
    root: join(root, "web"),
    logLevel: "error",
    plugins: [mockModules, svelte(), tailwindcss()],
    build: {
      outDir: output,
      emptyOutDir: true,
      lib: { entry, formats: ["es"], fileName: () => "check.js", cssFileName: "check" },
      rollupOptions: { output: { codeSplitting: false } },
    },
  });

  const page = `<!doctype html><html data-theme="librepaper"><head><meta charset="utf-8"><link rel="stylesheet" href="/check.css"><title>LibrePaper Settings</title></head><body><script type="module" src="/check.js"></script></body></html>`;
  server = createServer((request, response) => {
    if (request.url === "/check.js") {
      response.setHeader("content-type", "text/javascript");
      response.end(readFileSync(join(output, "check.js")));
    } else if (request.url === "/check.css") {
      response.setHeader("content-type", "text/css");
      response.end(readFileSync(join(output, "check.css")));
    } else {
      response.setHeader("content-type", "text/html");
      response.end(page);
    }
  });
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
  const appPort = server.address().port;
  const appOrigin = `http://127.0.0.1:${appPort}`;
  appEnv.LIBREPAPER_SERVER = `${appOrigin}/`;

  const companionPort = await freePort();
  const companionAddress = `http://127.0.0.1:${companionPort}/`;
  app = spawn(binary, ["start", "--foreground", "--port", String(companionPort)], {
    env: appEnv,
    stdio: ["ignore", "pipe", "pipe"],
  });
  appDone = new Promise((done) => app.once("exit", done));
  app.stdout.on("data", (chunk) => { appLog += chunk; });
  app.stderr.on("data", (chunk) => { appLog += chunk; });

  await until("companion health", async () => {
    const response = await fetch(`${companionAddress}librepaper/local/health`).catch(() => null);
    return Boolean(response && response.ok);
  }, 15000).catch((error) => { throw new Error(`${error.message}\ncompanion output:\n${appLog}`); });

  const tokenPath = join(stateHome, "librepaper", "local", "control-token.json");
  await until("private Settings credential", () => existsSync(tokenPath), 5000);
  const credential = JSON.parse(readFileSync(tokenPath, "utf8"));
  assert.equal(credential.server, `${appOrigin}/`, "the app target is persisted with the credential");
  assert.equal(credential.instance.length, 16);
  assert.equal(credential.token.length, 43);

  b = await browser("chromium", join(temporary, "chrome"), await freePort());
  const fragment = new URLSearchParams({
    settings: "local",
    companion_address: companionAddress,
    companion_control: credential.token,
    companion_instance: credential.instance,
  });
  await b.navigate(`${appOrigin}/#${fragment}`);
  await until("real companion Settings sections", () => b.evaluate(`[
    "companion-approvals-heading", "companion-sites-heading", "companion-folders-heading",
    "companion-activity-heading", "companion-agents-heading", "companion-tool-paths-heading"
  ].every((id) => document.querySelector("#" + id))`), 12000);

  assert.equal(await b.evaluate("location.hash"), "", "the secret handoff fragment is removed");
  assert.equal(await b.evaluate(`document.documentElement.innerHTML.includes(${JSON.stringify(credential.token)})`), false, "the token is not rendered into the page");
  assert.equal(await b.evaluate('document.querySelectorAll(".request-card").length'), 0, "approvals use existing SettingRow layout");
  assert.equal(await b.evaluate('document.querySelector("#tray-enabled") === null'), true, "there is no tray preference");
  assert.equal(await b.evaluate('document.querySelectorAll(".settings-subsection").length >= 6'), true, "management is organized under Settings sections");

  for (const path of ["/companion", "/companion/", "/companion/index.html", "/companion/app.js", "/companion/style.css"]) {
    const response = await fetch(`${companionAddress.replace(/\/$/, "")}${path}`);
    assert.equal(response.status, 404, `standalone companion page is gone: ${path}`);
  }

  const apiState = async (headers = {}) => fetch(`${companionAddress}companion/api/state`, {
    headers: { Origin: appOrigin, Authorization: `Bearer ${credential.token}`, ...headers },
  });
  const trusted = await apiState();
  assert.equal(trusted.status, 200, "the configured app Origin can use the control API");
  const foreign = await apiState({ Origin: "https://untrusted.example" });
  assert.equal(foreign.status, 403, "the credential is bound to the configured app Origin");
  const missingOrigin = await fetch(`${companionAddress}companion/api/state`, {
    headers: { Authorization: `Bearer ${credential.token}` },
  });
  assert.equal(missingOrigin.status, 403, "management requires an Origin header");

  await until("Settings control state loaded", () => b.evaluate(`
    document.querySelector('[aria-label="Extra tool search folders"]')?.disabled === false
  `), 10000);
  await captureSettingsScreenshot();

  const requestId = Buffer.from(`settings-allow-${Date.now()}`).toString("base64url").padEnd(32, "x");
  const challenge = "a".repeat(64);
  async function askForPair(id) {
    const response = await fetch(`${companionAddress}librepaper/local/pair/request`, {
      method: "POST",
      headers: { "content-type": "application/json", Origin: appOrigin },
      body: JSON.stringify({
        origin: appOrigin,
        request: id,
        challenge,
        return: `${appOrigin}/document`,
      }),
    });
    assert.equal(response.status, 202, `pair request queued: ${await response.text()}`);
  }
  async function pairStatus(id) {
    return fetch(`${companionAddress}librepaper/local/pair/status?request=${encodeURIComponent(id)}`)
      .then((response) => response.status);
  }
  async function waitForApproval() {
    await until("approval visible in Settings", () => b.evaluate(`
      [...document.querySelector("#companion-approvals-heading").closest("section").querySelectorAll(".setting-title")]
        .some((node) => /Connect|Pair/i.test(node.textContent))
    `), 10000);
  }
  async function waitForEnabledButton(label, scopeId) {
    await until(`${label} action enabled`, () => b.evaluate(`(() => {
      const scope = ${JSON.stringify(scopeId)}
        ? document.querySelector(${JSON.stringify(scopeId)}).closest("section")
        : document;
      return [...scope.querySelectorAll("button")].some((button) =>
        button.textContent.trim() === ${JSON.stringify(label)} && !button.disabled
      );
    })()`), 10000);
  }
  async function decide(label, decision, expected) {
    const id = `${requestId.slice(0, 26)}${label.padEnd(6, "x")}`;
    await askForPair(id);
    await waitForApproval();
    await waitForEnabledButton(decision === "allow" ? "Connect" : "Deny", "#companion-approvals-heading");
    await b.evaluate(`(() => {
      const section = document.querySelector("#companion-approvals-heading").closest("section");
      const row = [...section.querySelectorAll(".setting-row")].find((item) => /Connect|Pair/i.test(item.textContent));
      const labels = ${JSON.stringify(decision === "allow" ? ["Allow", "Connect"] : ["Deny"])};
      const action = [...row.querySelectorAll("button")].find((item) => labels.includes(item.textContent.trim()));
      if (!action) throw new Error("Could not find the " + ${JSON.stringify(decision)} + " action in Settings approvals");
      action.click();
    })()`);
    await until(`pair ${decision} result`, async () => (await pairStatus(id)) === expected, 10000);
    if (decision === "allow") {
      await until("approved request removed from Settings", () => b.evaluate(`
        ![...document.querySelector("#companion-approvals-heading").closest("section").querySelectorAll(".setting-title")]
          .some((node) => /Connect|Pair/i.test(node.textContent))
      `), 10000);
    }
  }

  await decide("allow", "allow", 200);
  const deniedId = `${requestId.slice(0, 26)}denyxx`;
  await askForPair(deniedId);
  await waitForApproval();
  await waitForEnabledButton("Deny", "#companion-approvals-heading");
  await b.evaluate(`(() => {
    const section = document.querySelector("#companion-approvals-heading").closest("section");
    const row = [...section.querySelectorAll(".setting-row")].find((item) => /Connect|Pair/i.test(item.textContent));
    [...row.querySelectorAll("button")].find((item) => item.textContent.trim() === "Deny").click();
  })()`);
  await until("pair deny result", async () => (await pairStatus(deniedId)) === 403, 10000);

  const paths = join(temporary, "extra-tools");
  await b.evaluate(`(() => {
    const field = document.querySelector('[aria-label="Extra tool search folders"]');
    field.value = ${JSON.stringify(paths)};
    field.dispatchEvent(new Event("input", { bubbles: true }));
  })()`);
  await new Promise((done) => setTimeout(done, 5500));
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Extra tool search folders"]').value`), paths, "polling does not overwrite an in-progress edit");
  await waitForEnabledButton("Save folders", "#companion-tool-paths-heading");
  await b.evaluate(`[...document.querySelectorAll("button")].find((item) => item.textContent.trim() === "Save folders").click()`);
  await until("tool folder persisted", async () => {
    const response = await apiState();
    if (!response.ok) return false;
    return (await response.json()).settings.tool_paths.includes(paths);
  }, 10000);

  const agentLabel = `Settings browser ${Date.now()}`;
  await b.evaluate(`(() => {
    const section = document.querySelector("#companion-agents-heading").closest("section");
    section.querySelector('input[name="label"]').value = ${JSON.stringify(agentLabel)};
    section.querySelector('input[name="label"]').dispatchEvent(new Event("input", { bubbles: true }));
    section.querySelector('input[name="command"]').value = ${JSON.stringify(process.execPath)};
    section.querySelector('input[name="command"]').dispatchEvent(new Event("input", { bubbles: true }));
    section.querySelector('textarea[name="args"]').value = "--version";
    section.querySelector('textarea[name="args"]').dispatchEvent(new Event("input", { bubbles: true }));
  })()`);
  await waitForEnabledButton("Add agent", "#companion-agents-heading");
  await b.evaluate(`(() => {
    const section = document.querySelector("#companion-agents-heading").closest("section");
    [...section.querySelectorAll("button")].find((item) => item.textContent.trim() === "Add agent").click();
  })()`);
  let agentId = "";
  await until("custom agent added through Settings", async () => {
    const response = await apiState();
    if (!response.ok) return false;
    const agent = (await response.json()).custom_agents.find((item) => item.label === agentLabel);
    if (!agent) return false;
    agentId = agent.id;
    return b.evaluate(`(() => {
      const section = document.querySelector("#companion-agents-heading").closest("section");
      return [...section.querySelectorAll(".setting-title")].filter((item) => item.textContent === ${JSON.stringify(agentLabel)}).length === 1;
    })()`);
  }, 10000);
  await b.evaluate("window.confirm = () => true");
  await waitForEnabledButton("Remove", "#companion-agents-heading");
  await b.evaluate(`(() => {
    const section = document.querySelector("#companion-agents-heading").closest("section");
    const row = [...section.querySelectorAll(".setting-row")].find((item) => item.textContent.includes(${JSON.stringify(agentLabel)}));
    row.querySelector("button").click();
  })()`);
  await until("custom agent removed through Settings", async () => {
    const response = await apiState();
    return response.ok && !(await response.json()).custom_agents.some((item) => item.id === agentId);
  }, 10000);

  await until("allowed site visible in Settings", () => b.evaluate(`
    [...document.querySelector("#companion-sites-heading").closest("section").querySelectorAll(".setting-row")]
      .some((item) => item.textContent.includes(${JSON.stringify(appOrigin)}))
  `), 10000);
  await waitForEnabledButton("Revoke", "#companion-sites-heading");
  await b.evaluate(`(() => {
    const section = document.querySelector("#companion-sites-heading").closest("section");
    const row = [...section.querySelectorAll(".setting-row")].find((item) => item.textContent.includes(${JSON.stringify(appOrigin)}));
    if (!row) throw new Error("allowed site did not appear in Settings");
    row.querySelector("button").click();
  })()`);
  await until("connected site revoked through Settings", async () => {
    const response = await apiState();
    return response.ok && !(await response.json()).pairings.some((item) => item.origin === appOrigin);
  }, 10000);

  await b.evaluate('window.showSettingsCategory("render")');
  await until("real Quarto integration in Render", () => b.evaluate(`
    document.querySelector(".settings-category")?.textContent.trim() === "Render"
      && document.querySelector("#quarto-executable input")?.disabled === false
  `), 10000);
  const quartoPath = process.execPath;
  const quartoArgs = "--profile=test --verbose";
  await b.evaluate(`(() => {
    const path = document.querySelector("#quarto-executable input");
    path.value = ${JSON.stringify(quartoPath)};
    path.dispatchEvent(new Event("input", { bubbles: true }));
    const args = document.querySelector("#quarto-arguments input");
    args.value = ${JSON.stringify(quartoArgs)};
    args.dispatchEvent(new Event("input", { bubbles: true }));
  })()`);
  await waitForEnabledButton("Save", "#quarto-executable");
  await b.evaluate(`(() => {
    const section = document.querySelector("#quarto-executable").closest(".settings-subsection");
    [...section.querySelectorAll(".integration-actions button")].find((item) => item.textContent.trim() === "Save").click();
  })()`);
  await until("Quarto integration path and arguments persisted", async () => {
    const response = await apiState();
    if (!response.ok) return false;
    const quarto = (await response.json()).settings.integrations.quarto;
    return quarto.path === quartoPath && quarto.args.join(" ") === quartoArgs;
  }, 10000);

  await captureSettingsScreenshot();

  console.log("companion-settings-browser: trusted fragment intake, unified Settings management, approval decisions, folder/tool saves, agent/site management and removed standalone page passed");
} finally {
  if (b) await b.close();
  if (app && app.exitCode === null) {
    app.kill();
    await Promise.race([appDone, new Promise((done) => setTimeout(done, 2000))]);
  }
  if (server) server.close();
  rmSync(temporary, { recursive: true, force: true });
}
