// The habits this reader keeps between visits.
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
  assert.equal(prefs.state.sourceSide, "left");
  assert.equal(prefs.state.keys, "default");
  assert.equal(prefs.state.completion, true);
  assert.equal(prefs.state.brackets, true);
  assert.equal(prefs.state.proofing, "browser");
  assert.equal(prefs.state.dialect, "american");
  assert.equal(prefs.state.check, true);
  assert.equal(store.size, 0, "reading remembers nothing");
}

// The editor helps are on unless a stored false turns one off. A junk value is
// not a false, so it reads as on.
{
  store.clear();
  put("librepaper-editor-completion", false);
  put("librepaper-editor-brackets", false);
  put("librepaper-editor-spellcheck", false);
  put("librepaper-editor-check", "no");
  const prefs = createPreferences();
  assert.equal(prefs.state.completion, false);
  assert.equal(prefs.state.brackets, false);
  assert.equal(prefs.state.proofing, "off");
  assert.equal(prefs.state.check, true);
}

// The grammar and spelling choice reads the old switch as it was: true is
// the browser, and a value that is neither is checked.
{
  store.clear();
  put("librepaper-editor-spellcheck", true);
  assert.equal(createPreferences().state.proofing, "browser");
  put("librepaper-editor-spellcheck", "nano");
  assert.equal(createPreferences().state.proofing, "browser");
  put("librepaper-editor-spellcheck", "harper");
  assert.equal(createPreferences().state.proofing, "harper");
  put("librepaper-editor-dialect", "klingon");
  assert.equal(createPreferences().state.dialect, "american");
}

// A value from a version that offered a choice this one does not is checked
// rather than trusted.
{
  store.clear();
  put("librepaper-source-side", "middle");
  put("librepaper-keymap", "nano");
  put("librepaper-editor-spellcheck", "harper");
  const prefs = createPreferences();
  assert.equal(prefs.state.sourceSide, "left");
  assert.equal(prefs.state.keys, "default");
  assert.equal(prefs.state.proofing, "harper");
}

// Every setter writes. This is the whole reason the module exists: setting
// one of these and remembering it used to be two statements at eight call
// sites, and the second was the one that got left out.
{
  store.clear();
  const prefs = createPreferences();
  prefs.setSourceSide("right");
  assert.equal(got("librepaper-source-side"), "right");

  prefs.setKeys("vim");
  assert.equal(prefs.state.keys, "vim");
  assert.equal(got("librepaper-keymap"), "vim");

  prefs.setSize(PANES.editor, 0.35);
  assert.equal(prefs.state.sizes[PANES.editor.key], 0.35);
  assert.equal(got(PANES.editor.key), 0.35);

  prefs.setCompletion(false);
  assert.equal(prefs.state.completion, false);
  assert.equal(got("librepaper-editor-completion"), false);

  prefs.setBrackets(false);
  assert.equal(prefs.state.brackets, false);
  assert.equal(got("librepaper-editor-brackets"), false);

  prefs.setProofing("harper");
  assert.equal(prefs.state.proofing, "harper");
  assert.equal(got("librepaper-editor-spellcheck"), "harper");

  prefs.setProofing("nano");
  assert.equal(prefs.state.proofing, "browser", "a value outside the choices falls back");

  prefs.setDialect("british");
  assert.equal(prefs.state.dialect, "british");
  assert.equal(got("librepaper-editor-dialect"), "british");

  prefs.setCheck(false);
  assert.equal(prefs.state.check, false);
  assert.equal(got("librepaper-editor-check"), false);

  prefs.setCheck(true);
  assert.equal(prefs.state.check, true);
  assert.equal(got("librepaper-editor-check"), true);
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
  assert.equal(prefs.state.sourceSide, "left");
  prefs.setSourceSide("right");
  assert.equal(prefs.state.sourceSide, "right", "the choice still applies to this page");
  globalThis.localStorage = working;
}

console.log("preferences: remembered habits, checked on the way in and written on the way out");
