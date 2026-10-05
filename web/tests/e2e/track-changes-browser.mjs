// Real-server acceptance for suggestion painting, review, persistence, and history comparison.
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
const browserBase = deployment.browserBase;
const source = "# A Paper\n\nThe first paragraph.\n\nA second paragraph.\n";
const revised = "# A Paper\n\nThe opening paragraph.\n\nA second paragraph.\n";

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
  await editor.setCookie("librepaper_session", cookie.slice(cookie.indexOf("=") + 1), browserBase);
  await editor.navigate(browserBase);
  await editor.resize(1400, 900);

  const document = await deployment.publish({ title: "Track changes acceptance", source_format: "markdown", source });
  const slug = document.slug;
  const historyPath = `/api/documents/${slug}/history`;
  const baseline = await request(`${historyPath}/current`, "PATCH", {
    label: "Before review", request_id: randomUUID(),
  });
  await editor.navigate(new URL(`/docs/${slug}`, browserBase).href);
  try {
    await until("document frame", () => editor.frameEvaluate("document.body.innerText.includes('The first paragraph.')"));
  } catch (error) {
    const [frameUrl, frameBody, targets, frames] = await Promise.all([
      editor.evaluate('document.querySelector("iframe[title=Document]")?.src || ""').catch((cause) => `unavailable: ${cause.message}`),
      editor.frameEvaluate("document.body.innerText").catch((cause) => `unavailable: ${cause.message}`),
      editor.command("Target.getTargets").catch((cause) => `unavailable: ${cause.message}`),
      editor.command("Page.getFrameTree").catch((cause) => `unavailable: ${cause.message}`),
    ]);
    const targetsOnly = targets?.targetInfos?.filter((target) => target.type === "iframe" || target.url.includes(`/raw/${slug}/`)) || targets;
    throw new Error(`${error.message}; iframe src: ${frameUrl}; frame body: ${frameBody}; matching CDP targets: ${JSON.stringify(targetsOnly)}; frame tree: ${JSON.stringify(frames)}`);
  }

  const suggestionId = randomUUID();
  const note = `Suggestion ${suggestionId}`;
  const rendered = await editor.frameEvaluate("document.body.innerText");
  const passageAt = rendered.indexOf("The first paragraph.");
  assert.notEqual(passageAt, -1, "the rendered passage is available for the suggestion anchor");
  const prefix = rendered.slice(Math.max(0, passageAt - 64), passageAt);
  const suffix = rendered.slice(passageAt + "The first paragraph.".length, passageAt + "The first paragraph.".length + 64);
  const posted = await editor.evaluate(`(async () => await new Promise((resolve, reject) => {
    const address = new URL(${JSON.stringify(`/ws/${slug}`)}, location.href);
    address.protocol = address.protocol === 'https:' ? 'wss:' : 'ws:';
    const ws = new WebSocket(address);
    const requestId = ${JSON.stringify(randomUUID())};
    let joined = false;
    let sent = false;
    const timer = setTimeout(() => { ws.close(); reject(new Error('suggestion comment timed out')); }, 15000);
    ws.addEventListener('error', () => { clearTimeout(timer); reject(new Error('suggestion websocket failed')); }, { once: true });
    ws.addEventListener('message', ({ data }) => {
      const frame = JSON.parse(data);
      if (frame.type === 'hello' && !joined) {
        joined = true;
        ws.send(JSON.stringify({ type: 'doc-open', protocol: 'librepaper.room.v3', vector: '', request_id: requestId }));
      } else if (frame.type === 'doc-rows' && !sent) {
        sent = true;
        ws.send(JSON.stringify({
          type: 'comment', exact: 'The first paragraph.', prefix: ${JSON.stringify(prefix)}, suffix: ${JSON.stringify(suffix)},
          position: ${passageAt},
          motivation: 'editing', body: ${JSON.stringify(note)}, proposed: 'The opening paragraph.',
          render_digest: '', temp_id: ${JSON.stringify(suggestionId)},
        }));
      } else if (frame.type === 'comment' && frame.comment?.body === ${JSON.stringify(note)}) {
        clearTimeout(timer); ws.close(); resolve(frame.comment);
      } else if (frame.type === 'error' && sent) {
        clearTimeout(timer); ws.close(); reject(new Error(frame.message || 'suggestion was refused'));
      }
    });
  }))()`);
  assert.ok(posted.id && posted.proposal, `suggestion was refused: ${JSON.stringify(posted)}`);
  assert.equal(posted.proposed, "The opening paragraph.");

  const changesTab = `([...document.querySelectorAll('button, a')]
    .find((button) => [button.getAttribute('aria-label'), button.title, button.textContent.trim()]
      .includes('Changes')))`;
  await until("Changes tab", () => editor.evaluate(`Boolean(${changesTab})`));
  await editor.evaluate(`${changesTab}.click()`);

  await until("suggestion in Changes", () => editor.evaluate(`Boolean(document.querySelector('.change-row.suggestion .row-main'))`));
  const row = await editor.evaluate(`(() => {
    const item = document.querySelector('.change-row.suggestion');
    return item ? {
      before: item.querySelector('.deletion')?.textContent.trim(),
      after: item.querySelector('.insertion')?.textContent.trim(),
    } : null;
  })()`);
  assert.equal(row.before, "− The first paragraph.");
  assert.equal(row.after, "+ The opening paragraph.");
  const painted = await until("suggestion painted in the document frame", () => editor.frameEvaluate(`(() => {
    const mark = [...document.querySelectorAll('mark[data-librepaper]')]
      .find((node) => node.dataset.librepaper.split(/\\s+/).includes(${JSON.stringify(posted.id)}));
    const insertion = [...document.querySelectorAll('.librepaper-suggestion-synthetic')]
      .find((node) => node.dataset.librepaperSuggestion === ${JSON.stringify(posted.id)});
    return mark && insertion ? {
      strike: getComputedStyle(mark).textDecorationLine,
      inserted: insertion.textContent,
      label: insertion.getAttribute('aria-label'),
    } : null;
  })()`));
  assert.ok(painted.strike.includes("line-through"));
  assert.equal(painted.inserted, "The opening paragraph.");
  assert.equal(painted.label, "Suggested insertion: The opening paragraph.");
  await until("proposal list ready", () => editor.evaluate(`Boolean(document.querySelector('.change-row.suggestion .row-action.accept:not(:disabled)'))`));
  await editor.evaluate(`document.querySelector('.change-row.suggestion .row-main').click()`);
  await until("accept action", () => editor.evaluate(`Boolean(document.querySelector('.change-row.suggestion .row-action.accept:not(:disabled)'))`));
  await editor.evaluate(`document.querySelector('.change-row.suggestion .row-action.accept').click()`);
  await until("accepted source painted in the document frame", () => editor.frameEvaluate("document.body.innerText.includes('The opening paragraph.')"));

  await until("accepted source durable", async () => {
    const snapshot = await request(`/api/documents/${slug}/snapshot`);
    return snapshot.texts?.[snapshot.main] === revised || snapshot.source === revised;
  });
  const labels = (await request(historyPath)).labels;
  const accepted = labels.find((label) => label.reason === "accept");
  assert.ok(accepted, "acceptance records a history label");
  const { comments } = await request(`/api/documents/${slug}/comments`);
  assert.ok(!comments.some((comment) => comment.id === posted.id), "the decided suggestion leaves the pending comments list");

  await editor.evaluate('document.querySelector(".sidebar-activity [aria-label=History]")?.click()');
  await until("history panel", () => editor.evaluate(`Boolean(document.querySelector('[data-sha="${accepted.sha}"]'))`));
  await editor.evaluate(`document.querySelector('li[data-sha=${JSON.stringify(baseline.sha)}] .timeline-point')?.click()`);
  await until("history source diff", () => editor.evaluate(`(() => {
    const oldText = document.querySelector('.cm-merge-a .cm-content')?.textContent || '';
    const newText = document.querySelector('.cm-merge-b .cm-content')?.textContent || '';
    return oldText.includes('The first paragraph.') && newText.includes('The opening paragraph.');
  })()`));
  console.log("track changes e2e: suggestion paint, accept, durable source, and history comparison passed");
}

try {
  await main();
} finally {
  await editor?.close();
  await deployment.stop();
  rmSync(profile, { recursive: true, force: true });
}
