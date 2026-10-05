// Real-server acceptance for suggestion painting, review, persistence, and history redlines.
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
const deployment = await startDeployment({ label: "track_changes_browser", binary });
if (!deployment || deployment.unavailable) {
  throw new Error(deployment?.unavailable || `no librepaper binary at ${binary}; build it first`);
}

const profile = mkdtempSync(join(tmpdir(), "librepaper-track-changes-browser-"));
const port = 33000 + Math.floor(Math.random() * 20000);
let editor;
const cookie = deployment.cookie;
const source = "# A Paper\n\nThe first paragraph.\n\nA second paragraph.\n";

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
  editor = await browser("chromium", profile, port);
  await editor.setCookie("librepaper_session", cookie.slice(cookie.indexOf("=") + 1), deployment.base);
  await editor.navigate(deployment.base);
  await editor.resize(1400, 900);

  const document = await deployment.publish({ title: "Track changes acceptance", source_format: "markdown", source });
  const slug = document.slug;
  const historyPath = `/api/documents/${slug}/history`;
  const baseline = await request(`${historyPath}/current`, "PATCH", {
    label: "Before review", request_id: randomUUID(),
  });
  await editor.navigate(new URL(`/docs/${slug}`, deployment.base).href);
  await until("document frame", () => editor.frameEvaluate("document.body.innerText.includes('The first paragraph.')"));

  const posted = await request(`/api/documents/${slug}/comments`, "POST", {
    type: "comment",
    motivation: "editing",
    exact: "The first paragraph.",
    prefix: "",
    suffix: "",
    position: 14,
    proposed: "The opening paragraph.",
    body: "clearer",
  });
  assert.ok(posted.id || posted.comment?.id, `suggestion was refused: ${JSON.stringify(posted)}`);

  const changesTab = `([...document.querySelectorAll('button, a')]
    .find((button) => [button.getAttribute('aria-label'), button.title, button.textContent.trim()]
      .includes('Changes')))`;
  await until("Changes tab", () => editor.evaluate(`Boolean(${changesTab})`));
  await editor.evaluate(`${changesTab}.click()`);

  const suggestion = await until("suggestion stored", async () => {
    const { comments } = await request(`/api/documents/${slug}/comments`);
    return comments.find((comment) => comment.motivation === "editing" && !comment.temp_id);
  });
  assert.equal(suggestion.proposed, "The opening paragraph.");
  assert.ok(suggestion.proposal, "suggestion is backed by a proposal");

  const painted = await until("suggestion painted in the document frame", () => editor.frameEvaluate(`(() => {
    const mark = document.querySelector('mark[data-proposed]');
    return mark ? { proposed: mark.dataset.proposed, strike: getComputedStyle(mark).textDecorationLine } : null;
  })()`));
  assert.equal(painted.proposed, "The opening paragraph.");
  assert.ok(painted.strike.includes("line-through"));

  const cardDiff = await until("suggestion word diff", () => editor.evaluate(`(() => {
    const card = document.querySelector('.suggestion-diff');
    return card ? { del: card.querySelector('del')?.textContent, ins: card.querySelector('ins')?.textContent } : null;
  })()`));
  assert.deepEqual(cardDiff, { del: "first", ins: "opening" });

  await editor.evaluate(`([...document.querySelectorAll('button')]
    .find((button) => button.textContent.trim() === 'Accept')?.click())`);
  await until("accepted source painted in the document frame", () => editor.frameEvaluate("document.body.innerText.includes('The opening paragraph.')"));

  const after = await until("accepted outcome stored", async () => {
    const { comments } = await request(`/api/documents/${slug}/comments`);
    const current = comments.find((comment) => comment.id === suggestion.id);
    return current?.outcome === "accepted" ? current : null;
  });
  assert.equal(after.resolved, true);
  const labels = (await request(historyPath)).labels;
  const accepted = labels.find((label) => label.reason === "accept");
  assert.ok(accepted, "acceptance records a history label");
  assert.equal(after.resolved_in, accepted.sha, "the suggestion outcome points to its accepting label");
  const live = await request(`/api/documents/${slug}/snapshot`);
  assert.ok(live.texts[live.main].includes("The opening paragraph."), "accepted words persist in the live source");

  await editor.evaluate(`([...document.querySelectorAll('button, a')]
    .find((button) => [button.getAttribute('aria-label'), button.title, button.textContent.trim()]
      .includes('History'))?.click())`);
  await until("history panel", () => editor.evaluate('Boolean(document.querySelector(".timeline-list"))'));
  await editor.evaluate(`document.querySelector('li[data-sha=${JSON.stringify(baseline.sha)}] .timeline-point')?.click()`);
  await until("history word diff", () => editor.evaluate(`document.body.innerText.includes('opening')`));
  const toggle = await editor.evaluate(`Boolean([...document.querySelectorAll('input[type=checkbox]')]
    .find((input) => input.closest('label')?.textContent.includes('Show in document'))?.checked)`);
  if (!toggle) {
    await editor.evaluate(`([...document.querySelectorAll('input[type=checkbox]')]
      .find((input) => input.closest('label')?.textContent.includes('Show in document'))?.click()`);
  }
  const redlined = await until("history redlines painted in the document", () => editor.frameEvaluate(`(() => {
    const ins = document.querySelector('mark.librepaper-ins');
    const del = document.querySelector('mark.librepaper-del');
    return ins || del ? { ins: ins?.textContent, del: del?.dataset.deleted, text: document.body.innerText } : null;
  })()`));
  assert.equal(redlined.ins, "opening");
  assert.equal(redlined.del, "first");
  assert.ok(!redlined.text.includes("first"), "redlines do not alter rendered document text");

  await editor.evaluate(`${changesTab}.click()`);
  await until("leaving history clears redlines", () => editor.frameEvaluate(
    "!document.querySelector('mark.librepaper-ins, mark.librepaper-del')",
  ));
  console.log("track changes e2e: suggestion paint, accept, persistence, history label, and redlines passed");
}

try {
  await main();
} finally {
  await editor?.close();
  await deployment.stop();
  rmSync(profile, { recursive: true, force: true });
}
