<script>
  // The source, in CodeMirror, shared with everyone else editing it.
  //
  // The binding is y-codemirror: the editor's document *is* the Yjs text, so
  // two people typing in the same sentence converge without either waiting for
  // the other, and each of them keeps their own caret, selection and undo
  // history. Everyone else's caret is drawn where they are, labelled with
  // their name.
  import { EditorState, Transaction } from "@codemirror/state";
  import { EditorView, keymap, lineNumbers, highlightActiveLine, drawSelection } from "@codemirror/view";
  import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
  import { searchKeymap, highlightSelectionMatches } from "@codemirror/search";
  import { syntaxHighlighting, defaultHighlightStyle, StreamLanguage } from "@codemirror/language";
  import { markdown } from "@codemirror/lang-markdown";
  import { html as htmlLanguage } from "@codemirror/lang-html";
  import {
    lintGutter,
    lintKeymap,
    setDiagnostics as setLintDiagnostics,
    openLintPanel,
  } from "@codemirror/lint";
  import { yCollab } from "y-codemirror.next";

  import { typstLanguage } from "../lib/typst-mode.js";
  import { untrack } from "svelte";

  let { session, format, file = "", onchange, oncaret, onfilechange, onsave } = $props();

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
    return EditorState.create({
      doc: text.toString(),
      extensions: [
        lineNumbers(),
        history(),
        drawSelection(),
        highlightActiveLine(),
        highlightSelectionMatches(),
        syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
        languageOf(path, format),
        // Where a compile's errors are shown: the gutter mark, and with it the
        // underline and the hover the lint extension draws.
        lintGutter(),
        EditorView.lineWrapping,
        keymap.of([
          // Everyone tries Ctrl/Cmd-S in an editor.
          { key: "Mod-s", preventDefault: true, run: () => (onsave?.(), true) },
          indentWithTab,
          ...defaultKeymap,
          ...historyKeymap,
          ...searchKeymap,
          // Ctrl-Shift-M opens the list of what the compiler said.
          ...lintKeymap,
        ]),
        // The shared document, and everyone else's cursors in it. The
        // positions y-codemirror publishes are relative to the text they were
        // made in, so a caret already paints only in the file it is in; what
        // says *which* file that is, for the file list, is the awareness field
        // the reader sets beside it.
        yCollab(text, session.awareness),
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
    const initial = untrack(() => {
      const first = file || session.mainId?.() || "";
      showing = first;
      if (first && !states.has(first)) states.set(first, stateFor(first));
      return { first, state: states.get(first) || stateFor(first) };
    });
    view = new EditorView({ state: initial.state, parent: host });
    if (initial.first) session.inFile?.(initial.first);
    view.focus();
    return () => {
      view?.destroy();
      view = null;
      states.clear();
      showing = "";
    };
  });

  // Opening another file is a swap, not a rebuild. Kept out of the effect
  // above so that creating the view and changing which file it shows are two
  // separate things: the first happens once, the second whenever somebody
  // chooses a name in the list.
  $effect(() => {
    if (view && file) show(file);
  });
</script>

<div class="editorhost" bind:this={host}></div>
