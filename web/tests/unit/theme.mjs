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

const { CHOICES, choice, mode, setChoice, onMode, onChoice, start, palette, setPalette, onPalette, PALETTES } = await import("../../src/lib/theme.js");

// A first visit follows the computer, which node says is light.
{
  store.clear();
  assert.deepEqual(CHOICES, ["system", "light", "dark"]);
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

// A value that is not a choice changes nothing.
{
  store.clear();
  setChoice("dark");
  setChoice("neon");
  assert.equal(choice(), "dark");
  assert.equal(store.get("librepaper-theme"), '"dark"');
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

// A browser that refuses the write still gets the choice: the store is best
// effort.
{
  store.clear();
  const setItem = globalThis.localStorage.setItem;
  globalThis.localStorage.setItem = () => { throw new Error("quota"); };
  setChoice("dark");
  assert.equal(choice(), "dark");
  assert.equal(mode(), "dark");
  assert.equal(store.has("librepaper-theme"), false);
  globalThis.localStorage.setItem = setItem;
}

// Choice listeners hear the choice at once and after each one, until they
// leave, whether or not the mode moved.
{
  store.clear();
  setChoice("light");
  const heard = [];
  const stop = onChoice((now) => heard.push(now));
  assert.deepEqual(heard, ["light"]);
  setChoice("system");
  assert.deepEqual(heard, ["light", "system"]);
  assert.equal(mode(), "light");
  stop();
  setChoice("dark");
  assert.deepEqual(heard, ["light", "system"]);
}

// A choice made in another tab arrives as a storage event and is followed;
// other keys are ignored, and a cleared store means system.
{
  store.clear();
  const handlers = {};
  globalThis.addEventListener = (type, handler) => { handlers[type] = handler; };
  start();
  setChoice("dark");
  const modes = [];
  const choices = [];
  const stopMode = onMode((now) => modes.push(now));
  const stopChoice = onChoice((now) => choices.push(now));
  store.set("librepaper-theme", JSON.stringify("light"));
  handlers.storage({ key: "librepaper-theme" });
  assert.equal(choice(), "light");
  assert.deepEqual(modes, ["dark", "light"]);
  assert.deepEqual(choices, ["dark", "light"]);
  handlers.storage({ key: "librepaper-keymap" });
  assert.equal(choice(), "light");
  store.delete("librepaper-theme");
  handlers.storage({ key: null });
  assert.equal(choice(), "system");
  assert.deepEqual(choices, ["dark", "light", "system"]);
  stopMode();
  stopChoice();
}

// The colour theme defaults to librepaper, and a stored value this version
// does not offer reads back as librepaper too.
{
  store.clear();
  assert.deepEqual(PALETTES, ["librepaper", "flexoki", "solarized", "gruvbox", "catppuccin", "nord"]);
  assert.equal(palette(), "librepaper");
  store.set("librepaper-palette", JSON.stringify("neon"));
  assert.equal(palette(), "librepaper");
  store.delete("librepaper-palette");
}

// A palette that is not offered changes nothing.
{
  store.clear();
  globalThis.document = { documentElement: { dataset: {} } };
  setPalette("bogus");
  assert.equal(palette(), "librepaper");
  assert.equal(store.has("librepaper-palette"), false);
  assert.equal(globalThis.document.documentElement.dataset.palette, undefined);
}

// A palette is written as JSON and the page carries it on the root element.
{
  store.clear();
  setPalette("nord");
  assert.equal(store.get("librepaper-palette"), '"nord"');
  assert.equal(palette(), "nord");
  assert.equal(globalThis.document.documentElement.dataset.palette, "nord");
  delete globalThis.document;
}

// Palette listeners hear the palette at once and after each change, until they
// leave.
{
  store.clear();
  globalThis.document = { documentElement: { dataset: {} } };
  setPalette("librepaper");
  const heard = [];
  const stop = onPalette((now) => heard.push(now));
  assert.deepEqual(heard, ["librepaper"]);
  setPalette("gruvbox");
  assert.deepEqual(heard, ["librepaper", "gruvbox"]);
  stop();
  setPalette("nord");
  assert.deepEqual(heard, ["librepaper", "gruvbox"]);
  delete globalThis.document;
}
