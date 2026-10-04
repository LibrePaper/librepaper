// End-to-end check for the private companion dashboard and its loopback API.
// Uses a real companion process and browser with isolated HOME/state/cache.
// Run with LIBREPAPER_TEST_BINARY=dist/librepaper; never run this against a
// person's existing companion state.

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until } from "../../tools/browser-driver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const binary = process.env.LIBREPAPER_TEST_BINARY || join(root, "dist", "librepaper");
if (!existsSync(binary)) {
  console.log(`companion-dashboard-browser: no binary at ${binary}; skipping (build it or set LIBREPAPER_TEST_BINARY)`);
  process.exit(0);
}

const temporary = mkdtempSync(join(tmpdir(), "librepaper-dashboard-check-"));
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
};
delete appEnv.DISPLAY;
delete appEnv.WAYLAND_DISPLAY;

async function freePort() {
  const server = createServer();
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
  const port = server.address().port;
  await new Promise((done) => server.close(done));
  return port;
}

const port = await freePort();
const address = `http://127.0.0.1:${port}`;
const app = spawn(binary, ["start", "--foreground", "--port", String(port)], { env: appEnv, stdio: ["ignore", "pipe", "pipe"] });
const appDone = new Promise((done) => app.once("exit", done));
let appLog = "";
app.stdout.on("data", (chunk) => { appLog += chunk; });
app.stderr.on("data", (chunk) => { appLog += chunk; });
let b;

try {
  await until("companion dashboard", async () => {
    const response = await fetch(`${address}/companion/`).catch(() => null);
    return Boolean(response && response.ok);
  }, 15000).catch((error) => { throw new Error(`${error.message}\ncompanion output:\n${appLog}`); });
  const tokenFile = join(stateHome, "librepaper", "local", "control-token.json");
  await until("private dashboard token fixture", () => existsSync(tokenFile), 5000);
  const control = JSON.parse(readFileSync(tokenFile, "utf8"));
  assert.equal(typeof control.token, "string");
  assert.ok(control.token.length >= 32);

  b = await browser("chromium", join(temporary, "chrome"), await freePort());
  await b.navigate(`${address}/companion/`);
  await until("open dashboard instructions", () => b.evaluate('document.querySelector("#locked") && !document.querySelector("#locked").hidden'));
  assert.match(await b.evaluate('document.querySelector("#locked").innerText'), /librepaper desktop/);

  // Force a document navigation so the deferred dashboard script reads the
  // bootstrap fragment; changing only the hash would not reload this page.
  await b.navigate("about:blank");
  await b.navigate(`${address}/companion/#token=${encodeURIComponent(control.token)}`);
  await until("authenticated dashboard", () => b.evaluate('document.querySelector("#dashboard") && !document.querySelector("#dashboard").hidden'));
  await until("companion status loaded", () => b.evaluate('document.querySelector("#connection-label").textContent === "Companion is running"'));
  assert.equal(await b.evaluate("location.hash"), "", "the bootstrap fragment is removed immediately");
  assert.equal(await b.evaluate(`sessionStorage.getItem("librepaper-companion-control") === ${JSON.stringify(control.token)}`), true, "the private token is kept in this tab's session storage");
  assert.equal(await b.evaluate('document.querySelector("#connection-label").textContent'), "Companion is running");
  const auth = await b.evaluate(`fetch("/companion/api/state", {headers:{Authorization:"Bearer " + sessionStorage.getItem("librepaper-companion-control")}}).then(r => r.status)`);
  assert.equal(auth, 200, "the UI's token authorizes status requests");

  // A website cannot use the dashboard bearer token. The authenticated API
  // rejects an unrelated Origin even when it presents the same bearer.
  const foreign = await fetch(`${address}/companion/api/state`, {
    headers: { Authorization: `Bearer ${control.token}`, Origin: "https://untrusted.example" },
  });
  assert.ok([401, 403].includes(foreign.status), `foreign origin was rejected (${foreign.status})`);

  const requestId = (suffix) => `${Buffer.from(`dashboard-${suffix}-${Date.now()}`).toString("base64url")}abcdefghijklmnop`;
  async function askForPair(label) {
    const id = requestId(label);
    const challenge = "a".repeat(64);
    const response = await fetch(`${address}/librepaper/local/pair/request`, {
      method: "POST",
      headers: { "content-type": "application/json", origin: address },
      body: JSON.stringify({ origin: address, request: id, challenge, return: `${address}/companion/` }),
    });
    assert.equal(response.status, 202, `pair request queued: ${await response.text()}`);
    return id;
  }
  async function waitForApproval() {
    await until("dashboard approval card", async () => b.evaluate('document.querySelectorAll("#approvals .request-card").length === 1'), 8000);
  }
  async function pairingStatus(id) {
    const response = await fetch(`${address}/librepaper/local/pair/status?request=${encodeURIComponent(id)}`);
    return response.status;
  }

  const allowedRequest = await askForPair("allow");
  await waitForApproval();
  const approvalTitle = await b.evaluate('document.querySelector("#approvals .request-card h3").textContent');
  assert.match(approvalTitle, /Connect/i);
  await b.evaluate('document.querySelector("#approvals .request-card .button.primary").click()');
  await until("pair request allowed", async () => (await pairingStatus(allowedRequest)) === 200, 8000);
  await until("approval removed after decision", () => b.evaluate('document.querySelectorAll("#approvals .request-card").length === 0'));

  const deniedRequest = await askForPair("deny");
  await waitForApproval();
  await b.evaluate('document.querySelector("#approvals .request-card .button.quiet").click()');
  await until("pair request denied", async () => (await pairingStatus(deniedRequest)) === 403, 8000);

  // Reusing a consumed approval is stale. The API keeps one-time decisions
  // idempotently closed and the dashboard reports that refusal clearly.
  const stale = await fetch(`${address}/companion/api/approvals/stale-approval-id`, {
    method: "POST", headers: { "content-type": "application/json", Authorization: `Bearer ${control.token}` },
    body: JSON.stringify({ decision: "allow" }),
  });
  assert.equal(stale.status, 404, "an expired or unknown approval is rejected");

  // Exercise the actual settings form and confirm persistence through a fresh
  // state response. Editing remains intact across several status polls.
  await b.evaluate(`(() => {
    const paths = document.querySelector("#tool-paths");
    paths.value = ${JSON.stringify(join(temporary, "extra-tools"))};
    paths.dispatchEvent(new Event("input", {bubbles:true}));
    const tray = document.querySelector("#tray-enabled");
    tray.checked = true;
    tray.dispatchEvent(new Event("input", {bubbles:true}));
    const integration = document.querySelector('.integration-card[data-integration="quarto"]');
    const executable = integration.querySelector('[data-setting="path"]');
    executable.value = ${JSON.stringify("/usr/bin/quarto")};
    executable.dispatchEvent(new Event("input", {bubbles:true}));
    const args = integration.querySelector('[data-setting="args"]');
    args.value = ${JSON.stringify("--verbose\n--profile=test")};
    args.dispatchEvent(new Event("input", {bubbles:true}));
  })()`);
  await new Promise((done) => setTimeout(done, 4300));
  assert.equal(await b.evaluate('document.querySelector("#tool-paths").value'), join(temporary, "extra-tools"), "polling does not overwrite an in-progress settings edit");
  assert.equal(await b.evaluate('document.querySelector(".integration-card[data-integration=quarto] [data-setting=path]").value'), "/usr/bin/quarto", "polling preserves the integration path field");
  assert.equal(await b.evaluate('document.querySelector(".integration-card[data-integration=quarto] [data-setting=args]").value'), "--verbose\n--profile=test", "polling preserves integration arguments");
  await b.evaluate('document.querySelector("#settings-form").requestSubmit()');
  await until("settings saved", async () => {
    const response = await fetch(`${address}/companion/api/state`, { headers: { Authorization: `Bearer ${control.token}` } });
    if (!response.ok) return false;
    const state = await response.json();
    const quarto = state.settings.integrations.quarto;
    return state.settings.tray_enabled === true
      && state.settings.tool_paths.includes(join(temporary, "extra-tools"))
      && quarto.path === "/usr/bin/quarto"
      && quarto.args.join("\n") === "--verbose\n--profile=test";
  }, 8000);

  const testAgent = `Dashboard check ${Date.now()}`;
  await b.evaluate(`(() => {
    document.querySelector("#agent-name").value = ${JSON.stringify(testAgent)};
    document.querySelector("#agent-command").value = "/bin/echo";
    document.querySelector("#agent-args").value = "hello";
    document.querySelector("#agent-form").requestSubmit();
  })()`);
  let agentId = "";
  await until("custom agent persisted and shown", async () => {
    const response = await fetch(`${address}/companion/api/state`, { headers: { Authorization: `Bearer ${control.token}` } });
    if (!response.ok) return false;
    const state = await response.json();
    const agent = state.custom_agents.find((entry) => entry.label === testAgent);
    if (!agent) return false;
    agentId = agent.id;
    return await b.evaluate(`document.querySelector("#agents").innerText.includes(${JSON.stringify(testAgent)})`);
  }, 8000);
  await b.evaluate('window.confirm = () => true');
  await b.evaluate(`(() => {
    const card = [...document.querySelectorAll("#agents .list-card")].find((item) => item.innerText.includes(${JSON.stringify(testAgent)}));
    card.querySelector("button").click();
  })()`);
  await until("custom agent removed", async () => {
    const response = await fetch(`${address}/companion/api/state`, { headers: { Authorization: `Bearer ${control.token}` } });
    return response.ok && !(await response.json()).custom_agents.some((entry) => entry.id === agentId);
  }, 8000);

  await b.evaluate(`(() => {
    const card = [...document.querySelectorAll("#pairings .list-card")].find((item) => item.innerText.includes(${JSON.stringify(address)}));
    if (!card) throw new Error("allowed site did not appear in the dashboard");
    card.querySelector("button").click();
  })()`);
  await until("site access revoked from dashboard", async () => {
    const response = await fetch(`${address}/companion/api/state`, { headers: { Authorization: `Bearer ${control.token}` } });
    return response.ok && !(await response.json()).pairings.some((entry) => entry.origin === address);
  }, 8000);

  if (process.env.LIBREPAPER_SCREENSHOT) {
    const shot = await b.command("Page.captureScreenshot", { format: "png", captureBeyondViewport: true });
    writeFileSync(resolve(process.env.LIBREPAPER_SCREENSHOT), Buffer.from(shot.data, "base64"));
  }

  // Feed a hostile-looking state record through the page renderer once. It
  // must remain text, never become markup or execute an event handler.
  await b.evaluate(`(() => {
    const realFetch = window.fetch.bind(window);
    window.fetch = (input, init) => String(input).includes("/companion/api/state")
      ? Promise.resolve(new Response(JSON.stringify({version:"test",standalone:true,tools:{tools:{},platform:"test"},pairings:[],bindings:[],approvals:[{id:"xss",title:"<img src=x onerror=window.__xss=1>",message:"<script>window.__xss=2</script>",allow_label:"Allow"}],jobs:[{id:"job-x",kind:"Quarto",status:"running",stage:"Rendering",log_tail:"recent output"}],previews:[],agents:[],sessions:[],settings:{tool_paths:[],tray_enabled:true,integrations:[],custom_agents:[]}}), {headers:{"content-type":"application/json"}}))
      : realFetch(input, init);
  })()`);
  await b.evaluate('document.dispatchEvent(new Event("visibilitychange"))');
  await until("hostile text rendered", () => b.evaluate('document.querySelector("#approvals h3")?.textContent.includes("<img")'));
  assert.equal(await b.evaluate('document.querySelector("#approvals img, #approvals script") !== null'), false, "untrusted state is rendered with textContent");
  assert.equal(await b.evaluate("window.__xss || 0"), 0, "untrusted state did not execute");
  await b.evaluate('document.querySelector("#activity details summary").click()');
  await new Promise((done) => setTimeout(done, 2300));
  assert.equal(await b.evaluate('document.querySelector("#activity details").open'), true, "polling preserves expanded bounded logs");

  console.log("companion-dashboard-browser: private bootstrap, auth, approvals, settings and agent CRUD, access revocation, safe text rendering and polling stability passed");
} finally {
  if (b) await b.close();
  if (app.exitCode === null) {
    app.kill();
    await Promise.race([
      appDone,
      new Promise((done) => setTimeout(done, 2000)),
    ]);
  }
  rmSync(temporary, { recursive: true, force: true });
}
