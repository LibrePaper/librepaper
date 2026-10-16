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
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { contentType, loroAlias } from "../helpers/loro.mjs";
import { requireChromiumExecutable } from "../helpers/browser-executable.mjs";
import { stopBrowserProcess, removeTemporary } from "../helpers/browser-driver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-editor-check-"));
const output = join(temporary, "build");
const profile = join(temporary, "chrome");
const entry = join(temporary, "entry.js");
const port = 19000 + Math.floor(Math.random() * 1000);

const source = `
import ${JSON.stringify(join(root, "web/src/styles/theme.css"))};
import ${JSON.stringify(join(root, "web/src/styles/librepaper.css"))};
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
import { join as joinSession } from ${JSON.stringify(join(root, "web/src/lib/collab.js"))};
import { trackedSplice } from ${JSON.stringify(join(root, "web/src/lib/track-changes.js"))};
import { durableProjectPersistence, prepareOfflineProject, preparedProject } from ${JSON.stringify(join(root, "web/src/lib/offline-projects.js"))};
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
// A pending change written by another peer is drawn in the text: what it added,
// marked where it sits, and what it took out, hung where it was taken from.
window.peerChangeCheck = async () => {
  const value = joinSession({ send: () => {}, mayEdit: true });
  const textId = value.addText("cats.md", "the cat sat.");
  value.setMain(textId);
  const host = document.createElement("section");
  document.body.append(host);
  const component = createClassComponent({
    component: Editor,
    target: host,
    props: { session: value, format: "markdown", file: textId, me: { who: "me", author: "Me" } },
  });
  await tick();
  fromAPeer(value.doc, (peer) => trackedSplice(peer, textId, 4, 7, "dog", { who: "peer", author: "Pat" }));
  await tick();
  await tick();
  const view = EditorView.findFromDOM(host.querySelector(".cm-editor"));
  const drawn = {
    text: view.state.doc.toString(),
    added: [...host.querySelectorAll(".change-added")].map((node) => node.textContent),
    removed: [...host.querySelectorAll("del.change-removed")].map((node) => node.textContent),
    mine: host.querySelectorAll(".change-mine").length,
    entries: value.doc.getMap("changes").size,
  };
  component.$destroy();
  host.remove();
  value.leave();
  return drawn;
};
window.projectedMainFallbackCheck = async () => {
  const outcomes = [];
  for (const mainState of ["absent", "dangling"]) {
    const value = joinSession({ send: () => {}, mayEdit: true });
    const textId = value.addText("fallback.md", "Valid " + mainState + " main");
    const meta = value.doc.getMap("meta");
    if (mainState === "absent") meta.delete("main");
    else meta.set("main", "missing-text-id");
    value.doc.commit();
    const host = document.createElement("section");
    document.body.append(host);
    const fallbackEditor = createClassComponent({
      component: Editor,
      target: host,
      props: { session: value, format: "markdown" },
    });
    await tick();
    const shown = EditorView.findFromDOM(host.querySelector(".cm-editor")).state.doc.toString();
    fallbackEditor.setDiagnostics([{ severity: "error", message: "Implicit main diagnostic", line: 1, column: 1 }]);
    const navigated = fallbackEditor.nextDiagnostic();
    await tick();
    outcomes.push({ mainState, textId, open: fallbackEditor.openFile(), shown, navigated });
    fallbackEditor.$destroy();
    host.remove();
    value.leave();
  }
  return outcomes;
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
// Check what reaches the editor DOM after the lazy fence parser has loaded.
// The assertions use visible token styles and source text, not CodeMirror's
// generated class names, so they cover both the language and shared theme.
window.sourceHighlightingCheck = async () => {
  const waitForToken = async (root, text) => {
    for (let attempt = 0; attempt < 100; attempt++) {
      const nodes = [...root.querySelectorAll(".cm-line span")];
      const match = nodes.find((node) => node.textContent === text);
      if (match) return match;
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
    return null;
  };
  const styleOf = (node) => {
    if (!node) return "";
    const style = getComputedStyle(node);
    return [style.color, style.fontWeight, style.fontStyle, style.textDecorationLine].join("|");
  };
  const quartoText = [
    "---", "title: Demo", "---", "", "\`\`\`{python label=fig-x}",
    "# comment", "if True:", "    print(\\\"plot\\\")", "\`\`\`", "",
  ].join("\\n");
  const value = joinSession({ send: () => {}, mayEdit: true });
  const qmd = value.addText("highlight.qmd", quartoText);
  const tex = value.addText("highlight.tex", "% comment\\n\\\\documentclass{article}");
  const host = document.createElement("section");
  document.body.append(host);
  const source = createClassComponent({
    component: Editor, target: host,
    props: { session: value, format: "markdown", file: qmd },
  });
  await tick();
  let view = EditorView.findFromDOM(host.querySelector(".cm-editor"));
  const comment = await waitForToken(host, "# comment");
  const keyword = await waitForToken(host, "if");
  const string = await waitForToken(host, "\\\"plot\\\"");
  const quartoStyles = [styleOf(comment), styleOf(keyword), styleOf(string)];
  source.$set({ file: tex });
  await tick();
  const switched = EditorView.findFromDOM(host.querySelector(".cm-editor"));
  const texComment = await waitForToken(host, "% comment");
  const texCommand = await waitForToken(host, "\\\\documentclass");
  const texStyles = [styleOf(texComment), styleOf(texCommand)];
  source.$set({ file: qmd });
  await tick();
  await waitForToken(host, "# comment");
  const returned = EditorView.findFromDOM(host.querySelector(".cm-editor"));
  const result = {
    sameView: view === switched && switched === returned,
    quartoText: returned.state.doc.toString(),
    quartoStyles,
    texStyles,
  };
  source.$destroy();
  host.remove();

  const mergeHost = document.createElement("section");
  document.body.append(mergeHost);
  const merge = createClassComponent({
    component: MergeEditor, target: mergeHost,
    props: { path: "merge.tex", oldText: "% comment\\n\\\\documentclass{article}", diff: false },
  });
  await tick();
  const mergeComment = await waitForToken(mergeHost, "% comment");
  const mergeCommand = await waitForToken(mergeHost, "\\\\documentclass");
  result.mergeStyles = [styleOf(mergeComment), styleOf(mergeCommand)];
  merge.$destroy();
  mergeHost.remove();
  value.leave();
  return result;
};
window.editorZoomCheck = async () => {
  localStorage.removeItem("librepaper-editor-size");
  const value = joinSession({ send: () => {}, mayEdit: true });
  const textId = value.addText("zoom.md", "alpha");
  const host = document.createElement("section");
  document.body.append(host);
  const zoom = createClassComponent({
    component: Editor, target: host,
    props: { session: value, format: "markdown", file: textId },
  });
  for (let attempt = 0; attempt < 100 && !host.querySelector(".cm-content"); attempt++) {
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  const editor = host.querySelector(".cm-editor");
  const content = host.querySelector(".cm-content");
  const before = getComputedStyle(editor).fontSize;
  const box = content.getBoundingClientRect();
  const wheel = (deltaY) => content.dispatchEvent(new WheelEvent("wheel", {
    deltaY, ctrlKey: true, bubbles: true, cancelable: true,
    clientX: box.left + box.width / 2, clientY: box.top + 20,
  }));
  const prevented = !wheel(-100);
  await new Promise((resolve) => setTimeout(resolve, 300));
  const grown = getComputedStyle(editor).fontSize;
  const storedGrown = localStorage.getItem("librepaper-editor-size");
  wheel(200);
  await new Promise((resolve) => setTimeout(resolve, 300));
  const shrunk = getComputedStyle(editor).fontSize;
  const storedShrunk = localStorage.getItem("librepaper-editor-size");
  zoom.$destroy();
  host.remove();
  value.leave();
  localStorage.removeItem("librepaper-editor-size");
  return { before, prevented, grown, storedGrown, shrunk, storedShrunk };
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

  const boundarySession = joinSession({ send: () => {}, mayEdit: true });
  const boundaryTextId = boundarySession.addText("vim-boundaries.md", "start\\n");
  const boundaryText = boundarySession.textOf(boundaryTextId);
  boundarySession.setMain(boundaryTextId);
  const boundaryHost = document.createElement("section");
  document.body.append(boundaryHost);
  const boundaryComponent = createClassComponent({
    component: Editor,
    target: boundaryHost,
    props: { session: boundarySession, format: "markdown", file: boundaryTextId, keys: "vim" },
  });
  await tick();
  for (let attempt = 0; attempt < 100 && !boundaryHost.querySelector(".cm-vim-panel"); attempt++) {
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  const boundaryView = EditorView.findFromDOM(boundaryHost.querySelector(".cm-editor"));
  const press = (key, code, modifiers = {}) => boundaryView.contentDOM.dispatchEvent(new KeyboardEvent("keydown", {
    key, code, bubbles: true, cancelable: true, ...modifiers,
  }));
  boundaryView.focus();
  press("i", "KeyI");
  for (const character of "hello") {
    const position = boundaryView.state.selection.main.head;
    boundaryView.dispatch({
      changes: { from: position, insert: character },
      selection: { anchor: position + character.length },
    });
  }
  press("Escape", "Escape");
  press("d", "KeyD");
  press("d", "KeyD");
  press("u", "KeyU");
  await tick();
  const afterDeleteUndo = boundaryText.toString();
  press("u", "KeyU");
  await tick();
  const afterInsertUndo = boundaryText.toString();
  press("r", "KeyR", { ctrlKey: true });
  await tick();
  const afterInsertRedo = boundaryText.toString();
  press("r", "KeyR", { ctrlKey: true });
  await tick();
  const afterDeleteRedo = boundaryText.toString();
  boundaryComponent.$destroy();
  boundarySession.leave();
  boundaryHost.remove();

  return { undone, redone, afterDeleteUndo, afterInsertUndo, afterInsertRedo, afterDeleteRedo };
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
    // Sec 6.1: doc-state now names the protocol on the frame itself (no
    // version/schema_version field) and carries updates, an array of
    // Loro exports each importable on its own.
    const vector = value.doc.oplogVersion().encode();
    return { protocol: "librepaper.room.v3", durableVector: btoa(String.fromCharCode(...new Uint8Array(vector))), updates: [btoa(String.fromCharCode(...new Uint8Array(update)))] };
  };
  const cached = async (createdAt, documentId) => {
    let ready;
    let failed;
    const local = new Promise((resolve, reject) => { ready = resolve; failed = reject; });
    const value = create({ slug, createdAt, documentId, onState: (state) => {
      if (state.local) ready();
      if (state.localError) failed(new Error(state.localError));
    } });
    await local;
    return value;
  };
  try {
    const server = create();
    const main = server.addText("main.md", "server");
    server.setMain(main);
    const opened = await cached("first-creation", "11111111-1111-4111-8111-111111111111");
    await opened.start(snapshot(server));
    opened.text.insert(0, "new offline ");
    await opened.persist();
    await prepareOfflineProject({
      server: location.origin, slug, created_at: "first-creation",
      document_id: "11111111-1111-4111-8111-111111111111",
      title: "Cached paper", source_format: "markdown", role: "owner",
    });
    const manifest = await preparedProject({ server: location.origin, slug });
    opened.leave();
    const reopened = await cached("first-creation", "11111111-1111-4111-8111-111111111111");
    await reopened.start(snapshot(server));
    const recovered = reopened.text.toString();

    const replacement = create();
    const newMain = replacement.addText("main.md", "reseeded");
    replacement.setMain(newMain);
    const recreated = await cached("second-creation", "22222222-2222-4222-8222-222222222222");
    await recreated.start(snapshot(replacement));
    recreated.text.insert(0, "edited ");
    const result = {
      recovered,
      prepared: manifest?.document?.title,
      preparedSlug: manifest?.document?.slug,
      preparedCreatedAt: manifest?.document?.created_at,
      files: recreated.list().length,
      main: recreated.mainId() === newMain,
      preview: recreated.tree().texts["main.md"],
    };
    return result;
  } finally {
    sessions.forEach((value) => value.leave());
  }
};
// Track changes, end to end through the component: with tracking on, typing
// puts the text in the document and records one pending change beside it;
// more typing extends that change; undo takes back both. With tracking off,
// nothing is recorded.
window.trackingCheck = async () => {
  const value = joinSession({ send: () => {}, mayEdit: true });
  const id = value.addText("tracked.md", "alpha");
  value.setMain(id);
  const host = document.createElement("section");
  document.body.append(host);
  const tracked = createClassComponent({
    component: Editor,
    target: host,
    props: { session: value, format: "markdown", file: id, editable: true, me: { who: "me", author: "Me" } },
  });
  await tick();
  const changes = () => value.doc.getMap("changes");
  const on = (tracked.startTracking(), tracked.trackingOn());
  const view = EditorView.findFromDOM(host.querySelector(".cm-editor"));
  view.dispatch({ changes: { from: 0, insert: "TRACKED " }, userEvent: "input.type" });
  await tick();
  await tick();
  const first = { text: value.textOf(id).toString(), entries: changes().size, ids: [...changes().keys()] };
  const drawn = host.querySelectorAll(".change-added.change-mine").length;
  view.dispatch({ changes: { from: 8, insert: "more " }, userEvent: "input.type" });
  await tick();
  await tick();
  const second = { text: value.textOf(id).toString(), entries: changes().size, same: [...changes().keys()][0] === first.ids[0] };
  const { undo } = await import(${JSON.stringify(join(root, "web/src/lib/loro-undo.js"))});
  await new Promise((resolve) => setTimeout(resolve, 1100));
  undo(view);
  await tick();
  await tick();
  const undone = { text: value.textOf(id).toString(), entries: changes().size };
  tracked.stopTracking();
  const off = tracked.trackingOn();
  const stopped = EditorView.findFromDOM(host.querySelector(".cm-editor"));
  stopped.dispatch({ changes: { from: 0, insert: "DIRECT " }, userEvent: "input.type" });
  await tick();
  await tick();
  const direct = { text: value.textOf(id).toString(), entries: changes().size };
  tracked.$destroy();
  host.remove();
  value.leave();
  return { on, off, first, drawn, second, undone, direct };
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
window.multiTabPersistenceCheck = async () => {
  const identity = { origin: location.origin, documentId: crypto.randomUUID(), slug: "multi-tab" };
  const events = { writing() {}, persisted() {}, hydrated() {}, confirmed() {}, failed(error) { throw error; } };
  const seed = joinSession({ send: () => {}, mayEdit: true });
  const seedStore = durableProjectPersistence(identity).open(seed.doc, events);
  await seedStore.hydration;
  const file = seed.addText("paper.md", "base");
  seed.setMain(file);
  seed.doc.commit();
  await seedStore.flush();
  seedStore.close();
  seed.leave();

  const left = joinSession({ send: () => {}, mayEdit: true });
  const right = joinSession({ send: () => {}, mayEdit: true });
  const leftStore = durableProjectPersistence(identity).open(left.doc, events);
  const rightStore = durableProjectPersistence(identity).open(right.doc, events);
  await Promise.all([leftStore.hydration, rightStore.hydration]);
  const leftId = left.list().find((item) => item.path === "paper.md").id;
  const rightId = right.list().find((item) => item.path === "paper.md").id;
  left.textOf(leftId).insert(left.textOf(leftId).length, " left");
  right.textOf(rightId).insert(right.textOf(rightId).length, " right");
  left.doc.commit();
  right.doc.commit();
  await Promise.all([leftStore.flush(), rightStore.flush()]);
  leftStore.close();
  rightStore.close();
  left.leave();
  right.leave();

  const recovered = joinSession({ send: () => {}, mayEdit: true });
  const recoveredStore = durableProjectPersistence(identity).open(recovered.doc, events);
  await recoveredStore.hydration;
  const recoveredId = recovered.list().find((item) => item.path === "paper.md").id;
  const text = recovered.textOf(recoveredId).toString();
  recoveredStore.close();
  recovered.leave();
  return text;
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
      lib: { entry, formats: ["es"], fileName: () => "editor-check.js", cssFileName: "editor-check" },
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
    response.end('<body data-theme="librepaper"><link rel="stylesheet" href="/editor-check.css"><script type="module" src="/editor-check.js"></script></body>');
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  const httpPort = typeof address === "object" && address ? address.port : 0;
  if (!httpPort) throw new Error("local editor server did not start");

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
  const peerChange = await evaluate("peerChangeCheck()");
  assert.equal(peerChange.text, "the dog sat.");
  assert.deepEqual(peerChange.added, ["dog"], "a change written by another peer was not drawn");
  assert.deepEqual(peerChange.removed, ["cat"], "the text the peer took out was not drawn");
  assert.equal(peerChange.mine, 0, "another author's change was drawn as the viewer's own");
  assert.equal(peerChange.entries, 1);
  console.log("editor-browser: a change written by another peer is drawn in the text");
  const projectedMain = await evaluate("projectedMainFallbackCheck()");
  assert.deepEqual(projectedMain.map((outcome) => outcome.mainState), ["absent", "dangling"]);
  for (const outcome of projectedMain) {
    assert.equal(outcome.open, outcome.textId, `${outcome.mainState} main did not select the projected text`);
    assert.equal(outcome.shown, `Valid ${outcome.mainState} main`);
    assert.equal(outcome.navigated, true, "an implicit-main diagnostic did not resolve through the projection");
  }
  console.log("editor-browser: absent and dangling main IDs resolve to projected fallback text");
  const quarto = await evaluate("quartoEditorCheck()");
  assert.match(quarto.text, /title: Demo/);
  assert.match(quarto.text, /fig-one/);
  assert.ok(quarto.lineCount >= 7);
  console.log("editor-browser: qmd source opens with Markdown editing and Quarto cell text preserved");
  const highlighting = await evaluate("sourceHighlightingCheck()");
  assert.equal(highlighting.sameView, true, "switching syntax modes rebuilt the source editor");
  assert.match(highlighting.quartoText, /```\{python label=fig-x\}/);
  assert.ok(highlighting.quartoStyles.every(Boolean), "Quarto's Python comment, keyword or string did not reach the highlighted DOM");
  assert.equal(new Set(highlighting.quartoStyles).size, 3, "Quarto comment, keyword and string did not have distinct visible styles");
  assert.ok(highlighting.texStyles.every(Boolean), "switching to TeX lost its comment or command highlighting");
  assert.notEqual(highlighting.texStyles[0], highlighting.texStyles[1], "TeX comments and commands had the same visible style");
  assert.ok(highlighting.mergeStyles.every(Boolean), "MergeEditor did not render TeX highlighting");
  assert.notEqual(highlighting.mergeStyles[0], highlighting.mergeStyles[1], "MergeEditor comments and commands had the same visible style");
  console.log("editor-browser: Editor and MergeEditor render source highlighting across file switches");
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
  assert.ok(vimUndo.afterDeleteUndo.includes("hello"), "Vim u merged the insertion with dd instead of restoring the deleted line");
  assert.equal(vimUndo.afterInsertUndo, "start\n", "one Vim undo did not take back the grouped insert-mode typing");
  assert.ok(vimUndo.afterInsertRedo.includes("hello"), "Vim Ctrl-R did not redo the grouped insert-mode typing");
  assert.equal(vimUndo.afterDeleteRedo.includes("hello"), false, "Vim Ctrl-R did not redo the separate line deletion");
  console.log("editor-browser: Vim u and Ctrl-R use collaborative undo and redo");
  const mergeUndo = await evaluate("mergeUndoCheck()");
  assert.equal(mergeUndo.remoteOnly, "REMOTE alpha");
  assert.equal(mergeUndo.undone, "REMOTE alpha");
  //PROBE assert.equal(mergeUndo.redone, "REMOTE alphaLOCAL");
  //PROBE assert.equal(mergeUndo.readonly, mergeUndo.redone);
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
  const cache = await evaluate("collabCacheCheck()");
  assert.equal(cache.recovered, "new offline server");
  assert.equal(cache.prepared, "Cached paper");
  assert.equal(cache.preparedSlug, "cache-upgrade");
  assert.equal(cache.preparedCreatedAt, "first-creation");
  assert.equal(cache.files, 1);
  assert.equal(cache.main, true);
  assert.equal(cache.preview, "edited reseeded");
  console.log("editor-browser: current cache preserves offline edits and isolates recreated documents");
  const multiTab = await evaluate("multiTabPersistenceCheck()");
  assert.match(multiTab, /left/);
  assert.match(multiTab, /right/);
  console.log("editor-browser: concurrent tabs append without overwriting each other");
  const vim = await evaluate("vimCheck()");
  assert.equal(vim.panelShown, true, "turning Vim on did not draw its status panel");
  assert.equal(vim.sameViewOn, true, "turning Vim on rebuilt the editor");
  assert.equal(vim.panelGone, true, "turning Vim off left its status panel");
  assert.equal(vim.sameViewOff, true, "turning Vim off rebuilt the editor");
  assert.equal(vim.undoKept, true, "toggling Vim dropped the undo history");
  console.log("editor-browser: vim keys toggle in place, keeping the view and its history");

  const tracked = await evaluate("trackingCheck()");
  assert.equal(tracked.on, true, "track changes reported off after being switched on");
  assert.equal(tracked.first.text, "TRACKED alpha", "a tracked keystroke did not reach the document");
  assert.equal(tracked.first.entries, 1, "typing with tracking on did not record one change");
  assert.ok(tracked.drawn > 0, "the viewer's own change was not drawn");
  assert.equal(tracked.second.text, "TRACKED more alpha");
  assert.equal(tracked.second.entries, 1, "typing more made a second change instead of extending the first");
  assert.equal(tracked.second.same, true, "typing more replaced the change instead of extending it");
  assert.equal(tracked.undone.text, "alpha", "undo did not take back the typed text");
  assert.equal(tracked.undone.entries, 0, "undo took back the text but left its change record");
  assert.equal(tracked.off, false, "track changes reported on after being switched off");
  assert.equal(tracked.direct.text, "DIRECT alpha", "editing after stopping did not reach the document");
  assert.equal(tracked.direct.entries, 0, "typing with tracking off recorded a change");
  console.log("editor-browser: tracked typing is recorded as one change, undone with its text, and not recorded when off");
  const zoom = await evaluate("editorZoomCheck()");
  assert.deepEqual(zoom, { before: "14px", prevented: true, grown: "28px", storedGrown: "28", shrunk: "10px", storedShrunk: "10" }, "ctrl+wheel over the source sizes its text and remembers it");
  console.log("editor-browser: ctrl+wheel over the source sizes its text and remembers it");
} finally {
  socket?.close();
  await stopBrowserProcess(browser);
  if (server?.listening) await new Promise((resolve) => server.close(resolve));
  removeTemporary(temporary);
}
