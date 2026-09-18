<script module>
  // Vim is switched on for every file at once, held in one compartment per
  // view rather than baked into a file's state, so flipping the setting
  // reconfigures the live editor without dropping the caret, the scroll
  // position or the undo history.
  import { Compartment } from "@codemirror/state";
  import { undo as undoFn, redo as redoFn } from "../lib/loro-undo.js";
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
  function loadVim() {
    if (!vimPromise) {
      vimPromise = import("@replit/codemirror-vim").then(({ Vim, vim }) => {
        defineExCommands(Vim);
        resolvedVim = vim({ status: true });
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
    Vim.defineAction("loroUndo", (cm) => undoFn(cm.cm6));
    Vim.defineAction("loroRedo", (cm) => redoFn(cm.cm6));
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
  import { autocompletion, closeBrackets, closeBracketsKeymap, completionKeymap, startCompletion } from "@codemirror/autocomplete";
  import { searchKeymap, highlightSelectionMatches, openSearchPanel } from "@codemirror/search";
  import { syntaxHighlighting, HighlightStyle, defaultHighlightStyle, StreamLanguage, bracketMatching, foldGutter, foldKeymap, indentOnInput } from "@codemirror/language";
  import { markdown } from "@codemirror/lang-markdown";
  import { html as htmlLanguage } from "@codemirror/lang-html";
  import {
    lintGutter,
    lintKeymap,
    setDiagnostics as setLintDiagnostics,
    openLintPanel,
  } from "@codemirror/lint";
  import { LoroExtensions } from "../../vendor/loro-codemirror/index.ts";
  import { undoManagerField, undo as undoCommand, redo as redoCommand } from "../lib/loro-undo.js";
  import { UndoManager } from "loro-crdt";

  import { typstLanguage } from "../lib/typst-mode.js";
  import { analyzeBibliography } from "../lib/bibliography-engine.js";
  import { bibliographyCache, bibliographyCacheKey, bibliographyCompletion, bibliographyNeedsAnalysis, citationContext, planZoteroImport } from "../lib/bibliography.js";
  import { hasPairing, searchZotero, zoteroItem } from "../lib/companion/client.js";
  import { createProposals } from "../lib/proposals.js";
  import { proposalMarks, setProposalMarks } from "../lib/proposal-marks.js";
  import { DIRECTORY_ORIGIN } from "../lib/project-session.js";
  import { untrack } from "svelte";

  // Whether what is typed here goes into a proposal rather than into the paper
  // is not a prop. It is `startTracking`/`stopTracking`, and the answer lives
  // in the proposals API: a branch is opened the moment it is switched on and
  // the binding is rebuilt against that branch in the same call, before any
  // prop could have arrived (§3.3).
  let { session, format = "", file = "", keys = "default", editable = true,
    analyze = analyzeBibliography, send = null, onproposal = null,
    // What is being proposed for this document, and which hunk of it the
    // reviewer is looking at. `review` is the list the Reader keeps from
    // `proposal-list`, each entry `{id, author, hunks}` with hunks already in
    // this browser's UTF-16 basis; `reviewing` is `{proposal, hunk}` or null.
    //
    // These are drawn, never applied: a proposal reaches the document when
    // the server resolves it (§5.1a), and until then the text on screen still
    // says what it said.
    review = [], reviewing = null,
    onchange, oncaret, onfilechange, onbibliography, onsave, onquit } = $props();

  // One field instance for the component, because it is one editor: a state
  // field is identified by the object, so building a fresh one per file would
  // make each file's state carry a field the view's other states do not have.
  const proposalReview = proposalMarks();

  /// Redraw the proposals over whichever file is on screen.
  ///
  /// Everything is recomputed rather than mapped: a hunk's offsets are a claim
  /// about the proposal's base, and moving them through an edit would quietly
  /// turn them into a claim about something else. `proposal-marks.js` drops
  /// its decorations on any document change for the same reason, so this has
  /// to run again after one.
  function drawProposals() {
    if (!view) return;
    // This browser's own draft is drawn from the other basis. While one is
    // open the binding is attached to the branch, so what the author typed is
    // the text in front of them: their hunks, which are offsets into the base,
    // would strike through words that are no longer there and show every
    // insertion a second time. `draftMarks` reports the same branch against
    // the text on screen, which is what the author is actually reading.
    const drafting = Boolean(proposals?.drafting?.());
    const mine = drafting ? proposals.id() : "";
    view.dispatch({
      effects: setProposalMarks.of({
        proposals: mine ? (review || []).filter((one) => one.id !== mine) : review || [],
        showing,
        selected: reviewing,
        draft: drafting ? proposals.draftMarks() : [],
      }),
    });
  }

  let parsedBibliography = $state(null);
  const insertTargets = new Map();
  const undoManagers = new Map();

  // Proposals API for tracked editing. When tracking is on, the editor binds to
  // the proposal's text instead of the room document's text. The send function
  // is passed from the Reader and routes messages through the room socket.
  let proposals = null;
  $effect(() => {
    if (!send || !session) return;
    // Never over a branch somebody is typing into: replacing the API would
    // orphan the fork, which is still holding unflushed work and would have
    // nothing left to flush it.
    if (proposals?.drafting?.()) return;
    proposals = createProposals({
      session,
      send: (message) => send?.(message),
      mayEdit: editable,
    });
  });

  /// Rebind the editor to whichever text it should now be writing into.
  ///
  /// Switching tracking on or off changes which LoroText the binding is
  /// attached to -- the branch's copy of the file, or the room's -- and that
  /// is fixed when the state is built. So the state is thrown away and made
  /// again. Everything held in it goes with it: the caret, the scroll
  /// position and the undo history. That is the honest cost of the switch,
  /// because an undo history over one text cannot be replayed onto another.
  function rebind() {
    if (!view || !showing) return;
    const id = showing;
    states.delete(id);
    showing = "";
    show(id);
  }

  // A branch lives in this browser until it is flushed, and nothing that is
  // only here can be reviewed -- not by a coauthor, and not by the author,
  // whose own queue is drawn from what the server says is open. So typing
  // sends: after a pause long enough that a keystroke is not a round trip,
  // and short enough that the Changes panel keeps up with the sentence being
  // written.
  const PROPOSAL_FLUSH_MS = 400;
  let proposalFlushTimer = null;
  function scheduleProposalFlush() {
    if (!proposals?.drafting?.()) return;
    clearTimeout(proposalFlushTimer);
    proposalFlushTimer = setTimeout(() => {
      proposalFlushTimer = null;
      proposals?.flush?.();
    }, PROPOSAL_FLUSH_MS);
  }

  /// Whether a branch is open here, which is the only truthful answer to
  /// "is track changes on". The switch in the Changes panel reads it back
  /// rather than assuming it got what it asked for: without a socket to send
  /// a proposal on there is nothing to fork into, and a switch that shows on
  /// while every keystroke goes into the paper is the worst of both.
  export function trackingOn() {
    return Boolean(proposals?.drafting?.());
  }

  /// Start proposing rather than editing. Forks the document, and from here
  /// what is typed accumulates on the branch and is flushed to the server
  /// instead of reaching the paper.
  export function startTracking() {
    if (!proposals || proposals.drafting()) return;
    proposals.start();
    rebind();
  }

  /// Stop, leaving the branch with the server.
  ///
  /// What has been typed is not discarded and not applied: it is a proposal,
  /// and it waits for somebody to answer it (§5.1a). Anything not yet sent
  /// goes up before the local branch is let go.
  export function stopTracking() {
    if (!proposals || !proposals.drafting()) return;
    // Whatever the pause timer was about to send goes now, in this call,
    // rather than after the branch has been let go.
    clearTimeout(proposalFlushTimer);
    proposalFlushTimer = null;
    proposals.flush();
    // The manager is keyed by the document it takes back operations on, and
    // that document is about to be destroyed. Dropping it here keeps one
    // dead manager per tracking session from accumulating in the map.
    const branch = proposals.doc();
    proposals.stop();
    if (branch) undoManagers.delete(branch);
    rebind();
  }

  // When proposal messages arrive, apply them. The Reader's receive function
  // routes them here. On stale error, do not silently retry: the hunks may have
  // changed and the reviewer needs to recompute.
  export function receiveProposal(message) {
    if (!proposals) return;
    const error = message.type === "error" && message.stale;
    if (error) {
      // The proposal has moved on. Recompute and let the reviewer decide again.
      return;
    }
    proposals.apply?.(message);
  }

  let bibliographyGeneration = 0;
  let bibliographyTimer = null;
  let lastBibliographyKey = "";
  let zoteroSearchGeneration = 0;
  let lastZoteroError = "";
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

  async function remoteBibliography(query) {
    if (!hasPairing() || !["markdown", "quarto"].includes(formatOf(session?.paths?.get(showing)))) return [];
    const generation = ++zoteroSearchGeneration;
    await new Promise((resolve) => setTimeout(resolve, 180));
    if (generation !== zoteroSearchGeneration) return [];
    let response;
    try {
      response = await searchZotero(query);
      lastZoteroError = "";
    } catch (error) {
      const message = error?.message || String(error);
      if (message !== lastZoteroError) {
        lastZoteroError = message;
        onbibliography?.({ entries: bibliographyEntries(), diagnostics: [{ severity: "warning", message: `Zotero unavailable: ${message}`, file: session?.paths?.get(showing) || "", line: 0 }] }, bibliographyRequest());
      }
      return [];
    }
    if (generation !== zoteroSearchGeneration) return [];
    return (response?.entries || []).map((entry) => ({
      ...entry, key: entry.citation_key, year: entry.year || "", type: entry.item_type,
    }));
  }

  async function importZotero(entry, target) {
    if (!editable || !session || target.view !== view || !showing) return false;
    const active = session.textOf(showing);
    const fromUnicode = active.convertPos(target.from, "utf16", "unicode") ?? target.from;
    const toUnicode = active.convertPos(target.to, "utf16", "unicode") ?? target.to;
    // Reassigned below when the document moved while the item was fetched.
    let fromCursor = active.getCursor(fromUnicode, 0);
    let toCursor = active.getCursor(toUnicode, 0);
    const activeSession = session, activeFile = showing;
    try {
      const fetched = await zoteroItem(entry.zotero_item);
      if (session !== activeSession || showing !== activeFile || view !== target.view || !editable) return false;

      const fromResult = session.doc.getCursorPos(fromCursor);
      if (!fromResult) return false;
      if (fromResult.update) fromCursor = fromResult.update;
      const toResult = session.doc.getCursorPos(toCursor);
      if (!toResult) return false;
      if (toResult.update) toCursor = toResult.update;

      const start_utf16 = active.convertPos(fromResult.offset, "unicode", "utf16") ?? 0;
      const end_utf16 = active.convertPos(toResult.offset, "unicode", "utf16") ?? 0;

      const tree = session.tree();
      const mainPath = tree.main;
      const mainId = session.idOf(mainPath);
      const main = session.textOf(mainId);
      const plan = planZoteroImport({
        source: main?.toString() || "", mainPath, texts: tree.texts,
        item: { ...fetched, zotero_item: entry.zotero_item },
      });
      const bibId = session.idOf(plan.path);
      const bib = bibId ? session.textOf(bibId) : null;
      const changes = [{ from: Math.min(start_utf16, end_utf16), to: Math.max(start_utf16, end_utf16), insert: plan.key }];
      if (plan.registration && main === active) changes.push(plan.registration);
      changes.sort((a, b) => a.from - b.from || a.to - b.to);
      const setupShift = plan.registration && main === active && plan.registration.from <= Math.min(start_utf16, end_utf16)
        ? plan.registration.insert.length - (plan.registration.to - plan.registration.from) : 0;
      if (bib) {
        if (plan.addition) bib.insert(bib.toString().length, plan.addition);
      } else session.addText(plan.path, plan.bibtex);
      if (plan.registration && main && main !== active) {
        if (plan.registration.to > plan.registration.from) main.delete(plan.registration.from, plan.registration.to - plan.registration.from);
        main.insert(plan.registration.from, plan.registration.insert);
      }
      target.view.dispatch({ changes, selection: { anchor: Math.min(start_utf16, end_utf16) + setupShift + plan.key.length }, annotations: Transaction.userEvent.of("input.complete") });
      session.doc.commit();
      proposals?.flush?.();
      scheduleBibliography();
      return true;
    } catch (error) {
      onbibliography?.({ entries: bibliographyEntries(), diagnostics: [{ severity: "warning", message: `Zotero import failed: ${error?.message || error}`, file: session?.paths?.get(showing) || "", line: 0 }] }, bibliographyRequest());
      return false;
    }
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
  export function text(expectedFile) {
    if (expectedFile && showing !== expectedFile) return null;
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

  export function editAvailability() {
    if (!view) return {};
    const selected = !view.state.selection.main.empty;
    const manager = undoManagers.get(boundDoc());
    return {
      undo: editable && (manager?.canUndo?.() ?? false),
      redo: editable && (manager?.canRedo?.() ?? false),
      cut: editable && selected, copy: selected,
      paste: editable, "select-all": true, find: true, replace: editable,
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
    const loroText = session.textOf?.(showing) || session.text;
    if (!loroText) return null;
    const { from, to } = view.state.selection.main;
    // Convert UTF-16 offsets to Unicode code points for cursor creation.
    const fromUnicode = loroText.convertPos(from, "utf16", "unicode") ?? from;
    const toUnicode = loroText.convertPos(to, "utf16", "unicode") ?? to;
    return {
      session,
      file: showing,
      fromCursor: loroText.getCursor(fromUnicode, 0),
      toCursor: loroText.getCursor(toUnicode, 0),
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
  // snippet share the active editor's Loro transaction and undo entry.
  export function applyInsertResult(result, capturedContext = null) {
    if (!result || typeof result.text !== "string" || !view || !session || !editable) return false;
    const target = insertTargets.get(capturedContext?.targetId);
    if (!target || target.session !== session || target.file !== showing || session.paths.get(showing) !== capturedContext.path) return false;
    const loroText = session.textOf(showing);
    const doc = session.doc;

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
    const plans = new Map([[showingKey, { text: loroText, edits: [] }]]);
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
      const key = keyOf(fileText);
      if (editFrom == null || editTo == null || (key === showingKey && editFrom < to && editTo > from)) return false;
      if (!plans.has(key)) plans.set(key, { text: fileText, edits: [] });
      plans.get(key).edits.push({ from: editFrom, to: editTo, insert: edit.insert });
    }
    const changes = plans.get(showingKey).edits;
    const shift = changes.filter(edit => edit.to <= from).reduce((sum, edit) => sum + edit.insert.length - (edit.to - edit.from), 0);
    changes.push({ from, to, insert: result.text });
    for (const plan of plans.values()) {
      plan.edits.sort((a, b) => a.from - b.from || a.to - b.to);
      for (let i = 1; i < plan.edits.length; i++) if (plan.edits[i - 1].to > plan.edits[i].from) return false;
    }
    const selection = result.selection;
    const offset = value => from + shift + Math.max(0, Math.min(result.text.length, Number.isInteger(value) ? value : result.text.length));
    const anchor = offset(selection?.anchor), head = offset(selection?.head);
    view.dispatch({ changes, selection: { anchor, head }, effects: EditorView.scrollIntoView(head) });
    // Every other file the insertion touches. The one on screen went through
    // the transaction above; writing it here as well would apply it twice.
    for (const [key, plan] of plans) if (key !== showingKey) {
      for (const edit of [...plan.edits].reverse()) {
        if (edit.to > edit.from) plan.text.delete(edit.from, edit.to - edit.from);
        if (edit.insert) plan.text.insert(edit.from, edit.insert);
      }
    }
    session.doc.commit();
    proposals?.flush?.();
    releaseInsertContext(capturedContext);
    view.focus();
    return true;
  }

  /// What a document is written in decides how the *file being edited* is
  /// coloured, which is not always the document's own format: a `.bib` beside
  /// a `.tex` is neither LaTeX nor markdown, and colouring it as though it
  /// were is worse than colouring it as nothing.
  function languageOf(path, fallback) {
    const lower = (path || "").toLowerCase();
    if (lower.endsWith(".typ")) return language("typst");
    if (lower.endsWith(".md") || lower.endsWith(".markdown") || lower.endsWith(".qmd")) return language("markdown");
    if (lower.endsWith(".html") || lower.endsWith(".htm")) return language("html");
    // A file with no mode of its own -- .bib, .sty, .csv -- is shown as plain
    // text rather than coloured by the document's format, which would be a
    // guess dressed as knowledge.
    if (lower && !lower.endsWith(".txt")) return [];
    return language(fallback);
  }

  /// Builds the state for one file, bound to its own LoroText.
  /// When tracking is on, binds to the proposal's text; otherwise binds to the room's.
  function stateFor(id) {
    const text = getTextFromDoc(id);
    const path = session.paths?.get(id) || "";
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
    const bound = boundDoc();
    const undoManager = undoManagers.get(bound)
      || new UndoManager(bound, { excludeOriginPrefixes: [DIRECTORY_ORIGIN] });
    undoManagers.set(bound, undoManager);

    // Who the caret belongs to. The session publishes {name, color, tab}; the
    // binding wants {name, colorClassName}, and paints through a class rather
    // than a value. The palette in lib/collab.js is six fixed colours with a
    // rule apiece in the stylesheet, so the class is the colour with its hash
    // dropped. Somebody with no colour yet gets no class, and the stylesheet
    // falls back to a theme token rather than to a colour written down here.
    const who = session.ephemeral?.get("user");
    const userName = who?.name || "Anonymous";
    const colorClassName = typeof who?.color === "string" && who.color.startsWith("#")
      ? `user-color-${who.color.slice(1).toLowerCase()}`
      : "";

    return EditorState.create({
      doc: text.toString(),
      extensions: [
        EditorView.editable.of(Boolean(editable)),
        EditorView.contentAttributes.of({ "aria-label": path ? `Source of ${path}` : "Source" }),
        // Vim, when the setting says so, and always first: an earlier
        // extension has precedence, and Vim has to see a key before the
        // default keymap does, or `j` inserts a letter instead of moving.
        vimCompartment.of(resolvedKeys(untrack(() => keys)) || []),
        lineNumbers(),
        drawSelection(),
        highlightActiveLine(),
        highlightActiveLineGutter(),
        highlightSpecialChars(),
        dropCursor(),
        EditorState.allowMultipleSelections.of(true),
        rectangularSelection(),
        bracketMatching(),
        closeBrackets(),
        foldGutter(),
        indentOnInput(),
        highlightSelectionMatches(),
        syntaxHighlighting(sourceHighlightStyle, { fallback: true }),
        languageOf(path, format),
        autocompletion({
          activateOnTyping: true,
          override: [bibliographyCompletion({ entries: bibliographyEntries, format: () => formatOf(session?.paths?.get(showing)), remote: remoteBibliography, onRemote: importZotero })],
        }),
        // Where a compile's errors are shown: the gutter mark, and with it the
        // underline and the hover the lint extension draws.
        lintGutter(),
        EditorView.lineWrapping,
        // What somebody has proposed, drawn in the text it would change.
        proposalReview,
        keymap.of([
          // Everyone tries Ctrl/Cmd-S in an editor.
          { key: "Mod-s", preventDefault: true, run: () => (onsave?.(), true) },
          ...closeBracketsKeymap,
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
        undoManagerField.init(() => undoManager),
        Prec.highest(keymap.of([
          { key: "Mod-z", run: undoCommand, preventDefault: true },
          { key: "Mod-y", mac: "Mod-Shift-z", run: redoCommand, preventDefault: true },
          { key: "Mod-Shift-z", run: redoCommand, preventDefault: true },
        ])),
        // The shared document, and everyone else's cursors in it. The editor
        // is bound to one text at a time (the one in 'showing'), and the
        // getTextFromDoc function returns that text. Multiple editors may be
        // open on different files at once; they all share the same LoroDoc.
        LoroExtensions(bound, session.ephemeral && { user: { name: userName, colorClassName }, ephemeral: session.ephemeral }, undoManager, () => text),
        EditorView.updateListener.of((update) => {
          // A document change clears the proposal decorations, because the
          // offsets they were placed at have moved. Drawing them again from
          // the hunks is the only honest way back, and it has to happen after
          // the transaction rather than inside it.
          // While tracking is on this also redraws the author's own draft, which
          // every keystroke changes -- the marks are recomputed from the branch
          // rather than mapped, for the same reason.
          if (update.docChanged && ((review || []).length || proposals?.drafting?.())) queueMicrotask(drawProposals);
          if (update.docChanged) {
            onchange?.();
            // What was typed went into the branch, not the paper. Send it.
            scheduleProposalFlush();
          }
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

  // Get the text for a file, from either the branch being drafted or the room
  // document.
  //
  // The test is whether a branch is open, not what the `tracking` prop says.
  // `startTracking` forks and rebinds in one call, and the prop that reports
  // the switch has not reached this component by then -- reading it here bound
  // the editor to the room's text with tracking on, so every keystroke went
  // into the paper and the branch stayed empty. Which is what "track changes
  // records nothing" looked like.
  function getTextFromDoc(id) {
    if (proposals?.drafting?.()) {
      const proposalText = proposals.text(id);
      if (proposalText) return proposalText;
    }
    return session.textOf?.(id) || (id === session.mainId?.() ? session.text : null);
  }

  /// The document the binding belongs to: the branch while one is being
  /// drafted, the room's otherwise.
  ///
  /// The binding commits this document after writing into the text it was
  /// given, and applies to the view whatever arrives in it. Handing it the
  /// room's document while the text is the branch's leaves the branch's
  /// operations uncommitted and paints a coauthor's edit into a text that has
  /// not got it.
  function boundDoc() {
    return proposals?.drafting?.() ? proposals.doc() || session.doc : session.doc;
  }

  /// Shows a file, keeping the state of the one being left. The view is made
  /// once and re-stated, rather than destroyed and rebuilt, so that switching
  /// files does not flash.
  function show(id) {
    if (!view || !id || id === showing) return;
    if (showing) states.set(showing, view.state);
    if (!states.has(id)) states.set(id, stateFor(id));
    const state = states.get(id);
    const text = getTextFromDoc(id);
    // While a file is inactive its LoroText can still receive remote edits, but
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
    // A state carries no decorations until something puts them there, and this
    // is a fresh one -- including after `rebind`, where the file has not
    // changed and so nothing upstream would ask for a redraw.
    drawProposals();
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
      view = new EditorView({
        state: initial.state,
        parent: host,
        // No `dispatchTransactions` here. There was one, and all it did was
        // hand each edit to the deleted revision mechanism before applying
        // it; with that gone it called `view.update(transactions)`, which is
        // exactly what CodeMirror does when nothing is supplied. A proposal
        // is not intercepted on its way through the editor -- what is typed
        // goes into whichever text the binding is attached to, and which text
        // that is was decided when the state was built (see `getTextFromDoc`).
      });
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
        // A pause that the editor going away cut short is still work somebody
        // did, so it goes up on the way out rather than with the view.
        if (proposalFlushTimer) {
          clearTimeout(proposalFlushTimer);
          proposalFlushTimer = null;
          proposals?.flush?.();
        }
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

  // Draw the review whenever the list, the selection or the file on screen
  // changes. Showing another file is a swap of the view's state, and a state
  // carries no decorations until something puts them there.
  $effect(() => {
    void review;
    void reviewing;
    void file;
    if (view) drawProposals();
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

<!-- A proposal is painted by `.proposal-added`, `.proposal-removed` and
     `.proposal-selected` in `styles/librepaper.css`, beside the rest of the
     review. The rules that used to be here dressed the deleted revision
     mechanism, and nothing wore them. -->
