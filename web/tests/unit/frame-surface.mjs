// The preview frame sits on the reader's editor surface: the theme message's
// `surface` sets the house variables on <html>, and a message without one
// clears them. Exercise the shipped code with a fake document.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const frame = readFileSync(new URL("../../src/agent/frame.js", import.meta.url), "utf8");
const start = frame.indexOf('  let paper = "light";');
const end = frame.indexOf("  // Live previews sit in a reading canvas", start);
assert.notEqual(start, -1, "paper state is present");
assert.notEqual(end, -1, "paper code has a boundary");

function harness() {
  const props = new Map();
  const classes = new Set();
  const root = {
    classList: { toggle(name, on) { if (on) classes.add(name); else classes.delete(name); } },
    style: {
      setProperty(name, value) { props.set(name, value); },
      removeProperty(name) { props.delete(name); },
    },
    dataset: {},
  };
  const run = new Function(
    "document", "lastRanges", "highlight",
    `${frame.slice(start, end)}\nreturn { setPaper, setSurface };`,
  );
  const api = run({ documentElement: root }, [], () => {});
  return { ...api, props, classes };
}

// A surface sets the paper and ink on the root, over either palette's house value.
{
  const { setPaper, setSurface, props, classes } = harness();
  setSurface({ paper: "rgb(1, 2, 3)", ink: "rgb(4, 5, 6)" });
  assert.equal(props.get("--lp-paper"), "rgb(1, 2, 3)");
  assert.equal(props.get("--lp-ink"), "rgb(4, 5, 6)");
  // Going dark keeps the surface: it is inline, so it beats .lp-dark.
  setPaper("dark");
  assert.ok(classes.has("lp-dark"));
  assert.equal(props.get("--lp-paper"), "rgb(1, 2, 3)");
  // A new surface replaces it.
  setSurface({ paper: "rgb(9, 9, 9)", ink: "rgb(8, 8, 8)" });
  assert.equal(props.get("--lp-paper"), "rgb(9, 9, 9)");
}

// A message without a surface clears them; so does a malformed one.
{
  const { setSurface, props } = harness();
  setSurface({ paper: "#fff", ink: "#000" });
  setSurface(undefined);
  assert.equal(props.size, 0);
  setSurface({ paper: "#fff", ink: "#000" });
  setSurface(null);
  assert.equal(props.size, 0);
  setSurface({ paper: "#fff" });
  assert.equal(props.size, 0);
}
