// The connector consent page and the Agent panel's connector entry, end to end:
// a real server, a real Chromium, an owner signed in.
//
//   1. a chat app registers, then asks to work on the owner's project;
//   2. the consent page names the app and the project, and Suggest is preselected;
//   3. Approve sends the decision with every parameter of the request, and the
//      server answers with the redirect back to the app carrying code and state;
//   4. the owner's Agent panel shows the connector URL, and the new connection
//      is listed under the app's name.
//
// The app's redirect URI is a small HTTP server in this process, standing in
// for the chat app: it records the code and state the browser brings back, and
// the test then redeems the code at /oauth/token the way the app would.
//
// Usage: LIBREPAPER_TEST_POSTGRES_URL=<administrative postgres url> \
//        connect-browser.mjs <path-to-librepaper-binary>
// Needs: the binary (the first argument, or the default deployment binary), a
// Postgres (LIBREPAPER_TEST_POSTGRES_URL) and Chromium. Fails when the
// binary or Postgres is missing.
import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import { createServer } from "node:http";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { browser, removeTemporary, until } from "../helpers/browser-driver.mjs";
import { deploymentBinary, startDeployment } from "../helpers/deployment.mjs";

const name = "connect-browser";
const log = (line) => console.log(`${name}: ${line}`);
const CLIENT_NAME = "Test app";
const STATE = "xyz";

const binary = resolve(process.argv[2] || deploymentBinary());

// One multipart upload makes the project; the same shape the transport check uses.
function multipart(title, files) {
  const boundary = `----lp${randomBytes(8).toString("hex")}`;
  const parts = [Buffer.from(`--${boundary}\r\nContent-Disposition: form-data; name="title"\r\n\r\n${title}\r\n`)];
  for (const [path, bytes] of files) {
    parts.push(Buffer.from(`--${boundary}\r\nContent-Disposition: form-data; name="file"; filename="${path}"\r\nContent-Type: application/octet-stream\r\n\r\n`), bytes, Buffer.from("\r\n"));
  }
  parts.push(Buffer.from(`--${boundary}--\r\n`));
  return { body: Buffer.concat(parts), type: `multipart/form-data; boundary=${boundary}` };
}

async function createProject(deployment, title) {
  const { body, type } = multipart(title, [["paper.md", Buffer.from("# Paper\n\nA draft for the connector check.\n")]]);
  const response = await fetch(`${deployment.base}/api/documents`, {
    method: "POST",
    headers: { "content-type": type, "x-librepaper-client": "1", cookie: deployment.cookie },
    body,
  });
  const payload = await response.json().catch(() => ({}));
  if (response.status !== 201) throw new Error(`project upload failed: ${response.status} ${JSON.stringify(payload)}`);
  return payload.slug;
}

// The chat app's callback: the first request the browser makes to it.
const callbacks = [];
let REDIRECT;

// A chat app registers itself. Registration is stateless, so it needs no sign-in.
async function register(deployment) {
  const response = await fetch(`${deployment.base}/oauth/register`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ redirect_uris: [REDIRECT], client_name: CLIENT_NAME }),
  });
  const payload = await response.json().catch(() => ({}));
  if (response.status !== 201 || !payload.client_id) throw new Error(`registration failed: ${response.status} ${JSON.stringify(payload)}`);
  return payload.client_id;
}

const randomPort = () => 19000 + Math.floor(Math.random() * 3000);

const scratch = mkdtempSync(join(tmpdir(), "librepaper-connect-check-"));
let app;
let deployment;
let tab;
try {
  // The listener and the deployment start inside the cleanup scope, so a
  // setup that throws still closes whichever of them came up.
  app = createServer((request, response) => {
    callbacks.push(new URL(request.url, "http://127.0.0.1"));
    response.writeHead(200, { "content-type": "text/plain" });
    response.end("connected");
  });
  await new Promise((done) => app.listen(0, "127.0.0.1", done));
  REDIRECT = `http://127.0.0.1:${app.address().port}/cb`;

  deployment = await startDeployment({ label: "connect_browser", binary });
  // A deployment scenario fails rather than skips, like the others in this
  // suite: the suite provides the binary and Postgres, so a missing one is a fault.
  if (!deployment || deployment.unavailable) {
    const reason = deployment?.unavailable || `no librepaper binary at ${binary}; build it first`;
    deployment = undefined;
    throw new Error(reason);
  }
  const slug = await createProject(deployment, "Connect project");
  const clientId = await register(deployment);

  // PKCE: the verifier stays with the app; the challenge is what the request carries.
  const verifier = randomBytes(32).toString("base64url");
  const challenge = createHash("sha256").update(verifier).digest("base64url");
  const resource = `${deployment.browserBase}/mcp/${slug}`;
  const query = new URLSearchParams({
    response_type: "code",
    client_id: clientId,
    redirect_uri: REDIRECT,
    code_challenge: challenge,
    code_challenge_method: "S256",
    state: STATE,
    resource,
  });
  const consent = `${deployment.browserBase}/oauth/authorize?${query}`;

  tab = await browser(process.env.BROWSER || "chromium", join(scratch, "owner"), randomPort());
  const cookie = deployment.cookie.slice(deployment.cookie.indexOf("=") + 1);
  await tab.setCookie("librepaper_session", cookie, deployment.browserBase);
  await tab.resize(1300, 900);
  await tab.navigate(consent);

  /* ------------------------------------------------------- the consent page */

  await until("the consent page", () => tab.evaluate(`Boolean(document.querySelector('input[name="role"]'))`), 45000);
  const heading = await tab.evaluate(`document.querySelector("h1")?.textContent.trim()`);
  assert.equal(heading, `${CLIENT_NAME} wants to work on Connect project`, "the heading names the app and the project");
  const sentBack = await tab.evaluate(`document.body.innerText`);
  assert.ok(sentBack.includes("127.0.0.1"), "the page says where the person is sent back to");
  const checked = await tab.evaluate(`document.querySelector('input[name="role"]:checked')?.value`);
  assert.equal(checked, "suggest", "Suggest is preselected");
  const roleLabels = await tab.evaluate(`[...document.querySelectorAll('input[name="role"]')].map((node) => node.value).join("|")`);
  assert.equal(roleLabels, "read|comment|suggest|edit", "the four roles, in order");
  assert.ok(await tab.evaluate(`document.body.innerText.includes("Access lasts 30 days")`), "the expiry note is shown");

  await tab.evaluate(`[...document.querySelectorAll("button")].find((node) => node.textContent.trim() === "Approve").click()`);
  const target = await until("the app's callback", () => callbacks[0], 30000);
  assert.equal(target.pathname, "/cb", "the browser is sent back to the app's redirect URI");
  const code = target.searchParams.get("code");
  assert.ok(code, "the redirect carries a code");
  assert.equal(target.searchParams.get("state"), STATE, "the redirect echoes state");

  // The app redeems the code; the connection exists from here on.
  const exchange = await fetch(`${deployment.base}/oauth/token`, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({ grant_type: "authorization_code", code, redirect_uri: REDIRECT, client_id: clientId, code_verifier: verifier }),
  });
  const granted = await exchange.json();
  assert.equal(exchange.status, 200, JSON.stringify(granted));
  assert.equal(granted.scope, "suggest", "the connection has the role the owner chose");
  const mcp = await fetch(`${deployment.base}/mcp/${slug}`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${granted.access_token}` },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list" }),
  });
  const listed = await mcp.json();
  assert.ok(listed.result?.tools?.some((tool) => tool.name === "document_read"), `the token reaches the MCP endpoint: ${JSON.stringify(listed)}`);

  /* ------------------------------------------------------- the Agent panel */

  await tab.navigate(`${deployment.browserBase}/docs/${slug}`);
  await until("the project", () => tab.evaluate(`Boolean(document.querySelector(".cm-content") || document.querySelector(".explorer-name"))`), 45000);
  await until("the Agent rail button", () => tab.evaluate(`[...document.querySelectorAll('[aria-label="Agent"]')].some((node) => node.getClientRects().length > 0)`), 30000);
  await tab.evaluate(`[...document.querySelectorAll('[aria-label="Agent"]')].find((node) => node.getClientRects().length > 0).click()`);
  // The connector lives on the Connect tab, which an owner opens by default; select it so the check does not depend on that.
  await until("the Connect tab", () => tab.evaluate(`[...document.querySelectorAll('.agent-panel [role="tab"]')].some((node) => node.textContent.trim() === "Connect")`), 30000);
  await tab.evaluate(`[...document.querySelectorAll('.agent-panel [role="tab"]')].find((node) => node.textContent.trim() === "Connect").click()`);
  await until("the connector block", () => tab.evaluate(`Boolean(document.querySelector(".agent-panel .agent-connector-url"))`), 30000);
  const connectorUrl = await tab.evaluate(`document.querySelector(".agent-panel .agent-connector-url").value`);
  assert.equal(connectorUrl, `${deployment.browserBase}/mcp/${slug}`, "the panel shows the connector URL for this project");
  assert.equal(await tab.evaluate(`document.querySelector(".agent-panel .agent-connector-url").readOnly`), true, "the URL is read-only");
  await until("the new connection in the list", () => tab.evaluate(`document.querySelector(".agent-panel")?.textContent.includes(${JSON.stringify(CLIENT_NAME)})`), 30000);

  log(`consent page: heading, Suggest preselected, Approve sends code and state to the app, the code redeems for a token that lists the MCP tools; the Agent panel shows ${connectorUrl} and lists "${CLIENT_NAME}"`);
} finally {
  await tab?.close();
  await deployment?.stop();
  app?.close();
  removeTemporary(scratch);
}
