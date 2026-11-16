// What is on screen: the panes and the panel, on a desktop and on a phone.
// The module is loaded as the page compiles it. What the page would supply
// (editing, a comparison, the home panel, the demo's blocked panels) is reactive
// data the checks change, and every effect is a recording callback.
import assert from "node:assert/strict";
import { loadRunes } from "../helpers/runes.mjs";

const { createArrangement } = await loadRunes(new URL("../../src/lib/reader/arrangement.svelte.js", import.meta.url));
const { showing } = await import("../../src/lib/panes.js");
const { reactive } = await loadRunes(new URL("../helpers/reactive.svelte.js", import.meta.url));

function page(overrides = {}) {
  const world = reactive({ editing: true, comparing: "", home: "files", blocked: [], width: 1280, ...overrides });
  const calls = { leaving: [], history: 0 };
  const arrangement = createArrangement({
    editing: () => world.editing,
    comparing: () => world.comparing,
    home: () => world.home,
    blocked: (name) => world.blocked.includes(name),
    leaving: (name) => calls.leaving.push(name),
    loadHistory: () => { calls.history += 1; return Promise.resolve("loaded"); },
    initialWidth: () => world.width,
    measureUnit: () => 16,
  });
  return { world, calls, arrangement, state: arrangement.state };
}

// A desktop shows what the pure arithmetic says for each layout.
{
  for (const layout of ["split", "source", "document"]) {
    const { arrangement, state } = page();
    state.layout = layout;
    assert.equal(arrangement.compact, false);
    assert.equal(arrangement.splitTight, false);
    assert.equal(arrangement.effectiveLayout, layout);
    assert.deepEqual(arrangement.shown, showing({ layout, comments: true, editing: true }), `desktop ${layout}`);
  }
  const { arrangement, state } = page({ editing: false });
  state.panel = "";
  assert.deepEqual(arrangement.shown, { source: false, document: true, comments: false }, "no source when nobody edits");
  // `editing` is plain data here, so a page that edits is made anew: in the
  // Reader it is reactive state and the same page would follow it.
  const editingPage = page({ editing: true });
  editingPage.state.panel = "";
  assert.equal(editingPage.arrangement.shown.source, true);
}

// A phone shows exactly one of source, document and sidebar.
{
  const { arrangement, state } = page();
  state.width = 400;
  assert.equal(arrangement.compact, true);
  assert.equal(arrangement.adapted, true);
  const count = () => Object.values(arrangement.shown).filter(Boolean).length;
  state.mobileView = "document";
  assert.deepEqual(arrangement.shown, { source: false, document: true, comments: false });
  state.mobileView = "source";
  assert.deepEqual(arrangement.shown, { source: true, document: false, comments: false });
  assert.equal(count(), 1);
  state.mobileView = "sidebar";
  assert.deepEqual(arrangement.shown, { source: false, document: false, comments: true });
  assert.equal(count(), 1);
}

// The source is open while editing or comparing.
{
  const { arrangement, world } = page({ editing: false });
  assert.equal(arrangement.sourceOpen, false);
  world.comparing = "history";
  assert.equal(arrangement.sourceOpen, true);
  world.comparing = "";
  world.editing = true;
  assert.equal(arrangement.sourceOpen, true);
}

// The phone's face falls back to the document, and a history comparison shows the source.
{
  const { arrangement, state, world } = page({ editing: false });
  state.width = 400;
  state.mobileView = "source";
  assert.equal(arrangement.activeMobileView, "document", "no source open");
  world.editing = true;
  assert.equal(arrangement.activeMobileView, "source");
  state.mobileView = "sidebar";
  state.panel = "";
  assert.equal(arrangement.activeMobileView, "document", "a sidebar with no panel");
  state.panel = "files";
  assert.equal(arrangement.activeMobileView, "sidebar");
  state.mobileView = "document";
  world.comparing = "history";
  assert.equal(arrangement.activeMobileView, "source", "comparing history on the document view");
  assert.equal(arrangement.effectiveLayout, "source");
}

// showMobileView
{
  const { arrangement, state } = page({ editing: false });
  state.width = 400;
  arrangement.showMobileView("source");
  assert.equal(state.mobileView, "document", "no source open goes to the document");
  assert.equal(state.preferredPane, "document");

  const desktop = page();
  desktop.state.layout = "source";
  desktop.arrangement.showMobileView("document");
  assert.equal(desktop.state.layout, "document", "a desktop that is not split changes layout");
  desktop.state.layout = "split";
  desktop.arrangement.showMobileView("source");
  assert.equal(desktop.state.layout, "split", "a split stays a split");
  assert.equal(desktop.state.preferredPane, "source");

  const phone = page();
  phone.state.width = 400;
  phone.state.panel = "";
  phone.world.home = "collaboration";
  phone.arrangement.showMobileView("sidebar");
  assert.equal(phone.state.panel, "collaboration", "an empty column shows home()");
  assert.equal(phone.state.mobileView, "sidebar");
  assert.equal(phone.state.preferredPane, "document", "the sidebar is not a preferred pane");
}

// selectPanel
{
  const desktop = page();
  desktop.arrangement.selectPanel("files");
  assert.equal(desktop.state.panel, "", "the same panel closes on a desktop");
  desktop.arrangement.selectPanel("history");
  assert.equal(desktop.state.panel, "history");

  const phone = page();
  phone.state.width = 400;
  phone.arrangement.selectPanel("files");
  assert.equal(phone.state.panel, "files", "a phone not in the sidebar opens the panel");
  assert.equal(phone.state.mobileView, "sidebar");
  phone.arrangement.selectPanel("files");
  assert.equal(phone.state.panel, "", "in the sidebar, the same panel closes");
  assert.equal(phone.state.mobileView, "document");

  const blocked = page({ blocked: ["history"] });
  blocked.arrangement.selectPanel("history");
  assert.equal(blocked.state.panel, "files");
  assert.deepEqual(blocked.calls.leaving, []);
}

// openFilesPanel
{
  const phone = page();
  phone.state.width = 400;
  phone.state.panel = "history";
  phone.arrangement.openFilesPanel();
  assert.equal(phone.state.panel, "files");
  assert.equal(phone.state.mobileView, "sidebar");
}

// showPanel
{
  const { arrangement, state, calls } = page({ blocked: ["agent"] });
  state.width = 400;
  state.preferredPane = "source";
  assert.equal(await arrangement.showPanel("agent"), undefined);
  assert.equal(state.panel, "files", "a blocked panel does nothing");
  assert.deepEqual(calls.leaving, []);

  const files = arrangement.showPanel("changes");
  assert.ok(files instanceof Promise);
  await files;
  assert.deepEqual(calls.leaving, ["changes"]);
  assert.equal(state.panel, "changes");
  assert.equal(state.mobileView, "sidebar");
  assert.equal(calls.history, 0, "only history loads the manifest");

  await arrangement.showPanel("");
  assert.equal(state.panel, "");
  assert.equal(state.mobileView, "source", "closing returns to the preferred pane");

  assert.equal(await arrangement.showPanel("history"), "loaded");
  assert.equal(calls.history, 1);
  assert.deepEqual(calls.leaving, ["changes", "", "history"]);
}

// A tight split shows one pane, the preferred one.
{
  const { arrangement, state } = page();
  state.width = 800;
  assert.equal(arrangement.compact, false);
  assert.equal(arrangement.splitTight, true);
  assert.equal(arrangement.adapted, true);
  state.preferredPane = "source";
  assert.equal(arrangement.effectiveLayout, "source");
  state.preferredPane = "document";
  assert.equal(arrangement.effectiveLayout, "document");
  state.layout = "source";
  assert.equal(arrangement.adapted, false, "a tight window with one pane chosen is not adapted");
}
console.log("arrangement: ok");
