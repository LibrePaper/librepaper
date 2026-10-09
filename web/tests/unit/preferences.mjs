// What this reader arranged, and what they find when they come back.
//
// The point of this module is that a habit changed and a habit remembered are
// one act, so what is checked is that the habits write, that the arrangement
// does not, and that a stored value from an older version of the application
// is not trusted as a choice.
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
  assert.equal(prefs.state.panel, "files", "every project opens on its files");
  assert.equal(prefs.state.mobileView, "sidebar", "a phone opens on the project view");
  assert.equal(store.size, 0, "reading remembers nothing");
}

// The arrangement is not remembered, so values stored by an older version are
// ignored. A source side this one does not offer is checked rather than trusted.
{
  store.clear();
  put("librepaper-layout", "source");
  put("librepaper-panel", { paper: "history" });
  put("librepaper-mobile-view", "document");
  put("librepaper-source-side", "middle");
  const prefs = createPreferences();
  assert.equal(prefs.state.layout, "split", "a stored layout is not read back");
  assert.equal(prefs.state.panel, "files", "a stored panel is not read back");
  assert.equal(prefs.state.mobileView, "sidebar", "a stored mobile view is not read back");
  assert.equal(prefs.state.preferredPane, "document");
  assert.equal(prefs.state.sourceSide, "left");
}

// The habits are written when they change.
{
  store.clear();
  const prefs = createPreferences();
  prefs.setSourceSide("right");
  assert.equal(prefs.state.sourceSide, "right");
  assert.equal(got("librepaper-source-side"), "right");

  prefs.setKeys("vim");
  assert.equal(prefs.state.keys, "vim");
  assert.equal(got("librepaper-keymap"), "vim");

  prefs.setSize(PANES.editor, 0.35);
  assert.equal(prefs.state.sizes[PANES.editor.key], 0.35);
  assert.equal(got(PANES.editor.key), 0.35);
}

// The arrangement changes the state for this visit and writes nothing.
{
  store.clear();
  const prefs = createPreferences();
  prefs.setLayout("source");
  assert.equal(prefs.state.layout, "source");

  prefs.setPanel("history");
  assert.equal(prefs.state.panel, "history");

  prefs.setMobileView("document");
  assert.equal(prefs.state.mobileView, "document");

  assert.equal(store.size, 0, "layout, panel and mobile view are not written down");
}

// A panel set on one visit does not carry to the next one.
{
  store.clear();
  const prefs = createPreferences();
  prefs.setPanel("history");
  assert.equal(prefs.state.panel, "history");
  assert.equal(createPreferences().state.panel, "files", "a fresh visit opens on its files");
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
  prefs.setPanel("history");
  assert.equal(prefs.state.panel, "history", "the choice still applies to this page");
  prefs.setSourceSide("right");
  assert.equal(prefs.state.sourceSide, "right", "the choice still applies to this page");
  globalThis.localStorage = working;
}

console.log("preferences: every project opens the same way; only the reader's habits are remembered");
