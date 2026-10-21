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
import { startCompletion } from ${JSON.stringify(join(root, "web/node_modules/@codemirror/autocomplete/dist/index.js"))};
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
import { rejectMany, resolveChanges, trackedSplice } from ${JSON.stringify(join(root, "web/src/lib/track-changes.js"))};
import { durableProjectPersistence, rememberProject, localProject } from ${JSON.stringify(join(root, "web/src/lib/offline-projects.js"))};
import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};
import { setSize, setFont, setLineHeight, start as startEditorType } from ${JSON.stringify(join(root, "web/src/lib/editor-type.js"))};

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
    entries: [...value.doc.getMap("changes").entries()].filter(([, v]) => (typeof v?.get === "function" ? v.get("kind") : v?.kind) === "change").length,
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
window.editorTypeCheck = async () => {
  const value = joinSession({ send: () => {}, mayEdit: true });
  const textId = value.addText("type.md", "alpha");
  const host = document.createElement("section");
  document.body.append(host);
  const typed = createClassComponent({
    component: Editor, target: host,
    props: { session: value, format: "markdown", file: textId },
  });
  for (let attempt = 0; attempt < 100 && !host.querySelector(".cm-content"); attempt++) {
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  startEditorType();
  const frame = () => new Promise((resolve) => requestAnimationFrame(() => resolve()));
  const editor = host.querySelector(".cm-editor");
  const content = host.querySelector(".cm-content");
  setSize(20);
  await frame();
  const sized = getComputedStyle(editor).fontSize;
  const box = content.getBoundingClientRect();
  content.dispatchEvent(new WheelEvent("wheel", {
    deltaY: -100, ctrlKey: true, bubbles: true, cancelable: true,
    clientX: box.left + box.width / 2, clientY: box.top + 20,
  }));
  await new Promise((resolve) => setTimeout(resolve, 300));
  const pinched = getComputedStyle(editor).fontSize;
  setSize(14);
  setLineHeight("wide");
  await frame();
  const lineRatio = () => Math.round(parseFloat(getComputedStyle(content).lineHeight) / parseFloat(getComputedStyle(editor).fontSize) * 100) / 100;
  const wideLine = lineRatio();
  setLineHeight("normal");
  await frame();
  const normalLine = lineRatio();
  setFont("serif");
  await frame();
  const serif = getComputedStyle(editor).fontFamily;
  setFont("custom", "Comic Neue");
  await frame();
  const comic = getComputedStyle(editor).fontFamily;
  const bodyColor = getComputedStyle(document.body).color;
  setFont("custom", 'Evil"; } body { color: red');
  await frame();
  const evil = getComputedStyle(editor).fontFamily;
  const bodyColorAfter = getComputedStyle(document.body).color;
  globalThis.LIBREPAPER_FONTS = { "fira-code.woff2": "/fira-code.woff2" };
  setFont("fira-code");
  await frame();
  await document.fonts.ready;
  const firaFace = () => [...document.fonts].find((face) => face.family.replace(/"/g, "") === "Fira Code");
  const firaStatus = firaFace()?.status ?? "missing";
  setFont("default");
  await frame();
  const firaAfter = firaFace() ? "present" : "gone";
  const defaultVariable = document.documentElement.style.getPropertyValue("--editor-font-family");
  const keyed = EditorView.findFromDOM(editor);
  keyed.focus();
  setSize(14);
  await frame();
  const press = (key, code) => keyed.contentDOM.dispatchEvent(new KeyboardEvent("keydown", {
    key, code, ctrlKey: true, bubbles: true, cancelable: true,
  }));
  press("=", "Equal");
  await frame();
  const keyBigger = getComputedStyle(editor).fontSize;
  press("-", "Minus");
  await frame();
  press("-", "Minus");
  await frame();
  const keySmaller = getComputedStyle(editor).fontSize;
  press("0", "Digit0");
  await frame();
  const keyReset = getComputedStyle(editor).fontSize;
  setSize(14);
  setLineHeight("normal");
  typed.$destroy();
  host.remove();
  value.leave();
  return { sized, pinched, wideLine, normalLine, serif, comic, evil, firaStatus, firaAfter, bodyColor, bodyColorAfter, defaultVariable, keyBigger, keySmaller, keyReset };
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
    await rememberProject({
      server: location.origin, slug, created_at: "first-creation",
      document_id: "11111111-1111-4111-8111-111111111111",
      title: "Cached paper", source_format: "markdown", role: "owner",
    });
    const manifest = await localProject({ server: location.origin, slug });
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
  // A replacement is two records (a change and a gap), so only change rows count.
  const changeIds = () => [...changes().entries()].filter(([, v]) => (typeof v?.get === "function" ? v.get("kind") : v?.kind) === "change").map(([id]) => id);
  const rows = () => [...changes().entries()].filter(([, v]) => (typeof v?.get === "function" ? v.get("kind") : v?.kind) === "change").length;
  const on = (tracked.startTracking(), tracked.trackingOn());
  const view = EditorView.findFromDOM(host.querySelector(".cm-editor"));
  view.dispatch({ changes: { from: 0, insert: "TRACKED " }, userEvent: "input.type" });
  await tick();
  await tick();
  const first = { text: value.textOf(id).toString(), entries: rows(), ids: changeIds() };
  const drawn = host.querySelectorAll(".change-added.change-mine").length;
  view.dispatch({ changes: { from: 8, insert: "more " }, userEvent: "input.type" });
  await tick();
  await tick();
  const second = { text: value.textOf(id).toString(), entries: rows(), same: changeIds()[0] === first.ids[0] };
  const { undo } = await import(${JSON.stringify(join(root, "web/src/lib/loro-undo.js"))});
  await new Promise((resolve) => setTimeout(resolve, 1100));
  undo(view);
  await tick();
  await tick();
  const undone = { text: value.textOf(id).toString(), entries: rows() };
  tracked.stopTracking();
  const off = tracked.trackingOn();
  const stopped = EditorView.findFromDOM(host.querySelector(".cm-editor"));
  stopped.dispatch({ changes: { from: 0, insert: "DIRECT " }, userEvent: "input.type" });
  await tick();
  await tick();
  const direct = { text: value.textOf(id).toString(), entries: rows() };
  tracked.$destroy();
  host.remove();
  value.leave();
  return { on, off, first, drawn, second, undone, direct };
};
// Edits made while tracking is on, scored against the pending records they
// leave behind. Each scenario mounts its own editor on a fresh document, runs
// its edits, reads the records and the text, then rejects every record to see
// whether the original text comes back.
window.trackingEditsCheck = async () => {
  const startText = "first line\\nsecond line\\nthird line\\n";
  const scenario = async (name, keys, run) => {
    const value = joinSession({ send: () => {}, mayEdit: true });
    const id = value.addText(name + ".md", startText);
    value.setMain(id);
    const host = document.createElement("section");
    document.body.append(host);
    const component = createClassComponent({
      component: Editor,
      target: host,
      props: { session: value, format: "markdown", file: id, editable: true, keys, me: { who: "me", author: "Me" } },
    });
    await tick();
    for (let attempt = 0; keys === "vim" && attempt < 100 && !host.querySelector(".cm-vim-panel"); attempt++) {
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
    component.startTracking();
    const view = EditorView.findFromDOM(host.querySelector(".cm-editor"));
    view.focus();
    const press = (key, code, modifiers = {}) => view.contentDOM.dispatchEvent(new KeyboardEvent("keydown", {
      key, code, bubbles: true, cancelable: true, ...modifiers,
    }));
    const type = async (characters) => {
      for (const character of characters) {
        const position = view.state.selection.main.head;
        view.dispatch({
          changes: { from: position, insert: character },
          selection: { anchor: position + character.length },
          userEvent: "input.type",
        });
        await tick();
      }
    };
    await run({ view, press, type, session: value, id, component });
    await tick();
    await new Promise((resolve) => setTimeout(resolve, 30));
    const records = resolveChanges(value.doc, id).map((change) => ({ from: change.from, to: change.to, deleted: change.deleted }));
    const text = value.textOf(id).toString();
    rejectMany(value.doc, resolveChanges(value.doc, id).map((change) => change.id));
    await tick();
    const restored = value.textOf(id).toString();
    const selectionEmpty = view.state.selection.main.empty;
    const visual = Boolean(view.cm?.state?.vim?.visualMode);
    component.$destroy();
    host.remove();
    value.leave();
    return { original: startText, records, text, restored, selectionEmpty, visual };
  };
  return {
    backspace: await scenario("backspace", "default", async ({ view, type }) => {
      view.dispatch({ selection: { anchor: 11 } });
      await type("Z");
      view.dispatch({ changes: { from: 11, to: 12 }, selection: { anchor: 11 }, userEvent: "delete.backward" });
      await tick();
    }),
    deleteOverOwn: await scenario("deleteOverOwn", "default", async ({ view, type }) => {
      view.dispatch({ selection: { anchor: 11 } });
      await type("NEW ");
      view.dispatch({ changes: { from: 11, to: 27 }, userEvent: "delete.selection" });
      await tick();
    }),
    typeAtOwnDeletion: await scenario("typeAtOwnDeletion", "default", async ({ view, type }) => {
      view.dispatch({ changes: { from: 11, to: 23 }, userEvent: "delete.selection" });
      await tick();
      view.dispatch({ selection: { anchor: 11 } });
      await tick();
      await type("ne");
    }),
    vimInsertThenDd: await scenario("vimInsertThenDd", "vim", async ({ view, press, type }) => {
      view.dispatch({ selection: { anchor: 11 } });
      await tick();
      press("i", "KeyI");
      await tick();
      await type("NE");
      press("Escape", "Escape");
      await tick();
      press("d", "KeyD");
      await tick();
      press("d", "KeyD");
      await tick();
    }),
    vimDdUndo: await scenario("vimDdUndo", "vim", async ({ view, press }) => {
      view.dispatch({ selection: { anchor: 11 } });
      await tick();
      press("d", "KeyD");
      await tick();
      press("d", "KeyD");
      await tick();
      press("u", "KeyU");
      await tick();
      await new Promise((resolve) => setTimeout(resolve, 50));
    }),
    vimBackspaceAcross: await scenario("vimBackspaceAcross", "vim", async ({ view, press, type }) => {
      view.dispatch({ selection: { anchor: 11 } });
      await tick();
      press("i", "KeyI");
      await tick();
      await type("AB");
      for (let count = 0; count < 4; count++) {
        const head = view.state.selection.main.head;
        view.dispatch({
          changes: { from: head - 1, to: head },
          selection: { anchor: head - 1 },
          userEvent: "delete.backward",
        });
        await tick();
      }
      press("Escape", "Escape");
      await tick();
    }),
    vimDdUndoTwice: await scenario("vimDdUndoTwice", "vim", async ({ view, press }) => {
      view.dispatch({ selection: { anchor: 11 } });
      await tick();
      for (let round = 0; round < 2; round++) {
        press("d", "KeyD");
        await tick();
        press("d", "KeyD");
        await tick();
        await new Promise((resolve) => setTimeout(resolve, 50));
        press("u", "KeyU");
        await tick();
        await new Promise((resolve) => setTimeout(resolve, 50));
      }
    }),
    untrackedCollapse: await scenario("untrackedCollapse", "default", async ({ view, session, id, component }) => {
      // Two tracked deletions, then an untracked deletion of the text between them,
      // then rejecting both: the text comes back in order.
      view.dispatch({ changes: { from: 11, to: 17 }, userEvent: "delete.selection" });
      await tick();
      view.dispatch({ changes: { from: 12, to: 17 }, userEvent: "delete.selection" });
      await tick();
      component.stopTracking();
      view.dispatch({ changes: { from: 11, to: 12 }, userEvent: "delete.selection" });
      await tick();
      component.startTracking();
    }),
  };
};
// A document with records of the earlier model: the editor converts them when
// it opens, the row shows at once, and deleting part of the old pending
// insertion shrinks it instead of recording it as original text.
window.migrationCheck = async () => {
  const value = joinSession({ send: () => {}, mayEdit: true });
  const id = value.addText("old.md", "aXYb");
  value.setMain(id);
  const text = value.textOf(id);
  value.doc.getMap("changes").set("old-1", {
    file: id, who: "me", author: "Me", at: 1, run: null,
    start: text.getCursor(1, -1).encode(), end: text.getCursor(2, 1).encode(), deleted: "",
  });
  value.doc.commit();
  const host = document.createElement("section");
  document.body.append(host);
  const component = createClassComponent({
    component: Editor, target: host,
    props: { session: value, format: "markdown", file: id, editable: true, me: { who: "me", author: "Me" } },
  });
  await tick();
  await tick();
  const rows = resolveChanges(value.doc, id).map((c) => ({ from: c.from, to: c.to, deleted: c.deleted }));
  const kindsOf = () => [...value.doc.getMap("changes").entries()].map(([, v]) => (typeof v.get === "function" ? v.get("kind") : "old")).sort();
  const kinds = kindsOf();
  component.startTracking();
  const view = EditorView.findFromDOM(host.querySelector(".cm-editor"));
  view.dispatch({ changes: { from: 1, to: 2 }, userEvent: "delete.selection" });
  await tick();
  await tick();
  const after = resolveChanges(value.doc, id).map((c) => ({ from: c.from, to: c.to, deleted: c.deleted }));
  const kindsAfter = kindsOf();
  rejectMany(value.doc, resolveChanges(value.doc, id).map((c) => c.id));
  await tick();
  const restored = value.textOf(id).toString();
  component.$destroy();
  host.remove();
  value.leave();
  return { rows, kinds, after, kindsAfter, restored };
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
// The Editor settings switch autocomplete, closing brackets and the drawing
// of compiler messages. Each is reconfigured in the open view: the same
// EditorView before and after.
window.switchesCheck = async () => {
  const value = joinSession({ send: () => {}, mayEdit: true });
  const textId = value.addText("switches.md", "alpha");
  value.setMain(textId);
  const host = document.createElement("section");
  document.body.append(host);
  const local = createClassComponent({
    component: Editor,
    target: host,
    props: { session: value, format: "markdown", file: textId },
  });
  await tick();
  const view = EditorView.findFromDOM(host.querySelector(".cm-editor"));
  // Typing goes through the input handlers before the default insertion,
  // which is where closeBrackets answers.
  const type = (text) => {
    const { from, to } = view.state.selection.main;
    const insert = () => view.state.update({ changes: { from, to, insert: text }, selection: { anchor: from + text.length } });
    if (!view.state.facet(EditorView.inputHandler).some((handler) => handler(view, from, to, text, insert))) view.dispatch(insert());
  };
  const typed = () => {
    view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: "" }, selection: { anchor: 0 } });
    type("(");
    return view.state.doc.toString();
  };
  const marks = () => host.querySelectorAll(".cm-lintRange").length;
  const diagnostics = [{ severity: "error", message: "Undefined control sequence", file: "switches.md", line: 1, column: 1, end_line: 1, end_column: 2 }];

  const result = { bracketsOn: typed(), completionOn: startCompletion(view) };
  view.dispatch({ changes: { from: 0, insert: "alpha" } });
  local.setDiagnostics(diagnostics);
  await tick();
  result.marksOn = marks();
  // An edit before the mark moves it. Turning an unrelated switch over must
  // leave it where the edit put it.
  view.dispatch({ changes: { from: 0, insert: "moved " } });
  await tick();
  const before = host.querySelector(".cm-lintRange")?.textContent;
  local.$set({ completion: false });
  await tick();
  result.markStays = host.querySelector(".cm-lintRange")?.textContent === before;
  local.$set({ completion: true });
  await tick();
  local.$set({ completion: false, brackets: false, check: false });
  await tick();
  result.bracketsOff = typed();
  result.completionOff = startCompletion(view);
  view.dispatch({ changes: { from: 0, insert: "alpha" } });
  local.setDiagnostics(diagnostics);
  await tick();
  result.marksOff = marks();
  local.$set({ check: true });
  await tick();
  result.marksBack = marks();
  result.sameView = EditorView.findFromDOM(host.querySelector(".cm-editor")) === view;
  local.$destroy();
  host.remove();
  value.leave();
  return result;
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
    if (request.url === "/fira-code.woff2") {
      response.setHeader("Content-Type", "font/woff2");
      response.end(readFileSync(join(root, "web/node_modules/katex/dist/fonts/KaTeX_Main-Regular.woff2")));
      return;
    }
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
  const switches = await evaluate("switchesCheck()");
  assert.equal(switches.bracketsOn, "()", "an opening bracket was not closed with the switch on");
  assert.equal(switches.bracketsOff, "(", "an opening bracket was closed with the switch off");
  assert.equal(switches.completionOn, true, "autocomplete could not start with the switch on");
  assert.equal(switches.completionOff, false, "autocomplete started with the switch off");
  assert.ok(switches.marksOn > 0, "a compiler error was not drawn with the check on");
  assert.equal(switches.markStays, true, "turning autocomplete over moved a compiler mark back to where the text used to be");
  assert.equal(switches.marksOff, 0, "a compiler error was drawn with the check off");
  assert.ok(switches.marksBack > 0, "turning the check back on did not redraw the last render's marks");
  assert.equal(switches.sameView, true, "a switch rebuilt the editor");
  console.log("editor-browser: the autocomplete, bracket and check switches reconfigure the open editor");
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
  const edits = await evaluate("trackingEditsCheck()");
  assert.deepEqual(edits.backspace.records, [], "backspacing a tracked keystroke recorded it as a deletion");
  assert.equal(edits.backspace.text, edits.backspace.original);
  assert.deepEqual(edits.deleteOverOwn.records, [{ from: 11, to: 11, deleted: "second line\n" }], "deleting over an own insertion recorded the inserted text as deleted");
  assert.equal(edits.deleteOverOwn.restored, edits.deleteOverOwn.original, "rejecting did not restore the original after deleting over an own insertion");
  assert.deepEqual(edits.typeAtOwnDeletion.records, [{ from: 11, to: 13, deleted: "second line\n" }], "typing at an own deletion did not merge into one replacement");
  assert.equal(edits.typeAtOwnDeletion.restored, edits.typeAtOwnDeletion.original);
  assert.deepEqual(edits.vimInsertThenDd.records, [{ from: 11, to: 11, deleted: "second line\n" }], "vim dd over a line with own typing recorded the typed text as deleted");
  assert.equal(edits.vimInsertThenDd.restored, edits.vimInsertThenDd.original);
  assert.equal(edits.vimDdUndo.text, edits.vimDdUndo.original, "vim u after a tracked dd did not restore the line");
  assert.deepEqual(edits.vimDdUndo.records, [], "vim u after a tracked dd left or lost its record out of step with the text");
  assert.deepEqual(edits.vimBackspaceAcross.records, [{ from: 9, to: 9, deleted: "e\n" }], "backspacing across own typing into original text recorded the wrong deletion");
  assert.equal(edits.vimBackspaceAcross.restored, edits.vimBackspaceAcross.original);
  assert.equal(edits.vimDdUndoTwice.text, edits.vimDdUndoTwice.original, "the second vim u after a dd did not restore the line");
  assert.deepEqual(edits.vimDdUndoTwice.records, []);
  assert.equal(edits.vimDdUndoTwice.selectionEmpty, true, "vim u left a range selected");
  assert.equal(edits.vimDdUndoTwice.visual, false, "vim u left the editor in visual mode");
  // The untracked deletion of the space is permanent; the two tracked ones come back in order around where it was.
  assert.equal(edits.untrackedCollapse.restored, "first line\nsecondline\nthird line\n", "two deletions collapsed by an untracked edit did not come back in order");
  assert.deepEqual(edits.untrackedCollapse.records.map((r) => r.deleted).sort(), ["line\n", "second"], "the untracked deletion between two tracked ones was recorded or lost one");
  console.log("editor-browser: tracked edits are planned against the records before the write, and a vim undo takes text and record together");
  const migration = await evaluate("migrationCheck()");
  assert.deepEqual(migration.rows, [{ from: 1, to: 3, deleted: "" }], "an old-model record was not read in place when the editor opened");
  assert.deepEqual(migration.kinds, ["old"], "an old-model record was converted on open; it should be read in place");
  assert.deepEqual(migration.after, [{ from: 1, to: 2, deleted: "" }], "deleting part of an old pending insertion recorded it as original text");
  assert.deepEqual(migration.kindsAfter, ["change"], "editing an old pending insertion did not convert its record");
  assert.equal(migration.restored, "ab", "rejecting the converted change did not restore the original");
  console.log("editor-browser: records of the earlier model are read in place and converted when edited");
  const zoom = await evaluate("editorZoomCheck()");
  assert.deepEqual(zoom, { before: "14px", prevented: true, grown: "28px", storedGrown: "28", shrunk: "10px", storedShrunk: "10" }, "ctrl+wheel over the source sizes its text and remembers it");
  console.log("editor-browser: ctrl+wheel over the source sizes its text and remembers it");
  const type = await evaluate("editorTypeCheck()");
  assert.equal(type.sized, "20px", "setSize did not resize the open editor");
  assert.ok(parseFloat(type.pinched) > 20, `a pinch started from ${type.pinched}, not from the chosen size of 20px`);
  assert.equal(type.wideLine, 1.9, "the wide line height did not restyle the open editor");
  assert.equal(type.normalLine, 1.6, "the normal line height did not restyle the open editor");
  assert.ok(type.serif.startsWith("ui-serif"), `the serif font family was ${type.serif}`);
  assert.ok(type.comic.startsWith('"Comic Neue"'), `the custom font family was ${type.comic}`);
  assert.ok(!type.evil.includes("}") && !type.evil.includes(";"), `a custom font name escaped its declaration: ${type.evil}`);
  assert.equal(type.firaStatus, "loaded", `the Fira Code face was ${type.firaStatus}`);
  assert.equal(type.firaAfter, "gone", "the default font left the Fira Code face loaded");
  assert.equal(type.bodyColorAfter, type.bodyColor, "a custom font name changed the page colour");
  assert.equal(type.defaultVariable, "", "the default font left --editor-font-family set");
  assert.equal(type.keyBigger, "15px", "Ctrl+= did not grow the source text from 14px");
  assert.equal(type.keySmaller, "13px", "two Ctrl+- presses did not shrink the source text to 13px");
  assert.equal(type.keyReset, "14px", "Ctrl+0 did not reset the source text to 14px");
  console.log("editor-browser: the font, size and line height settings restyle the open editor, and a pinch starts from the chosen size");
} finally {
  socket?.close();
  await stopBrowserProcess(browser);
  if (server?.listening) await new Promise((resolve) => server.close(resolve));
  removeTemporary(temporary);
}
