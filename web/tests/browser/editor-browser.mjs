// A browser-level regression check for the source editor.
//
// This deliberately exercises the component through its public props and DOM,
// while Loro supplies the same shared-text changes a peer would make. It does
// not inspect the component's state cache or effects.

import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { contentType, loroAlias } from "../helpers/loro.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-editor-check-"));
const output = join(temporary, "build");
const profile = join(temporary, "chrome");
const entry = join(temporary, "entry.js");
const port = 19000 + Math.floor(Math.random() * 1000);

const source = `
import { LoroDoc } from "loro-crdt";
import { tick } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import { EditorView } from ${JSON.stringify(join(root, "web/node_modules/@codemirror/view/dist/index.js"))};
import { undoManagerStateField } from ${JSON.stringify(join(root, "web/vendor/loro-codemirror/undo.ts"))};
// The binding offers whether an undo is available, not how many are stacked
// up. What these checks are really about is whether the history survived, so
// they ask that instead.
// A change from somebody else, arriving the way one actually does: made on a
// separate document and imported. Editing this browser's own document instead
// would be a local change, which the binding rightly ignores -- the old
// version could label a local transaction "remote", and Loro has no such
// pretence.
const fromAPeer = (target, edit) => {
  const peer = new LoroDoc();
  peer.import(target.export({ mode: "update" }));
  edit(peer);
  peer.commit();
  target.import(peer.export({ mode: "update", from: target.oplogVersion() }));
};
const undoDepth = (state) => Boolean(state.field(undoManagerStateField, false)?.canUndo());
import MergeEditor from ${JSON.stringify(join(root, "web/src/components/MergeEditor.svelte"))};
import Editor from ${JSON.stringify(join(root, "web/src/components/Editor.svelte"))};
import Diagnostics from ${JSON.stringify(join(root, "web/src/components/reader/Diagnostics.svelte"))};
import History from ${JSON.stringify(join(root, "web/src/components/reader/History.svelte"))};
import { join as joinSession } from ${JSON.stringify(join(root, "web/src/lib/collab.js"))};
import { prepareOfflineProject, preparedProject } from ${JSON.stringify(join(root, "web/src/lib/offline-projects.js"))};
import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};

const session = joinSession({ send: () => {}, mayEdit: true });
const firstFile = session.addText("a.md", "alpha");
const secondFile = session.addText("b.md", "beta");
session.setMain(firstFile);
const component = createClassComponent({
  component: Editor,
  target: document.body,
  props: { session, format: "markdown", file: firstFile },
});

window.editorCheck = async () => {
  await tick();
  const firstView = EditorView.findFromDOM(document.querySelector(".cm-editor"));
  firstView.dispatch({
    changes: { from: 0, insert: "LOCAL " },
    selection: { anchor: 3 },
  });
  const before = {
    undo: undoDepth(firstView.state),
    caret: firstView.state.selection.main.head,
    text: firstView.state.doc.toString(),
  };

  component.$set({ file: secondFile });
  await tick();
  const secondText = EditorView.findFromDOM(document.querySelector(".cm-editor")).state.doc.toString();

  // A peer changes A while this browser is looking at B.
  fromAPeer(session.doc, (peer) => peer.getMap("files").get(firstFile).insert(0, "REMOTE "));
  component.$set({ file: firstFile });
  await tick();
  await tick();
  const finalView = EditorView.findFromDOM(document.querySelector(".cm-editor"));
  return {
    before,
    secondText,
    after: {
      undo: undoDepth(finalView.state),
      caret: finalView.state.selection.main.head,
      text: finalView.state.doc.toString(),
    },
    sameView: firstView === finalView,
  };
};
window.remoteUndoCheck = async () => {
  const value = joinSession({ send: () => {}, mayEdit: true });
  const textId = value.addText("undo.md", "alpha");
  const text = value.textOf(textId);
  value.setMain(textId);
  // Its own host, so this scenario finds its own editor. Counting editors in
  // the body means every scenario that mounted earlier and did not tidy up
  // shifts the index, and the one found belongs to a different document.
  const host = document.createElement("section");
  document.body.append(host);
  const localComponent = createClassComponent({
    component: Editor,
    target: host,
    props: { session: value, format: "markdown", file: textId },
  });
  await tick();
  const view = EditorView.findFromDOM(host.querySelector(".cm-editor"));
  fromAPeer(value.doc, (peer) => peer.getMap("files").get(textId).insert(0, "REMOTE "));
  await tick();
  view.focus();
  const press = (key, options = {}) => view.contentDOM.dispatchEvent(new KeyboardEvent("keydown", {
    key, code: "Key" + key.toUpperCase(), bubbles: true, cancelable: true, ...options,
  }));
  press("z", { ctrlKey: true });
  await tick();
  const remoteOnly = text.toString();
  // A dispatch into a view is in the view's coordinates. Reaching for the
  // document's length instead only worked while the two were kept in exact
  // step, which is an assumption about the binding rather than about the
  // editor.
  view.dispatch({ changes: { from: view.state.doc.length, insert: "LOCAL" } });
  press("z", { ctrlKey: true });
  await tick();
  const localUndone = text.toString();
  press("y", { ctrlKey: true });
  await tick();
  const localRedone = text.toString();
  await localComponent.editCommand("undo");
  const menuUndone = text.toString();
  await localComponent.editCommand("redo");
  const menuRedone = text.toString();
  await localComponent.editCommand("select-all");
  const selected = view.state.sliceDoc(view.state.selection.main.from, view.state.selection.main.to);
  await localComponent.editCommand("find");
  const searchOpen = !!view.dom.querySelector('[name="search"]');
  await localComponent.editCommand("replace");
  const replaceFocused = document.activeElement?.getAttribute("name") === "replace";
  localComponent.$set({ editable: false });
  await tick();
  await localComponent.editCommand("undo");
  const readonly = text.toString();
  const cutDisabled = !localComponent.editAvailability().cut;
  localComponent.$destroy();
  value.leave();
  return { remoteOnly, localUndone, localRedone, menuUndone, menuRedone, selected, searchOpen, replaceFocused, readonly, cutDisabled };
};
window.quartoEditorCheck = async () => {
  const fence = String.fromCharCode(96).repeat(3);
  const qmd = session.addText("paper.qmd", ["---", "title: Demo", "---", "", fence + "{r}", "#| label: fig-one", "plot(1)", fence].join("\\n"));
  component.$set({ file: qmd, format: "quarto" });
  await tick();
  const view = EditorView.findFromDOM(document.querySelector(".cm-editor"));
  return { text: view.state.doc.toString(), lineCount: view.state.doc.lines, mode: document.querySelector(".cm-editor")?.className || "" };
};
window.vimUndoCheck = async () => {
  const value = joinSession({ send: () => {}, mayEdit: true });
  const textId = value.addText("vim-undo.md", "alpha");
  const text = value.textOf(textId);
  value.setMain(textId);
  const host = document.createElement("section");
  document.body.append(host);
  const vimComponent = createClassComponent({
    component: Editor,
    target: host,
    props: { session: value, format: "markdown", file: textId, keys: "vim" },
  });
  await tick();
  for (let attempt = 0; attempt < 100 && !host.querySelector(".cm-vim-panel"); attempt++) {
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  const view = EditorView.findFromDOM(host.querySelector(".cm-editor"));
  view.dispatch({ changes: { from: view.state.doc.length, insert: "LOCAL" } });
  view.focus();
  view.contentDOM.dispatchEvent(new KeyboardEvent("keydown", {
    key: "u", code: "KeyU", bubbles: true, cancelable: true,
  }));
  await tick();
  const undone = text.toString();
  view.contentDOM.dispatchEvent(new KeyboardEvent("keydown", {
    key: "r", code: "KeyR", ctrlKey: true, bubbles: true, cancelable: true,
  }));
  await tick();
  const redone = text.toString();
  vimComponent.$destroy();
  value.leave();
  return { undone, redone };
};
window.mergeUndoCheck = async () => {
  const value = joinSession({ send: () => {}, mayEdit: true });
  const id = value.addText("merge.md", "alpha");
  const text = value.textOf(id);
  const host = document.createElement("section");
  document.body.append(host);
  const mergeComponent = createClassComponent({ component: MergeEditor, target: host,
    props: { oldText: "alpha", newText: "alpha", liveText: text, loroDoc: value.doc, ephemeral: value.ephemeral, editable: true },
  });
  await tick();
  const views = [...host.querySelectorAll(".cm-editor")].map((el) => EditorView.findFromDOM(el));
  const press = (view, key, options = {}) => {
    view.focus();
    view.contentDOM.dispatchEvent(new KeyboardEvent("keydown", {
      key: options.shiftKey ? key.toUpperCase() : key,
      keyCode: key.toUpperCase().charCodeAt(0),
      code: "Key" + key.toUpperCase(), ctrlKey: true, bubbles: true, cancelable: true, ...options,
    }));
  };
  const errors = [];
  const onError = (event) => errors.push(event.message);
  window.addEventListener("error", onError);
  fromAPeer(value.doc, (peer) => peer.getMap("files").get(id).insert(0, "REMOTE "));
  press(views[0], "z");
  press(views[1], "z");
  const remoteOnly = text.toString();
  views[1].dispatch({ changes: { from: views[1].state.doc.length, insert: "LOCAL" } });
  press(views[1], "z");
  const undone = text.toString();
  press(views[1], "z", { shiftKey: true });
  const redone = text.toString();
  mergeComponent.$set({ editable: false });
  await tick();
  for (const el of host.querySelectorAll(".cm-editor")) press(EditorView.findFromDOM(el), "z");
  const readonly = text.toString();
  window.removeEventListener("error", onError);
  mergeComponent.$destroy();
  host.remove();
  value.leave();
  return { remoteOnly, undone, redone, readonly, errors };
};
window.collabCacheCheck = async () => {
  const slug = "cache-upgrade";
  const sessions = [];
  const create = (props = {}) => {
    const value = joinSession({ send: () => {}, mayEdit: true, ...props });
    sessions.push(value);
    return value;
  };
  const snapshot = (value) => {
    const update = value.doc.export({ mode: "update" });
    return { update: btoa(String.fromCharCode(...new Uint8Array(update))) };
  };
  const cached = async (createdAt) => {
    let ready;
    const local = new Promise((resolve) => { ready = resolve; });
    const value = create({ slug, createdAt, onState: (state) => { if (state.local) ready(); } });
    await local;
    return value;
  };
  try {
    const server = create();
    const main = server.addText("main.md", "server");
    server.setMain(main);
    const opened = await cached("first-creation");
    await opened.start(snapshot(server));
    opened.text.insert(0, "new offline ");
    await opened.persist();
    await prepareOfflineProject({
      server: location.origin, slug, created_at: "first-creation",
      title: "Cached paper", source_format: "markdown", role: "owner",
    });
    const manifest = await preparedProject({ server: location.origin, slug });
    opened.leave();
    const reopened = await cached("first-creation");
    await reopened.start(snapshot(server));
    const recovered = reopened.text.toString();

    const replacement = create();
    const newMain = replacement.addText("main.md", "reseeded");
    replacement.setMain(newMain);
    const recreated = await cached("second-creation");
    await recreated.start(snapshot(replacement));
    recreated.text.insert(0, "edited ");
    const result = {
      recovered,
      prepared: manifest?.document?.title,
      files: recreated.list().length,
      main: recreated.mainId() === newMain,
      preview: recreated.tree().texts["main.md"],
    };
    return result;
  } finally {
    sessions.forEach((value) => value.leave());
  }
};
// Track changes, end to end through the component: switching it on has to
// take the keystrokes out of the paper and put them on a branch, and that
// branch has to reach the socket while the author is still typing. Both of
// those were broken -- the binding was rebuilt while the prop that said
// "tracking" had not arrived yet, so every tracked keystroke went straight
// into the document, and nothing flushed until tracking was switched off.
window.trackingCheck = async () => {
  const sent = [];
  const value = joinSession({ send: () => {}, mayEdit: true });
  const id = value.addText("tracked.md", "alpha");
  value.setMain(id);
  const host = document.createElement("section");
  document.body.append(host);
  const tracked = createClassComponent({
    component: Editor,
    target: host,
    props: {
      session: value, format: "markdown", file: id, editable: true,
      send: (message) => { sent.push(message); return true; },
    },
  });
  await tick();
  tracked.startTracking();
  await tick();
  const on = tracked.trackingOn();
  const opened = sent.some((message) => message.type === "proposal-open");
  // The server is what names a branch, and an update has nothing to address
  // itself to until it has.
  tracked.receiveProposal({ type: "proposal-opened", proposal_id: "p1" });
  const view = EditorView.findFromDOM(host.querySelector(".cm-editor"));
  view.dispatch({ changes: { from: 0, insert: "TRACKED " } });
  // Longer than the flush pause: the point of the check is that typing sends,
  // without anything else having to happen.
  await new Promise((resolve) => setTimeout(resolve, 700));
  const update = sent.find((message) => message.type === "proposal-update");
  const shown = view.state.doc.toString();
  const paper = value.textOf(id).toString();
  tracked.stopTracking();
  await tick();
  const off = tracked.trackingOn();
  const afterStop = value.textOf(id).toString();
  const back = EditorView.findFromDOM(host.querySelector(".cm-editor"));
  back.dispatch({ changes: { from: 0, insert: "DIRECT " } });
  await tick();
  const direct = value.textOf(id).toString();
  tracked.$destroy();
  host.remove();
  value.leave();
  return {
    on, off, opened, shown, paper, afterStop, direct,
    flushed: Boolean(update && update.proposal_id === "p1" && update.update),
  };
};
window.diagnosticsCheck = async () => {
  const host = document.createElement("aside");
  document.body.append(host);
  let chosen;
  const component = createClassComponent({ component: Diagnostics, target: host, props: {
    main: "main.typ",
    diagnostics: [
      { severity: "warning", message: "General compiler warning", hints: ["Try this suggestion"], line: 0 },
      { severity: "error", message: "Unknown name", file: "chapter.typ", line: 4, column: 2 },
    ],
    canOpen: (item) => item.line > 0,
    onopen: (item) => { chosen = item; },
  } });
  await tick();
  const text = host.textContent;
  const links = host.querySelectorAll("button");
  const link = links[0]?.textContent.trim();
  links[0]?.click();
  component.$set({ diagnostics: [] });
  await tick();
  const empty = host.textContent;
  // The transcript itself: what a compile that stopped for a reason the
  // parser has no line for leaves a person to read.
  component.$set({
    main: "librepaper.tex",
    diagnostics: [{ severity: "error", message: "Emergency stop.", line: 0 }],
    log: "This is pdfTeX\\n! Emergency stop.\\n<*> &pdflatex librepaper.tex\\nTranscript written on librepaper.log.",
  });
  await tick();
  const logBlock = host.querySelector("pre[aria-label='Compile log']");
  const downloadButton = [...host.querySelectorAll("button")].find((button) => button.textContent.includes("Download"));
  const openByDefault = Boolean(host.querySelector("details[open] pre[aria-label='Compile log']"));
  // A failure that stopped in a later stage leaves the engine's transcript on
  // the attempt and nothing on the result. The panel showed only a download
  // button from "Earlier attempts" then -- no box, nothing to copy.
  component.$set({
    log: "",
    attempts: [
      { stage: "browser", backend: "browser", log: "This is pdfTeX\\n! Emergency stop." },
      { stage: "browser-biber", backend: "browser", log: "" },
    ],
  });
  await tick();
  const fromAttempt = host.querySelector("pre[aria-label='Compile log']")?.textContent || "";
  const copyFromAttempt = [...host.querySelectorAll("button")].some((button) => button.textContent.trim() === "Copy");
  const result = {
    text, links: links.length, link, chosen, empty,
    logShown: logBlock?.textContent || "",
    logWhole: logBlock?.textContent.startsWith("This is pdfTeX") || false,
    download: downloadButton?.textContent.trim() || "",
    openByDefault,
    fromAttempt,
    copyFromAttempt,
  };
  component.$destroy();
  host.remove();
  return result;
};

// Switching Vim keys on and off must reconfigure the view in place: the same
// EditorView, with the undo history it had.
// The history panel for a document nobody has checkpointed: a month that
// marks the days somebody wrote on, and days that are nothing but the minutes
// nobody saved -- plus arrow keys that move the month without a mouse. The
// version side of the same panel is checked in history-panel-browser.mjs.
window.activityCheck = async () => {
  const host = document.createElement("aside");
  document.body.append(host);
  const opened = [];
  const rows = [
    { at: "2026-09-14T09:05:00Z", peer: "", changes: 2, state_bytes: 1000, frontier: "one" },
    { at: "2026-09-14T09:40:00Z", peer: "", changes: 3, state_bytes: 1600, frontier: "two" },
    { at: "2026-09-15T15:00:00Z", peer: "", changes: 1, state_bytes: 1750, frontier: "three" },
  ];
  const component = createClassComponent({ component: History, target: host, props: {
    checkpoints: [], activity: rows, viewingMoment: "",
    onmoment: (frontier) => opened.push(frontier), onview: () => {},
  } });
  const versions = () => [...host.querySelectorAll(".day-row")];
  const unsaved = () => [...host.querySelectorAll(".unsaved-row")];
  const more = () => host.querySelector(".unsaved-more");
  const back = async () => { host.querySelector("#history-tab-calendar").click(); await tick(); };
  const into = async (day) => {
    host.querySelector('[data-history-day="' + day + '"]').click();
    await tick();
  };
  const chosen = () => host.querySelector("[data-history-day][aria-pressed=true]")?.dataset.historyDay;
  await tick();

  // It opens on a day, with no month beside it. Nothing here was ever saved
  // as a version, so the day has no versions to list and everything it does
  // have is behind the one folded line.
  const first = {
    month: host.querySelectorAll("[data-history-day]").length,
    versions: versions().length,
    folded: more()?.getAttribute("aria-expanded") ?? null,
    built: unsaved().length,
    says: more()?.textContent.replace(/\\s+/g, " ").trim() ?? null,
  };
  more().click();
  await tick();
  const shown = unsaved().map((node) => node.textContent.trim());
  unsaved()[0].click();
  await tick();

  // Back to the month: two days marked as written on, and no count on either,
  // because neither holds a version.
  await back();
  const month = {
    cells: host.querySelectorAll("[data-history-day]").length,
    marked: host.querySelectorAll(".cal-mark").length,
    counted: host.querySelectorAll(".cal-count").length,
    versions: versions().length,
    picked: chosen(),
  };

  // A day at a time across the calendar, and a week at a time up it.
  const press = async (key) => {
    host.querySelector(".cal-grid").dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }));
    await tick();
    return chosen();
  };
  const afterLeft = await press("ArrowLeft");
  const afterUp = await press("ArrowUp");

  // A day nobody worked says so, rather than drawing an empty anything.
  await into("2026-09-07");
  const emptyDay = versions().length === 0 && !more()
    && host.textContent.includes("Nothing was written on this day");

  // The document starts in September, so there is nowhere to the left of its
  // first day to go.
  await back();
  await into("2026-09-01");
  await back();
  const atTheStart = await press("ArrowLeft");

  const result = { ...first, ...month, shown, opened, afterLeft, afterUp, emptyDay, atTheStart };
  component.$destroy();
  host.remove();
  return result;
};

window.vimCheck = async () => {
  await tick();
  const view = EditorView.findFromDOM(document.querySelector(".cm-editor"));
  const undo = undoDepth(view.state);
  const settle = async (want) => {
    for (let attempt = 0; attempt < 100; attempt++) {
      if (Boolean(document.querySelector(".cm-vim-panel")) === want) return true;
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    return false;
  };
  component.$set({ keys: "vim" });
  const panelShown = await settle(true);
  const onView = EditorView.findFromDOM(document.querySelector(".cm-editor"));
  component.$set({ keys: "default" });
  const panelGone = await settle(false);
  const offView = EditorView.findFromDOM(document.querySelector(".cm-editor"));
  return {
    panelShown,
    panelGone,
    sameViewOn: view === onView,
    sameViewOff: view === offView,
    undoKept: undoDepth(offView.state) === undo,
  };
};
window.editorCheckReady = true;
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
      lib: { entry, formats: ["es"], fileName: () => "editor-check.js" },
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
    response.end('<body><script type="module" src="/editor-check.js"></script></body>');
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  const httpPort = typeof address === "object" && address ? address.port : 0;
  if (!httpPort) throw new Error("local editor server did not start");

  browser = spawn("chromium", [
    "--headless=new",
    "--no-sandbox",
    "--disable-gpu",
    `--user-data-dir=${profile}`,
    `--remote-debugging-port=${port}`,
    "about:blank",
  ], { stdio: "ignore" });
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
    ready = await evaluate("Boolean(window.editorCheckReady && document.querySelector('.cm-editor'))");
    if (ready) break;
    await wait(100);
  }
  assert.equal(ready, true, "Editor did not mount in Chromium");
  const result = await evaluate("editorCheck()");
  assert.equal(result.sameView, true);
  assert.equal(result.secondText, "beta");
  assert.equal(result.before.undo, true);
  assert.equal(result.after.undo, true, "the undo history did not survive switching files");
  assert.equal(result.before.caret + 7, result.after.caret);
  assert.equal(result.after.text, "REMOTE LOCAL alpha");
  console.log("editor-browser: file state, undo, caret, and inactive remote text preserved");
  const quarto = await evaluate("quartoEditorCheck()");
  assert.match(quarto.text, /title: Demo/);
  assert.match(quarto.text, /fig-one/);
  assert.ok(quarto.lineCount >= 7);
  console.log("editor-browser: qmd source opens with Markdown editing and Quarto cell text preserved");
  const remoteUndo = await evaluate("remoteUndoCheck()");
  assert.equal(remoteUndo.remoteOnly, "REMOTE alpha");
  assert.equal(remoteUndo.localUndone, "REMOTE alpha");
  assert.equal(remoteUndo.localRedone, "REMOTE alphaLOCAL");
  console.log("editor-browser: remote changes stay out of undo, local undo and redo work");
  assert.equal(remoteUndo.menuUndone, "REMOTE alpha");
  assert.equal(remoteUndo.menuRedone, "REMOTE alphaLOCAL");
  assert.equal(remoteUndo.selected, remoteUndo.menuRedone);
  assert.equal(remoteUndo.searchOpen, true);
  assert.equal(remoteUndo.replaceFocused, true);
  assert.equal(remoteUndo.readonly, remoteUndo.menuRedone);
  assert.equal(remoteUndo.cutDisabled, true);
  console.log("editor-browser: Edit commands preserve remote changes, select source, open search, and respect read-only mode");
  const vimUndo = await evaluate("vimUndoCheck()");
  assert.equal(vimUndo.undone, "alpha");
  assert.equal(vimUndo.redone, "alphaLOCAL");
  console.log("editor-browser: Vim u and Ctrl-R use collaborative undo and redo");
  const mergeUndo = await evaluate("mergeUndoCheck()");
  assert.equal(mergeUndo.remoteOnly, "REMOTE alpha");
  assert.equal(mergeUndo.undone, "REMOTE alpha");
  assert.equal(mergeUndo.redone, "REMOTE alphaLOCAL");
  assert.equal(mergeUndo.readonly, mergeUndo.redone);
  assert.deepEqual(mergeUndo.errors, []);
  console.log("editor-browser: merge undo preserves remote edits and read-only panes stay inert");
  const diagnostics = await evaluate("diagnosticsCheck()");
  assert.match(diagnostics.text, /General compiler warning/);
  assert.match(diagnostics.text, /Try this suggestion/);
  assert.match(diagnostics.text, /Unknown name/);
  assert.equal(diagnostics.links, 1);
  assert.equal(diagnostics.link, "chapter.typ:4:2");
  assert.equal(diagnostics.chosen.file, "chapter.typ");
  assert.match(diagnostics.empty, /No warnings or errors/);
  // The engine's own transcript, whole, named as TeX names it, and open when
  // the compile failed -- the parsed list cannot explain an Emergency stop.
  assert.match(diagnostics.logShown, /Emergency stop/);
  assert.equal(diagnostics.logWhole, true, "the panel showed a tail rather than the whole transcript");
  assert.equal(diagnostics.download, "Download librepaper.log");
  assert.equal(diagnostics.openByDefault, true, "a failed compile must not hide its transcript behind a twisty");
  assert.match(diagnostics.fromAttempt, /Emergency stop/, "a transcript carried by the attempt was not shown");
  assert.equal(diagnostics.copyFromAttempt, true, "the attempt's transcript was shown without a way to take it");
  console.log("editor-browser: diagnostics, hints, source links, empty state and the compile log passed");
  const activity = await evaluate("activityCheck()");
  assert.equal(activity.month, 0, "the panel opens on a day, with the month nowhere on screen");
  assert.equal(activity.versions, 0, "a document nobody saved from has no versions to list");
  assert.equal(activity.folded, "false", "and what it does have is folded away");
  assert.equal(activity.built, 0, "with none of it built until it is asked for");
  assert.match(activity.says, /1 moment not saved as a version/, "counted in the singular");
  assert.deepEqual(activity.shown.length, 1, "opened, it is the minute nobody saved");
  assert.match(activity.shown[0], /3:00/, "which says when it was");
  assert.deepEqual(activity.opened, ["three"],
    "and clicking it asks for the document as it stood then");
  assert.equal(activity.cells % 7, 0, "the month draws whole weeks");
  assert.equal(activity.marked, 2, "with the two days somebody wrote on marked");
  assert.equal(activity.counted, 0, "and no version counted on any of them");
  assert.equal(activity.picked, "2026-09-15", "the panel opened on the last day anything happened");
  assert.equal(activity.afterLeft, "2026-09-14", "ArrowLeft moves the calendar a day back");
  assert.equal(activity.afterUp, "2026-09-07", "ArrowUp moves a week");
  assert.equal(activity.emptyDay, true, "a day nobody worked says so rather than drawing nothing");
  assert.equal(activity.atTheStart, "2026-09-01",
    "arrowing past the first month the document has must go nowhere");
  console.log("editor-browser: the history panel's month, its marked days and its arrow keys passed");
  const cache = await evaluate("collabCacheCheck()");
  assert.equal(cache.recovered, "new offline server");
  assert.equal(cache.prepared, "Cached paper");
  assert.equal(cache.files, 1);
  assert.equal(cache.main, true);
  assert.equal(cache.preview, "edited reseeded");
  console.log("editor-browser: current cache preserves offline edits and isolates recreated documents");
  const vim = await evaluate("vimCheck()");
  assert.equal(vim.panelShown, true, "turning Vim on did not draw its status panel");
  assert.equal(vim.sameViewOn, true, "turning Vim on rebuilt the editor");
  assert.equal(vim.panelGone, true, "turning Vim off left its status panel");
  assert.equal(vim.sameViewOff, true, "turning Vim off rebuilt the editor");
  assert.equal(vim.undoKept, true, "toggling Vim dropped the undo history");
  console.log("editor-browser: vim keys toggle in place, keeping the view and its history");

  const tracked = await evaluate("trackingCheck()");
  assert.equal(tracked.on, true, "track changes reported off after being switched on");
  assert.equal(tracked.opened, true, "switching track changes on did not open a proposal");
  assert.equal(tracked.shown, "TRACKED alpha", "the author cannot see what they typed");
  assert.equal(tracked.paper, "alpha", "a tracked keystroke reached the document instead of the branch");
  assert.equal(tracked.flushed, true, "the branch was never sent, so nobody could review it");
  assert.equal(tracked.off, false, "track changes reported on after being switched off");
  assert.equal(tracked.afterStop, "alpha", "stopping applied the proposal instead of leaving it for review");
  assert.equal(tracked.direct, "DIRECT alpha", "editing after stopping did not reach the document");
  console.log("editor-browser: tracked edits go to a branch, are sent while typing, and leave the paper alone");
} finally {
  socket?.close();
  browser?.kill();
  if (server?.listening) await new Promise((resolve) => server.close(resolve));
  rmSync(temporary, { recursive: true, force: true });
}
