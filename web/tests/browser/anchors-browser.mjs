// A browser-level check that a comment's anchor follows its words in the
// source editor.
//
// The comment is made on one word. The paragraph that holds it is cut and
// pasted to the end of the file, and the anchor follows the paste. Undo takes
// the paste and then the cut back, and the anchor returns to its word. With
// tracking on, the word is deleted as a tracked change, which leaves the anchor
// pending; rejecting that change the way the reader does (rejectRestoring, the
// planner's record of its edits, then restoreByFragments) brings the anchor back
// exact.
// Two checks follow the rejection. The anchor index must match the record once
// the editor reconciles. Two transactions sent as one update must both land,
// and the index must match the record after them.
// Last, a write to a second file through the insert API must move that file's
// comment, and that file's index must match its text afterwards.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { contentType, loroAlias } from "../helpers/loro.mjs";
import { requireChromiumExecutable } from "../helpers/browser-executable.mjs";
import { stopBrowserProcess, removeTemporary } from "../helpers/browser-driver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-anchors-check-"));
const output = join(temporary, "build");
const profile = join(temporary, "chrome");
const entry = join(temporary, "entry.js");
const port = 19000 + Math.floor(Math.random() * 1000);

const source = `
import ${JSON.stringify(join(root, "web/src/styles/theme.css"))};
import ${JSON.stringify(join(root, "web/src/styles/librepaper.css"))};
import { tick } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import { EditorView } from ${JSON.stringify(join(root, "web/node_modules/@codemirror/view/dist/index.js"))};
import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};
import Editor from ${JSON.stringify(join(root, "web/src/components/Editor.svelte"))};
import { join as joinSession } from ${JSON.stringify(join(root, "web/src/lib/collab.js"))};
import {
  ANCHORS_ORIGIN,
  composeEdits,
  createAnchor,
  resolveAnchors,
  restoreByFragments,
  statusOf,
} from ${JSON.stringify(join(root, "web/src/lib/anchors.js"))};
import { listChanges, rejectRestoring } from ${JSON.stringify(join(root, "web/src/lib/track-changes.js"))};

const TEXT = "First paragraph stays put.\\n\\nThe second paragraph holds the anchored word here.\\n\\nThird paragraph ends the note.\\n";
const WORD = "anchored";
const COMMENT = "comment-anchored";
const NOTES = "Notes start here. The remark sits on this word in the notes.\\n";
const REMARK = "remark";
const COMMENT_NOTES = "comment-remark";
const wait = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

window.anchorsBrowserCheck = async () => {
  const value = joinSession({ send: () => {}, mayEdit: true });
  const fileId = value.addText("paper.md", TEXT);
  value.setMain(fileId);
  const notesId = value.addText("notes.md", NOTES);
  const from = TEXT.indexOf(WORD);
  const to = from + WORD.length;
  createAnchor(value.doc, fileId, COMMENT, from, to, {
    exact: WORD,
    prefix: TEXT.slice(Math.max(0, from - 32), from),
    suffix: TEXT.slice(to, to + 32),
  });
  const notesFrom = NOTES.indexOf(REMARK);
  const notesTo = notesFrom + REMARK.length;
  createAnchor(value.doc, notesId, COMMENT_NOTES, notesFrom, notesTo, {
    exact: REMARK,
    prefix: NOTES.slice(Math.max(0, notesFrom - 32), notesFrom),
    suffix: NOTES.slice(notesTo, notesTo + 32),
  });
  value.doc.commit();

  const host = document.createElement("section");
  document.body.append(host);
  const component = createClassComponent({
    component: Editor,
    target: host,
    props: { session: value, format: "markdown", file: fileId, me: { who: "me", author: "Me" } },
  });
  await tick();
  const view = EditorView.findFromDOM(host.querySelector(".cm-editor"));
  const body = () => value.textOf(fileId).toString();
  const record = () => resolveAnchors(value.doc, fileId).find((item) => item.id === COMMENT);
  const place = () => {
    const found = record();
    return { text: body(), from: found.from, to: found.to, status: statusOf(value.doc, found) };
  };

  // Cut the paragraph that holds the word: select it, then delete it as Cut does.
  const cutFrom = TEXT.indexOf("The second");
  const cutTo = TEXT.indexOf("Third");
  const moved = TEXT.slice(cutFrom, cutTo);
  view.dispatch({ selection: { anchor: cutFrom, head: cutTo } });
  view.dispatch({ changes: { from: cutFrom, to: cutTo }, selection: { anchor: cutFrom }, userEvent: "delete.cut" });
  await tick();
  const cut = place();

  // Paste it at the end. A second apart from the cut, so undo takes the paste alone.
  await wait(1100);
  const end = view.state.doc.length;
  view.dispatch({ changes: { from: end, insert: moved }, selection: { anchor: end + moved.length }, userEvent: "input.paste" });
  await tick();
  const pasted = place();

  await component.editCommand("undo");
  await tick();
  const pasteUndone = place();
  await component.editCommand("undo");
  await tick();
  const undone = place();

  // Delete the passage with tracking on: the change is pending, and so is the comment.
  component.startTracking();
  const word = record();
  view.dispatch({ changes: { from: word.from, to: word.to }, selection: { anchor: word.from }, userEvent: "delete.selection" });
  await tick();
  const deleted = { ...place(), changes: listChanges(value.doc).filter((item) => item.file === fileId).length };

  // Reject the change the way the reader does: put the text back, re-pin the
  // anchors that were in it, and read the file's anchors again.
  const change = listChanges(value.doc).find((item) => item.file === fileId);
  const { ok, restored, edits } = rejectRestoring(value.doc, change ? change.id : "");
  value.anchors.record([{ fileId, edits: composeEdits(edits.map((edit) => [edit])), fragments: [] }], value.anchorMemory);
  restoreByFragments(value.doc, fileId, restored);
  value.doc.commit({ origin: ANCHORS_ORIGIN });
  value.anchors.load(fileId);
  await tick();
  await wait(0);
  const entry = value.anchors.get(fileId);
  const indexed = entry.anchors.find((item) => item.id === COMMENT);
  const rejected = {
    ...place(),
    ok,
    restored: restored.length,
    indexText: entry.text,
    indexFrom: indexed ? indexed.from : null,
    indexTo: indexed ? indexed.to : null,
  };

  // Two transactions in one update. The second is built on the first's state,
  // so the editor sees one update with two changes, and both must land.
  const target = record();
  const first = view.state.update({ changes: { from: target.from, to: target.from + 3 }, userEvent: "delete.selection" });
  const second = first.state.update({ changes: { from: 0, insert: "New lead. " }, userEvent: "input.type" });
  view.dispatch([first, second]);
  await tick();
  await wait(0);
  const after = place();
  const later = value.anchors.get(fileId);
  const found = later.anchors.find((item) => item.id === COMMENT);
  const indexFrom = found ? found.from : null;
  const indexTo = found ? found.to : null;
  const multi = { ...after, quote: after.text.slice(after.from, after.to), indexText: later.text, indexFrom, indexTo };

  // An insertion into notes.md through the insert API, while paper.md is on
  // screen. The index loads notes.md first, so a stale cache would show here.
  value.anchors.get(notesId);
  view.dispatch({ selection: { anchor: 0 } });
  component.captureInsertTarget();
  const context = component.getInsertContext();
  const applied = component.applyInsertResult({ text: "", additionalEdits: [{ path: "notes.md", from: 0, to: 0, insert: "Lead-in. " }] }, context);
  await tick();
  const notesRecord = resolveAnchors(value.doc, notesId).find((item) => item.id === COMMENT_NOTES);
  const notesEntry = value.anchors.get(notesId);
  const notesIndexed = notesEntry.anchors.find((item) => item.id === COMMENT_NOTES);
  const programmatic = {
    applied,
    text: value.textOf(notesId).toString(),
    from: notesRecord.from,
    to: notesRecord.to,
    indexText: notesEntry.text,
    indexFrom: notesIndexed ? notesIndexed.from : null,
    indexTo: notesIndexed ? notesIndexed.to : null,
  };

  component.$destroy();
  value.leave();
  host.remove();
  return { original: TEXT, notes: NOTES, moved, cutFrom, cut, pasted, pasteUndone, undone, deleted, rejected, multi, programmatic };
};
window.anchorsCheckReady = true;
`;

writeFileSync(entry, source);
let server;
let browser;
let socket;
try {
  await build({
    configFile: false,
    root: join(root, "web"),
    resolve: { alias: loroAlias },
    plugins: [svelte()],
    logLevel: "error",
    build: {
      outDir: output,
      emptyOutDir: true,
      lib: { entry, formats: ["es"], fileName: () => "anchors-check.js", cssFileName: "anchors-check" },
    },
  });

  server = createServer((request, response) => {
    const file = join(output, request.url.slice(1));
    if (request.url !== "/" && existsSync(file)) {
      response.setHeader("Content-Type", contentType(file));
      response.end(readFileSync(file));
      return;
    }
    response.setHeader("Content-Type", "text/html");
    response.end('<body data-theme="librepaper"><link rel="stylesheet" href="/anchors-check.css"><script type="module" src="/anchors-check.js"></script></body>');
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  const httpPort = typeof address === "object" && address ? address.port : 0;
  if (!httpPort) throw new Error("local anchors server did not start");

  browser = spawn(requireChromiumExecutable(), [
    "--headless=new",
    "--no-sandbox",
    "--disable-gpu",
    `--user-data-dir=${profile}`,
    `--remote-debugging-port=${port}`,
    "about:blank",
  ], { stdio: "ignore", detached: true });
  const wait = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));
  let browserInfo;
  for (let attempt = 0; attempt < 100; attempt++) {
    try {
      browserInfo = await fetch(`http://127.0.0.1:${port}/json`).then((answer) => answer.json());
      break;
    } catch {
      await wait(100);
    }
  }
  const page = browserInfo?.find((target) => target.type === "page");
  if (!page) throw new Error("Chromium did not expose a page target");
  socket = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, { once: true });
    socket.addEventListener("error", reject, { once: true });
  });

  let sequence = 0;
  const pending = new Map();
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(event.data);
    const resolve = pending.get(message.id);
    if (resolve) {
      pending.delete(message.id);
      resolve(message);
    }
  });
  const send = (method, params = {}) => new Promise((resolve) => {
    const id = ++sequence;
    pending.set(id, resolve);
    socket.send(JSON.stringify({ id, method, params }));
  });
  const evaluate = async (expression) => {
    const message = await send("Runtime.evaluate", {
      expression,
      awaitPromise: true,
      returnByValue: true,
    });
    if (message.result?.exceptionDetails) throw new Error(JSON.stringify(message.result.exceptionDetails));
    return message.result.result.value;
  };

  await send("Page.navigate", { url: `http://127.0.0.1:${httpPort}/` });
  let ready = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    ready = await evaluate("Boolean(window.anchorsCheckReady)");
    if (ready) break;
    await wait(100);
  }
  assert.equal(ready, true, "the anchors check page did not load in Chromium");

  const result = await evaluate("anchorsBrowserCheck()");
  const word = "anchored";
  assert.equal(result.cut.text, result.original.replace(result.moved, ""), "the cut did not remove the paragraph");
  assert.equal(result.cut.status, "deleted", "a cut passage did not detach its comment");
  assert.equal(result.pasted.text, result.original.replace(result.moved, "") + result.moved, "the paste did not put the paragraph at the end");
  assert.equal(result.pasted.status, "exact", "the paste did not carry the comment to its word");
  assert.equal(result.pasted.from, result.pasted.text.indexOf(word), "the comment is not on the moved word");
  assert.equal(result.pasted.to - result.pasted.from, word.length);
  assert.equal(result.pasteUndone.text, result.original.replace(result.moved, ""), "undoing the paste did not take it back");
  assert.equal(result.pasteUndone.status, "deleted", "undoing the paste did not detach the comment again");
  assert.equal(result.undone.text, result.original, "undo did not restore the paragraph");
  assert.equal(result.undone.status, "exact", "undo did not bring the comment back exact");
  assert.equal(result.undone.from, result.original.indexOf(word), "undo did not return the comment to its word");
  assert.equal(result.undone.to, result.original.indexOf(word) + word.length);
  assert.equal(result.deleted.changes, 1, "the tracked deletion did not record one change");
  assert.equal(result.deleted.status, "pending_deletion", "a tracked deletion of the passage did not leave the comment pending");
  assert.equal(result.rejected.ok, true, "the tracked deletion was not rejected");
  assert.equal(result.rejected.restored, 1, "the rejection did not list the text it put back");
  assert.equal(result.rejected.text, result.original, "rejecting the deletion did not restore the word");
  assert.equal(result.rejected.status, "exact", "rejecting the deletion did not bring the comment back exact");
  assert.equal(result.rejected.from, result.original.indexOf(word), "the comment is not on the restored word");
  assert.equal(result.rejected.indexText, result.rejected.text, "the index text drifted from the document after the reject was reconciled");
  assert.equal(result.rejected.indexFrom, result.rejected.from, "the index anchor drifted from the record after the reject was reconciled");
  assert.equal(result.rejected.indexTo, result.rejected.to, "the index anchor drifted from the record after the reject was reconciled");
  assert.equal(result.multi.text, "New lead. " + result.original.replace("anchored", "hored"), "the two transactions did not both land");
  assert.equal(result.multi.quote, "hored", "the record's cursors were captured at an intermediate offset");
  assert.equal(result.multi.status, "modified", "the comment is not modified after a partial deletion and an insert");
  assert.equal(result.multi.indexText, result.multi.text, "the index drifted from the record after a two-transaction update");
  assert.equal(result.multi.indexFrom, result.multi.from, "the index drifted from the record after a two-transaction update");
  assert.equal(result.multi.indexTo, result.multi.to, "the index drifted from the record after a two-transaction update");
  const lead = "Lead-in. ";
  assert.equal(result.programmatic.applied, true, "the insertion into the other file was refused");
  assert.equal(result.programmatic.text, lead + result.notes, "the other file did not take the insertion");
  assert.equal(result.programmatic.from, (lead + result.notes).indexOf("remark"), "the comment in the other file did not move with the insertion");
  assert.equal(result.programmatic.indexText, result.programmatic.text, "the index of the other file drifted after a programmatic write");
  assert.equal(result.programmatic.indexFrom, result.programmatic.from, "the index of the other file drifted after a programmatic write");
  assert.equal(result.programmatic.indexTo, result.programmatic.to, "the index of the other file drifted after a programmatic write");
  console.log("anchors-browser: a comment follows a cut and a paste, returns on undo, and comes back exact when its tracked deletion is rejected, with the index in step; two transactions in one update both land, and an insertion into a second file moves its comment with that file's index in step");
} finally {
  socket?.close();
  await stopBrowserProcess(browser);
  if (server?.listening) await new Promise((resolve) => server.close(resolve));
  removeTemporary(temporary);
}
