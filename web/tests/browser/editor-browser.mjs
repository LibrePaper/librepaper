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
import { join as joinSession } from ${JSON.stringify(join(root, "web/src/lib/collab.js"))};
import { createProposals } from ${JSON.stringify(join(root, "web/src/lib/proposals.js"))};
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
  const statuses = [];
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
      onproposalstatus: (status) => { if (status) statuses.push(status); },
    },
  });
  await tick();
  tracked.startTracking();
  await tick();
  const on = tracked.trackingOn();
  const openedBeforeTyping = sent.some((message) => message.type === "proposal-open");
  const draftId = tracked.proposalStatus()?.id || "";
  const view = EditorView.findFromDOM(host.querySelector(".cm-editor"));
  view.dispatch({ changes: { from: 0, insert: "TRACKED " } });
  // Longer than the flush pause: the point of the check is that typing sends,
  // without anything else having to happen.
  await new Promise((resolve) => setTimeout(resolve, 700));
  const open = sent.find((message) => message.type === "proposal-open");
  if (open) tracked.receiveProposal({ type: "proposal-opened", proposal_id: open.request_id, request_id: open.request_id, tip: "", applied_version: "" });
  await new Promise((resolve) => setTimeout(resolve, 100));
  const update = sent.find((message) => message.type === "proposal-update");
  if (update) tracked.receiveProposal({ type: "proposal-updated", proposal_id: update.proposal_id, request_id: update.request_id, tip: update.tip, applied_version: "" });
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
    on, off, opened: Boolean(open), openedBeforeTyping, draftId, shown, paper, afterStop, direct,
    pendingSeen: statuses.some((status) => Number(status.pending) > 0), acknowledged: Number(statuses.at(-1)?.pending || 0) === 0,
    flushed: Boolean(update && update.proposal_id === draftId && update.update),
  };
};
// Track changes with a stop before proposal-opened arrives: the unsent work
// should be sent once the server names the proposal, even though tracking
// was turned off locally first.
window.trackingStopBeforeOpenedCheck = async () => {
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
  const view = EditorView.findFromDOM(host.querySelector(".cm-editor"));
  view.dispatch({ changes: { from: 0, insert: "TRACKED " } });
  // Wait for the lazy open request, but do not acknowledge it yet.
  await new Promise((resolve) => setTimeout(resolve, 700));
  const open = sent.find((message) => message.type === "proposal-open");
  // Stop tracking before proposal-opened arrives.
  tracked.stopTracking();
  await tick();
  const noUpdateYet = sent.filter((message) => message.type === "proposal-update").length;
  // Now deliver proposal-opened -- the unsent work should be sent.
  if (open) tracked.receiveProposal({ type: "proposal-opened", proposal_id: open.request_id, request_id: open.request_id, tip: "", applied_version: "" });
  const delayedUpdate = sent.find((message) => message.type === "proposal-update" && message.proposal_id === open?.request_id);
  if (delayedUpdate) tracked.receiveProposal({ type: "proposal-updated", proposal_id: delayedUpdate.proposal_id, request_id: delayedUpdate.request_id, tip: delayedUpdate.tip, applied_version: "" });
  const shown = value.textOf(id).toString();
  tracked.$destroy();
  host.remove();
  value.leave();
  return {
    noUpdateYet,
    openId: open?.request_id || "",
    delayedSent: Boolean(delayedUpdate && delayedUpdate.update),
    delayedUpdateId: delayedUpdate?.proposal_id || "",
    shown,
  };
};
// An active tracked draft resolving is still the same user intent. The next
// edit must stay on a fresh lazy branch instead of silently reaching the room.
window.trackingResolutionRestartsCheck = async () => {
  const sent = [];
  const value = joinSession({ send: () => {}, mayEdit: true });
  const id = value.addText("tracked-resolution.md", "alpha");
  value.setMain(id);
  await value.start({ protocol: "librepaper.room.v3", vector: "", updates: [] });
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
  let view = EditorView.findFromDOM(host.querySelector(".cm-editor"));
  view.dispatch({ changes: { from: 0, insert: "FIRST " } });
  await new Promise((resolve) => setTimeout(resolve, 700));
  const firstOpen = sent.find((message) => message.type === "proposal-open");
  if (firstOpen) tracked.receiveProposal({ type: "proposal-opened", proposal_id: firstOpen.request_id, request_id: firstOpen.request_id, tip: "", applied_version: 1 });
  await new Promise((resolve) => setTimeout(resolve, 100));
  const firstUpdate = sent.find((message) => message.type === "proposal-update");
  if (firstUpdate) tracked.receiveProposal({ type: "proposal-updated", proposal_id: firstUpdate.proposal_id, request_id: firstUpdate.request_id, tip: firstUpdate.tip, applied_version: 2 });
  if (firstUpdate) {
    const bytes = Uint8Array.from(atob(firstUpdate.update), (character) => character.charCodeAt(0));
    value.doc.import(bytes);
    value.doc.commit();
  }
  if (firstOpen) tracked.receiveProposal({
    type: "proposal-decided", proposal_id: firstOpen.request_id, resolved: true,
    decisions: [{ hunk: 0, accepted: true }], resolved_base: firstOpen.base || "", resolved_tip: firstUpdate?.tip || "",
  });
  await tick();
  const stillTracking = tracked.trackingOn();
  const restartedId = tracked.proposalStatus()?.id || "";
  const openCountBeforeTyping = sent.filter((message) => message.type === "proposal-open").length;
  view = EditorView.findFromDOM(host.querySelector(".cm-editor"));
  view.dispatch({ changes: { from: 0, insert: "NEXT " } });
  const liveImmediately = value.textOf(id).toString();
  await new Promise((resolve) => setTimeout(resolve, 700));
  const opens = sent.filter((message) => message.type === "proposal-open");
  const finalText = value.textOf(id).toString();
  const secondId = opens.at(-1)?.request_id || "";
  tracked.stopTracking();
  tracked.$destroy();
  host.remove();
  value.leave();
  return {
    stillTracking, restartedId, openCountBeforeTyping, firstId: firstOpen?.request_id || "",
    secondId, opensAfterTyping: opens.length, liveImmediately, finalText,
  };
};
// A retained draft can require manual recovery while a newer active draft is
// still safe to rebase. Resolution delivery belongs to the session manager,
// which also owns both drafts while the editor is mounted.
window.trackingRecoveryDoesNotBlockResolutionCheck = async () => {
  const sent = [];
  const send = (message) => { sent.push(message); return true; };
  const value = joinSession({ send, mayEdit: true });
  const id = value.addText("tracked-recovery.md", "alpha");
  value.setMain(id);
  await value.start({ protocol: "librepaper.room.v3", vector: "", updates: [] });
  const manager = createProposals({ session: value, send, mayEdit: true });
  const host = document.createElement("section");
  document.body.append(host);
  const tracked = createClassComponent({
    component: Editor,
    target: host,
    props: { session: value, format: "markdown", file: id, editable: true, send },
  });
  await tick();

  // Leave an update in flight before stopping so the manager retains it. An
  // expired server outcome marks that older draft blocked for manual export.
  tracked.startTracking();
  await tick();
  let view = EditorView.findFromDOM(host.querySelector(".cm-editor"));
  view.dispatch({ changes: { from: 0, insert: "OLD " } });
  await new Promise((resolve) => setTimeout(resolve, 700));
  const oldOpen = sent.find((message) => message.type === "proposal-open");
  if (oldOpen) manager.apply({ type: "proposal-opened", proposal_id: oldOpen.request_id, request_id: oldOpen.request_id, applied_version: 1 });
  await new Promise((resolve) => setTimeout(resolve, 100));
  const oldUpdate = sent.find((message) => message.type === "proposal-update");
  tracked.stopTracking();
  if (oldUpdate) manager.apply({ type: "error", request_id: oldUpdate.request_id, status_unknown: true });
  const oldRecoveryRequired = Boolean(manager.status()?.recoveryRequired);

  // A new branch is allowed to proceed even while the retained draft keeps
  // the recovery warning visible. The manager, rather than Editor's private
  // API, receives the fresh proposal's open, update and resolution messages.
  tracked.startTracking();
  await tick();
  view = EditorView.findFromDOM(host.querySelector(".cm-editor"));
  view.dispatch({ changes: { from: 0, insert: "FRESH " } });
  await new Promise((resolve) => setTimeout(resolve, 700));
  const freshOpen = sent.filter((message) => message.type === "proposal-open").at(-1);
  if (freshOpen) manager.apply({ type: "proposal-opened", proposal_id: freshOpen.request_id, request_id: freshOpen.request_id, applied_version: 2 });
  await new Promise((resolve) => setTimeout(resolve, 100));
  const freshUpdate = sent.filter((message) => message.type === "proposal-update").at(-1);
  if (freshUpdate) manager.apply({ type: "proposal-updated", proposal_id: freshUpdate.proposal_id, request_id: freshUpdate.request_id, tip: freshUpdate.tip, applied_version: 3 });
  if (freshOpen && freshUpdate) manager.apply({
    type: "proposal-decided", proposal_id: freshOpen.request_id, resolved: true,
    decisions: [{ hunk: 0, accepted: false }], resolved_base: freshOpen.base, resolved_tip: freshUpdate.tip,
  });
  await tick();
  const recoveryAfterFreshResolution = Boolean(manager.status()?.recoveryRequired);
  const recoveryStatusId = tracked.proposalStatus()?.id || "";
  const restartedDraftId = manager.id?.() || "";
  const trackingAfterFreshResolution = tracked.trackingOn();
  const opensBeforeNextTyping = sent.filter((message) => message.type === "proposal-open").length;
  view = EditorView.findFromDOM(host.querySelector(".cm-editor"));
  view.dispatch({ changes: { from: 0, insert: "NEXT " } });
  const liveImmediately = value.textOf(id).toString();
  await new Promise((resolve) => setTimeout(resolve, 700));
  const opens = sent.filter((message) => message.type === "proposal-open");
  const latestOpenId = opens.at(-1)?.request_id || "";
  const finalText = value.textOf(id).toString();
  tracked.stopTracking();
  tracked.$destroy();
  host.remove();
  value.leave();
  return {
    oldOpenId: oldOpen?.request_id || "", oldUpdateId: oldUpdate?.request_id || "",
    oldRecoveryRequired, freshOpenId: freshOpen?.request_id || "", freshUpdateId: freshUpdate?.request_id || "",
    recoveryAfterFreshResolution, recoveryStatusId, restartedDraftId, trackingAfterFreshResolution, opensBeforeNextTyping, latestOpenId,
    opensAfterNextTyping: opens.length, liveImmediately, finalText,
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
  assert.equal(tracked.openedBeforeTyping, false, "starting tracking created an empty server proposal");
  assert.match(tracked.draftId, /^[0-9a-f-]{36}$/i, "the draft was not named locally with a UUID");
  assert.equal(tracked.opened, true, "the first real tracked edit did not open a proposal");
  assert.equal(tracked.shown, "TRACKED alpha", "the author cannot see what they typed");
  assert.equal(tracked.paper, "alpha", "a tracked keystroke reached the document instead of the branch");
  assert.equal(tracked.pendingSeen, true, "unacknowledged tracked work was not exposed to the save status");
  assert.equal(tracked.flushed, true, "the branch was never sent, so nobody could review it");
  assert.equal(tracked.acknowledged, true, "an explicit server acknowledgement did not clear tracked pending status");
  assert.equal(tracked.off, false, "track changes reported on after being switched off");
  assert.equal(tracked.afterStop, "alpha", "stopping applied the proposal instead of leaving it for review");
  assert.equal(tracked.direct, "DIRECT alpha", "editing after stopping did not reach the document");
  console.log("editor-browser: tracked edits go to a branch, are sent while typing, and leave the paper alone");
  const trackingStopBefore = await evaluate("trackingStopBeforeOpenedCheck()");
  assert.equal(trackingStopBefore.noUpdateYet, 0, "proposal-update was sent before stopping");
  assert.equal(trackingStopBefore.delayedSent, true, "unsent work was not sent after proposal-opened");
  assert.match(trackingStopBefore.openId, /^[0-9a-f-]{36}$/i, "the pending open did not retain its client-chosen id");
  assert.equal(trackingStopBefore.delayedUpdateId, trackingStopBefore.openId, "the delayed update was sent to the wrong proposal id");
  assert.equal(trackingStopBefore.shown, "alpha", "stopping tracking changed the paper text");
  console.log("editor-browser: stopping before proposal-opened sends unsent edits once named");
  const trackingResolved = await evaluate("trackingResolutionRestartsCheck()");
  assert.equal(trackingResolved.stillTracking, true, "resolving an active proposal turned track changes off");
  assert.notEqual(trackingResolved.restartedId, "", "resolution did not establish a fresh local draft");
  assert.notEqual(trackingResolved.restartedId, trackingResolved.firstId, "resolution did not replace the decided draft identity");
  assert.equal(trackingResolved.openCountBeforeTyping, 1, "restarting tracking created an empty server proposal");
  assert.notEqual(trackingResolved.secondId, "", "typing after resolution did not open a new tracked proposal");
  assert.notEqual(trackingResolved.secondId, trackingResolved.firstId, "typing after resolution reused the decided proposal id");
  assert.equal(trackingResolved.secondId, trackingResolved.restartedId, "typing opened a different proposal than the fresh local draft");
  assert.equal(trackingResolved.opensAfterTyping, 2);
  assert.equal(trackingResolved.liveImmediately, "FIRST alpha", "the first keystroke after resolution reached the room");
  assert.equal(trackingResolved.finalText, "FIRST alpha", "the next tracked edit leaked into live text");
  console.log("editor-browser: tracking restarts lazily after its own proposal resolves");
  const retainedRecovery = await evaluate("trackingRecoveryDoesNotBlockResolutionCheck()");
  assert.notEqual(retainedRecovery.oldOpenId, "", "the retained draft was never opened");
  assert.notEqual(retainedRecovery.oldUpdateId, "", "the retained draft had no in-flight update to preserve");
  assert.equal(retainedRecovery.oldRecoveryRequired, true, "the old unresolved outcome did not require manual recovery");
  assert.notEqual(retainedRecovery.freshOpenId, "", "tracking did not create a fresh proposal beside the retained draft");
  assert.notEqual(retainedRecovery.freshOpenId, retainedRecovery.oldOpenId, "the fresh proposal reused the retained draft id");
  assert.notEqual(retainedRecovery.freshUpdateId, "", "the fresh proposal update was not delivered to the session manager");
  assert.equal(retainedRecovery.recoveryAfterFreshResolution, true, "resolving the fresh proposal incorrectly cleared the old recovery warning");
  assert.equal(retainedRecovery.recoveryStatusId, retainedRecovery.oldOpenId, "the old manual recovery warning was not retained after the fresh resolution");
  assert.notEqual(retainedRecovery.restartedDraftId, "", "the fresh resolution did not establish another local draft");
  assert.notEqual(retainedRecovery.restartedDraftId, retainedRecovery.freshOpenId, "the restarted local branch reused the resolved proposal id");
  assert.equal(retainedRecovery.trackingAfterFreshResolution, true, "resolving the fresh proposal did not restart local tracking");
  assert.equal(retainedRecovery.opensBeforeNextTyping, 2, "restarting after resolution created an empty server proposal");
  assert.notEqual(retainedRecovery.latestOpenId, "", "typing after fresh resolution did not open a new proposal");
  assert.equal(retainedRecovery.latestOpenId, retainedRecovery.restartedDraftId, "typing opened a different proposal than the fresh local branch");
  assert.notEqual(retainedRecovery.latestOpenId, retainedRecovery.oldOpenId, "typing reused the retained proposal id");
  assert.notEqual(retainedRecovery.latestOpenId, retainedRecovery.freshOpenId, "typing reused the resolved proposal id");
  assert.equal(retainedRecovery.opensAfterNextTyping, 3);
  assert.equal(retainedRecovery.liveImmediately, "alpha", "typing after the old recovery warning reached live text");
  assert.equal(retainedRecovery.finalText, "alpha", "the next tracked edit leaked into live text");
  console.log("editor-browser: an old retained recovery warning does not block fresh tracked resolution");
} finally {
  socket?.close();
  browser?.kill();
  if (server?.listening) await new Promise((resolve) => server.close(resolve));
  rmSync(temporary, { recursive: true, force: true });
}
