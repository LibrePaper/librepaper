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
// The redirect is not loaded: the consent page sends the browser there with
// location.assign, which nothing in this driver can intercept, so the check
// reads the reply the page received from /api/oauth/approve (wrapped in the
// page before Approve is pressed) and then waits for the page to leave the
// consent screen. Nothing listens on port 9, so the navigation fails once the
// reply is read.
//
// Needs: LIBREPAPER_BIN (or LIBREPAPER_TEST_BINARY, or target/debug/librepaper),
// a Postgres (LIBREPAPER_TEST_POSTGRES_URL) and Chromium. Skips politely when the
// binary or Postgres is missing.
import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import { existsSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { browser, removeTemporary, until } from "../helpers/browser-driver.mjs";
import { deploymentBinary, startDeployment } from "../helpers/deployment.mjs";

const name = "connect-browser";
const log = (line) => console.log(`${name}: ${line}`);
const CLIENT_NAME = "Test app";
const REDIRECT = "http://127.0.0.1:9/cb";
const STATE = "xyz";

const binary = process.env.LIBREPAPER_BIN ? resolve(process.env.LIBREPAPER_BIN) : deploymentBinary();
if (!existsSync(binary)) {
  log(`no librepaper binary at ${binary}; skipping (set LIBREPAPER_BIN)`);
  process.exit(0);
}

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

const deployment = await startDeployment({ label: "connect_browser" });
if (!deployment) {
  log("no binary; skipping");
  process.exit(0);
}
if (deployment.unavailable) {
  log(`${deployment.unavailable}; skipping`);
  process.exit(0);
}

const scratch = mkdtempSync(join(tmpdir(), "librepaper-connect-check-"));
let tab;
try {
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

  // Keep the answer to Approve, without changing what the page does with it.
  await tab.evaluate(`(() => {
    const original = window.fetch;
    window.approveReply = null;
    window.fetch = async (...args) => {
      const response = await original(...args);
      if (String(args[0]) === "/api/oauth/approve") window.approveReply = await response.clone().json();
      return response;
    };
  })()`);

  await tab.evaluate(`[...document.querySelectorAll("button")].find((node) => node.textContent.trim() === "Approve").click()`);
  const reply = await until("the approve reply", () => tab.evaluate(`window.approveReply`), 30000);
  const target = new URL(reply.redirect);
  assert.equal(`${target.origin}${target.pathname}`, REDIRECT, "the reply sends the person back to the app's redirect URI");
  assert.ok(target.searchParams.get("code"), "the redirect carries a code");
  assert.equal(target.searchParams.get("state"), STATE, "the redirect echoes state");
  await until("the page to leave the consent screen", async () => (await tab.evaluate("location.href")) !== consent, 15000);

  /* ------------------------------------------------------- the Agent panel */

  await tab.navigate(`${deployment.browserBase}/docs/${slug}`);
  await until("the project", () => tab.evaluate(`Boolean(document.querySelector(".cm-content") || document.querySelector(".explorer-name"))`), 45000);
  await until("the Agent rail button", () => tab.evaluate(`[...document.querySelectorAll('[aria-label="Agent"]')].some((node) => node.getClientRects().length > 0)`), 30000);
  await tab.evaluate(`[...document.querySelectorAll('[aria-label="Agent"]')].find((node) => node.getClientRects().length > 0).click()`);
  await until("the connector block", () => tab.evaluate(`Boolean(document.querySelector(".agent-panel .agent-connector-url"))`), 30000);
  const connectorUrl = await tab.evaluate(`document.querySelector(".agent-panel .agent-connector-url").value`);
  assert.equal(connectorUrl, `${deployment.browserBase}/mcp/${slug}`, "the panel shows the connector URL for this project");
  assert.equal(await tab.evaluate(`document.querySelector(".agent-panel .agent-connector-url").readOnly`), true, "the URL is read-only");
  await until("the new connection in the list", () => tab.evaluate(`document.querySelector(".agent-panel")?.textContent.includes(${JSON.stringify(CLIENT_NAME)})`), 30000);

  log(`consent page: heading, Suggest preselected, Approve sends code and state to the app; the Agent panel shows ${connectorUrl} and lists "${CLIENT_NAME}"`);
} finally {
  await tab?.close();
  await deployment.stop();
  removeTemporary(scratch);
}
