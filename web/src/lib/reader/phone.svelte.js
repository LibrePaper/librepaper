// The phone's shell: the layers it opens over the document (the More sheet,
// the command lists, a panel, the settings), the header's title, the bars
// around the workspace, and what each row of the More sheet and each command
// list does.
//
// The state these layers live in belongs to the arrangement (`moreOpen`,
// `mobileCommand`, `keyboardOpen`, the phone's view); what is private here is
// the Insert list, which is read when it opens. Everything else the shell
// needs from the page (the panels on offer, the role, the source, the outputs
// and the actions it routes to) comes in as lazy callbacks, so that the owner
// can be made before they exist and driven without a page. The module holds no
// effect: the page keeps the four that tie the shell to the browser (the
// history, the resize to desktop, the keyboard, the Insert release) and calls
// the methods here from them.

import { m } from "../paraglide/messages.js";
import { basename } from "../file-manager.js";
import { fileCommands, editCommands, viewCommands } from "../mobile/commands.js";
import { DOCUMENTATION_URL } from "../site-links.js";

// The toggles stay on screen after a press, so the reader sees the tick move.
const STAYS_OPEN = ["linked", "visual-markup", "run-code", "format-html", "format-pdf", "text-bigger", "text-smaller", "text-reset"];

// A panel is listed only where the reader is offered it, so a link holder's sheet is
// as short as their rail was.
const PANEL_ICONS = { files: "folder", outline: "list-tree", history: "history", changes: "file-pen-line",
  agent: "bot", git: "git-branch", diagnostics: "triangle-alert", share: "users" };

/**
 * @param {{
 *   arrangement: any,
 *   comparing?: () => string,
 *   comparisonSays?: () => string,
 *   settingsOpen?: () => boolean,
 *   closeSettings?: () => void,
 *   endComparison?: () => void,
 *   barHidden?: () => boolean,
 *   tabs?: () => any[],
 *   badges?: () => Record<string, any>,
 *   participants?: () => any[],
 *   role?: () => { mayEdit: boolean, suggestOnly: boolean, mayChat: boolean, me: any },
 *   title?: () => string,
 *   source?: () => { format: string, compilesHere: boolean, toolbarPath: string, editing: boolean, editorReady: boolean, figureShown: boolean },
 *   demo?: () => { on: boolean, hint: string },
 *   hasDesktop?: () => boolean,
 *   exportable?: () => any,
 *   canExportComments?: () => boolean,
 *   can?: () => any,
 *   editGroups?: () => any,
 *   outputs?: () => { displayedFormat: string, latexOutput: string, typstOutput: string, markdownOutput: string },
 *   view?: () => { linked: boolean, visual: boolean, runCodeOffered: boolean, localExecution: boolean },
 *   insertMenu?: () => any,
 *   refreshEditAvailability?: () => void,
 *   chooseFileCommand?: (id: string) => unknown,
 *   chooseEditCommand?: (id: string) => unknown,
 *   chooseViewCommand?: (id: string) => unknown,
 *   toggleLocalExecution?: () => void,
 *   openSettings?: () => unknown,
 *   runCommand?: (id: string) => unknown,
 *   signOut?: () => unknown,
 *   signInHref?: () => string,
 *   openPanel?: (name: string) => unknown,
 *   setCollaborationTab?: (tab: string) => void,
 *   setTimer?: (run: () => void, ms: number) => unknown,
 * }} options
 */
export function createPhone({
  // The `createArrangement` instance: the overlays and the phone's view.
  arrangement,
  // The kind of comparison showing in place of the source, or "".
  comparing = () => "",
  // What the header calls the comparison.
  comparisonSays = () => "",
  // Whether the settings dialog is open, and closing it.
  settingsOpen = () => false,
  closeSettings = () => {},
  // Ends the comparison.
  endComparison = () => {},
  // Whether the top bar is scrolled out of sight.
  barHidden = () => false,
  // The panels the reader is offered, their badges, and who is in the project.
  tabs = () => [],
  badges = () => ({}),
  participants = () => [],
  // What the reader may do, and who they are.
  role = () => ({ mayEdit: false, suggestOnly: false, mayChat: false, me: {} }),
  // The document's title.
  title = () => "",
  // The source: its format, whether it compiles here, the open file's path,
  // whether the reader edits, whether the editor is there, and whether a
  // figure is open.
  source = () => ({ format: "", compilesHere: false, toolbarPath: "", editing: false, editorReady: false, figureShown: false }),
  // Whether this is the demo, and what it says beside what it turns off.
  demo = () => ({ on: false, hint: "" }),
  hasDesktop = () => false,
  exportable = () => ({}),
  canExportComments = () => false,
  can = () => ({}),
  editGroups = () => [],
  // The format on screen and what each format builds to.
  outputs = () => ({ displayedFormat: "", latexOutput: "", typstOutput: "", markdownOutput: "" }),
  // The View list's toggles.
  view = () => ({ linked: false, visual: false, runCodeOffered: false, localExecution: false }),
  // The Insert menu's component instance.
  insertMenu = () => null,
  // The actions the lists and rows route to.
  refreshEditAvailability = () => {},
  chooseFileCommand = (_id) => {},
  chooseEditCommand = (_id) => {},
  chooseViewCommand = (_id) => {},
  toggleLocalExecution = () => {},
  openSettings = () => {},
  runCommand = (_id) => {},
  signOut = () => {},
  signInHref = () => "",
  openPanel = (_name) => {},
  setCollaborationTab = (_tab) => {},
  // Runs `run` after `ms`.
  setTimer = (run, ms) => setTimeout(run, ms),
}) {
  // The Insert list is read when it opens, because reading it takes the
  // editor's insertion context, which a derived value must not do.
  let insertGroups = $state.raw([]);

  // How many layers are open over the document, for the browser's Back
  // button: a panel, a command list or the settings is one; the document being
  // compared inside a version is a second; the More sheet is one more on top
  // of whichever of those is open. Back closes only the top layer.
  const overlayView = $derived(arrangement.compact && (Boolean(arrangement.state.mobileCommand) || settingsOpen() || arrangement.activeMobileView === "sidebar"
    || (comparing() !== "" && arrangement.activeMobileView === "source")));
  const overlayDepth = $derived(!arrangement.compact ? 0
    : (comparing() !== "" && arrangement.activeMobileView === "source" ? 2 : overlayView ? 1 : 0) + (arrangement.state.moreOpen ? 1 : 0));

  // The history's Back button closed one layer, `depth` being what is left.
  function popped(depth) {
    if (arrangement.state.moreOpen) arrangement.state.moreOpen = false;
    else if (depth >= 1 && comparing()) arrangement.showMobileView("sidebar");
    else closeOverlays();
  }

  // Back out of the topmost layer, and leave `panel` set: panels stay mounted
  // so their scroll and drafts survive, and only a different panel replaces it.
  function closeOverlays() {
    arrangement.state.moreOpen = false;
    arrangement.state.mobileCommand = "";
    closeSettings();
    if (arrangement.activeMobileView === "sidebar") {
      if (arrangement.state.panel === "history" && comparing()) endComparison();
      arrangement.showMobileView(arrangement.state.preferredPane === "source" && arrangement.sourceOpen ? "source" : "document");
    }
  }
  function back() {
    if (comparing() && arrangement.activeMobileView === "source") arrangement.showMobileView("sidebar");
    else closeOverlays();
  }

  const mobileCommandTitles = $derived({
    file: m.common_file(), edit: m.mobilenav_edit(), view: m.mobilenav_view(), insert: m.common_insert(),
  });
  // What the header calls the place the reader is in.
  const mobileTitle = $derived.by(() => (
    arrangement.state.mobileCommand ? mobileCommandTitles[arrangement.state.mobileCommand]
    : arrangement.activeMobileView === "sidebar" ? (arrangement.state.panel === "collaboration" ? m.common_comments() : tabs().find((tab) => tab.id === arrangement.state.panel)?.says || "")
    : comparing() ? comparisonSays()
    : source().toolbarPath ? basename(source().toolbarPath) : title() || "LibrePaper"
  ));

  // The Source / Document switch is for a project with a source to switch to,
  // and is not offered over a panel or a command list.
  const modeSwitchShown = $derived(arrangement.sourceOpen && !comparing() && !arrangement.state.mobileCommand && !arrangement.state.keyboardOpen && arrangement.activeMobileView !== "sidebar");
  const keyboardBarShown = $derived(arrangement.state.keyboardOpen && arrangement.shown.source && role().mayEdit && !comparing() && !arrangement.state.mobileCommand);
  // Everything the phone adds around the workspace, summed for its height.
  const phoneChrome = $derived.by(() => (arrangement.compact ? [
    tabsShown ? "env(safe-area-inset-top)" : "",
    modeSwitchShown && !barHidden() ? "2.75rem" : "",
    tabBarShown && !barHidden() ? "3.5rem + env(safe-area-inset-bottom)" : "",
    (!tabBarShown || barHidden()) && !arrangement.state.keyboardOpen ? "env(safe-area-inset-bottom)" : "",
    keyboardBarShown ? "2.75rem" : "",
    arrangement.state.keyboardOpen ? "var(--librepaper-keyboard, 0px)" : "",
  ].filter(Boolean).join(" + ") || "0px" : "0px"));

  function openCommand(kind) {
    if (kind === "edit") refreshEditAvailability();
    if (kind === "insert") insertGroups = insertMenu()?.mobileGroups?.() || [];
    arrangement.state.moreOpen = false;
    arrangement.state.mobileCommand = kind;
  }
  // An Insert list left without a choice lets go of the context it took.
  let insertChosen = false;
  let lastCommand = "";
  // The page's effect calls this with the command now open, untracked.
  function commandChanged(now) {
    if (lastCommand === "insert" && now !== "insert" && !insertChosen) insertMenu()?.releaseMobile?.();
    insertChosen = false;
    lastCommand = now;
  }

  const outputFormat = $derived(outputs().displayedFormat === "latex" ? outputs().latexOutput : outputs().displayedFormat === "typst" ? outputs().typstOutput
    : ["markdown", "quarto"].includes(outputs().displayedFormat) ? outputs().markdownOutput : "html");
  const outputSelectable = $derived(["latex", "typst", "markdown", "quarto"].includes(outputs().displayedFormat));
  const mobileCommandGroups = $derived.by(() => (
    arrangement.state.mobileCommand === "file" ? fileCommands({
      mayEdit: role().mayEdit, suggestOnly: role().suggestOnly, demo: demo().on, demoHint: demo().hint, signedIn: Boolean(role().me?.id),
      sourceFormatLatex: source().format === "latex", compilesHere: source().compilesHere, desktop: hasDesktop(),
      exportable: exportable(), canExportComments: canExportComments() && !demo().on, hasOpenFile: Boolean(source().toolbarPath),
    })
    : arrangement.state.mobileCommand === "edit" ? editCommands(editGroups(), can())
    : arrangement.state.mobileCommand === "view" ? viewCommands({
      can: can(), selectedFormat: outputFormat, selectableFormat: outputSelectable, linked: view().linked, visual: view().visual,
      runCodeOffered: view().runCodeOffered, localExecution: view().localExecution,
    })
    : arrangement.state.mobileCommand === "insert" ? insertGroups
    : []
  ));
  function chooseCommand(id) {
    const kind = arrangement.state.mobileCommand;
    if (kind === "insert") insertChosen = true;
    arrangement.state.mobileCommand = "";
    if (kind === "file") return void chooseFileCommand(id);
    if (kind === "edit") return void chooseEditCommand(id);
    if (kind === "insert") {
      arrangement.showMobileView("source");
      insertMenu()?.insertAction?.(id);
      return;
    }
    if (kind === "view") {
      if (id === "run-code") toggleLocalExecution();
      else chooseViewCommand(id);
      if (STAYS_OPEN.includes(id)) arrangement.state.mobileCommand = "view";
    }
  }

  // The More sheet: everything the tab bar has no room for.
  function panelRow(id) {
    const tab = tabs().find((item) => item.id === id);
    if (!tab) return null;
    const total = (badges()[id]?.counts || []).reduce((sum, item) => sum + item.of, 0);
    return {
      id, icon: PANEL_ICONS[id] || tab.icon,
      label: tab.disabled ? m.common_demo_unavailable({ says: tab.says }) : tab.says,
      disabled: tab.disabled || undefined, dot: badges()[id]?.dot || undefined, count: total || undefined,
    };
  }
  // The More sheet has three sections, as the desktop does: the menus (File,
  // Edit, View, Insert, as on the menubar), then the panels in rail order
  // with Collaborators last, then the application rows (settings, projects,
  // help, sign in or out, as in the account menu). Sections have no headings;
  // a thin divider separates them.
  const moreSections = $derived.by(() => {
    // Files and Comments are header tabs, not panels, so they are not listed here.
    const panelRows = tabs()
      .filter((tab) => tab.id !== "files" && tab.id !== "collaboration")
      .map((tab) => panelRow(tab.id))
      .filter(Boolean);
    const menuRows = [
      { id: "command:file", icon: "file", label: m.common_file(), submenu: true },
      ...(source().editing && role().mayEdit ? [
        { id: "command:edit", icon: "pencil", label: m.mobilenav_edit(), disabled: !source().editorReady || !!source().figureShown || undefined, submenu: true },
      ] : []),
      ...(source().editing ? [{ id: "command:view", icon: "eye", label: m.mobilenav_view(), submenu: true }] : []),
      ...(source().editing && role().mayEdit ? [
        { id: "command:insert", icon: "plus", label: m.common_insert(), disabled: !source().editorReady || undefined, submenu: true },
      ] : []),
    ];
    const collaborationRows = tabs().some((tab) => tab.id === "collaboration") ? [
      { id: "collaborators", icon: "users", label: m.mobilenav_collaborators(), count: participants().length > 1 ? participants().length : undefined },
    ] : [];
    return [
      { rows: menuRows },
      { rows: [...panelRows, ...collaborationRows] },
      { rows: [
        { id: "settings", icon: "settings", label: m.common_settings() },
        { id: "palette", icon: "search", label: m.common_command_palette() },
        { id: "home", icon: "house", label: m.common_projects(), href: "/" },
        { id: "help", icon: "circle-help", label: m.mobilenav_help(), href: DOCUMENTATION_URL, external: true },
        ...(role().me.name ? [{ id: "signout", icon: "log-out", label: m.common_sign_out() }]
          : role().me.providers?.length ? [{ id: "signin", icon: "log-in", label: m.common_sign_in(), href: signInHref() }] : []),
      ] },
    ].filter((section) => section.rows.length);
  });
  function chooseMoreRow(id) {
    // The sheet has finished closing by the time the settings dialog, which
    // traps focus too, is opened.
    if (id === "settings") setTimer(() => openSettings(), 0);
    else if (id === "palette") setTimer(() => void runCommand("palette"), 0);
    else if (id === "signout") void signOut();
    else if (id === "home" || id === "help" || id === "signin") return;
    else if (id === "collaborators") showCollaborators();
    else if (id.startsWith("command:")) openCommand(id.slice("command:".length));
    else void openPanel(id);
  }

  // The bottom tab bar: Document, Files and Comments (and More).
  const tabsShown = $derived(arrangement.compact && !arrangement.state.mobileCommand && !comparing()
    && (arrangement.activeMobileView !== "sidebar" || arrangement.state.panel === "files" || arrangement.state.panel === "collaboration"));
  const tabActive = $derived(arrangement.activeMobileView === "sidebar" && arrangement.state.panel === "files" ? "files"
    : arrangement.activeMobileView === "sidebar" && arrangement.state.panel === "collaboration" ? "comments" : "document");
  // The bar hides while the keyboard is up; the keyboard bar takes its place.
  const tabBarShown = $derived(tabsShown && !arrangement.state.keyboardOpen);
  function showDocument() {
    arrangement.state.mobileCommand = "";
    arrangement.showMobileView(arrangement.state.preferredPane === "source" && arrangement.sourceOpen ? "source" : "document");
  }
  function showComments() {
    setCollaborationTab("comments");
    void openPanel("collaboration");
  }
  // The header's Collaborators button: the live conversation where the reader
  // may take part in it, the comments otherwise.
  function showCollaborators() {
    setCollaborationTab(role().mayChat ? "chat" : "comments");
    void openPanel("collaboration");
  }

  return {
    get overlayView() { return overlayView; },
    get overlayDepth() { return overlayDepth; },
    get mobileTitle() { return mobileTitle; },
    get modeSwitchShown() { return modeSwitchShown; },
    get keyboardBarShown() { return keyboardBarShown; },
    get chrome() { return phoneChrome; },
    get outputFormat() { return outputFormat; },
    get outputSelectable() { return outputSelectable; },
    get commandGroups() { return mobileCommandGroups; },
    get moreSections() { return moreSections; },
    get tabsShown() { return tabsShown; },
    get tabActive() { return tabActive; },
    get tabBarShown() { return tabBarShown; },
    popped,
    closeOverlays,
    back,
    openCommand,
    commandChanged,
    chooseCommand,
    chooseMoreRow,
    showDocument,
    showComments,
    showCollaborators,
  };
}
