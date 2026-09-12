// Browser regression check for the checkpoint merge view. It mounts the real
// Svelte component, binds the editable side to a real Y.Text, and exercises
// the same revert controls a reviewer uses in the reader.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, pause, until } from "../../tools/browser-driver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-history-merge-check-"));
const output = join(temporary, "build");
const profile = join(temporary, "browser");
const entry = join(temporary, "entry.js");

const source = `
import * as Y from ${JSON.stringify(join(root, "web/node_modules/yjs/dist/yjs.mjs"))};
import { tick } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import MergeEditor from ${JSON.stringify(join(root, "web/src/components/MergeEditor.svelte"))};
import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};

const doc = new Y.Doc();
const liveText = doc.getText("main");
liveText.insert(0, "alpha\\nnew sentence\\nomega\\none\\ntwo\\nthree\\nfour\\nfive");
const peer = new Y.Doc();
let component;

window.mergeCheck = async () => {
  component = createClassComponent({ component: MergeEditor, target: document.body, props: {
    path: "main.md",
    oldText: "alpha\\nold sentence\\nomega\\none\\ntwo\\nthree\\nfour\\nfive",
    newText: liveText.toString(),
    liveText,
    editable: true,
  }});
  await tick();
  await new Promise((resolve) => setTimeout(resolve, 250));
  // An independent peer update arriving while the merge remains open must
  // reach the live Y.Text before the reviewer clicks a hunk.
  Y.applyUpdate(peer, Y.encodeStateAsUpdate(doc));
  const peerText = peer.getText("main");
  peerText.insert(peerText.length, " peer");
  Y.applyUpdate(doc, Y.encodeStateAsUpdate(peer));
  await tick();
  await new Promise((resolve) => setTimeout(resolve, 100));
  const first = document.querySelector(".cm-merge-revert button");
  if (!first) throw new Error("merge view did not render a per-hunk revert control");
  // Revert the selected hunk through the actual CodeMirror control.
  first.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
  await new Promise((resolve) => setTimeout(resolve, 100));
  await tick();
  const afterPeer = liveText.toString();

  // A second independent peer edit made while the merge remains open must
  // flow through the live Y.Text and survive the next comparison.
  const secondPeer = new Y.Doc();
  Y.applyUpdate(secondPeer, Y.encodeStateAsUpdate(doc));
  const secondPeerText = secondPeer.getText("main");
  secondPeerText.insert(secondPeerText.length, "\\npeer note");
  Y.applyUpdate(doc, Y.encodeStateAsUpdate(secondPeer));
  await tick();
  await new Promise((resolve) => setTimeout(resolve, 100));
  const afterPeerEdit = liveText.toString();

  // Switching snapshots reconstructs the comparison while retaining the
  // same live Y.Text. The second revert applies the newly selected hunk.
  const sentenceAt = "alpha\\n".length;
  liveText.delete(sentenceAt, "old sentence".length);
  liveText.insert(sentenceAt, "third sentence");
  const switchedLive = liveText.toString();
  component.$set({ oldText: "alpha\\nold sentence\\nomega\\none\\ntwo\\nthree\\nfour\\nfive peer\\npeer note", newText: switchedLive });
  await tick();
  await new Promise((resolve) => setTimeout(resolve, 250));
  const second = document.querySelector(".cm-merge-revert button");
  const switched = Boolean(second);
  second?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
  await tick();
  const afterSwitch = liveText.toString();

  // Revocation removes the action and makes the live side read-only. This
  // guards against a stale mounted merge view retaining write permission.
  component.$set({ editable: false });
  await tick();
  const controlsAfterRevoke = document.querySelectorAll(".cm-merge-revert button").length;
  const editableAfterRevoke = [...document.querySelectorAll(".cm-merge-b .cm-content")]
    .some((node) => node.getAttribute("contenteditable") !== "false");
  component.$destroy();
  return { afterPeer, afterPeerEdit, switched, afterSwitch, controlsAfterRevoke, editableAfterRevoke };
};
`;

writeFileSync(entry, source);
let server;
let tab;
try {
  await build({
    configFile: false,
    root: join(root, "web"),
    plugins: [svelte()],
    logLevel: "error",
    build: {
      outDir: output,
      emptyOutDir: true,
      lib: { entry, formats: ["es"], fileName: () => "history-merge-check.js" },
    },
  });
  server = createServer((request, response) => {
    const file = join(output, request.url.slice(1));
    if (request.url !== "/" && existsSync(file)) {
      response.setHeader("Content-Type", "text/javascript");
      response.end(readFileSync(file));
      return;
    }
    response.setHeader("Content-Type", "text/html");
    response.end('<body><script type="module" src="/history-merge-check.js"></script></body>');
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  if (!port) throw new Error("history merge check server did not start");
  tab = await browser("chromium", profile, 22000 + Math.floor(Math.random() * 1000));
  await tab.navigate(`http://127.0.0.1:${port}/`);
  await until("merge component", () => tab.evaluate("Boolean(window.mergeCheck)"));
  const result = await tab.evaluate("window.mergeCheck()");
  assert.equal(result.afterPeer, "alpha\nold sentence\nomega\none\ntwo\nthree\nfour\nfive peer", "hunk restore preserves an independent peer edit");
  assert.equal(result.afterPeerEdit, "alpha\nold sentence\nomega\none\ntwo\nthree\nfour\nfive peer\npeer note", "a concurrent peer edit reaches the live Y.Text");
  assert.equal(result.switched, true, "changing comparison snapshots renders a fresh hunk");
  assert.equal(result.afterSwitch, "alpha\nold sentence\nomega\none\ntwo\nthree\nfour\nfive peer\npeer note", "the switched snapshot remains independently applicable");
  assert.equal(result.controlsAfterRevoke, 0, "permission revocation removes revert controls");
  assert.equal(result.editableAfterRevoke, false, "permission revocation makes the live side read-only");
  console.log("history merge: hunk restore, peer edits, snapshot switching, and revocation passed");
} finally {
  await tab?.close();
  server?.close();
  rmSync(temporary, { recursive: true, force: true });
}
