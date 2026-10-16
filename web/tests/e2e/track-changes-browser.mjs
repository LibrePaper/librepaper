// Real-server acceptance for tracked changes: type with tracking on, see the
// change in the Changes panel, reject it, and find the text restored, also
// after a reload.
// Run with `npm run check:e2e`; requires the built binary, Chromium, and a
// disposable PostgreSQL connection in LIBREPAPER_TEST_POSTGRES_URL.
import assert from "node:assert/strict";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";
import { deploymentBinary, startDeployment } from "../helpers/deployment.mjs";

const binary = process.argv[2] || deploymentBinary();
const deployment = await startDeployment({ label: "track_changes_browser", binary });
if (!deployment || deployment.unavailable) {
  throw new Error(deployment?.unavailable || `no librepaper binary at ${binary}; build it first`);
}

const profile = mkdtempSync(join(tmpdir(), "librepaper-track-changes-browser-"));
const port = 33000 + Math.floor(Math.random() * 20000);
let editor;
const cookie = deployment.cookie;
const browserBase = deployment.browserBase;
const source = "# A Paper\n\nThe first paragraph.\n\nA second paragraph.\n";
const typed = "TRACKED WORDS";

async function request(path) {
  const response = await fetch(new URL(path, deployment.base), {
    headers: { "x-librepaper-client": "1", cookie },
  });
  const text = await response.text();
  assert.ok(response.ok, `GET ${path}: ${response.status} ${text}`);
  return JSON.parse(text);
}

const sourceText = async (slug) => {
  const snapshot = await request(`/api/documents/${slug}/snapshot`);
  return snapshot.texts?.[snapshot.main] ?? snapshot.source;
};

const openChanges = async () => {
  const changesTab = `([...document.querySelectorAll('button, a')]
    .find((button) => [button.getAttribute('aria-label'), button.title, button.textContent.trim()]
      .includes('Changes')))`;
  await until("Changes tab", () => editor.evaluate(`Boolean(${changesTab})`));
  await editor.evaluate(`${changesTab}.click()`);
  await until("Changes panel", () => editor.evaluate(`Boolean(document.querySelector('.changes-panel'))`));
};

async function main() {
  editor = await browser("chromium", profile, port);
  await editor.setCookie("librepaper_session", cookie.slice(cookie.indexOf("=") + 1), browserBase);
  await editor.navigate(browserBase);
  await editor.resize(1400, 900);

  const document = await deployment.publish({ title: "Track changes acceptance", source_format: "markdown", source });
  const slug = document.slug;
  await editor.navigate(new URL(`/docs/${slug}`, browserBase).href);
  await until("editor", () => editor.evaluate(`document.querySelector('.cm-content')?.textContent.includes('The first paragraph.')`));

  // With tracking on, what is typed goes into the document at once and is
  // recorded as a pending change.
  await openChanges();
  await until("tracking switch", () => editor.evaluate(`Boolean(document.querySelector('.tracking-toggle input:not(:disabled)'))`));
  await editor.evaluate(`document.querySelector('.tracking-toggle').click()`);
  await until("tracking on", () => editor.evaluate(`document.querySelector('.tracking-toggle input')?.checked === true`));
  await editor.evaluate(`document.querySelector('.cm-content').focus()`);
  await editor.insert(typed);
  await until("the typing is in the editor", () => editor.evaluate(`document.querySelector('.cm-content').textContent.includes(${JSON.stringify(typed)})`));
  await until("a change in the panel", () => editor.evaluate(`[...document.querySelectorAll('.change-row .insertion')]
    .some((node) => node.textContent.includes(${JSON.stringify(typed)}))`));
  await until("the typing reached the server", async () => (await sourceText(slug)).includes(typed));

  // Reject puts the text back as it was and drops the change.
  await editor.evaluate(`document.querySelector('.change-row .row-main').click()`);
  await until("reject offered", () => editor.evaluate(`Boolean(document.querySelector('.change-row .row-action.reject:not(:disabled)'))`));
  await editor.evaluate(`document.querySelector('.change-row .row-action.reject').click()`);
  await until("the text is restored in the editor", () => editor.evaluate(`!document.querySelector('.cm-content').textContent.includes(${JSON.stringify(typed)})`));
  await until("the change is gone", () => editor.evaluate(`document.querySelectorAll('.change-row').length === 0`));
  await until("the restored text is durable", async () => (await sourceText(slug)) === source);

  // And it stays restored.
  await editor.navigate(new URL(`/docs/${slug}`, browserBase).href);
  await until("editor after reload", () => editor.evaluate(`document.querySelector('.cm-content')?.textContent.includes('The first paragraph.')`));
  assert.ok(await editor.evaluate(`!document.querySelector('.cm-content').textContent.includes(${JSON.stringify(typed)})`),
    "the rejected text does not come back after a reload");
  await openChanges();
  assert.equal(await editor.evaluate(`document.querySelectorAll('.change-row').length`), 0, "no pending change returns after a reload");
  assert.equal(await sourceText(slug), source);
  console.log("track changes e2e: tracked typing, a Changes row, reject restores the text, and it stays restored after a reload");
}

try {
  await main();
} finally {
  await editor?.close();
  await deployment.stop();
  removeTemporary(profile);
}
