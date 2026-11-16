// Real-server acceptance for owner history comparisons and restore, plus the
// read-link role's lack of a history workspace.
// Run with `npm run check:e2e`; requires the built binary, Chromium, and a
// disposable PostgreSQL connection in LIBREPAPER_TEST_POSTGRES_URL.
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";
import { deploymentBinary, sessionCookie, startDeployment } from "../helpers/deployment.mjs";

const binary = process.argv[2] || deploymentBinary();
const deployment = await startDeployment({ label: "history_browser", binary });
if (!deployment || deployment.unavailable) {
  throw new Error(deployment?.unavailable || `no librepaper binary at ${binary}; build it first`);
}

const profileRoot = mkdtempSync(join(tmpdir(), "librepaper-history-browser-"));
const port = 31000 + Math.floor(Math.random() * 20000);
let owner;
let comparisonOwner;
let reader;
const original = "# History acceptance\n\nThe red fox watches the quiet river.\n\nA paragraph to remove.\n";
const revised = "# History acceptance\n\nThe blue fox watches the quiet river.\n\nA newly added paragraph.\n";
const cookie = deployment.cookie;
const browserBase = deployment.browserBase;

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
  await owner.setCookie("librepaper_session", cookie.slice(cookie.indexOf("=") + 1), browserBase);
  await owner.navigate(browserBase);
  await owner.resize(1400, 900);

  const document = await deployment.publish({ title: "History acceptance", source_format: "markdown", source: original });
  const slug = document.slug;
  const shareUrl = new URL(document.share_url, browserBase);
  const historyPath = `/api/documents/${slug}/history`;
  const first = await request(`${historyPath}/current`, "PATCH", {
    label: "Original draft", request_id: randomUUID(),
  });
  assert.ok(first.sha, "the original source receives a named label");

  await owner.navigate(new URL(`/docs/${slug}`, browserBase).href);
  await until("editor", () => owner.evaluate('!!document.querySelector(".cm-content")'));
  await until("initial source", () => owner.evaluate('document.querySelector(".cm-content")?.textContent.includes("red fox")'));
  await owner.insert(revised, true);
  try {
    await until("revised preview", async () => (await owner.text()).includes("blue fox"));
  } catch (error) {
    const [editorSource, frameUrl, frameBody, snapshot, targets, frames] = await Promise.all([
      owner.evaluate('document.querySelector(".cm-content")?.textContent || ""').catch((cause) => `unavailable: ${cause.message}`),
      owner.evaluate('document.querySelector("iframe[title=Document]")?.src || ""').catch((cause) => `unavailable: ${cause.message}`),
      owner.frameEvaluate("document.body.innerText").catch((cause) => `unavailable: ${cause.message}`),
      request(`/api/documents/${slug}/snapshot`).catch((cause) => `unavailable: ${cause.message}`),
      owner.command("Target.getTargets").catch((cause) => `unavailable: ${cause.message}`),
      owner.command("Page.getFrameTree").catch((cause) => `unavailable: ${cause.message}`),
    ]);
    const truncate = (value) => String(typeof value === "object" ? JSON.stringify(value) : value).slice(0, 1200);
    const framesOnly = targets?.targetInfos?.filter((target) => target.type === "iframe" || target.url.includes(`/raw/${slug}/`)) || targets;
    throw new Error(`${error.message}; editor source: ${truncate(editorSource)}; iframe src: ${truncate(frameUrl)}; frame body: ${truncate(frameBody)}; source snapshot: ${truncate(snapshot)}; matching CDP targets: ${truncate(framesOnly)}; frame tree: ${truncate(frames)}`);
  }
  await until("edited source reached the server", async () => {
    const snapshot = await request(`/api/documents/${slug}/snapshot`);
    return snapshot.texts?.["document.md"] === revised;
  });
  const second = await request(`${historyPath}/current`, "PATCH", {
    label: "Revised draft", request_id: randomUUID(),
  });
  assert.ok(second.sha);
  assert.notEqual(second.sha, first.sha);

  // The read link still needs a registered identity. This distinct account
  // exercises its read-only role without inheriting the owner's edit session.
  reader = await browser("chromium", join(profileRoot, "reader"), port + 1);
  const readerId = deployment.postgres.seedRegisteredAccount({
    provider: "github", subject: "github:history-reader", handle: "history-reader", displayName: "History Reader",
  });
  const readerCookie = sessionCookie({
    dataDirectory: deployment.data, accountId: readerId, handle: "history-reader", name: "History Reader",
  });
  await reader.setCookie("librepaper_session", readerCookie.slice(readerCookie.indexOf("=") + 1), browserBase);
  await reader.resize(1400, 900);
  await reader.navigate(shareUrl.href);
  await until("reader preview", async () => (await reader.text()).includes("blue fox"));
  assert.equal(await reader.evaluate('!!document.querySelector(".cm-content")'), false);
  assert.equal(await reader.evaluate(`Boolean([...document.querySelectorAll('button, a')].some((item) =>
    [item.getAttribute("aria-label"), item.getAttribute("data-tip"), item.textContent.trim()].includes('History')))`), false,
    "a read-link reader has no History workspace");
  assert.equal(await reader.evaluate('!!document.querySelector(".history-workspace")'), false);
  assert.equal(await reader.evaluate(`[...document.querySelectorAll('button')]
    .some((button) => button.textContent.trim() === 'Restore this version')`), false,
    "a read-link reader has no restore control");

  // History is an owner workspace. Keep its captured current side open in a
  // second owner tab while the first tab makes a newer edit.
  comparisonOwner = await browser("chromium", join(profileRoot, "comparison-owner"), port + 2);
  await comparisonOwner.setCookie("librepaper_session", cookie.slice(cookie.indexOf("=") + 1), browserBase);
  await comparisonOwner.resize(1400, 900);
  await comparisonOwner.navigate(new URL(`/docs/${slug}`, browserBase).href);
  await until("comparison owner editor", () => comparisonOwner.evaluate('!!document.querySelector(".cm-content")'));
  await comparisonOwner.evaluate('document.querySelector(".sidebar-activity [aria-label=History]")?.click()');
  // Each saved version is a row of its own in the panel. The row is found by
  // its name: its id is the row's, not the version id the save answered with.
  await until("owner history", () => comparisonOwner.evaluate('document.body.innerText.includes("Original draft")'));
  await comparisonOwner.evaluate(`([...document.querySelectorAll('li.hx-event[data-sha]')]
    .find((row) => row.textContent.includes('Original draft'))?.querySelector('button.hx-single')?.click())`);
  // A chosen version is compared with the one before it by default. Compare it
  // with the live source instead, which is what the old panel showed.
  await until("compare-with controls", () => comparisonOwner.evaluate(`!!document.querySelector('.history-compare-with')`));
  await comparisonOwner.evaluate(`([...document.querySelectorAll('.history-compare-with button')]
    .find((button) => button.textContent.trim() === 'Latest')?.click())`);
  await until("history source comparison", () => comparisonOwner.evaluate(`(() => {
    const oldText = document.querySelector('.cm-merge-a .cm-content')?.textContent || '';
    const newText = document.querySelector('.cm-merge-b .cm-content')?.textContent || '';
    return oldText.includes('red fox') && newText.includes('blue fox');
  })()`));
  // The project has one file, so the panel names it in place of the file
  // selector (which only appears when several files changed) and offers the
  // change stepper, which exists only when that file differs. A project with
  // several changed files would show "document.md · changed" in the selector.
  const fileSummary = await comparisonOwner.evaluate(`(() => {
    const select = document.querySelector('[aria-label="History file"]');
    const name = select ? select.selectedOptions[0]?.textContent : document.querySelector('.history-source-name')?.textContent;
    return { name: name || '', stepper: !!document.querySelector('.history-changes'),
      none: document.querySelector('.history-workspace')?.textContent.includes('No differences') };
  })()`);
  assert.match(fileSummary.name, /document\.md/);
  assert.ok(fileSummary.stepper && !fileSummary.none, "the comparison reports that document.md changed");
  await owner.insert(revised.replace("blue fox", "green fox"), true);
  await until("newer edit is durable", async () => {
    const snapshot = await request(`/api/documents/${slug}/snapshot`);
    return snapshot.texts?.["document.md"] === revised.replace("blue fox", "green fox");
  });
  await until("newer edit reached the comparison tab", async () => (await comparisonOwner.text()).includes("green fox"));
  assert.ok(await comparisonOwner.evaluate(`document.querySelector('.cm-merge-b .cm-content')?.textContent.includes('blue fox')`),
    "the selected comparison keeps its captured current source until refreshed");
  // The old "Refresh" button is now the icon button "Reload the latest version".
  await comparisonOwner.evaluate(`document.querySelector('.history-workspace [aria-label="Reload the latest version"]')?.click()`);
  await until("refreshed history comparison", () => comparisonOwner.evaluate(`(() => {
    const newText = document.querySelector('.cm-merge-b .cm-content')?.textContent || '';
    return newText.includes('green fox') && !newText.includes('blue fox');
  })()`));
  // Restoring an earlier label creates a new event and updates the live source.
  await comparisonOwner.evaluate(`document.querySelector('.history-workspace .history-restore')?.click()`);
  await until("restore confirmation", () => comparisonOwner.evaluate('document.body.innerText.includes("Restore this version?")'));
  await comparisonOwner.evaluate(`([...document.querySelectorAll('button')].filter(button => ['Restore this version', 'Restore anyway'].includes(button.textContent.trim())).at(-1)?.click())`);
  await until("restore label", async () => (await request(historyPath)).labels?.some((label) => label.reason === "restore"));
  const restored = (await request(historyPath)).labels.find((label) => label.reason === "restore");
  assert.ok(restored);
  assert.notEqual(restored.sha, first.sha);
  const restoredTree = await request(`${historyPath}/${restored.sha}`);
  assert.equal(restoredTree.texts["document.md"], original);
  const live = await request(`/api/documents/${slug}/snapshot`);
  assert.equal(live.texts["document.md"], original, "the live document holds the restored source");
  console.log("history e2e: reader role, owner comparison refresh, restore, and durable source passed");
}

try {
  await main();
} finally {
  await owner?.close();
  await comparisonOwner?.close();
  await reader?.close();
  await deployment.stop();
  removeTemporary(profileRoot);
}
