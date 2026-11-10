import assert from "node:assert/strict";
import test from "node:test";
import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { EditorState } from "@codemirror/state";
import { loadTypstSyntax } from "../../src/lib/typst-language/module.js";
import { typstSupport } from "../../src/lib/typst-language/index.js";
import { nextTableCell, previousTableCell, typstTableKeymap } from "../../src/lib/typst-language/table-keys.js";

const wasm = process.env.LIBREPAPER_TYPST_SYNTAX_WASM || fileURLToPath(new URL("../../wasm/typst-syntax.wasm", import.meta.url));

/** Runs a command on a state with a cursor (or selection) and returns whether it handled the key and the resulting state. */
function press(command, doc, { at, to = at, extensions }) {
  const state = EditorState.create({ doc, selection: { anchor: at, head: to }, extensions });
  let next = state;
  const handled = command({ state, dispatch: (tr) => { next = tr.state; } });
  return { handled, state: next };
}

if (!existsSync(wasm)) {
  test("typst-syntax.wasm is missing", (t) => t.skip(`${wasm} not found; set LIBREPAPER_TYPST_SYNTAX_WASM`));
} else {
  const api = await loadTypstSyntax(new Uint8Array(readFileSync(wasm)));
  const extensions = () => typstSupport(api);

  test("the keymap binds Tab and Shift-Tab", () => {
    assert.deepEqual(typstTableKeymap.map((binding) => binding.key), ["Tab", "Shift-Tab"]);
  });

  test("Tab moves to the next cell's inside start", () => {
    const doc = "#table(\n  columns: 3,\n  [a], [b], [c],\n)";
    const { handled, state } = press(nextTableCell, doc, { at: doc.indexOf("[a]") + 1, extensions: extensions() });
    assert.equal(handled, true);
    assert.equal(state.doc.toString(), doc);
    assert.deepEqual(state.selection.main.from, doc.indexOf("[b]") + 1);
    assert.equal(state.selection.main.empty, true);
  });

  test("Tab from inside a cell's text moves on and selects the next cell's text", () => {
    const doc = "#table(columns: 2, [alpha], [beta])";
    const at = doc.indexOf("alpha") + 2;
    const { handled, state } = press(nextTableCell, doc, { at, extensions: extensions() });
    assert.equal(handled, true);
    const from = doc.indexOf("beta");
    assert.deepEqual([state.selection.main.from, state.selection.main.to], [from, from + 4]);
  });

  test("Shift-Tab moves to the previous cell and selects its text", () => {
    const doc = "#table(columns: 3, [alpha], [beta], [c])";
    const at = doc.indexOf("[c]") + 2;
    const { handled, state } = press(previousTableCell, doc, { at, extensions: extensions() });
    assert.equal(handled, true);
    const from = doc.indexOf("beta");
    assert.deepEqual([state.selection.main.from, state.selection.main.to], [from, from + 4]);
  });

  test("Shift-Tab at the first cell stays put", () => {
    const doc = "#table(columns: 2, [a], [b])";
    const at = doc.indexOf("[a]") + 1;
    const { handled, state } = press(previousTableCell, doc, { at, extensions: extensions() });
    assert.equal(handled, true);
    assert.equal(state.doc.toString(), doc);
    assert.equal(state.selection.main.head, at);
  });

  test("Tab at the last cell of columns: 3 appends a row after the trailing comma", () => {
    const doc = "#table(\n  columns: 3,\n  [a], [b], [c],\n)";
    const at = doc.indexOf("[c]") + 2;
    const { handled, state } = press(nextTableCell, doc, { at, extensions: extensions() });
    assert.equal(handled, true);
    const expected = "#table(\n  columns: 3,\n  [a], [b], [c],\n  [], [], [],\n)";
    assert.equal(state.doc.toString(), expected);
    assert.equal(state.selection.main.head, expected.indexOf("[], [], [],") + 1);
    assert.equal(state.selection.main.empty, true);
  });

  test("Tab at the last cell appends a row for columns: (1fr, 2fr)", () => {
    const doc = "#table(\n  columns: (1fr, 2fr),\n  [a], [b],\n)";
    const at = doc.indexOf("[b]") + 2;
    const { handled, state } = press(nextTableCell, doc, { at, extensions: extensions() });
    assert.equal(handled, true);
    const expected = "#table(\n  columns: (1fr, 2fr),\n  [a], [b],\n  [], [],\n)";
    assert.equal(state.doc.toString(), expected);
    assert.equal(state.selection.main.head, expected.indexOf("[], [],") + 1);
  });

  test("Tab at the last cell of a grid without a trailing comma adds the separator", () => {
    const doc = "#grid(\n  columns: 2,\n  [a], [b]\n)";
    const at = doc.indexOf("[b]") + 1;
    const { handled, state } = press(nextTableCell, doc, { at, extensions: extensions() });
    assert.equal(handled, true);
    const expected = "#grid(\n  columns: 2,\n  [a], [b],\n  [], []\n)";
    assert.equal(state.doc.toString(), expected);
    assert.equal(state.selection.main.head, expected.indexOf("[], []") + 1);
  });

  test("a table outside markup is handled the same way", () => {
    const doc = "Intro text.\n\n#grid(columns: 2, [a], [b])";
    const at = doc.indexOf("[a]") + 1;
    const { handled, state } = press(nextTableCell, doc, { at, extensions: extensions() });
    assert.equal(handled, true);
    assert.equal(state.selection.main.head, doc.indexOf("[b]") + 1);
  });

  test("outside a table the commands return false and change nothing", () => {
    const doc = "Plain text here.\n\n#table(columns: 2, [a], [b])";
    for (const command of [nextTableCell, previousTableCell]) {
      const { handled, state } = press(command, doc, { at: 3, extensions: extensions() });
      assert.equal(handled, false);
      assert.equal(state.doc.toString(), doc);
      assert.equal(state.selection.main.head, 3);
    }
  });

  test("without the typst tree the commands return false", () => {
    const doc = "#table(columns: 2, [a], [b])";
    for (const command of [nextTableCell, previousTableCell]) {
      const { handled, state } = press(command, doc, { at: doc.indexOf("[a]") + 1 });
      assert.equal(handled, false);
      assert.equal(state.doc.toString(), doc);
    }
  });
}
