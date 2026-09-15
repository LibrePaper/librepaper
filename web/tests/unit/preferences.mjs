// What this reader arranged, and what they find when they come back.
//
// The point of this module is that changing one of these and remembering it
// are one act rather than two, so what is checked is that every setter wrote
// -- and that a stored value from an older version of the application does
// not come back as a choice this one no longer offers.
import assert from "node:assert/strict";
import { loadRunes } from "../helpers/runes.mjs";

// The module reads its values through lib/storage.js, which talks to
// localStorage and quietly gives up when there is none. A plain object is
// enough to be that store here.
const store = new Map();
globalThis.localStorage = {
  getItem: (key) => (store.has(key) ? store.get(key) : null),
  setItem: (key, value) => store.set(key, String(value)),
  removeItem: (key) => store.delete(key),
};

const { createPreferences } = await loadRunes(
  new URL("../../src/lib/reader/preferences.svelte.js", import.meta.url),
);
const { PANES } = await import("../../src/lib/panes.js");

const put = (key, value) => store.set(key, JSON.stringify(value));
const got = (key) => JSON.parse(store.get(key));

// A first visit: the defaults, and nothing written until something is chosen.
{
  store.clear();
  const prefs = createPreferences();
  assert.equal(prefs.state.layout, "split");
  assert.equal(prefs.state.sourceSide, "left");
  assert.equal(prefs.state.keys, "default");
  assert.equal(prefs.state.panel, "files", "an editor's first visit opens on the shape of the project");
  assert.equal(prefs.state.mobileView, "document");
  assert.equal(store.size, 0, "reading remembers nothing");
}

// A value from a version that offered a choice this one does not is checked
// rather than trusted.
{
  store.clear();
  put("librepaper-layout", "three-columns");
  put("librepaper-source-side", "middle");
  put("librepaper-panel", "something-retired");
  put("librepaper-mobile-view", "elsewhere");
  const prefs = createPreferences();
  assert.equal(prefs.state.layout, "split");
  assert.equal(prefs.state.sourceSide, "left");
  assert.equal(prefs.state.panel, "files");
  assert.equal(prefs.state.mobileView, "document");
}

// Two panels were folded into one. A browser that last left the column on
// either of the old names finds where they went.
for (const old of ["comments", "chat"]) {
  store.clear();
  put("librepaper-panel", old);
  const prefs = createPreferences();
  assert.equal(prefs.state.panel, "collaboration", `${old} became the collaboration panel`);
  assert.equal(
    prefs.state.collaborationTab,
    old === "chat" ? "chat" : "comments",
    "and it opens on the tab that name meant",
  );
}

// Every setter writes. This is the whole reason the module exists: setting
// one of these and remembering it used to be two statements at eight call
// sites, and the second was the one that got left out.
{
  store.clear();
  const prefs = createPreferences();
  prefs.setLayout("source");
  assert.equal(prefs.state.layout, "source");
  assert.equal(got("librepaper-layout"), "source");

  prefs.setSourceSide("right");
  assert.equal(got("librepaper-source-side"), "right");

  prefs.setKeys("vim");
  assert.equal(prefs.state.keys, "vim");
  assert.equal(got("librepaper-keymap"), "vim");

  prefs.setMobileView("sidebar");
  assert.equal(got("librepaper-mobile-view"), "sidebar");

  prefs.setSize(PANES.editor, 0.35);
  assert.equal(prefs.state.sizes[PANES.editor.key], 0.35);
  assert.equal(got(PANES.editor.key), 0.35);
}

// A panel opened for this visit only is shown but not remembered: following
// a link into the timeline is not choosing to start there next time.
{
  store.clear();
  const prefs = createPreferences();
  prefs.setPanel("history", false);
  assert.equal(prefs.state.panel, "history");
  assert.equal(store.has("librepaper-panel"), false, "an unremembered panel writes nothing");
  prefs.setPanel("files");
  assert.equal(got("librepaper-panel"), "files");
}

// Storage can be switched off, and a page that threw when it was would be a
// page that does not load at all.
{
  const working = globalThis.localStorage;
  globalThis.localStorage = {
    getItem: () => { throw new Error("storage is off"); },
    setItem: () => { throw new Error("storage is off"); },
  };
  const prefs = createPreferences();
  assert.equal(prefs.state.layout, "split");
  prefs.setLayout("document");
  assert.equal(prefs.state.layout, "document", "the choice still applies to this page");
  globalThis.localStorage = working;
}

console.log("preferences: remembered arrangements, checked on the way in and written on the way out");
