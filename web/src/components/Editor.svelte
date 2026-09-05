<script>
  // The source, in CodeMirror, shared with everyone else editing it.
  //
  // The binding is y-codemirror: the editor's document *is* the Yjs text, so
  // two people typing in the same sentence converge without either waiting for
  // the other, and each of them keeps their own caret, selection and undo
  // history. Everyone else's caret is drawn where they are, labelled with
  // their name.
  import { EditorState } from "@codemirror/state";
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

  let { session, format, onchange, oncaret, onsave } = $props();

  let host = $state(null);
  let view = null;

  // What a document is written in decides how it is coloured. Markdown has a
  // maintained mode; typst has the small one beside this file, which knows the
  // handful of things worth telling apart in a source you are editing.
  function language(format) {
    if (format === "typst") return StreamLanguage.define(typstLanguage);
    if (format === "html") return htmlLanguage();
    return markdown();
  }

  // What the compiler last said, in the editor's own coordinates, so
  // `nextDiagnostic` can walk them without asking for the list again.
  let shown = [];

  /// Paints what a compile had to say: an underline on the span it names, a
  /// mark in the gutter of that line, and the message and hints on hover.
  /// Called with an empty list to clear, which a successful render does.
  ///
  /// Lines and columns are one-based and count UTF-16 units, which is what
  /// CodeMirror counts in; a diagnostic with no span (line 0) has no place to
  /// be painted and is left to the badge and the panel.
  export function setDiagnostics(list) {
    if (!view) return;
    const doc = view.state.doc;
    const place = (line, column) => {
      const at = doc.line(Math.min(Math.max(line, 1), doc.lines));
      return Math.min(at.from + Math.max(column - 1, 0), at.to);
    };
    shown = (list || [])
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
  export function nextDiagnostic() {
    if (!view || shown.length === 0) return false;
    const at = view.state.selection.main.head;
    const next = shown.find((diagnostic) => diagnostic.from > at) || shown[0];
    goTo(next.from);
    return true;
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

  $effect(() => {
    if (!host || !session || view) return;
    const state = EditorState.create({
      doc: session.text.toString(),
      extensions: [
        lineNumbers(),
        history(),
        drawSelection(),
        highlightActiveLine(),
        highlightSelectionMatches(),
        syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
        language(format),
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
        // The shared document, and everyone else's cursors in it.
        yCollab(session.text, session.awareness),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) onchange?.();
          // Only a deliberate move counts: typing moves the caret constantly,
          // and scrolling the document on every keystroke would make the
          // preview unreadable.
          if (update.selectionSet && !update.docChanged) oncaret?.();
        }),
      ],
    });
    view = new EditorView({ state, parent: host });
    view.focus();
    return () => {
      view?.destroy();
      view = null;
    };
  });
</script>

<div class="editorhost" bind:this={host}></div>
