<script module>
  // Vim is switched on for every file at once, held in one compartment per
  // view rather than baked into a file's state, so flipping the setting
  // reconfigures the live editor without dropping the caret, the scroll
  // position or the undo history.
  import { Compartment, Prec as vimPrec } from "@codemirror/state";
  import { ViewPlugin } from "@codemirror/view";
  import { undo as undoFn, redo as redoFn, undoManagerField as vimUndoManagerField } from "../lib/loro-undo.js";
  import { loroSyncAnnotation } from "../../vendor/loro-codemirror/sync.ts";
  import { keymap } from "@codemirror/view";
  import { mode, onMode } from "../lib/theme.js";

  // The keys the editor answers to: Vim's, Emacs's, or nothing extra. One
  // compartment, whichever is chosen, so switching swaps one for the other.
  const vimCompartment = new Compartment();
  // CodeMirror's own light or dark base styles; the colours come from the
  // page's variables. One theme of each, made once: every EditorView.theme()
  // call mints a new class and style sheet, and a state is told which of the
  // two it holds by that class.
  const themeCompartment = new Compartment();
  const darkTheme = EditorView.theme({}, { dark: true });
  const lightTheme = EditorView.theme({}, { dark: false });
  const themeFor = (dark) => (dark ? darkTheme : lightTheme);
  // The compartment lives in each state, so a cached state keeps the theme it
  // was last shown with. It is set again whenever a state is activated.
  function syncTheme(target) {
    target?.dispatch({ effects: themeCompartment.reconfigure(themeFor(mode() === "dark")) });
  }

  // Each switch in the Editor settings is a compartment of its own, so turning
  // one over changes open editors in place. Like the theme, a cached state is
  // brought into line with the setting when it is activated.
  const completionCompartment = new Compartment();
  const bracketsCompartment = new Compartment();
  const checkCompartment = new Compartment();
  // The bracket keymap goes with auto-closing brackets. Its Backspace deletes an
  // empty pair even without closeBrackets, so leaving it in the main keymap
  // would leave the switch half off.
  const completionOn = (on) => (on ? autocompletion({ activateOnTyping: true }) : []);
  const bracketsOn = (on) => (on ? [closeBrackets(), keymap.of(closeBracketsKeymap)] : []);
  const checkOn = (on) => (on ? lintGutter() : []);

  // Loro merges nearby commits by default. A handled Vim key starts a new
  // command boundary; the first local edit after it consumes that boundary
  // only after the Loro sync plugin has committed the edit.
  // Plain insert-mode keystrokes fall through to CodeMirror, so they keep
  // grouping under Loro's normal one-second interval.
  const vimUndoBoundaries = vimPrec.lowest(ViewPlugin.fromClass(class {
    constructor(view) {
      this.view = view;
      this.manager = view.state.field(vimUndoManagerField, false);
      this.cm = view.cm;
      this.pendingBoundary = true;
      this.manager?.setMergeInterval(0);
      this.onKeypress = () => {
        this.adoptManager();
        this.pendingBoundary = true;
        this.manager?.setMergeInterval(0);
      };
      this.cm?.on("vim-keypress", this.onKeypress);
    }

    adoptManager() {
      const manager = this.view.state.field(vimUndoManagerField, false);
      if (manager === this.manager) return;
      this.manager?.setMergeInterval(1000);
      this.manager = manager;
      this.pendingBoundary = true;
      this.manager?.setMergeInterval(0);
    }

    update(update) {
      this.adoptManager();
      if (!this.pendingBoundary || !this.manager) return;
      const localEdit = update.transactions.some((transaction) =>
        transaction.docChanged && transaction.annotation(loroSyncAnnotation) === undefined
      );
      if (localEdit) {
        this.manager.setMergeInterval(1000);
        this.pendingBoundary = false;
      }
    }

    destroy() {
      this.cm?.off("vim-keypress", this.onKeypress);
      this.manager?.setMergeInterval(1000);
    }
  }));

  // Each package is fetched only once a browser asks for its keys, and every
  // editor on the page shares the one download and the one module. Once it
  // has resolved, `resolvedVim` (or `resolvedEmacs`) lets a later switch
  // reconfigure a compartment right away, with nothing to await.
  let vimPromise = null;
  let resolvedVim = null;
  let emacsPromise = null;
  let resolvedEmacs = null;
  function loadVim() {
    if (!vimPromise) {
      vimPromise = import("@replit/codemirror-vim").then(({ Vim, vim }) => {
        defineExCommands(Vim);
        resolvedVim = [vim({ status: true }), vimUndoBoundaries];
        return resolvedVim;
      });
    }
    return vimPromise;
  }

  // Emacs undoes with C-/ (and C-_ and C-x u), which route through the Loro
  // undo manager so an undo stays this collaborator's, as Vim's `u` is above.
  function loadEmacs() {
    if (!emacsPromise) {
      emacsPromise = import("@replit/codemirror-emacs").then(({ emacs }) => {
        resolvedEmacs = [keymap.of([
          { key: "Ctrl-/", run: undoFn, preventDefault: true },
          { key: "Ctrl-_", run: undoFn, preventDefault: true },
          { key: "Ctrl-x u", run: undoFn, preventDefault: true }
        ]), emacs()];
        return resolvedEmacs;
      });
    }
    return emacsPromise;
  }

  // The extension for a choice of keys, when it has already been fetched.
  function resolvedKeys(want) {
    return want === "vim" ? resolvedVim : want === "emacs" ? resolvedEmacs : [];
  }

  // `Vim` is a module-wide singleton: defining these twice would be
  // redefining them, not adding a second copy, so the guard is what keeps a
  // second mounted editor -- or a hot reload -- from doing that.
  let exCommandsDefined = false;
  // The Ex commands close over no component: the view they run against is
  // whatever `cm.cm6` names, looked up here to find which editor asked.
  const viewCallbacks = new WeakMap();

  // Vim's `u` leaves one cursor at the start of what came back, in normal
  // mode. The undo plugin puts back the selection the change was made with,
  // in a timeout of its own, and Vim's `dd` made that selection a line; a
  // range put back from outside reads to Vim as visual mode, and the next
  // keys go wrong. So once that selection has landed it is collapsed, which
  // also takes Vim out of visual mode.
  function settleAfterHistory(view) {
    setTimeout(() => {
      const range = view.state.selection.main;
      if (!range.empty) view.dispatch({ selection: { anchor: range.from } });
    }, 0);
  }

  function defineExCommands(Vim) {
    if (exCommandsDefined) return;
    exCommandsDefined = true;
    const save = (cm) => viewCallbacks.get(cm.cm6)?.onsave?.();
    const quit = (cm) => viewCallbacks.get(cm.cm6)?.onquit?.();
    // `:w` reports whether this browser's work has reached the server; there
    // is no save to perform, so the status is the whole of the answer.
    Vim.defineEx("write", "w", save);
    // `:q` closes the source pane, showing the document alone.
    Vim.defineEx("quit", "q", quit);
    Vim.defineEx("wq", "wq", (cm) => {
      save(cm);
      quit(cm);
    });
    Vim.defineEx("xit", "x", (cm) => {
      save(cm);
      quit(cm);
    });
    // Vim's built-in `u` and Ctrl-R route through their respective handlers.
    Vim.defineAction("loroUndo", (cm) => {
      const done = undoFn(cm.cm6);
      if (done) settleAfterHistory(cm.cm6);
      return done;
    });
    Vim.defineAction("loroRedo", (cm) => {
      const done = redoFn(cm.cm6);
      if (done) settleAfterHistory(cm.cm6);
      return done;
    });
    Vim.mapCommand("u", "action", "loroUndo", {}, {});
    Vim.mapCommand("<C-r>", "action", "loroRedo", {}, {});
  }
</script>

<script>
  // The source, in CodeMirror, shared with everyone else editing it.
  //
  // The binding is loro-codemirror: multiple editors across files share the
  // same LoroDoc, and each file has a LoroText within it. Two people typing
  // in the same sentence converge without either waiting for the other, and
  // each of them keeps their own caret, selection and undo history.
  import { EditorState, Prec, Transaction } from "@codemirror/state";
  import { EditorView, lineNumbers, highlightActiveLine, highlightActiveLineGutter, highlightSpecialChars, dropCursor, rectangularSelection, drawSelection } from "@codemirror/view";
  import { defaultKeymap, indentWithTab, selectAll } from "@codemirror/commands";
  import { autocompletion, closeBrackets, closeBracketsKeymap, completionKeymap, snippet, startCompletion } from "@codemirror/autocomplete";
  import { searchKeymap, highlightSelectionMatches, openSearchPanel } from "@codemirror/search";
  import { bracketMatching, foldGutter, foldKeymap, indentOnInput } from "@codemirror/language";
  import {
    lintGutter,
    lintKeymap,
    setDiagnostics as setLintDiagnostics,
    openLintPanel,
  } from "@codemirror/lint";
  import { LoroExtensions } from "../../vendor/loro-codemirror/index.ts";
  import { undoManagerField, undo as undoCommand, redo as redoCommand } from "../lib/loro-undo.js";
  import { LoroText, UndoManager } from "loro-crdt";

  import { sourceLanguage } from "../lib/source-language.js";
  import { emphasisCommand } from "../lib/emphasis.js";
  import { sourceHighlighting } from "../lib/source-highlight.js";
  import { analyzeBibliography } from "../lib/bibliography-engine.js";
  import { bibliographyCache, bibliographyCacheKey, bibliographyCompletion, bibliographyNeedsAnalysis, citationContext, planZoteroImport } from "../lib/bibliography.js";
  import { completionExtension, formatOfPath } from "../lib/completion/index.js";
  import { desktop, searchZotero, zoteroItem } from "../lib/desktop/client.js";
  import { changeMarks, setChangeMarks } from "../lib/change-marks.js";
  import { CHANGES, planEdits, writePlanned, resolveFile, trackedSplice, untrackedSplice, prune } from "../lib/track-changes.js";
  import { DIRECTORY_ORIGIN } from "../lib/project-session.js";
  import { ANCHORS_ORIGIN, composeEdits, fragmentRangesFor } from "../lib/anchors.js";
  import { projectDirectory } from "../lib/projection.js";
  import { attachEditorZoom } from "../lib/editor-zoom.js";
  import { snippetTemplate } from "../lib/snippet-template.js";
  import { formatOf as documentFormat } from "../lib/formats.js";
  import { untrack } from "svelte";

  // Whether what is typed here is recorded as a tracked change is not a prop
  // to follow. It starts from `tracking` and is flipped by `startTracking` and
  // `stopTracking`; the plugin below reads it through a function, so flipping
  // it does not rebuild any state. Pending changes are drawn either way.
  //
  // `me` is who the tracked edits belong to, `{who, author}`. `selectedChange`
  // is the id of the change the reviewer is looking at.
  /** @type {{ me?: { who: string, author: string }, [key: string]: any }} */
  let { session, format = "", file = "", keys = "default", editable = true,
    completion = true, brackets = true, check = true,
    tracking = false, me = { who: "", author: "" }, selectedChange = null,
    analyze = analyzeBibliography,
    onchange, oncaret, onfilechange, onbibliography, onsave, onquit,
    rules = null } = $props();

  let tracked = $state(untrack(() => tracking));

  // One field instance for the component, because it is one editor: a state
  // field is identified by the object, so building a fresh one per file would
  // make each file's state carry a field the view's other states do not have.
  const pendingChanges = changeMarks();

  /// Redraw the pending changes over whichever file is on screen.
  ///
  /// A state carries no decorations until something puts them there, so this
  /// runs when a file is shown, after every document change (local or
  /// imported), and when the selection or the viewer changes.
  function drawChanges() {
    if (!view || !showing || !session) return;
    view.dispatch({
      effects: setChangeMarks.of({
        ...resolveFile(session.doc, showing),
        me,
        selected: selectedChange,
      }),
    });
  }

  // One redraw per tick, however many commits and transactions asked for it.
  let drawQueued = false;
  function scheduleDraw() {
    if (drawQueued) return;
    drawQueued = true;
    queueMicrotask(() => {
      drawQueued = false;
      drawChanges();
    });
  }

  /// Takes each local, user transaction as a tracked change while tracking is
  /// on. The sync binding sits between the two plugins returned here, and
  /// neither of them writes text.
  ///
  /// Loro delivers subscription events synchronously inside `doc.commit()`.
  /// A list cached from those events would already be re-resolved against the
  /// edited text by the time a plugin behind the binding runs, so the records
  /// are read from the document at the point they are needed.
  ///
  /// `before` runs ahead of the binding and reads the records while the text
  /// is still as it was. `after` runs behind the binding. It plans against
  /// that snapshot, in the coordinates of each transaction's start state,
  /// writes the records, prunes and commits.
  ///
  /// `after` has the lowest precedence. Vim's undo boundary plugin also has
  /// the lowest precedence, and it sits earlier in the extensions. It restores
  /// Loro's merge interval right after the binding commits the first edit of
  /// a Vim command. The records committed here then join that edit's undo step
  /// instead of making their own.
  ///
  /// A transaction carrying the binding's annotation is its own write or an
  /// undo, and is already in the document.
  ///
  /// Every local edit, tracked or not, also prunes: an untracked edit can
  /// delete the character a pending change's cursor is attached to, and the
  /// prune rewrites that cursor against live text while the history that
  /// locates it is still there.
  ///
  /// While tracking is off, an edit is still planned, untracked. It records
  /// nothing and changes no text, but pending spans shrink, move or split
  /// around it, and the gaps it collapses merge in order. With no records in
  /// the document, nothing is planned.
  function trackChanges(fileId) {
    /** @type {{changes: any[], gaps: any[]}} */
    let snapshot = { changes: [], gaps: [] };
    // The fragments that the last local update's deletions created, in document
    // order. Kept for the anchors plugin, which runs behind this one.
    let created = [];
    // A local edit is a user transaction that changed the text.
    const isLocal = (update) => update.docChanged
      && update.transactions.some((transaction) => transaction.docChanged && transaction.annotation(loroSyncAnnotation) === undefined);
    const before = ViewPlugin.define(() => {
      const doc = session.doc;
      return {
        update(update) {
          if (isLocal(update) && doc.getMap(CHANGES).size) snapshot = resolveFile(doc, fileId);
        },
      };
    });
    const after = Prec.lowest(ViewPlugin.define(() => {
      const doc = session.doc;
      return {
        update(update) {
          if (!isLocal(update)) return;
          created = [];
          // The sync plugin has written every transaction of this update to
          // the LoroText already. Each one is planned against the text it
          // started from, chained, and the result is written once so that no
          // cursor is made at an intermediate offset.
          let state = snapshot;
          const touched = new Set();
          const now = Date.now();
          if (tracked || snapshot.changes.length || snapshot.gaps.length) {
            for (const transaction of update.transactions) {
              if (!transaction.docChanged || transaction.annotation(loroSyncAnnotation) !== undefined) continue;
              const edits = [];
              transaction.changes.iterChanges((fromA, toA, fromB, toB, insert) => {
                edits.push({ from: fromA, to: toA, insert: insert.sliceString(0, insert.length, "\n") });
              });
              /** @type {any} */
              const plan = planEdits(transaction.startState.doc.toString(), state, edits, me, {
                run: null,
                untracked: !tracked,
                now,
                mintId: () => crypto.randomUUID(),
              });
              for (const item of [...plan.changes, ...plan.gaps]) if (item.state !== "kept") touched.add(item.id);
              state = {
                changes: plan.changes.filter((change) => change.state !== "removed"),
                gaps: plan.gaps.filter((gap) => gap.state !== "removed"),
              };
            }
            if (touched.size) {
              // Everything the batch touched is written once, against the final
              // text; the rest keeps its cursors.
              // Gaps are named by position, which moves between steps, so every
              // gap of the final state is handed over as set: a fragment whose
              // cursor and rank already hold is not written again.
              const keptChanges = new Set(state.changes.map((change) => change.id));
              const planned = {
                changes: [
                  ...state.changes.map((change) => ({ ...change, state: touched.has(change.id) ? "set" : "kept" })),
                  ...snapshot.changes.filter((change) => !keptChanges.has(change.id)).map((change) => ({ ...change, state: "removed" })),
                ],
                gaps: [
                  ...state.gaps.map((gap) => ({ ...gap, state: "set" })),
                  ...snapshot.gaps.map((gap) => ({ ...gap, state: "removed" })),
                ],
              };
              writePlanned(doc, fileId, planned);
              // The anchors plugin runs behind this one and needs the ids that
              // the deletions of this update were stored under. Gaps resolve in
              // position order and fragments in rank order, which is document
              // order.
              const known = new Set(snapshot.gaps.flatMap((gap) => gap.fragments.map((f) => f.id)));
              created = resolveFile(doc, fileId).gaps.flatMap((gap) => gap.fragments.filter((f) => !known.has(f.id)).map((f) => ({ id: f.id, text: f.text, at: gap.at })));
            }
          }
          if (doc.getMap(CHANGES).size) prune(doc);
          doc.commit();
          snapshot = { changes: [], gaps: [] };
        },
      };
    }));
    return { before, after, created: () => created };
  }

  /// Whether the anchor index records a transaction. Only the user's own
  /// transactions are recorded here. Undo and redo are recorded by the index
  /// itself, from the document's events (see `attach` in lib/anchors.js), and
  /// so is a decision from the Changes panel, which the reader reads back into
  /// the index. A transaction carrying the binding's mark is already in the
  /// document and in the index.
  function recordsEdits(transaction) {
    return transaction.annotation(loroSyncAnnotation) === undefined;
  }

  /// Moves the comment anchors with the text, as the edits are made. Every
  /// transaction of the update is planned as one operation against the text the
  /// first one started from, and the records are written once against the final
  /// text. No cursor is made at an intermediate offset, which is the same reason
  /// `trackChanges` chains its transactions. The fragments come from the change
  /// plugin, which stored them in this same update. Its commit is labelled as
  /// anchor work. The index is loaded when the plugin is built, while the view
  /// and the document still agree. Imports are followed by the index itself.
  function anchorsPlugin(fileId, createdFragments) {
    return Prec.lowest(ViewPlugin.define(() => {
      session.anchors.load(fileId);
      return {
        update(update) {
          if (!update.docChanged || !update.transactions.some(recordsEdits)) return;
          const doc = session.doc;
          const steps = [];
          for (const transaction of update.transactions) {
            if (!transaction.docChanged || !recordsEdits(transaction)) continue;
            const edits = [];
            transaction.changes.iterChanges((fromA, toA, fromB, toB, insert) => {
              edits.push({ from: fromA, to: toA, insert: insert.sliceString(0, insert.length, "\n") });
            });
            steps.push({ transaction, edits });
          }
          if (!steps.length) return;
          const composed = composeEdits(steps.map((step) => step.edits));
          const before = steps[0].transaction.startState.doc.toString();
          const fragments = composed.some((edit) => edit.to > edit.from) ? fragmentRangesFor(before, composed, createdFragments()) : [];
          session.anchors.record([{ fileId, edits: composed, fragments }], session.anchorMemory);
          doc.commit({ origin: ANCHORS_ORIGIN });
        },
      };
    }));
  }

  let parsedBibliography = $state(null);
  const insertTargets = new Map();
  // One undo manager for the document the editor is bound to, shared by every
  // file's state. A session swap brings a new document and so a new manager.
  let undoManager = null;
  let undoManagerDoc = null;

  /// Writes a file that is not on screen, and records the edit in the anchor
  /// index, which only sees the file on screen through its plugin. The text is
  /// committed before the records are written, so the commit that carries the
  /// text keeps its origin and stays in the undo history.
  function writeText(fileId, text, from, to, insert) {
    const doc = session.doc;
    const before = text.toString();
    session.anchors.get(fileId);
    const known = new Set(doc.getMap(CHANGES).size ? resolveFile(doc, fileId).gaps.flatMap((gap) => gap.fragments.map((f) => f.id)) : []);
    if (tracked) trackedSplice(doc, fileId, from, to, insert, me);
    else untrackedSplice(doc, fileId, from, to, insert);
    doc.commit();
    const edits = [{ from, to, insert }];
    let fragments = [];
    if (to > from && doc.getMap(CHANGES).size) {
      const created = resolveFile(doc, fileId).gaps.flatMap((gap) => gap.fragments.filter((f) => !known.has(f.id)).map((f) => ({ id: f.id, text: f.text, at: gap.at })));
      fragments = fragmentRangesFor(before, edits, created);
    }
    session.anchors.record([{ fileId, edits, fragments }], session.anchorMemory);
    doc.commit({ origin: ANCHORS_ORIGIN });
  }

  /// Start recording what is typed as tracked changes.
  export function startTracking() {
    tracked = true;
  }

  /// Stop recording. The changes already made stay pending in the document.
  export function stopTracking() {
    tracked = false;
  }

  /// Whether tracking is on, which the switch in the Changes panel reads back.
  export function trackingOn() {
    return tracked;
  }

  let bibliographyGeneration = 0;
  let bibliographyTimer = null;
  let lastBibliographyKey = "";
  let zoteroSearchGeneration = 0;
  let lastZoteroError = "";
  const ZOTERO_NEEDS_DESKTOP = "Zotero needs the LibrePaper desktop app.";
  function formatOf(path) {
    const lower = String(path || "").toLowerCase();
    if (lower.endsWith(".typ")) return "typst";
    if (lower.endsWith(".md") || lower.endsWith(".markdown") || lower.endsWith(".qmd")) return "markdown";
    if (lower.endsWith(".tex") || lower.endsWith(".ltx")) return "latex";
    return "";
  }
  function bibliographyEntries() { return parsedBibliography?.entries || []; }
  function bibliographyRequest() {
    const doc = session.doc;
    const tree = doc ? treeForDoc(doc) : { main: "", texts: {} };
    const id = showing || (doc ? projectDirectory(doc, rules ?? null).mainId : "");
    const main = pathOfDoc(doc, id) || tree.main || "";
    const activeFormat = formatOf(main);
    return { main, format: activeFormat, source: getTextFromDoc(id)?.toString?.() ?? tree.texts?.[main] ?? "", texts: tree.texts || {} };
  }
  function refreshBibliography() {
    if (typeof analyze !== "function" || !session) return;
    const generation = ++bibliographyGeneration;
    const request = bibliographyRequest();
    const key = bibliographyCacheKey(request);
    if (!bibliographyNeedsAnalysis(request)) {
      parsedBibliography = null; lastBibliographyKey = key;
      onbibliography?.({ entries: [], diagnostics: [] }, request);
      return;
    }
    if (key !== lastBibliographyKey) parsedBibliography = null;
    lastBibliographyKey = key;
    bibliographyCache.get(request, analyze).then((result) => {
      if (generation !== bibliographyGeneration) return;
      const newlyAvailable = !parsedBibliography && result.entries.length > 0;
      parsedBibliography = result;
      onbibliography?.(result, request);
      const currentFormat = formatOf(pathOfDoc(session.doc, showing));
      const currentContext = view && citationContext(view.state.doc.toString(), view.state.selection.main.head, currentFormat);
      if (newlyAvailable && view?.hasFocus && currentContext) startCompletion(view);
    }).catch((error) => {
      if (generation !== bibliographyGeneration) return;
      parsedBibliography = null;
      onbibliography?.({ entries: [], diagnostics: [{ severity: "warning", message: `Bibliography unavailable: ${error?.message || "the parser could not be loaded"}`, file: request.main, line: 0 }] }, request);
    });
  }
  function scheduleBibliography() {
    bibliographyGeneration += 1;
    clearTimeout(bibliographyTimer);
    bibliographyTimer = setTimeout(refreshBibliography, 120);
  }

  function reportZotero(message) {
    if (message === lastZoteroError) return;
    lastZoteroError = message;
    onbibliography?.({ entries: bibliographyEntries(), diagnostics: [{ severity: "warning", message, file: pathOfDoc(session.doc, showing), line: 0 }] }, bibliographyRequest());
  }

  async function remoteBibliography(query) {
    if (!["markdown", "quarto"].includes(formatOf(pathOfDoc(session.doc, showing)))) return [];
    // Zotero lives on this computer, so a browser says where it can be used.
    if (!desktop()) {
      reportZotero(ZOTERO_NEEDS_DESKTOP);
      return [];
    }
    const generation = ++zoteroSearchGeneration;
    await new Promise((resolve) => setTimeout(resolve, 180));
    if (generation !== zoteroSearchGeneration) return [];
    let response;
    try {
      response = await searchZotero(query);
      lastZoteroError = "";
    } catch (error) {
      reportZotero(`Zotero unavailable: ${error?.message || String(error)}`);
      return [];
    }
    if (generation !== zoteroSearchGeneration) return [];
    return (response?.entries || []).map((entry) => ({
      ...entry, key: entry.citation_key, year: entry.year || "", type: entry.item_type,
    }));
  }

  async function importZotero(entry, target) {
    if (!editable || !session || target.view !== view || !showing) return false;
    const doc = session.doc;
    const active = getTextFromDoc(showing);
    if (!active) return false;
    const fromUnicode = active.convertPos(target.from, "utf16", "unicode") ?? target.from;
    const toUnicode = active.convertPos(target.to, "utf16", "unicode") ?? target.to;
    // Reassigned below when the document moved while the item was fetched.
    let fromCursor = active.getCursor(fromUnicode, 0);
    let toCursor = active.getCursor(toUnicode, 0);
    const activeSession = session, activeFile = showing, activePath = pathOfDoc(doc, showing);
    try {
      const fetched = await zoteroItem(entry.zotero_item);
      if (session !== activeSession || session.doc !== doc || showing !== activeFile || pathOfDoc(doc, activeFile) !== activePath || view !== target.view || !editable) return false;

      const fromResult = doc.getCursorPos(fromCursor);
      if (!fromResult) return false;
      if (fromResult.update) fromCursor = fromResult.update;
      const toResult = doc.getCursorPos(toCursor);
      if (!toResult) return false;
      if (toResult.update) toCursor = toResult.update;

      const start_utf16 = active.convertPos(fromResult.offset, "unicode", "utf16") ?? 0;
      const end_utf16 = active.convertPos(toResult.offset, "unicode", "utf16") ?? 0;

      const tree = treeForDoc(doc);
      // The bibliography is registered in the main file when it is Markdown, else in the citing file.
      const mainIsMarkdown = formatOf(tree.main) === "markdown";
      const regPath = mainIsMarkdown ? tree.main : activePath;
      const regId = mainIsMarkdown ? textIdForPath(doc, tree.main) : activeFile;
      const reg = getTextFromDoc(regId);
      const plan = planZoteroImport({
        source: reg?.toString() || "", targetPath: regPath, texts: tree.texts,
        item: { ...fetched, zotero_item: entry.zotero_item },
      });
      const bibId = textIdForPath(doc, plan.path);
      const bib = bibId ? getTextFromDoc(bibId) : null;
      const changes = [{ from: Math.min(start_utf16, end_utf16), to: Math.max(start_utf16, end_utf16), insert: plan.key }];
      if (plan.registration && regId === activeFile) changes.push(plan.registration);
      changes.sort((a, b) => a.from - b.from || a.to - b.to);
      const setupShift = plan.registration && regId === activeFile && plan.registration.from <= Math.min(start_utf16, end_utf16)
        ? plan.registration.insert.length - (plan.registration.to - plan.registration.from) : 0;
      if (bib) {
        if (plan.addition) {
          const end = bib.toString().length;
          writeText(bibId, bib, end, end, plan.addition);
        }
      } else addTextToDoc(doc, plan.path, plan.bibtex);
      if (plan.registration && reg && regId !== activeFile) {
        writeText(regId, reg, plan.registration.from, plan.registration.to, plan.registration.insert);
      }
      target.view.dispatch({ changes, selection: { anchor: Math.min(start_utf16, end_utf16) + setupShift + plan.key.length }, annotations: Transaction.userEvent.of("input.complete") });
      if (tracked) prune(doc);
      doc.commit();
      scheduleBibliography();
      return true;
    } catch (error) {
      onbibliography?.({ entries: bibliographyEntries(), diagnostics: [{ severity: "warning", message: `Zotero import failed: ${error?.message || error}`, file: pathOfDoc(session.doc, showing), line: 0 }] }, bibliographyRequest());
      return false;
    }
  }

  let host = $state(null);
  let view = null;
  // One editor state per file, made the first time that file is opened and
  // kept afterwards. Switching files swaps the state rather than rebuilding
  // it, so a visit to another chapter costs nothing and leaves the undo
  // history and the scroll position where they were -- which is the whole
  // difference between a directory of files and one text somebody keeps
  // scrolling through.
  // A CodeMirror state owns a Loro binding and undo field. File ids are only
  // unique inside a document, and a session swap brings a new document, so
  // cache states under their owning LoroDoc as well as the file id.
  let states = new WeakMap();
  let activeDocument = null;
  const stateMap = (document_) => {
    let map = states.get(document_);
    if (!map) { map = new Map(); states.set(document_, map); }
    return map;
  };
  // Which file the view is currently showing, so a swap can put the state it
  // is leaving back in the map before taking the next one.
  let showing = "";

  // What the compiler last said. `all` is every diagnostic, across every file
  // in the document, because walking them is a walk through the document and
  // not through one file of it; `shown` is the subset belonging to the file on
  // screen, in this editor's own coordinates, which is all that can be painted
  // here.
  let all = [];
  let shown = [];

  /// Which file a diagnostic is about. The engines agree on this: `file` is
  /// empty for the main file and a path otherwise, and `line: 0` means the
  /// diagnostic has no place at all -- which is a different thing from being
  /// about the main file, and is why the line is checked first.
  function fileOf(diagnostic) {
    const doc = session.doc;
    if (!diagnostic.file) return doc ? projectDirectory(doc, rules ?? null).mainId : "";
    return textIdForPath(doc, diagnostic.file);
  }

  /// Paints what a compile had to say: an underline on the span it names, a
  /// mark in the gutter of that line, and the message and hints on hover.
  /// Called with an empty list to clear, which a successful render does.
  ///
  /// Lines and columns are one-based and count UTF-16 units, which is what
  /// CodeMirror counts in; a diagnostic with no span (line 0) has no place to
  /// be painted and is left to the badge and the panel.
  export function setDiagnostics(list) {
    if (!view) return;
    all = list || [];
    const doc = view.state.doc;
    const place = (line, column) => {
      const at = doc.line(Math.min(Math.max(line, 1), doc.lines));
      return Math.min(at.from + Math.max(column - 1, 0), at.to);
    };
    shown = all
      // Only this file's. An underline drawn at line 40 of the chapter on
      // screen, for an error at line 40 of a different chapter, would be a
      // confident lie.
      .filter((diagnostic) => fileOf(diagnostic) === showing)
      .filter((diagnostic) => diagnostic.severity !== "info")
      .filter((diagnostic) => diagnostic.line > 0 && diagnostic.line <= doc.lines)
      .map((diagnostic) => {
        const from = place(diagnostic.line, diagnostic.column);
        const to = diagnostic.end_line
          ? Math.max(place(diagnostic.end_line, diagnostic.end_column), from + 1)
          : from + 1;
        const hints = (diagnostic.hints || []).join("\n");
        return {
          from,
          to: Math.min(to, doc.length),
          severity: diagnostic.severity === "warning" ? "warning" : "error",
          message: hints ? `${diagnostic.message}\n${hints}` : diagnostic.message,
        };
      })
      .sort((a, b) => a.from - b.from);
    // With the check off nothing is drawn in the source, and the reader's list
    // of messages is unchanged.
    view.dispatch(setLintDiagnostics(view.state, untrack(() => check) ? shown : []));
  }

  /// Moves the caret to the diagnostic after the one the caret is at, round
  /// the list, and scrolls it into the middle of the view. What clicking the
  /// badge does.
  ///
  /// The walk is over the whole document, not the open file: an error in a
  /// chapter is an error in the document, and a badge that walked only the
  /// file on screen would say "1 error" and then refuse to go to it. Reaching
  /// one in another file opens that file first.
  export function nextDiagnostic() {
    if (!view) return false;
    // In this file, after the caret: the ordinary case, and no swap.
    const at = view.state.selection.main.head;
    const here = shown.find((diagnostic) => diagnostic.from > at);
    if (here) {
      goTo(here.from);
      return true;
    }
    // Otherwise the next one anywhere, in the order the files are listed,
    // wrapping round to the first.
    const placed = all.filter((diagnostic) => diagnostic.line > 0 && fileOf(diagnostic));
    if (placed.length === 0) return false;
    const order = (diagnostic) => [fileOf(diagnostic), diagnostic.line, diagnostic.column];
    placed.sort((a, b) => String(order(a)).localeCompare(String(order(b))));
    const elsewhere = placed.find((diagnostic) => fileOf(diagnostic) !== showing) || placed[0];
    openAt(fileOf(elsewhere), elsewhere.line, elsewhere.column);
    return true;
  }

  /// Opens a file and puts the caret at an offset in it, in one go. The swap
  /// has to happen before the caret moves -- an offset means nothing until the
  /// view is showing the text it is an offset into -- which is why this is one
  /// function and not the caller doing both.
  export function goToIn(id, at) {
    if (!view || !id) return;
    show(id);
    goTo(at);
  }

  /// Opens a file and puts the caret on a line in it. What choosing a
  /// diagnostic in the panel does, and what the badge does when the next one
  /// is in another chapter.
  export function openAt(id, line, column = 1) {
    if (!view || !id) return;
    show(id);
    // The state has just been swapped, so the coordinates are this file's.
    const doc = view.state.doc;
    const at = doc.line(Math.min(Math.max(line, 1), doc.lines));
    goTo(Math.min(at.from + Math.max(column - 1, 0), at.to));
  }

  /// The panel CodeMirror provides, for the document with nine errors. The
  /// badge is the whole of it for the common case, which is one.
  export function showDiagnosticPanel() {
    if (view) openLintPanel(view);
  }

  /// The text as it stands, which is what a save publishes.
  export function text(expectedFile) {
    if (expectedFile && showing !== expectedFile) return null;
    return view ? view.state.doc.toString() : "";
  }

  /// Where the caret is, as an offset into that text.
  export function caret() {
    return view ? view.state.selection.main.head : 0;
  }

  /// The source selection, when it is not a caret: the file it is in and its
  /// UTF-16 range there. Null otherwise.
  export function selection() {
    if (!view || !showing) return null;
    const { from, to } = view.state.selection.main;
    return from < to ? { file: showing, from, to } : null;
  }

  /// Puts the caret at an offset and scrolls it into the middle of the view:
  /// what a click in the document does when the two are kept in step.
  export function goTo(at) {
    if (!view) return;
    view.dispatch({
      selection: { anchor: at },
      effects: EditorView.scrollIntoView(at, { y: "center" }),
    });
    view.focus();
  }

  export function focus() {
    view?.focus();
  }

  export function editAvailability() {
    if (!view) return {};
    const selected = !view.state.selection.main.empty;
    const manager = undoManager;
    return {
      undo: editable && (manager?.canUndo?.() ?? false),
      redo: editable && (manager?.canRedo?.() ?? false),
      cut: editable && selected, copy: selected,
      paste: editable, "select-all": true, find: true, replace: editable,
      bold: editable, italic: editable,
    };
  }

  export async function editCommand(command) {
    if (!editAvailability()[command]) return;
    const target = view;
    const state = target.state;
    target.focus();
    if (command === "undo") undoCommand(target);
    else if (command === "redo") redoCommand(target);
    else if (command === "select-all") selectAll(target);
    else if (command === "bold") boldCommand(target);
    else if (command === "italic") italicCommand(target);
    else if (command === "find" || command === "replace") {
      openSearchPanel(target);
      if (command === "replace") target.dom.querySelector('[name="replace"]')?.focus();
    } else if (command === "copy" || command === "cut") {
      const { from, to } = state.selection.main;
      await navigator.clipboard.writeText(state.sliceDoc(from, to));
      if (command === "cut") {
        if (view !== target || target.state !== state || !editable) throw new Error("The source changed before Cut finished. Select the text again.");
        target.dispatch({ changes: { from, to }, selection: { anchor: from }, annotations: Transaction.userEvent.of("delete.cut") });
      }
    } else if (command === "paste") {
      const text = await navigator.clipboard.readText();
      if (view !== target || target.state !== state || !editable) throw new Error("The source changed before Paste finished. Try again.");
      target.dispatch({ ...state.replaceSelection(text), annotations: Transaction.userEvent.of("input.paste"), scrollIntoView: true });
    }
  }

  // Capture the target using Loro cursors. Unlike raw CodeMirror offsets, these
  // survive edits made by collaborators while an Insert dialog is open. The
  // session and file identity are retained so a late dialog result cannot
  // write into a different document.
  export function captureInsertTarget() {
    if (!view || !session || !showing || editable === false) return null;
    const doc = session.doc;
    const loroText = getTextFromDoc(showing);
    if (!loroText) return null;
    const { from, to } = view.state.selection.main;
    // Convert UTF-16 offsets to Unicode code points for cursor creation.
    const fromUnicode = loroText.convertPos(from, "utf16", "unicode") ?? from;
    const toUnicode = loroText.convertPos(to, "utf16", "unicode") ?? to;
    return {
      session,
      doc,
      file: showing,
      fromCursor: loroText.getCursor(fromUnicode, 0),
      toCursor: loroText.getCursor(toUnicode, 0),
      capturedText: view.state.sliceDoc(from, to),
    };
  }

  // The format of the file being edited, for the emphasis commands.
  function emphasisFormat() {
    const path = pathOfDoc(session.doc, showing);
    return documentFormat(path) || (!path ? format : "");
  }
  const boldCommand = emphasisCommand("bold", emphasisFormat);
  const italicCommand = emphasisCommand("italic", emphasisFormat);

  // Context consumed by InsertMenu's format-aware generators. `target` is an
  // implementation detail used by applyInsertResult and intentionally lives
  // alongside the serializable context rather than in the menu itself.
  export function getInsertContext() {
    const target = captureInsertTarget();
    if (!target || !view) return null;
    const doc = target.doc;
    const path = pathOfDoc(doc, showing);
    const insertFormat = documentFormat(path) || (!path ? format : "");
    const tree = treeForDoc(doc);
    const mainPath = tree.main || path;
    const mainText = tree.texts?.[mainPath] || "";
    // The stable container behind each path, kept beside the target rather than
    // in the context. applyInsertResult compares against it, so a path that
    // names a different file by the time the dialog applies is refused.
    target.fileIds = new Map(Object.keys(tree.texts || {}).map(filePath => [filePath, textIdForPath(doc, filePath)]));
    if (path) target.fileIds.set(path, showing);
    return {
      format: insertFormat,
      path,
      text: view.state.doc.toString(),
      selection: { from: view.state.selection.main.from, to: view.state.selection.main.to, text: target.capturedText },
      mainText,
      mainPath,
      files: Object.entries(tree.texts || {}).map(([filePath, text]) => ({ path: filePath, text: String(text ?? "") })),
      bibliography: bibliographyEntries(),
      targetId: (() => {
        const id = `insert-${Date.now()}-${Math.random().toString(36).slice(2)}`;
        target.snapshotText = view.state.doc.toString();
        // Targets are independent. Each UI releases its own when it inserts or
        // gives up, and the cap only bounds a UI that forgets to.
        insertTargets.set(id, target);
        while (insertTargets.size > 32) insertTargets.delete(insertTargets.keys().next().value);
        return id;
      })(),
    };
  }

  export function releaseInsertContext(context) {
    if (context?.targetId) insertTargets.delete(context.targetId);
  }

  // Validate every change before touching shared text. Main-file setup and the
  // snippet share the active editor's Loro transaction and undo entry.
  export function applyInsertResult(result, capturedContext = null, { focus = true } = {}) {
    if (!result || typeof result.text !== "string" || !view || !session || !editable) return false;
    const target = insertTargets.get(capturedContext?.targetId);
    if (!target || target.session !== session || target.doc !== session.doc || target.file !== showing || pathOfDoc(target.doc, showing) !== capturedContext.path) return false;
    const loroText = getTextFromDoc(showing);
    const doc = target.doc;
    if (!loroText) return false;

    // Resolve cursors to current positions, updating them if Loro indicates staleness.
    let fromCursor = target.fromCursor;
    let toCursor = target.toCursor;
    let fromPos = null, toPos = null;

    try {
      const fromResult = doc.getCursorPos(fromCursor);
      if (!fromResult) return false;
      if (fromResult.update) fromCursor = fromResult.update;
      fromPos = fromResult.offset;

      const toResult = doc.getCursorPos(toCursor);
      if (!toResult) return false;
      if (toResult.update) toCursor = toResult.update;
      toPos = toResult.offset;
    } catch (e) {
      return false;
    }

    // Convert back to UTF-16 for CodeMirror use.
    const from = loroText.convertPos(Math.min(fromPos, toPos), "unicode", "utf16") ?? 0;
    const to = loroText.convertPos(Math.max(fromPos, toPos), "unicode", "utf16") ?? 0;

    if (loroText.toString().slice(from, to) !== target.capturedText) return false;
    // One plan per file, keyed by container id rather than by the handle.
    //
    // `session.textOf` builds a fresh JS wrapper around the same LoroText on
    // every call, so `===` between two of them is false even when they are
    // the same text. Keying on the handle filed an edit to the file on screen
    // under a second key: it was left out of the transaction the view was
    // given and written straight into the document by the loop below, which
    // is the one path here that bypasses CodeMirror. The document took the
    // change and the editor never saw it -- enabling a Quarto table of
    // contents wrote the frontmatter into the file and showed none of it --
    // and the two stayed apart until something else rebuilt the state.
    const keyOf = (text) => String(text.id);
    const showingKey = keyOf(loroText);
    const plans = new Map([[showingKey, { file: showing, text: loroText, edits: [] }]]);
    const mapOffset = (offset, snapshot, current) => {
      if (snapshot === current) return offset;
      let prefix = 0, suffix = 0;
      while (prefix < snapshot.length && prefix < current.length && snapshot[prefix] === current[prefix]) prefix++;
      while (suffix < snapshot.length - prefix && suffix < current.length - prefix && snapshot[snapshot.length - suffix - 1] === current[current.length - suffix - 1]) suffix++;
      if (offset < prefix) return offset;
      if (offset > snapshot.length - suffix) return offset + current.length - snapshot.length;
      return null;
    };
    for (const edit of result.additionalEdits || []) {
      if (!Number.isInteger(edit?.from) || !Number.isInteger(edit?.to) || typeof edit.insert !== "string") return false;
      const path = edit.path || capturedContext.path;
      const fileId = textIdForPath(doc, path);
      // A path can name a different file by the time the dialog applies: a
      // collaborator may have renamed the file away and created another at the
      // same path. Refuse the whole insertion rather than redirect the edit.
      if (target.fileIds.get(path) !== fileId) return false;
      const fileText = getTextFromDoc(fileId);
      const snapshot = path === capturedContext.path ? capturedContext.text : capturedContext.files?.find(file => file.path === path)?.text;
      if (!fileText || typeof snapshot !== "string" || edit.from < 0 || edit.to < edit.from || edit.to > snapshot.length) return false;
      const editFrom = mapOffset(edit.from, snapshot, fileText.toString()), editTo = mapOffset(edit.to, snapshot, fileText.toString());
      const key = keyOf(fileText);
      if (editFrom == null || editTo == null || (key === showingKey && editFrom < to && editTo > from)) return false;
      if (!plans.has(key)) plans.set(key, { file: fileId, text: fileText, edits: [] });
      plans.get(key).edits.push({ from: editFrom, to: editTo, insert: edit.insert });
    }
    const changes = plans.get(showingKey).edits;
    const shift = changes.filter(edit => edit.to <= from).reduce((sum, edit) => sum + edit.insert.length - (edit.to - edit.from), 0);
    // A snippet is inserted by CodeMirror's own command once the other edits
    // are in place, because only the command knows how to select its first
    // field and let Tab move to the rest. The file on screen goes through the
    // view. The other files are written and committed one edit at a time by
    // writeText, and the undo manager merges commits made within its merge
    // interval into one undo step.
    if (!result.snippet) changes.push({ from, to, insert: result.text });
    for (const plan of plans.values()) {
      plan.edits.sort((a, b) => a.from - b.from || a.to - b.to);
      for (let i = 1; i < plan.edits.length; i++) if (plan.edits[i - 1].to > plan.edits[i].from) return false;
    }
    if (result.snippet) {
      // The snippet command stretches its range to the live selection when the
      // two start together, so the selection is put on the captured range first.
      view.dispatch({ changes, selection: { anchor: from + shift, head: to + shift } });
      snippet(snippetTemplate(result.text))(view, null, from + shift, to + shift);
      view.dispatch({ effects: EditorView.scrollIntoView(view.state.selection.main.head) });
    } else {
      const selection = result.selection;
      const offset = value => from + shift + Math.max(0, Math.min(result.text.length, Number.isInteger(value) ? value : result.text.length));
      const anchor = offset(selection?.anchor), head = offset(selection?.head);
      view.dispatch({ changes, selection: { anchor, head }, effects: EditorView.scrollIntoView(head) });
    }
    // Every other file the insertion touches. The one on screen went through
    // the transaction above; writing it here as well would apply it twice.
    for (const [key, plan] of plans) if (key !== showingKey) {
      for (const edit of [...plan.edits].reverse()) writeText(plan.file, plan.text, edit.from, edit.to, edit.insert);
    }
    if (tracked) prune(doc);
    doc.commit();
    releaseInsertContext(capturedContext);
    // A picker that stays open keeps the keyboard. On a phone, focusing the editor would raise the virtual keyboard.
    if (focus) view.focus();
    return true;
  }

  /// Builds the state for one file, bound to its own LoroText.
  function stateFor(id) {
    const text = getTextFromDoc(id);
    const path = pathOfDoc(session.doc, id);
    // One per document, not per file: the undo manager is constructed from the
    // document and tracks the operations this peer made anywhere in it.
    // Handing it a text is a type error the binding reports as a failure to
    // mount, with nothing said about which argument was wrong.
    // Undo takes back what was typed, and not the existence of the file it was
    // typed into. A directory change is committed under its own origin
    // (`DIRECTORY_ORIGIN`) and named here, because this manager is one per
    // document and merges everything within a second: without the exclusion,
    // creating a file and typing into it are one undo step, and taking it
    // back deletes the file.
    const bound = session.doc;
    if (undoManagerDoc !== bound) {
      // The comment anchors are not undone either: their commits are labelled
      // ANCHORS_ORIGIN and follow the text back as undo and redo move it.
      undoManager = new UndoManager(bound, { excludeOriginPrefixes: [DIRECTORY_ORIGIN, ANCHORS_ORIGIN] });
      undoManagerDoc = bound;
    }
    const manager = undoManager;

    // Who the caret belongs to. The session publishes {name, color, tab}; the
    // binding wants {name, colorClassName}, and paints through a class rather
    // than a value. The palette in lib/presence-colour.js is six fixed colours
    // with a rule apiece in the stylesheet, so the class is the colour with
    // its hash dropped. Somebody with no colour yet gets no class, and the
    // stylesheet falls back to a theme token rather than to a colour written
    // down here.
    const who = session.localPresence?.();
    const userName = who?.name || "Anonymous";
    const colorClassName = typeof who?.color === "string" && who.color.startsWith("#")
      ? `user-color-${who.color.slice(1).toLowerCase()}`
      : "";

    const tracking = trackChanges(id);
    return EditorState.create({
      doc: text.toString(),
      extensions: [
        EditorView.editable.of(Boolean(editable)),
        EditorView.contentAttributes.of({ "aria-label": path ? `Source of ${path}` : "Source" }),
        // Vim, when the setting says so, and always first: an earlier
        // extension has precedence, and Vim has to see a key before the
        // default keymap does, or `j` inserts a letter instead of moving.
        vimCompartment.of(resolvedKeys(untrack(() => keys)) || []),
        themeCompartment.of(themeFor(mode() === "dark")),
        lineNumbers(),
        drawSelection(),
        highlightActiveLine(),
        highlightActiveLineGutter(),
        highlightSpecialChars(),
        dropCursor(),
        EditorState.allowMultipleSelections.of(true),
        rectangularSelection(),
        bracketMatching(),
        bracketsCompartment.of(bracketsOn(untrack(() => brackets))),
        foldGutter(),
        indentOnInput(),
        highlightSelectionMatches(),
        sourceHighlighting,
        sourceLanguage(path, format),
        completionCompartment.of(completionOn(untrack(() => completion))),
        completionExtension({
          project: completionProject,
          sources: [bibliographyCompletion({ entries: bibliographyEntries, format: () => formatOf(pathOfDoc(session.doc, showing)), remote: remoteBibliography, onRemote: importZotero })],
        }),
        // Where a compile's errors are shown: the gutter mark, and with it the
        // underline and the hover the lint extension draws. Drawn only while
        // the check is on.
        checkCompartment.of(checkOn(untrack(() => check))),
        EditorView.lineWrapping,
        // Pending changes, drawn in the text they changed.
        pendingChanges,
        keymap.of([
          // Everyone tries Ctrl/Cmd-S in an editor.
          { key: "Mod-s", preventDefault: true, run: () => (onsave?.(), true) },
          // Replace, which the search panel has always offered and no key
          // reached. The editor owns it, like everything else in the panel:
          // lib/commands.js names the binding so the menu and the shortcut
          // table can print it, and this is the one place it is run.
          { key: "Mod-Alt-f", preventDefault: true, run: () => (void editCommand("replace"), true) },
          // Bold and italic; lib/commands.js names these bindings. Not in a
          // highest-precedence keymap, so Vim and Emacs keep Ctrl+B and Ctrl+I.
          { key: "Mod-b", preventDefault: true, run: boldCommand },
          { key: "Mod-i", preventDefault: true, run: italicCommand },
          ...foldKeymap,
          indentWithTab,
          ...completionKeymap,
          ...defaultKeymap,
          ...searchKeymap,
          // Ctrl-Shift-M opens the list of what the compiler said.
          ...lintKeymap,
        ]),
        // Undo and redo are ours, not the binding's. The binding binds Mod-z
        // at Prec.high from inside LoroExtensions, so ours has to outrank it
        // to be the one that runs; `undoManagerField` hands it the same
        // manager the binding was given. See lib/loro-undo.js for why.
        undoManagerField.init(() => manager),
        Prec.highest(keymap.of([
          { key: "Mod-z", run: undoCommand, preventDefault: true },
          { key: "Mod-y", mac: "Mod-Shift-z", run: redoCommand, preventDefault: true },
          { key: "Mod-Shift-z", run: redoCommand, preventDefault: true },
        ])),
        // The shared document, and everyone else's cursors in it. The editor
        // is bound to one text at a time (the one in 'showing'), and the
        // getTextFromDoc function returns that text. Multiple editors may be
        // open on different files at once; they all share the same LoroDoc.
        // Before the binding, so the records are read before the text is written.
        tracking.before,
        LoroExtensions(bound, session.ephemeral && { user: { name: userName, colorClassName }, ephemeral: session.ephemeral }, manager, () => text),
        // After the binding, so the text is written when this one looks.
        tracking.after,
        // After the change plugin, so the anchors follow records already written.
        anchorsPlugin(id, tracking.created),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) {
            // The marks were mapped through the edit. Drawing them again from
            // the document picks up the change the edit just recorded, and
            // has to happen after the transaction rather than inside it. The
            // draw is queued, so the commits of one tick share one redraw.
            scheduleDraw();
            onchange?.();
          }
          // Only a deliberate move counts: typing moves the caret constantly,
          // and scrolling the document on every keystroke would make the
          // preview unreadable.
          if (update.selectionSet && !update.docChanged) oncaret?.();
          // On a phone the keyboard covers the lower half of the screen, and a
          // tap is what raises it, so the tapped line goes to the top of the
          // editor where it stays in sight. Only a pointer tap that places the
          // caret counts: a drag or a double tap selects a range to act on,
          // and typing, arrow keys, goTo and other people's cursors match
          // nothing here, so all of those leave the scroll position alone.
          // The scroll waits a frame because a view may not dispatch from
          // inside its own update.
          if (update.selectionSet && update.state.selection.main.empty
              && update.transactions.some((tr) => tr.isUserEvent("select.pointer"))
              && window.matchMedia("(max-width: 760px)").matches) {
            const head = update.state.selection.main.head;
            requestAnimationFrame(() => {
              if (!view || view.state.selection.main.head !== head) return;
              view.dispatch({ effects: EditorView.scrollIntoView(head, { y: "start", yMargin: 8 }) });
            });
          }
        }),
      ],
    });
  }

  /// Reconfigures the live view's Vim compartment to match the `keys` prop.
  /// A state built while the setting was different -- or fetched from the
  /// cache before the package had loaded -- is brought into line here rather
  /// than when it was built, which is the one place both are reconciled with
  /// the browser's current choice.
  function syncKeys(target) {
    if (!target) return;
    // Read without tracking: the mount effect and the file effect call this
    // too, and neither may re-run -- rebuilding the view -- when the setting
    // changes. The effect below is the one that follows it.
    const want = untrack(() => keys);
    if (want !== "vim" && want !== "emacs") {
      target.dispatch({ effects: vimCompartment.reconfigure([]) });
      return;
    }
    const ready = resolvedKeys(want);
    if (ready) {
      target.dispatch({ effects: vimCompartment.reconfigure(ready) });
      return;
    }
    (want === "vim" ? loadVim() : loadEmacs()).then((extension) => {
      // The setting, or the view, may have moved on while the download ran.
      if (view === target && untrack(() => keys) === want) {
        target.dispatch({ effects: vimCompartment.reconfigure(extension) });
      }
    });
  }

  /// Reconfigures the live view's completion, bracket and check switches to
  /// match their props, in one transaction. Like `syncKeys`, it reads the
  /// props without tracking, so the effect at the end of the script is the one
  /// that follows them.
  function syncFeatures(target) {
    if (!target) return;
    target.dispatch({
      effects: [
        completionCompartment.reconfigure(completionOn(untrack(() => completion))),
        bracketsCompartment.reconfigure(bracketsOn(untrack(() => brackets))),
        checkCompartment.reconfigure(checkOn(untrack(() => check))),
      ],
    });
  }

  // Get the text for a file from the room's document.
  function getTextFromDoc(id) {
    const doc = session.doc;
    const text = doc?.getMap("files")?.get(id);
    return text?.kind?.() === "Text" ? text : null;
  }

  function pathOfDoc(doc, id) {
    if (!doc || !id) return "";
    for (const [path, file] of projectDirectory(doc, rules ?? null).files) {
      if (file.kind === "text" && file.id === id) return path;
    }
    return "";
  }
  function textIdForPath(doc, path) {
    if (!doc || !path) return "";
    const file = projectDirectory(doc, rules ?? null).files.get(path);
    return file?.kind === "text" ? file.id : "";
  }
  // Project facts for completion sources. The format comes from the edited
  // file's raw path, which is cheap to read. The projected path and the project
  // walk run only when a source first asks for them.
  /** @returns {import("../lib/completion/paths.js").CompletionProject} */
  function completionProject() {
    const doc = session.doc;
    if (!doc) return { path: "", format: "", texts: {}, files: [] };
    const raw = doc.getMap("paths").get(showing);
    const rawPath = typeof raw === "string" ? raw : "";
    let tree = null;
    let projected = null;
    const load = () => {
      if (!tree) tree = treeForDoc(doc);
      return tree;
    };
    return {
      get path() {
        if (projected === null) projected = pathOfDoc(doc, showing) || rawPath;
        return projected;
      },
      format: formatOfPath(rawPath),
      get texts() {
        /** @type {Record<string, string>} */
        const texts = {};
        for (const [file, text] of Object.entries(load().texts)) texts[file] = String(text ?? "");
        return texts;
      },
      get files() {
        return Object.keys(load().files).sort();
      },
    };
  }
  function treeForDoc(doc) {
    const projection = projectDirectory(doc, rules ?? null);
    const texts = Object.create(null), files = Object.create(null), digests = Object.create(null);
    for (const [path, file] of projection.files) {
      if (file.kind === "text") {
        texts[path] = projection.texts.get(path);
        files[path] = { kind: "text", id: file.id };
      } else {
        digests[path] = file.digest;
        files[path] = { kind: "asset", sha: file.digest };
      }
    }
    return { main: projection.main, texts, files, digests };
  }
  function addTextToDoc(doc, path, body = "") {
    const existingMainId = projectDirectory(doc, rules ?? null).mainId;
    const files = doc.getMap("files"), paths = doc.getMap("paths");
    let id = "";
    do {
      const bytes = new Uint8Array(6);
      crypto.getRandomValues(bytes);
      const random = [...bytes].map((byte) => byte.toString(16).padStart(2, "0")).join("");
      // With no declared main, projection chooses the first text ID. Keep a
      // newly imported bibliography after the existing projected main.
      id = existingMainId ? `${existingMainId}~${random}` : random;
    } while (files.get(id) !== undefined || paths.get(id) !== undefined);
    const text = new LoroText();
    if (body) text.insert(0, body);
    files.setContainer(id, text);
    paths.set(id, path);
    doc.commit({ origin: DIRECTORY_ORIGIN });
    return id;
  }

  /// Shows a file, keeping the state of the one being left. The view is made
  /// once and re-stated, rather than destroyed and rebuilt, so that switching
  /// files does not flash.
  /// The one replacement that turns `old` into `next`: what lies between
  /// their common prefix and their common suffix.
  function textChange(old, next) {
    let from = 0;
    while (from < old.length && from < next.length && old.charCodeAt(from) === next.charCodeAt(from)) from++;
    let oldEnd = old.length;
    let nextEnd = next.length;
    while (oldEnd > from && nextEnd > from && old.charCodeAt(oldEnd - 1) === next.charCodeAt(nextEnd - 1)) {
      oldEnd--;
      nextEnd--;
    }
    return { from, to: oldEnd, insert: next.slice(from, nextEnd) };
  }

  /// Bring the file on screen up to its LoroText after a local write that did
  /// not come through this view: accepting or rejecting a change from the
  /// Changes panel, for one. The binding copies only imported edits into the
  /// view, so without this the screen would keep the old text. The dispatch is
  /// marked the way the undo plugin marks its own, as already in the document.
  /// The mark keeps the binding from writing the text back, and the anchors
  /// plugin from recording it. The decision that wrote the document already
  /// read the anchor index back (`settleAnchors` in Reader.svelte).
  function reconcileShown() {
    if (!view || !showing) return;
    const text = getTextFromDoc(showing);
    if (!text) return;
    const old = view.state.doc.toString();
    const next = text.toString();
    if (old === next) return;
    view.dispatch({
      changes: textChange(old, next),
      annotations: [loroSyncAnnotation.of("undo"), Transaction.addToHistory.of(false)],
    });
  }

  function show(id) {
    if (!view || !id || id === showing) return;
    if (showing && activeDocument) stateMap(activeDocument).set(showing, view.state);
    const document_ = session.doc;
    const documentStates = stateMap(document_);
    if (!documentStates.has(id)) documentStates.set(id, stateFor(id));
    const state = documentStates.get(id);
    const text = getTextFromDoc(id);
    // While a file is inactive its LoroText can still receive remote edits, but
    // its CodeMirror binding is not mounted to dispatch those edits. Reconcile
    // the cached state before showing it, preserving its history and selection.
    if (text && state.doc.toString() !== text.toString()) {
      documentStates.set(id, state.update({
        changes: textChange(state.doc.toString(), text.toString()),
        annotations: Transaction.addToHistory.of(false),
      }).state);
    }
    view.setState(documentStates.get(id));
    activeDocument = document_;
    syncKeys(view);
    syncFeatures(view);
    syncTheme(view);
    showing = id;
    // A file this browser has open is where its caret is, which is what the
    // file list shows beside each name.
    session.inFile?.(id);
    onfilechange?.(id);
    // The marks belong to files, so they are drawn again for the file now on
    // screen. What the compiler said has not changed; where it applies has.
    setDiagnostics(all);
    // A state carries no decorations until something puts them there.
    drawChanges();
    view.focus();
  }

  /// The file the view is showing, which the reader asks when it has a
  /// diagnostic to place or a caret to follow.
  export function openFile() {
    return showing;
  }

  // A pinch over the source sizes its text, and nothing else on the page.
  $effect(() => {
    if (!host) return;
    return attachEditorZoom(host, () => view);
  });

  $effect(() => {
    if (!host || !session || view) return;
    // Creating the editor calls reader callbacks that read and update UI
    // state. Only host/session own this lifecycle; incidental callback reads
    // must not recreate the editor when a pane or diagnostic changes.
    return untrack(() => {
      const initial = untrack(() => {
        const first = file || projectDirectory(session.doc, rules ?? null).mainId || "";
        showing = first;
        const documentStates = stateMap(session.doc);
        if (first && !documentStates.has(first)) documentStates.set(first, stateFor(first));
        activeDocument = session.doc;
        return { first, state: documentStates.get(first) || stateFor(first) };
      });
      view = new EditorView({
        state: initial.state,
        parent: host,
      });
      untrack(() => viewCallbacks.set(view, { onsave, onquit }));
      syncKeys(view);
      syncFeatures(view);
      const unsubscribeMode = onMode(() => syncTheme(view));
      drawChanges();
      if (initial.first) session.inFile?.(initial.first);
      const bibliographyWatcher = scheduleBibliography;
      const unsubscribeBibliography = session.onFiles?.(bibliographyWatcher);
      // Changes imported from other people, or decided by them, arrive in the
      // document without a keystroke here. A local write from outside the
      // view, such as a decision in the Changes panel, is copied into it.
      const unsubscribeChanges = session.doc.subscribe((batch) => queueMicrotask(() => {
        if (batch.by === "local") reconcileShown();
        scheduleDraw();
      }));
      refreshBibliography();
      view.focus();
      return () => {
        bibliographyGeneration += 1;
        clearTimeout(bibliographyTimer);
        if (typeof unsubscribeBibliography === "function") unsubscribeBibliography();
        unsubscribeChanges();
        unsubscribeMode();
        if (view) viewCallbacks.delete(view);
        view?.destroy();
        insertTargets.clear();
        // Cached states embed their UndoManager. Reset both caches together;
        // retaining one while clearing the other makes menu availability and
        // the state bound to the view disagree after a lifecycle rebind.
        states = new WeakMap();
        undoManager = null;
        undoManagerDoc = null;
        view = null;
        activeDocument = null;
        showing = "";
      };
    });
  });

  // Opening another file is a swap, not a rebuild. Kept out of the effect
  // above so that creating the view and changing which file it shows are two
  // separate things: the first happens once, the second whenever somebody
  // chooses a name in the list.
  $effect(() => {
    if (view && file) {
      show(file);
      scheduleBibliography();
    }
  });

  $effect(() => {
    void format;
    if (view) scheduleBibliography();
  });

  // Draw the pending changes again when the selection or the viewer changes.
  $effect(() => {
    void selectedChange;
    void me;
    if (view) drawChanges();
  });

  // `:w` and `:q` run through the WeakMap rather than closing over this
  // component, so it is kept current with whichever callbacks the reader
  // passed in this render.
  $effect(() => {
    if (view) viewCallbacks.set(view, { onsave, onquit });
  });

  // Flipping the menu item reconfigures the live view in place; a state
  // rebuilt from scratch would drop the caret, the scroll position and the
  // undo history.
  $effect(() => {
    void keys;
    if (view) syncKeys(view);
  });

  // The completion and bracket switches work the same way.
  $effect(() => {
    void completion;
    void brackets;
    if (view) syncFeatures(view);
  });

  // So does the check, and turning it back on redraws the last render's marks
  // while turning it off clears them. Only this switch redraws: the marks on
  // screen have moved with every edit since the render, and drawing them again
  // from the compiler's lines would put them back where the text used to be.
  $effect(() => {
    void check;
    if (view) {
      syncFeatures(view);
      untrack(() => setDiagnostics(all));
    }
  });

</script>

<div class="editorhost" bind:this={host}></div>

<!-- A pending change is painted by `.change-added`, `.change-removed` and
     `.change-selected` in `styles/librepaper.css`, beside the rest of the
     review. -->
