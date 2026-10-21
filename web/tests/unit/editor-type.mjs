// How the source is set: its font, size and line height. Under node there is no
// document, so apply() is checked through a fake one in its own case.
import assert from "node:assert/strict";

const store = new Map();
globalThis.localStorage = {
  getItem: (key) => (store.has(key) ? store.get(key) : null),
  setItem: (key, value) => store.set(key, String(value)),
  removeItem: (key) => store.delete(key),
};

const {
  DEFAULT_STACK,
  LINE_HEIGHTS,
  cleanFamily,
  clampSize,
  fontStack,
  font,
  setFont,
  onFont,
  lineHeight,
  setLineHeight,
  onLineHeight,
  size,
  setSize,
  onSize,
} = await import("../../src/lib/editor-type.js");

const SIZE_KEY = "librepaper-editor-size";
const FONT_KEY = "librepaper-editor-font";
const LINE_KEY = "librepaper-editor-line-height";

// A first visit has the defaults.
{
  store.clear();
  assert.equal(size(), 14);
  assert.deepEqual(font(), { id: "default", custom: "" });
  assert.equal(lineHeight(), "normal");
  assert.deepEqual(LINE_HEIGHTS, { compact: 1.35, normal: 1.6, wide: 1.9 });
}

// Stored values this version does not offer read back as the defaults.
{
  store.clear();
  store.set(FONT_KEY, JSON.stringify({ id: "comic-sans", custom: "" }));
  store.set(LINE_KEY, JSON.stringify("huge"));
  assert.deepEqual(font(), { id: "default", custom: "" });
  assert.equal(lineHeight(), "normal");
  store.clear();
}

// A size is clamped and rounded, and written as JSON.
{
  store.clear();
  setSize(9.6);
  assert.equal(size(), 10);
  setSize(40);
  assert.equal(size(), 28);
  setSize(15.4);
  assert.equal(size(), 15);
  assert.equal(store.get(SIZE_KEY), "15");
  assert.equal(clampSize(14), 14);
}

// A font is written as { id, custom } and its stack leads with the family.
{
  store.clear();
  setFont("fira-code");
  assert.deepEqual(JSON.parse(store.get(FONT_KEY)), { id: "fira-code", custom: "" });
  assert.deepEqual(font(), { id: "fira-code", custom: "" });
  assert.ok(fontStack(font()).startsWith('"Fira Code", "JetBrains Mono"'));
}

// A font this version does not offer changes nothing.
{
  store.clear();
  setFont("fira-code");
  setFont("bogus");
  assert.equal(font().id, "fira-code");
  assert.deepEqual(JSON.parse(store.get(FONT_KEY)), { id: "fira-code", custom: "" });
}

// A family name is stripped down to letters, digits, spaces and the marks
// - _ and ., with single spaces.
{
  assert.equal(cleanFamily('Evil"; } body {'), "Evil body");
  assert.equal(cleanFamily("  Iosevka   Term  "), "Iosevka Term");
  assert.equal(cleanFamily("Café_Mono.2-b"), "Café_Mono.2-b");
  assert.equal(cleanFamily("a".repeat(100)).length, 64);
}

// Another font with no name falls back to the default stack.
{
  store.clear();
  setFont("custom", "");
  assert.equal(fontStack(font()), DEFAULT_STACK);
  setFont("custom", 'Evil"; } body {');
  assert.equal(font().custom, "Evil body");
  assert.ok(fontStack(font()).startsWith('"Evil body", "JetBrains Mono"'));
}

// A line height is written as its name, and one this version does not offer is ignored.
{
  store.clear();
  setLineHeight("wide");
  assert.equal(store.get(LINE_KEY), '"wide"');
  assert.equal(lineHeight(), "wide");
  setLineHeight("huge");
  assert.equal(lineHeight(), "wide");
}

// The font and line height reach the page, and the default font removes its
// property so the theme's own font applies.
{
  store.clear();
  const calls = [];
  const style = {
    setProperty: (name, value) => calls.push(["set", name, value]),
    removeProperty: (name) => calls.push(["remove", name]),
  };
  globalThis.document = { documentElement: { style } };
  setFont("fira-code");
  assert.deepEqual(calls.find((call) => call[1] === "--font-editor"), ["set", "--font-editor", fontStack({ id: "fira-code", custom: "" })]);
  setLineHeight("compact");
  assert.deepEqual(calls.find((call) => call[1] === "--editor-line-height"), ["set", "--editor-line-height", "1.35"]);
  calls.length = 0;
  setFont("default");
  assert.deepEqual(calls, [["remove", "--font-editor"], ["set", "--editor-line-height", "1.35"]]);
  delete globalThis.document;
}

// Listeners hear the current value at once and after each change, until they leave.
{
  store.clear();
  setSize(15);
  const sizes = [];
  const stopSize = onSize((now) => sizes.push(now));
  assert.deepEqual(sizes, [15]);
  setSize(20);
  assert.deepEqual(sizes, [15, 20]);
  stopSize();
  setSize(12);
  assert.deepEqual(sizes, [15, 20]);

  setFont("serif");
  const fonts = [];
  const stopFont = onFont((now) => fonts.push(now.id));
  assert.deepEqual(fonts, ["serif"]);
  setFont("sans");
  assert.deepEqual(fonts, ["serif", "sans"]);
  stopFont();
  setFont("default");
  assert.deepEqual(fonts, ["serif", "sans"]);

  setLineHeight("normal");
  const heights = [];
  const stopHeight = onLineHeight((now) => heights.push(now));
  assert.deepEqual(heights, ["normal"]);
  setLineHeight("wide");
  assert.deepEqual(heights, ["normal", "wide"]);
  stopHeight();
  setLineHeight("compact");
  assert.deepEqual(heights, ["normal", "wide"]);
}

console.log("editor-type: all checks passed");
