// Real-server acceptance for editor history, shared reader comparisons, and restore.
// Run with `npm run check:e2e`; requires the built binary, Chromium, and a
// disposable PostgreSQL connection in LIBREPAPER_TEST_POSTGRES_URL.
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { browser, until } from "../helpers/browser-driver.mjs";
import { deploymentBinary, startDeployment } from "../helpers/deployment.mjs";

const binary = process.argv[2] || deploymentBinary();
const deployment = await startDeployment({ label: "history_browser", binary });
if (!deployment || deployment.unavailable) {
  throw new Error(deployment?.unavailable || `no librepaper binary at ${binary}; build it first`);
}

const profileRoot = mkdtempSync(join(tmpdir(), "librepaper-history-browser-"));
const port = 31000 + Math.floor(Math.random() * 20000);
let owner;
let reader;
const original = "# History acceptance\n\nThe red fox watches the quiet river.\n\nA paragraph to remove.\n";
const revised = "# History acceptance\n\nThe blue fox watches the quiet river.\n\nA newly added paragraph.\n";
const cookie = deployment.cookie;

async function request(path, method = "GET", body) {
  const response = await fetch(new URL(path, deployment.base), {
    method,
    headers: { "content-type": "application/json", "x-librepaper-client": "1", cookie },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  const text = await response.text();
  let value;
  try { value = JSON.parse(text); } catch { value = text; }
  assert.ok(response.ok, `${method} ${path}: ${response.status} ${text}`);
  return value;
}

async function main() {
  owner = await browser("chromium", join(profileRoot, "owner"), port);
  await owner.setCookie("librepaper_session", cookie.slice(cookie.indexOf("=") + 1), deployment.base);
  await owner.navigate(deployment.base);
  await owner.resize(1400, 900);

  const document = await deployment.publish({ title: "History acceptance", source_format: "markdown", source: original });
  const slug = document.slug;
  const shareUrl = new URL(document.share_url, deployment.base);
  const historyPath = `/api/documents/${slug}/history`;
  const first = await request(`${historyPath}/current`, "PATCH", {
    label: "Original draft", request_id: randomUUID(),
  });
  assert.ok(first.sha, "the original source receives a named label");

  await owner.navigate(new URL(`/docs/${slug}`, deployment.base).href);
  await until("editor", () => owner.evaluate('!!document.querySelector(".cm-content")'));
  await until("initial source", () => owner.evaluate('document.querySelector(".cm-content")?.textContent.includes("red fox")'));
  await owner.insert(revised, true);
  await until("revised preview", async () => (await owner.text()).includes("blue fox"));
  await until("edited source reached the server", async () => {
    const snapshot = await request(`/api/documents/${slug}/snapshot`);
    return snapshot.texts?.[snapshot.main] === revised || snapshot.source === revised;
  });
  await request(`/api/documents/${slug}/comments`, "POST", {
    type: "comment", exact: "blue fox", body: "Review the revision.",
  });
  const second = await request(`${historyPath}/current`, "PATCH", {
    label: "Revised draft", request_id: randomUUID(),
  });
  assert.ok(second.sha);
  assert.notEqual(second.sha, first.sha);

  // A clean browser profile exercises read-link access without the owner's
  // session cookie. The history panel supplies the visible word comparison.
  reader = await browser("chromium", join(profileRoot, "reader"), port + 1);
  await reader.resize(1400, 900);
  await reader.navigate(shareUrl.href);
  await until("reader preview", async () => (await reader.text()).includes("blue fox"));
  assert.equal(await reader.evaluate('!!document.querySelector(".cm-content")'), false);
  await reader.evaluate('document.querySelector(".sidebar-activity [aria-label=History]")?.click()');
  await until("reader history", () => reader.evaluate('document.body.innerText.includes("Original draft")'));
  await reader.evaluate(`document.querySelector('li[data-sha=${JSON.stringify(first.sha)}] .timeline-point')?.click()`);
  await until("history source comparison", () => reader.evaluate(`(() => {
    const oldText = document.querySelector('.cm-merge-a .cm-content')?.textContent || '';
    const newText = document.querySelector('.cm-merge-b .cm-content')?.textContent || '';
    return oldText.includes('red fox') && newText.includes('blue fox');
  })()`));
  assert.match(await reader.evaluate(`document.querySelector('[aria-label="History file"]')?.selectedOptions[0]?.textContent || ""`), /main\.md · changed/);
  await owner.insert(revised.replace("blue fox", "green fox"), true);
  await until("newer edit is durable", async () => {
    const snapshot = await request(`/api/documents/${slug}/snapshot`);
    return snapshot.texts?.[snapshot.main] === revised.replace("blue fox", "green fox");
  });
  assert.ok(await reader.evaluate(`document.querySelector('.cm-merge-b .cm-content')?.textContent.includes('blue fox')`),
    "the selected comparison keeps its captured current source until refreshed");
  await reader.evaluate(`([...document.querySelectorAll('.history-source-actions button')]
    .find((button) => button.textContent.trim() === 'Refresh comparison')?.click())`);
  await until("refreshed history comparison", () => reader.evaluate(`(() => {
    const newText = document.querySelector('.cm-merge-b .cm-content')?.textContent || '';
    return newText.includes('green fox') && !newText.includes('blue fox');
  })()`));
  assert.equal(await reader.evaluate(`[...document.querySelectorAll('button')].some((button) => button.textContent.trim() === 'Restore this version')`), false,
    "a reader has no restore control");

  // Restoring an earlier label creates a new event and updates the live source.
  await owner.navigate(new URL(`/docs/${slug}`, deployment.base).href);
  await owner.evaluate('document.querySelector(".sidebar-activity [aria-label=History]")?.click()');
  await until("owner history", () => owner.evaluate('document.body.innerText.includes("Original draft")'));
  await owner.evaluate(`document.querySelector('li[data-sha=${JSON.stringify(first.sha)}] .timeline-point')?.click()`);
  await until("restore control", () => owner.evaluate(`([...document.querySelectorAll('button')].some(button => button.textContent.trim() === 'Restore this version'))`));
  await owner.evaluate(`([...document.querySelectorAll('button')].find(button => button.textContent.trim() === 'Restore this version')).click()`);
  await until("restore confirmation", () => owner.evaluate('document.body.innerText.includes("Restore this version?")'));
  await owner.evaluate(`([...document.querySelectorAll('button')].find(button => button.textContent.trim() === 'Restore version')).click()`);
  await until("restore label", async () => (await request(historyPath)).labels?.some((label) => label.reason === "restore"));
  const restored = (await request(historyPath)).labels.find((label) => label.reason === "restore");
  assert.ok(restored);
  assert.notEqual(restored.sha, first.sha);
  const restoredTree = await request(`${historyPath}/${restored.sha}`);
  assert.equal(restoredTree.texts[restoredTree.main], original);
  const live = await request(`/api/documents/${slug}/snapshot`);
  assert.equal(live.texts[live.main], original, "the live document holds the restored source");
  console.log("history e2e: read-link comparison, owner restore, and durable source passed");
}

try {
  await main();
} finally {
  await owner?.close();
  await reader?.close();
  await deployment.stop();
  rmSync(profileRoot, { recursive: true, force: true });
}
