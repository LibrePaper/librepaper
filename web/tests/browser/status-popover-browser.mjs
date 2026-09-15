// Two small surfaces whose behaviour is not what their markup looks like:
// the preview status, which is a popover rather than a disclosure, and a
// resolved comment card, whose summary button stops existing the moment it is
// activated. Both were written by hand and got the interesting parts wrong --
// a panel that closed for neither Escape nor a click outside it, and focus
// dropped to the body -- so both are pinned here.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync, rmSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until } from "../../tools/browser-driver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-status-popover-check-"));
const output = join(temporary, "build");
const profile = join(temporary, "browser");
const entry = join(temporary, "entry.js");

const source = `
import PreviewStatus from ${JSON.stringify(join(root, "web/src/components/PreviewStatus.svelte"))};
import CommentCard from ${JSON.stringify(join(root, "web/src/components/CommentCard.svelte"))};
import { tick } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};

const flush = async () => { await tick(); await new Promise((resolve) => setTimeout(resolve, 60)); await tick(); };
const check = (condition, message) => { if (!condition) throw new Error(message); };

window.statusPopoverCheck = async () => {
  const statusHost = document.createElement('div');
  document.body.append(statusHost);
  const status = createClassComponent({ component: PreviewStatus, target: statusHost, props: {
    label: 'Built 3 seconds ago', tone: 'neutral', busy: false,
  } });
  await flush();

  const trigger = document.querySelector('.preview-status-trigger');
  check(trigger && trigger.tagName === 'BUTTON', 'the status is a button, not a summary');
  check(trigger.getAttribute('aria-expanded') === 'false', 'a shut popover says so');
  // The panel stays mounted and is hidden, which is Zag's doing; what matters
  // is whether it is shown, so that is what these ask.
  const node = () => document.querySelector('.preview-status-popover');
  const panel = () => { const found = node(); return found && found.dataset.state === 'open' ? found : null; };
  check(node() && !panel(), 'the panel is mounted but shut until it is asked for');

  trigger.click(); await flush();
  check(panel(), 'clicking the status opens the panel');
  check(trigger.getAttribute('aria-expanded') === 'true', 'an open popover says so');
  // Portalled to the body: the preview header is a positioned, clipping row,
  // and a panel confined to it was the reason this moved.
  check(node().closest('.preview-status-trigger') === null && !statusHost.contains(node()),
    'the panel is not rendered inside the header it was opened from');

  // The two dismissals a <details> never had.
  document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
  document.body.click(); await flush();
  check(!panel(), 'a click outside the panel closes it');

  trigger.click(); await flush();
  check(panel(), 'it opens again');
  document.activeElement.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); await flush();
  check(!panel(), 'Escape closes it');
  status.$destroy();
  statusHost.remove();

  // A resolved card collapses to one line, and the button that opens it is
  // replaced by the card itself. Focus has to land somewhere real.
  const cardHost = document.createElement('div');
  document.body.append(cardHost);
  const card = createClassComponent({ component: CommentCard, target: cardHost, props: {
    comment: { id: 'c1', body: 'Looks right to me', exact: 'the passage', resolved: true, created_at: '2026-09-15T10:00:00Z' },
    canComment: true,
  } });
  await flush();

  const summary = cardHost.querySelector('button.summary');
  check(summary, 'a resolved card collapses to a summary button');
  summary.focus();
  check(document.activeElement === summary, 'the summary takes focus');
  summary.click(); await flush();
  check(!cardHost.querySelector('button.summary'), 'opening the card replaces the summary');
  check(document.activeElement === cardHost.querySelector('article'),
    'focus moves to the card rather than being dropped to the body');
  card.$destroy();
  cardHost.remove();
  return true;
};
`;

writeFileSync(entry, source);
let server;
let tab;
try {
  await build({ configFile: false, root: join(root, "web"), plugins: [svelte()], logLevel: "error",
    build: { outDir: output, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "status-popover-check.js" } } });
  server = createServer((request, response) => {
    const file = join(output, request.url.slice(1));
    if (request.url !== "/" && existsSync(file)) { response.setHeader("Content-Type", "text/javascript"); response.end(readFileSync(file)); return; }
    response.setHeader("Content-Type", "text/html"); response.end('<body><script type="module" src="/status-popover-check.js"></script></body>');
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  tab = await browser("chromium", profile, 23000 + Math.floor(Math.random() * 1000));
  await tab.navigate(`http://127.0.0.1:${port}/`);
  await until("status popover component", () => tab.evaluate("Boolean(window.statusPopoverCheck)"));
  assert.equal(await tab.evaluate("window.statusPopoverCheck()"), true);
  console.log("status popover: the preview panel flips, dismisses and portals; a resolved card keeps focus when it opens");
} finally {
  await tab?.close(); server?.close(); rmSync(temporary, { recursive: true, force: true });
}
