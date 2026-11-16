// The figures a preview's HTML refers to, as the frame receives them. They go
// ahead of the page once per frame; a frame that has lost them (a navigation,
// an invalidation) gets them again on the next page.
import assert from "node:assert/strict";
import { createFramePreview } from "../../src/lib/reader/frame-preview.js";

const figure = (sha, type = "image/png") => ({ sha, type, bytes: Uint8Array.of(sha.length) });
const types = (sent) => sent.map((message) => message.type);

// A frame that has loaded and reported ready for its current epoch.
function harness(kind = "raw") {
  const sent = [];
  const preview = createFramePreview({
    slug: "paper",
    getDocsOrigin: () => "https://docs.example",
    framePath: () => kind,
    setSource: () => {},
    send: (message) => sent.push(message),
  });
  preview.navigate();
  preview.markReady();
  return { preview, sent };
}

// The figures come before the page that names them, and only the first time.
{
  const { preview, sent } = harness();
  preview.publish({ kind: "html", html: "<p>one</p>", figures: [figure("aa")] });
  assert.deepEqual(types(sent), ["figures", "preview"]);
  assert.deepEqual(sent[0].items.map((item) => item.sha), ["aa"]);
  assert.ok(sent[0].items[0].bytes instanceof Uint8Array, "the bytes travel with the figure");

  sent.length = 0;
  preview.publish({ kind: "html", html: "<p>two</p>", figures: [figure("aa")] });
  assert.deepEqual(types(sent), ["preview"], "a figure the frame already holds is not sent again");

  sent.length = 0;
  preview.publish({ kind: "html", html: "<p>three</p>", figures: [figure("aa"), figure("bb")] });
  assert.deepEqual(types(sent), ["figures", "preview"]);
  assert.deepEqual(sent[0].items.map((item) => item.sha), ["bb"], "only the figure the frame lacks is sent");
}

// A new epoch is a new frame, which has lost its figures.
{
  const { preview, sent } = harness();
  preview.publish({ kind: "html", html: "<p>a</p>", figures: [figure("aa")] });
  sent.length = 0;
  preview.navigate(true);
  preview.markReady();
  preview.publish({ kind: "html", html: "<p>b</p>", figures: [figure("aa")] });
  assert.deepEqual(types(sent), ["figures", "preview"], "a navigation sends the figures again");
}

// Replay after an invalidation resends the figures the payload carries.
{
  const { preview, sent } = harness();
  preview.publish({ kind: "html", html: "<p>a</p>", figures: [figure("aa")] });
  sent.length = 0;
  preview.invalidate();
  assert.equal(preview.markReady(), true);
  preview.replay();
  assert.deepEqual(types(sent), ["figures", "preview"], "the frame is ready again, so the replay sends the figures");
}

// A reload at the same address keeps the epoch, so readiness says nothing new;
// the load itself forgets the figures, and the replay sends them again.
{
  const { preview, sent } = harness();
  preview.publish({ kind: "html", html: "<p>a</p>", figures: [figure("aa")] });
  sent.length = 0;
  preview.forgetFigures();
  assert.equal(preview.markReady(), false, "the same epoch is not a first ready");
  preview.replay();
  assert.deepEqual(types(sent), ["figures", "preview"], "the reloaded frame gets its figures again");
  sent.length = 0;
  preview.publish({ kind: "html", html: "<p>b</p>", figures: [figure("aa")] });
  assert.deepEqual(types(sent), ["preview"], "and they are not sent a third time");
}

// A PDF is delivered as bytes and names no figures, so no figures message goes.
{
  const { preview, sent } = harness("pdf");
  preview.publish({ kind: "pdf", sha: "pdf-sha", bytes: Uint8Array.of(1, 2) });
  assert.deepEqual(types(sent), ["preview"]);
  assert.ok(sent[0].pdf instanceof ArrayBuffer);
}

console.log("frame-preview: figures reach the frame once per frame, ahead of the page that names them");
