// The theme choice and the mode it resolves to.
//
// Under node there is no document and no matchMedia, so "system" must come
// out light, and everything else is checked through the stored value.
import assert from "node:assert/strict";

const store = new Map();
globalThis.localStorage = {
  getItem: (key) => (store.has(key) ? store.get(key) : null),
  setItem: (key, value) => store.set(key, String(value)),
  removeItem: (key) => store.delete(key),
};

const { CHOICES, choice, mode, setChoice, onMode } = await import("../../src/lib/theme.js");

// A first visit follows the computer, which node says is light.
{
  store.clear();
  assert.deepEqual(CHOICES, ["system", "light", "dark"]);
  assert.equal(choice(), "system");
  assert.equal(mode(), "light");
}

// A choice is written as JSON and the mode follows it.
{
  store.clear();
  setChoice("dark");
  assert.equal(store.get("librepaper-theme"), '"dark"');
  assert.equal(choice(), "dark");
  assert.equal(mode(), "dark");
  setChoice("light");
  assert.equal(mode(), "light");
  setChoice("system");
  assert.equal(choice(), "system");
  assert.equal(mode(), "light");
}

// A stored value this version does not offer reads back as system.
{
  store.clear();
  store.set("librepaper-theme", JSON.stringify("sepia"));
  assert.equal(choice(), "system");
  assert.equal(mode(), "light");
}

// Listeners hear the mode at once and after each change, until they leave.
{
  store.clear();
  setChoice("light");
  const heard = [];
  const stop = onMode((now) => heard.push(now));
  assert.deepEqual(heard, ["light"]);
  setChoice("dark");
  assert.deepEqual(heard, ["light", "dark"]);
  stop();
  setChoice("light");
  assert.deepEqual(heard, ["light", "dark"]);
}

// A value that is not a choice changes nothing.
{
  store.clear();
  setChoice("dark");
  setChoice("neon");
  assert.equal(choice(), "dark");
  assert.equal(store.get("librepaper-theme"), '"dark"');
}
