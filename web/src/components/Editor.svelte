<script module>
  // Vim is switched on for every file at once, held in one compartment per
  // view rather than baked into a file's state, so flipping the setting
  // reconfigures the live editor without dropping the caret, the scroll
  // position or the undo history -- and without tearing down `yCollab`.
  import { Compartment } from "@codemirror/state";
  import { yUndoManagerKeymap as vimUndoKeymap } from "y-codemirror.next";
  import { keymap } from "@codemirror/view";

  // The keys the editor answers to: Vim's, Emacs's, or nothing extra. One
  // compartment, whichever is chosen, so switching swaps one for the other.
  const vimCompartment = new Compartment();

  // Each package is fetched only once a browser asks for its keys, and every
  // editor on the page shares the one download and the one module. Once it
  // has resolved, `resolvedVim` (or `resolvedEmacs`) lets a later switch
  // reconfigure a compartment right away, with nothing to await.
  let vimPromise = null;
  let resolvedVim = null;
  let emacsPromise = null;
  let resolvedEmacs = null;
  // `getCM` is the only way to reach the vim-specific state (`insertMode`,
  // `visualMode`, ...) that `vimMode()` below reads; it comes from the same
  // module as `vim`/`Vim`, so it is captured alongside them rather than
  // imported at the top of the file, which would pull the package in before
  // anyone had asked for Vim keys.
  let resolvedGetCM = null;
  function loadVim() {
    if (!vimPromise) {
      vimPromise = import("@replit/codemirror-vim").then(({ Vim, vim, getCM }) => {
        defineExCommands(Vim);
        resolvedVim = vim({ status: true });
        resolvedGetCM = getCM;
        return resolvedVim;
      });
    }
    return vimPromise;
  }

  // Emacs undoes with C-/ (and C-_ and C-x u), which the package points at
  // CodeMirror's own history. Routed to the Yjs manager first, so an undo
  // stays this collaborator's, as Vim's `u` is above.
  function loadEmacs() {
    if (!emacsPromise) {
      emacsPromise = import("@replit/codemirror-emacs").then(({ emacs }) => {
        const undo = vimUndoKeymap[0].run;
        resolvedEmacs = [keymap.of([{ key: "Ctrl-/", run: undo }, { key: "Ctrl-_", run: undo }, { key: "Ctrl-x u", run: undo }]), emacs()];
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
    // Vim's built-in `u` and Ctrl-R use CodeMirror's native history. Route
    // them through the Yjs manager so they remain local to this collaborator.
    Vim.defineAction("yUndo", (cm) => vimUndoKeymap[0].run(cm.cm6));
    Vim.defineAction("yRedo", (cm) => vimUndoKeymap[1].run(cm.cm6));
    Vim.mapCommand("u", "action", "yUndo", {}, {});
    Vim.mapCommand("<C-r>", "action", "yRedo", {}, {});
  }
</script>

<script>
  // The source, in CodeMirror, shared with everyone else editing it.
  //
  // The binding is y-codemirror: the editor's document *is* the Yjs text, so
  // two people typing in the same sentence converge without either waiting for
  // the other, and each of them keeps their own caret, selection and undo
  // history. Everyone else's caret is drawn where they are, labelled with
  // their name.
  import { EditorState, Transaction } from "@codemirror/state";
  import { EditorView, lineNumbers, highlightActiveLine, drawSelection } from "@codemirror/view";
  import { defaultKeymap, indentWithTab } from "@codemirror/commands";
  import { autocompletion, completionKeymap, startCompletion } from "@codemirror/autocomplete";
  import { searchKeymap, highlightSelectionMatches } from "@codemirror/search";
  import { syntaxHighlighting, HighlightStyle, defaultHighlightStyle, StreamLanguage } from "@codemirror/language";
  import { markdown } from "@codemirror/lang-markdown";
  import { html as htmlLanguage } from "@codemirror/lang-html";
  import {
    lintGutter,
    lintKeymap,
    setDiagnostics as setLintDiagnostics,
    openLintPanel,
  } from "@codemirror/lint";
  import { yCollab, yUndoManagerKeymap, ySyncFacet } from "y-codemirror.next";
  import * as Y from "yjs";

  import { typstLanguage } from "../lib/typst-mode.js";
  import { analyzeBibliography } from "../lib/bibliography-engine.js";
  import { bibliographyCache, bibliographyCacheKey, bibliographyCompletion, bibliographyNeedsAnalysis, citationContext } from "../lib/bibliography.js";
  import { untrack } from "svelte";

  let { session, format = "", file = "", keys = "default", editable = true, analyze = analyzeBibliography, onchange, oncaret, onfilechange, onbibliography, onsave, onquit } = $props();

  let parsedBibliography = $state(null);
  const insertTargets = new Map();
  const undoManagers = new Map();
  let bibliographyGeneration = 0;
  let bibliographyTimer = null;
  let lastBibliographyKey = "";
  function formatOf(path) {
    const lower = String(path || "").toLowerCase();
    if (lower.endsWith(".typ")) return "typst";
    if (lower.endsWith(".md") || lower.endsWith(".markdown") || lower.endsWith(".qmd")) return "markdown";
    if (lower.endsWith(".tex") || lower.endsWith(".ltx")) return "latex";
    return "";
  }
  function bibliographyEntries() { return parsedBibliography?.entries || []; }
  function bibliographyRequest() {
    const tree = session?.tree?.() || { main: "", texts: {} };
    const id = showing || session?.mainId?.() || "";
    const main = session?.paths?.get(id) || tree.main || "";
    const activeFormat = formatOf(main);
    return { main, format: activeFormat, source: session?.textOf?.(id)?.toString?.() ?? tree.texts?.[main] ?? "", texts: tree.texts || {} };
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
      const currentFormat = formatOf(session?.paths?.get(showing));
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

  const sourceHighlightStyle = HighlightStyle.define(
    defaultHighlightStyle.specs.map((rule) =>
      rule.fontWeight === "bold" ? { ...rule, fontWeight: "600" } : rule,
    ),
  );

  let host = $state(null);
  let view = null;

  // One editor state per file, made the first time that file is opened and
  // kept afterwards. Switching files swaps the state rather than rebuilding
  // it, so a visit to another chapter costs nothing and leaves the undo
  // history and the scroll position where they were -- which is the whole
  // difference between a directory of files and one text somebody keeps
  // scrolling through.
  const states = new Map();
  // Which file the view is currently showing, so a swap can put the state it
  // is leaving back in the map before taking the next one.
  let showing = "";

  // What a document is written in decides how it is coloured. Markdown has a
  // maintained mode; typst has the small one beside this file, which knows the
  // handful of things worth telling apart in a source you are editing.
  function language(format) {
    if (format === "typst") return StreamLanguage.define(typstLanguage);
    if (format === "html") return htmlLanguage();
    if (format === "quarto") return markdown();
    return markdown();
  }

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
    if (!diagnostic.file) return session.mainId?.() || "";
    return session.idOf?.(diagnostic.file) || "";
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
    view.dispatch(setLintDiagnostics(view.state, shown));
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
  export function text() {
    return view ? view.state.doc.toString() : "";
  }

  /// Where the caret is, as an offset into that text.
  export function caret() {
    return view ? view.state.selection.main.head : 0;
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

  /// Dictation's landing spot (SPEC-dictation.md 4.7, docs/dictation-interfaces.md
  /// "targets.js"): replaces the main selection with `text` through an
  /// ordinary CodeMirror transaction, so collaborators, undo history, and
  /// track changes see it the same as a keystroke. Returns false rather than
  /// throwing when there is no view, since the service checks `alive()`
  /// first but a target can still race a teardown between that check and
  /// this call.
  export function insertAtCaret(text) {
    if (!view) return false;
    const { from, to } = view.state.selection.main;
    const at = from + text.length;
    view.dispatch({
      changes: { from, to, insert: text },
      selection: { anchor: at },
      effects: EditorView.scrollIntoView(at),
    });
    return true;
  }

  // Capture the target as Yjs relative positions. Unlike raw CodeMirror
  // offsets, these survive edits made by collaborators while an Insert dialog
  // is open. The session and file identity are retained so a late dialog
  // result cannot write into a different document.
  export function captureInsertTarget() {
    if (!view || !session || !showing || editable === false) return null;
    const ytext = session.textOf?.(showing) || session.text;
    if (!ytext?.doc) return null;
    const { from, to } = view.state.selection.main;
    return {
      session,
      file: showing,
      from: Y.createRelativePositionFromTypeIndex(ytext, from),
      to: Y.createRelativePositionFromTypeIndex(ytext, to),
      capturedText: view.state.sliceDoc(from, to),
    };
  }

  // Context consumed by InsertMenu's format-aware generators. `target` is an
  // implementation detail used by applyInsertResult and intentionally lives
  // alongside the serializable context rather than in the menu itself.
  export function getInsertContext() {
    const target = captureInsertTarget();
    if (!target || !view) return null;
    const path = session.paths?.get(showing) || "";
    const lower = path.toLowerCase();
    const insertFormat = lower.endsWith(".qmd") ? "quarto" : formatOf(path) || (!path ? format : "");
    const tree = session.tree?.() || { main: "", texts: {} };
    const mainPath = tree.main || session.mainPath?.() || path;
    const mainText = session.textOf?.(session.idOf?.(mainPath))?.toString?.() || tree.texts?.[mainPath] || "";
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
        insertTargets.clear();
        insertTargets.set(id, target);
        return id;
      })(),
    };
  }

  export function releaseInsertContext(context) {
    if (context?.targetId) insertTargets.delete(context.targetId);
  }

  // Validate every change before touching shared text. Main-file setup and the
  // snippet share the active editor's Yjs transaction and undo entry.
  export function applyInsertResult(result, capturedContext = null) {
    if (!result || typeof result.text !== "string" || !view || !session || !editable) return false;
    const target = insertTargets.get(capturedContext?.targetId);
    if (!target || target.session !== session || target.file !== showing || session.paths.get(showing) !== capturedContext.path) return false;
    const ytext = session.textOf(showing);
    const start = Y.createAbsolutePositionFromRelativePosition(target.from, session.doc);
    const end = Y.createAbsolutePositionFromRelativePosition(target.to, session.doc);
    if (!start || !end || start.type !== ytext || end.type !== ytext) return false;
    const from = Math.min(start.index, end.index), to = Math.max(start.index, end.index);
    if (ytext.toString().slice(from, to) !== target.capturedText) return false;
    const plans = new Map([[ytext, []]]);
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
      const fileText = session.textOf(session.idOf(path));
      const snapshot = path === capturedContext.path ? capturedContext.text : capturedContext.files?.find(file => file.path === path)?.text;
      if (!fileText || typeof snapshot !== "string" || edit.from < 0 || edit.to < edit.from || edit.to > snapshot.length) return false;
      const editFrom = mapOffset(edit.from, snapshot, fileText.toString()), editTo = mapOffset(edit.to, snapshot, fileText.toString());
      if (editFrom == null || editTo == null || (fileText === ytext && editFrom < to && editTo > from)) return false;
      if (!plans.has(fileText)) plans.set(fileText, []);
      plans.get(fileText).push({ from: editFrom, to: editTo, insert: edit.insert });
    }
    const changes = plans.get(ytext);
    const shift = changes.filter(edit => edit.to <= from).reduce((sum, edit) => sum + edit.insert.length - (edit.to - edit.from), 0);
    changes.push({ from, to, insert: result.text });
    for (const edits of plans.values()) {
      edits.sort((a, b) => a.from - b.from || a.to - b.to);
      for (let i = 1; i < edits.length; i++) if (edits[i - 1].to > edits[i].from) return false;
    }
    const selection = result.selection;
    const offset = value => from + shift + Math.max(0, Math.min(result.text.length, Number.isInteger(value) ? value : result.text.length));
    const anchor = offset(selection?.anchor), head = offset(selection?.head);
    const manager = undoManagers.get(ytext);
    for (const text of plans.keys()) manager.addToScope(text);
    manager.stopCapturing();
    session.doc.transact(() => {
      view.dispatch({ changes, selection: { anchor, head }, effects: EditorView.scrollIntoView(head) });
      for (const [text, edits] of plans) if (text !== ytext) {
        for (const edit of [...edits].reverse()) {
          if (edit.to > edit.from) text.delete(edit.from, edit.to - edit.from);
          if (edit.insert) text.insert(edit.from, edit.insert);
        }
      }
    }, view.state.facet(ySyncFacet));
    manager.stopCapturing();
    releaseInsertContext(capturedContext);
    view.focus();
    return true;
  }

  /// What `assemble()` looks at to decide spacing and capitalization for the
  /// next dictated segment (SPEC-dictation.md 4.6): the document text just
  /// before the caret, capped so a huge file does not get copied on every
  /// segment.
  export function textBeforeCaret(limit = 200) {
    if (!view) return "";
    const head = view.state.selection.main.head;
    return view.state.sliceDoc(Math.max(0, head - limit), head);
  }

  /// `null` when Vim keys are not the live compartment's content -- either
  /// nobody has turned Vim on, or the package is still downloading -- and
  /// otherwise "insert" or "normal" from the vim state's own `insertMode`
  /// flag. Visual and replace read as "normal" here: SPEC 6 only cares
  /// whether typed characters land as text, and only insert mode does that.
  export function vimMode() {
    if (!view || !resolvedGetCM) return null;
    const cm = resolvedGetCM(view);
    const vim = cm?.state?.vim;
    if (!vim) return null;
    return vim.insertMode ? "insert" : "normal";
  }

  /// Whether this editor is still a live target: the service checks this
  /// before every insertion (SPEC 4.7) so a pane closed mid-dictation drops
  /// the text with a toast instead of writing nowhere.
  export function alive() {
    return !!view && !!view.dom && view.dom.isConnected;
  }

  /// Lets the reader map a focused element back to this editor instance
  /// (targets.js's `editorFor`) without reaching into `host` itself.
  export function contains(element) {
    return !!view && !!element && view.dom.contains(element);
  }

  /// What a document is written in decides how the *file being edited* is
  /// coloured, which is not always the document's own format: a `.bib` beside
  /// a `.tex` is neither LaTeX nor markdown, and colouring it as though it
  /// were is worse than colouring it as nothing.
  function languageOf(path, fallback) {
    const lower = (path || "").toLowerCase();
    if (lower.endsWith(".typ")) return language("typst");
    if (lower.endsWith(".md") || lower.endsWith(".markdown")) return language("markdown");
    if (lower.endsWith(".html") || lower.endsWith(".htm")) return language("html");
    // A file with no mode of its own -- .bib, .sty, .csv -- is shown as plain
    // text rather than coloured by the document's format, which would be a
    // guess dressed as knowledge.
    if (lower && !lower.endsWith(".txt")) return [];
    return language(fallback);
  }

  /// Builds the state for one file, bound to its own `Y.Text`.
  function stateFor(id) {
    const text = session.textOf?.(id) || session.text;
    const path = session.paths?.get(id) || "";
    const undoManager = undoManagers.get(text) || new Y.UndoManager(text);
    undoManagers.set(text, undoManager);
    return EditorState.create({
      doc: text.toString(),
      extensions: [
        // Vim, when the setting says so, and always first: an earlier
        // extension has precedence, and Vim has to see a key before the
        // default keymap does, or `j` inserts a letter instead of moving.
        vimCompartment.of(resolvedKeys(untrack(() => keys)) || []),
        lineNumbers(),
        drawSelection(),
        highlightActiveLine(),
        highlightSelectionMatches(),
        syntaxHighlighting(sourceHighlightStyle, { fallback: true }),
        languageOf(path, format),
        autocompletion({
          activateOnTyping: true,
          override: [bibliographyCompletion({ entries: bibliographyEntries, format: () => formatOf(session?.paths?.get(showing)) })],
        }),
        // Where a compile's errors are shown: the gutter mark, and with it the
        // underline and the hover the lint extension draws.
        lintGutter(),
        EditorView.lineWrapping,
        keymap.of([
          // Everyone tries Ctrl/Cmd-S in an editor.
          { key: "Mod-s", preventDefault: true, run: () => (onsave?.(), true) },
          indentWithTab,
          ...completionKeymap,
          ...defaultKeymap,
          ...yUndoManagerKeymap,
          ...searchKeymap,
          // Ctrl-Shift-M opens the list of what the compiler said.
          ...lintKeymap,
        ]),
        // The shared document, and everyone else's cursors in it. The
        // positions y-codemirror publishes are relative to the text they were
        // made in, so a caret already paints only in the file it is in; what
        // says *which* file that is, for the file list, is the awareness field
        // the reader sets beside it.
        yCollab(text, session.awareness, { undoManager }),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) onchange?.();
          // Only a deliberate move counts: typing moves the caret constantly,
          // and scrolling the document on every keystroke would make the
          // preview unreadable.
          if (update.selectionSet && !update.docChanged) oncaret?.();
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

  /// Shows a file, keeping the state of the one being left. The view is made
  /// once and re-stated, rather than destroyed and rebuilt, so that switching
  /// files does not flash.
  function show(id) {
    if (!view || !id || id === showing) return;
    if (showing) states.set(showing, view.state);
    if (!states.has(id)) states.set(id, stateFor(id));
    const state = states.get(id);
    const text = session.textOf?.(id) || (id === session.mainId?.() ? session.text : null);
    // While a file is inactive its Y.Text can still receive remote edits, but
    // its CodeMirror binding is not mounted to dispatch those edits. Reconcile
    // the cached state before showing it, preserving its history and selection.
    if (text && state.doc.toString() !== text.toString()) {
      const old = state.doc.toString();
      const next = text.toString();
      let from = 0;
      while (from < old.length && from < next.length && old.charCodeAt(from) === next.charCodeAt(from)) from++;
      let oldEnd = old.length;
      let nextEnd = next.length;
      while (oldEnd > from && nextEnd > from && old.charCodeAt(oldEnd - 1) === next.charCodeAt(nextEnd - 1)) {
        oldEnd--;
        nextEnd--;
      }
      states.set(id, state.update({
        changes: { from, to: oldEnd, insert: next.slice(from, nextEnd) },
        annotations: Transaction.addToHistory.of(false),
      }).state);
    }
    view.setState(states.get(id));
    syncKeys(view);
    showing = id;
    // A file this browser has open is where its caret is, which is what the
    // file list shows beside each name.
    session.inFile?.(id);
    onfilechange?.(id);
    // The marks belong to files, so they are drawn again for the file now on
    // screen. What the compiler said has not changed; where it applies has.
    setDiagnostics(all);
    view.focus();
  }

  /// The file the view is showing, which the reader asks when it has a
  /// diagnostic to place or a caret to follow.
  export function openFile() {
    return showing;
  }

  $effect(() => {
    if (!host || !session || view) return;
    // Creating the editor calls reader callbacks that read and update UI
    // state. Only host/session own this lifecycle; incidental callback reads
    // must not recreate the editor when a pane or diagnostic changes.
    return untrack(() => {
      const initial = untrack(() => {
        const first = file || session.mainId?.() || "";
        showing = first;
        if (first && !states.has(first)) states.set(first, stateFor(first));
        return { first, state: states.get(first) || stateFor(first) };
      });
      view = new EditorView({ state: initial.state, parent: host });
      untrack(() => viewCallbacks.set(view, { onsave, onquit }));
      syncKeys(view);
      if (initial.first) session.inFile?.(initial.first);
      const bibliographyWatcher = scheduleBibliography;
      const unsubscribeBibliography = session.onFiles?.(bibliographyWatcher);
      refreshBibliography();
      view.focus();
      return () => {
        bibliographyGeneration += 1;
        clearTimeout(bibliographyTimer);
        if (typeof unsubscribeBibliography === "function") unsubscribeBibliography();
        if (view) viewCallbacks.delete(view);
        view?.destroy();
        insertTargets.clear();
        undoManagers.clear();
        view = null;
        states.clear();
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
</script>

<div class="editorhost" bind:this={host}></div>
