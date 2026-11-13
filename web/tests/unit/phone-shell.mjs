// The phone's shell, driven without a page: a real arrangement at a phone
// width, reactive facts the checks change, and recording actions.
import assert from "node:assert/strict";
import { loadRunes } from "../helpers/runes.mjs";

const { createArrangement } = await loadRunes(new URL("../../src/lib/reader/arrangement.svelte.js", import.meta.url));
const { createPhone } = await loadRunes(new URL("../../src/lib/reader/phone.svelte.js", import.meta.url));
const { reactive } = await loadRunes(new URL("../helpers/reactive.svelte.js", import.meta.url));
const { m } = await import("../../src/lib/paraglide/messages.js");

const TABS = [
  { id: "files", icon: "folder", says: "Files" },
  { id: "outline", icon: "list", says: "Outline" },
  { id: "history", icon: "history", says: "History" },
  { id: "collaboration", icon: "users", says: "Comments" },
];

function page(overrides = {}) {
  const world = reactive({
    editing: true, comparing: "", comparisonSays: "Version history", settingsOpen: false, barHidden: false,
    tabs: TABS, badges: {}, participants: [], me: { id: "u1", name: "Ann" }, mayEdit: true, mayChat: false,
    title: "Paper", toolbarPath: "", editorReady: true, figureShown: false, format: "latex", width: 400,
    ...overrides,
  });
  const calls = [];
  const timers = [];
  const arrangement = createArrangement({
    editing: () => world.editing,
    comparing: () => world.comparing,
    home: () => "files",
    initialWidth: () => world.width,
    measureUnit: () => 16,
  });
  const insert = {
    released: 0,
    mobileGroups: () => [{ items: [{ id: "figure", label: "Figure" }] }],
    releaseMobile() { insert.released += 1; },
    insertAction: (id) => calls.push(["insert", id]),
  };
  const phone = createPhone({
    arrangement,
    comparing: () => world.comparing,
    comparisonSays: () => world.comparisonSays,
    settingsOpen: () => world.settingsOpen,
    closeSettings: () => { world.settingsOpen = false; },
    endComparison: () => { calls.push(["end"]); world.comparing = ""; },
    barHidden: () => world.barHidden,
    tabs: () => world.tabs,
    badges: () => world.badges,
    participants: () => world.participants,
    role: () => ({ mayEdit: world.mayEdit, mayChat: world.mayChat, me: world.me }),
    title: () => world.title,
    source: () => ({
      format: world.format, compilesHere: true, toolbarPath: world.toolbarPath, editing: world.editing,
      editorReady: world.editorReady, figureShown: world.figureShown,
    }),
    demo: () => ({ on: false, hint: "" }),
    hasDesktop: () => false,
    exportable: () => ({ html: { ok: true }, pdf: { ok: true }, docx: { ok: true } }),
    canExportComments: () => true,
    can: () => ({}),
    editGroups: () => [[["undo", "Undo"]]],
    outputs: () => ({ displayedFormat: "latex", latexOutput: "pdf", typstOutput: "pdf", markdownOutput: "html" }),
    view: () => ({ linked: false, visual: false, runCodeOffered: true, localExecution: false }),
    insertMenu: () => insert,
    refreshEditAvailability: () => calls.push(["refresh"]),
    chooseFileCommand: (id) => calls.push(["file", id]),
    chooseEditCommand: (id) => calls.push(["edit", id]),
    chooseViewCommand: (id) => calls.push(["view", id]),
    toggleLocalExecution: () => calls.push(["toggle"]),
    runQuarto: () => calls.push(["run-quarto"]),
    stopRun: () => calls.push(["stop-run"]),
    openSettings: () => calls.push(["settings"]),
    runCommand: (id) => calls.push(["run", id]),
    signOut: () => calls.push(["signout"]),
    signInHref: () => "/login",
    openPanel: (name) => calls.push(["panel", name]),
    setCollaborationTab: (tab) => calls.push(["tab", tab]),
    setTimer: (run, ms) => timers.push([run, ms]),
  });
  return { world, calls, timers, arrangement, state: arrangement.state, phone, insert };
}

// The depth counts a panel, a command list, a comparison, and the More sheet on top.
{
  const { phone, state, world } = page();
  assert.equal(phone.overlayDepth, 0);
  assert.equal(phone.overlayView, false);
  state.mobileView = "sidebar";
  assert.equal(phone.overlayDepth, 1);
  state.mobileView = "document";
  state.mobileCommand = "file";
  assert.equal(phone.overlayDepth, 1);
  state.moreOpen = true;
  assert.equal(phone.overlayDepth, 2);
  state.moreOpen = false;
  state.mobileCommand = "";
  world.comparing = "history";
  state.mobileView = "source";
  assert.equal(phone.overlayDepth, 2);
  state.moreOpen = true;
  assert.equal(phone.overlayDepth, 3);
}
{
  const { phone, world } = page({ width: 1280 });
  world.settingsOpen = true;
  assert.equal(phone.overlayDepth, 0, "a desktop has no layers");
  const compact = page();
  compact.world.settingsOpen = true;
  assert.equal(compact.phone.overlayDepth, 1, "settings are a layer");
}

// Back closes the top layer.
{
  const { phone, state, world, calls } = page();
  state.mobileView = "sidebar";
  state.moreOpen = true;
  state.mobileCommand = "view";
  world.settingsOpen = true;
  phone.popped(1);
  assert.equal(state.moreOpen, false, "More closes first");
  assert.equal(state.mobileCommand, "view");
  phone.popped(0);
  assert.equal(state.mobileCommand, "");
  assert.equal(world.settingsOpen, false);
  assert.equal(state.mobileView, "document");
  assert.deepEqual(calls, []);
}
{
  // A comparison's source goes back to the sidebar.
  const { phone, state, world } = page();
  world.comparing = "history";
  state.panel = "history";
  state.mobileView = "source";
  phone.popped(1);
  assert.equal(state.mobileView, "sidebar");
  state.mobileView = "source";
  phone.back();
  assert.equal(state.mobileView, "sidebar");
  // Closing from the history panel while comparing ends the comparison.
  phone.back();
  assert.equal(world.comparing, "");
  assert.notEqual(state.mobileView, "sidebar");
}
{
  // The panel stays set, and the source is where the reader was.
  const { phone, state } = page();
  state.preferredPane = "source";
  state.mobileView = "sidebar";
  phone.closeOverlays();
  assert.equal(state.mobileView, "source");
  assert.equal(state.panel, "files");
  state.mobileView = "sidebar";
  state.preferredPane = "document";
  phone.showDocument();
  assert.equal(state.mobileView, "document");
  state.mobileCommand = "edit";
  state.preferredPane = "source";
  phone.showDocument();
  assert.equal(state.mobileCommand, "");
  assert.equal(state.mobileView, "source");
}

// The header names the place the reader is in.
{
  const { phone, state, world } = page({ editing: false });
  assert.equal(phone.mobileTitle, "Paper");
  world.title = "";
  assert.equal(phone.mobileTitle, "LibrePaper");
  world.title = "Paper";
  world.toolbarPath = "src/main.tex";
  assert.equal(phone.mobileTitle, "main.tex");
  state.mobileCommand = "edit";
  assert.equal(phone.mobileTitle, m.mobilenav_edit());
  state.mobileCommand = "";
  state.panel = "outline";
  state.mobileView = "sidebar";
  assert.equal(phone.mobileTitle, "Outline");
  state.panel = "collaboration";
  assert.equal(phone.mobileTitle, m.common_comments());
  state.mobileView = "document";
  world.comparing = "history";
  assert.equal(phone.mobileTitle, "Version history");
}

// The mode switch and the keyboard bar.
{
  const { phone, state, world } = page();
  assert.equal(phone.modeSwitchShown, true);
  assert.equal(phone.keyboardBarShown, false);
  state.keyboardOpen = true;
  assert.equal(phone.modeSwitchShown, false);
  state.mobileView = "source";
  assert.equal(phone.keyboardBarShown, true);
  world.mayEdit = false;
  assert.equal(phone.keyboardBarShown, false);
  world.mayEdit = true;
  state.mobileCommand = "file";
  assert.equal(phone.keyboardBarShown, false);
  state.keyboardOpen = false;
  assert.equal(phone.modeSwitchShown, false, "not over a command list");
  state.mobileCommand = "";
  state.mobileView = "sidebar";
  assert.equal(phone.modeSwitchShown, false, "not over a panel");
  world.editing = false;
  state.mobileView = "document";
  assert.equal(phone.modeSwitchShown, false, "no source to switch to");
}

// The chrome adds up what the phone draws around the workspace.
{
  const { phone, state, world } = page();
  assert.equal(phone.chrome, "env(safe-area-inset-top) + 2.75rem + 3.5rem + env(safe-area-inset-bottom)");
  // Once the bar hides on scroll, the mode switch and the tab bar both leave the chrome.
  world.barHidden = true;
  assert.equal(phone.chrome, "env(safe-area-inset-top) + env(safe-area-inset-bottom)");
  world.barHidden = false;
  state.mobileView = "source";
  state.keyboardOpen = true;
  assert.equal(phone.chrome, "env(safe-area-inset-top) + 2.75rem + var(--librepaper-keyboard, 0px)");
  assert.equal(page({ width: 1280 }).phone.chrome, "0px");
}

// Each command picks its own builder.
{
  const { phone, state } = page();
  assert.deepEqual(phone.commandGroups, []);
  state.mobileCommand = "file";
  const ids = (groups) => groups.flatMap((group) => (group.items || []).map((item) => item.id));
  assert.ok(ids(phone.commandGroups).includes("download-html"));
  state.mobileCommand = "edit";
  assert.deepEqual(ids(phone.commandGroups), ["undo"]);
  state.mobileCommand = "view";
  assert.ok(ids(phone.commandGroups).includes("run-code"));
  assert.ok(!ids(phone.commandGroups).includes("run-quarto"));
  assert.equal(phone.outputFormat, "pdf");
  assert.equal(phone.outputSelectable, true);
  state.mobileCommand = "insert";
  assert.deepEqual(phone.commandGroups, []);
}

// Opening a list reads what it needs to; choosing routes by list.
{
  const { phone, state, calls } = page();
  state.moreOpen = true;
  phone.openCommand("edit");
  assert.deepEqual(calls, [["refresh"]]);
  assert.equal(state.moreOpen, false);
  assert.equal(state.mobileCommand, "edit");
  phone.chooseCommand("undo");
  assert.deepEqual(calls.at(-1), ["edit", "undo"]);
  assert.equal(state.mobileCommand, "");

  phone.openCommand("file");
  phone.chooseCommand("download");
  assert.deepEqual(calls.at(-1), ["file", "download"]);

  phone.openCommand("insert");
  assert.deepEqual(phone.commandGroups, [{ items: [{ id: "figure", label: "Figure" }] }]);
  state.mobileView = "document";
  phone.chooseCommand("figure");
  assert.equal(state.mobileView, "source");
  assert.deepEqual(calls.at(-1), ["insert", "figure"]);

  phone.openCommand("view");
  phone.chooseCommand("run-code");
  assert.deepEqual(calls.at(-1), ["toggle"]);
  assert.equal(state.mobileCommand, "view", "a toggle keeps the list open");
  phone.chooseCommand("run-quarto");
  assert.deepEqual(calls.at(-1), ["run-quarto"]);
  assert.equal(state.mobileCommand, "", "Run closes the list");
  phone.openCommand("view");
  phone.chooseCommand("outline");
  assert.deepEqual(calls.at(-1), ["view", "outline"]);
  assert.equal(state.mobileCommand, "", "a choice closes the list");
  phone.openCommand("view");
  phone.chooseCommand("text-bigger");
  assert.equal(state.mobileCommand, "view");
}

// An Insert list left without a choice lets go of the context it took.
{
  const { phone, insert } = page();
  phone.commandChanged("insert");
  phone.commandChanged("");
  assert.equal(insert.released, 1);
  phone.commandChanged("insert");
  phone.openCommand("insert");
  phone.chooseCommand("figure");
  phone.commandChanged("");
  assert.equal(insert.released, 1, "a chosen list is not released");
  phone.commandChanged("file");
  phone.commandChanged("");
  assert.equal(insert.released, 1);
}

// The More sheet.
{
  const { phone, world } = page({
    tabs: [...TABS, { id: "git", icon: "g", says: "Git", disabled: true }],
    badges: { history: { dot: true, counts: [{ of: 2 }, { of: 3 }] } },
  });
  const sections = () => phone.moreSections;
  const ids = (n) => sections()[n].rows.map((row) => row.id);
  assert.deepEqual(ids(0), ["command:file", "command:edit", "command:view", "command:insert"]);
  assert.deepEqual(ids(1), ["outline", "history", "git", "collaborators"]);
  assert.equal(sections()[1].rows[1].count, 5);
  assert.equal(sections()[1].rows[1].dot, true);
  assert.equal(sections()[1].rows[2].disabled, true);
  assert.equal(sections()[1].rows[2].label, m.common_demo_unavailable({ says: "Git" }));
  assert.equal(sections()[1].rows[0].icon, "list-tree");
  assert.deepEqual(ids(2), ["settings", "palette", "home", "help", "signout"]);
  world.mayEdit = false;
  assert.deepEqual(ids(0), ["command:file", "command:view"]);
  world.editing = false;
  assert.deepEqual(ids(0), ["command:file"]);
  world.tabs = TABS.filter((tab) => tab.id !== "collaboration");
  assert.deepEqual(ids(1), ["outline", "history"]);
  world.participants = [1, 2];
  world.tabs = TABS;
  assert.equal(sections()[1].rows.at(-1).count, 2);
  world.me = { id: "", providers: ["github"] };
  const last = sections()[2].rows.at(-1);
  assert.deepEqual([last.id, last.href], ["signin", "/login"]);
  world.me = { id: "" };
  assert.equal(ids(2).at(-1), "help");
  world.editorReady = false;
  world.editing = true;
  world.mayEdit = true;
  assert.equal(sections()[0].rows[1].disabled, true);
  world.editorReady = true;
  world.figureShown = true;
  assert.equal(sections()[0].rows[1].disabled, true);
  assert.equal(sections()[0].rows[3].disabled, undefined);
}

// Choosing a row of the sheet.
{
  const { phone, state, calls, timers } = page();
  phone.chooseMoreRow("settings");
  phone.chooseMoreRow("palette");
  assert.deepEqual(calls, [], "both wait for the sheet to close");
  assert.deepEqual(timers.map(([, ms]) => ms), [0, 0]);
  timers.forEach(([run]) => run());
  assert.deepEqual(calls, [["settings"], ["run", "palette"]]);
  calls.length = 0;
  phone.chooseMoreRow("signout");
  phone.chooseMoreRow("home");
  phone.chooseMoreRow("help");
  phone.chooseMoreRow("signin");
  assert.deepEqual(calls, [["signout"]]);
  calls.length = 0;
  phone.chooseMoreRow("collaborators");
  assert.deepEqual(calls, [["tab", "comments"], ["panel", "collaboration"]]);
  calls.length = 0;
  state.moreOpen = true;
  phone.chooseMoreRow("command:view");
  assert.equal(state.mobileCommand, "view");
  assert.equal(state.moreOpen, false);
  phone.chooseMoreRow("history");
  assert.deepEqual(calls, [["panel", "history"]]);
}

// The tab bar.
{
  const { phone, state, world } = page();
  assert.equal(phone.tabsShown, true);
  assert.equal(phone.tabActive, "document");
  assert.equal(phone.tabBarShown, true);
  state.panel = "files";
  state.mobileView = "sidebar";
  assert.equal(phone.tabActive, "files");
  assert.equal(phone.tabsShown, true);
  state.panel = "collaboration";
  assert.equal(phone.tabActive, "comments");
  state.panel = "outline";
  assert.equal(phone.tabsShown, false, "other panels cover the bar");
  state.panel = "files";
  state.keyboardOpen = true;
  assert.equal(phone.tabBarShown, false);
  state.keyboardOpen = false;
  state.mobileCommand = "file";
  assert.equal(phone.tabsShown, false);
  state.mobileCommand = "";
  world.comparing = "history";
  assert.equal(phone.tabsShown, false);
  assert.equal(page({ width: 1280 }).phone.tabsShown, false);
}

// Comments and Collaborators set the tab by role, then open the panel.
{
  const { phone, calls, world } = page();
  phone.showComments();
  assert.deepEqual(calls, [["tab", "comments"], ["panel", "collaboration"]]);
  calls.length = 0;
  phone.showCollaborators();
  assert.deepEqual(calls, [["tab", "comments"], ["panel", "collaboration"]]);
  calls.length = 0;
  world.mayChat = true;
  phone.showCollaborators();
  assert.deepEqual(calls, [["tab", "chat"], ["panel", "collaboration"]]);
}

console.log("phone-shell: ok");
