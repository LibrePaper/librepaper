// A comment card, whose summary button stops existing the moment it is activated and
// whose thread is a run of authors rather than a list of messages. It was written by hand and got the interesting parts wrong -- focus
// dropped to the body -- so it is pinned here.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync, rmSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until } from "../helpers/browser-driver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-comment-card-check-"));
const output = join(temporary, "build");
const profile = join(temporary, "browser");
const entry = join(temporary, "entry.js");

const source = `
import CommentCard from ${JSON.stringify(join(root, "web/src/components/CommentCard.svelte"))};
import { tick } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};

const flush = async () => { await tick(); await new Promise((resolve) => setTimeout(resolve, 60)); await tick(); };
const check = (condition, message) => { if (!condition) throw new Error(message); };

window.commentCardCheck = async () => {

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

  // A thread is grouped by author: the avatar and the name are written once
  // for a run of messages by the same person, and a reply from somebody else
  // opens the next run. The count of avatars is the whole point -- five
  // messages between two people used to be five badges.
  const threadHost = document.createElement('div');
  document.body.append(threadHost);
  const thread = createClassComponent({ component: CommentCard, target: threadHost, props: {
    comment: {
      id: 'c2', body: 'this is not ok', exact: 'the passage', creator: 'Vincent',
      created: '2026-09-17T11:04:00Z',
      replies: [
        { id: 'r1', creator: 'Vincent', body: 'Again, there is repetition here', created: '2026-09-17T11:05:00Z' },
        { id: 'r2', creator: 'Alice', body: 'I agree with the second point', created: '2026-09-17T11:08:00Z' },
        { id: 'r3', creator: 'Vincent', body: 'Fair enough', created: '2026-09-17T11:10:00Z' },
      ],
    },
    canComment: true,
  } });
  await flush();

  const runs = [...threadHost.querySelectorAll('.run')];
  check(runs.length === 3, 'four messages between two people are three runs, not four');
  check(threadHost.querySelectorAll('.avatar').length === 3, 'one avatar per run, not one per message');
  check(runs[0].querySelectorAll('.post').length === 2,
    'two things said in a row by one person sit in the same run');
  check([...threadHost.querySelectorAll('.run-author')].map((each) => each.textContent.trim()).join(',') === 'Vincent,Alice,Vincent',
    'a new author opens a new run, and the one who comes back opens a third');
  // The gutter that made a long reply wrap in half the sidebar is gone: a
  // message is laid out against the card, not against the avatar.
  check(runs[0].querySelector('.post').getBoundingClientRect().left
    <= runs[0].getBoundingClientRect().left + 1, 'a message uses the full width of the card');
  thread.$destroy();
  threadHost.remove();
  return true;
};
`;

writeFileSync(entry, source);
let server;
let tab;
try {
  await build({ configFile: false, root: join(root, "web"), plugins: [svelte({ emitCss: false })], logLevel: "error",
    build: { outDir: output, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "comment-card-check.js" } } });
  server = createServer((request, response) => {
    const file = join(output, request.url.slice(1));
    if (request.url !== "/" && existsSync(file)) { response.setHeader("Content-Type", "text/javascript"); response.end(readFileSync(file)); return; }
    response.setHeader("Content-Type", "text/html"); response.end('<body><script type="module" src="/comment-card-check.js"></script></body>');
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  tab = await browser("chromium", profile, 23000 + Math.floor(Math.random() * 1000));
  await tab.navigate(`http://127.0.0.1:${port}/`);
  await until("comment card component", () => tab.evaluate("Boolean(window.commentCardCheck)"));
  assert.equal(await tab.evaluate("window.commentCardCheck()"), true);
  console.log("comment card: a resolved card keeps focus when it opens; a thread is grouped by author with one avatar per run");
} finally {
  await tab?.close(); server?.close(); rmSync(temporary, { recursive: true, force: true });
}
