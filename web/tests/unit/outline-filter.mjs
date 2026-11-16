import assert from "node:assert/strict";
import test from "node:test";
import {
  OUTLINE_FILTER_ALL,
  OUTLINE_FILTER_OPEN,
  filterOutline,
  hasOutlineFilter,
  outlineTags,
  tagFilter,
} from "../../src/lib/outline-filter.js";

const heading = (title, level, extra = {}) => ({ title, level, from: 0, line: 1, ...extra });

// Project (1)
//   Plan (2)            TODO, tag work
//   Ship (2)            DONE
//     Notes (3)         tag work
// Home (1)
//   Chores (2)          NEXT, tag home
const outline = [
  heading("Project", 1),
  heading("Plan", 2, { todo: "TODO", done: false, tags: ["work"] }),
  heading("Ship", 2, { todo: "DONE", done: true }),
  heading("Notes", 3, { tags: ["work"] }),
  heading("Home", 1),
  heading("Chores", 2, { todo: "NEXT", done: false, tags: ["home"] }),
];

const rows = (filter) => filterOutline(outline, filter).map(({ index, context }) => [index, context]);

test("the all filter shows every heading as a match", () => {
  assert.deepEqual(rows(OUTLINE_FILTER_ALL), [[0, false], [1, false], [2, false], [3, false], [4, false], [5, false]]);
  assert.deepEqual(rows(), rows(OUTLINE_FILTER_ALL));
});

test("the open filter shows open TODOs and their ancestors only", () => {
  // Ship is done and Notes has no state, so neither is shown.
  assert.deepEqual(rows(OUTLINE_FILTER_OPEN), [[0, true], [1, false], [4, true], [5, false]]);
});

test("a tag filter keeps the ancestors of the tagged headings", () => {
  assert.deepEqual(rows(tagFilter("work")), [[0, true], [1, false], [2, true], [3, false]]);
  assert.deepEqual(rows(tagFilter("home")), [[4, true], [5, false]]);
});

test("a filter keeps grandparents and stops at the next heading of the same or higher level", () => {
  const nested = [
    heading("X", 1),
    heading("Y", 2),
    heading("Z", 3, { tags: ["t"] }),
    heading("P", 1),
    heading("Q", 2),
    heading("R", 1, { tags: ["t"] }),
  ];
  const shown = filterOutline(nested, tagFilter("t")).map(({ index, context }) => [index, context]);
  assert.deepEqual(shown, [[0, true], [1, true], [2, false], [5, false]]);
});

test("a tag the headings do not have shows nothing", () => {
  assert.deepEqual(filterOutline(outline, tagFilter("missing")), []);
});

test("outlineTags lists distinct tags sorted", () => {
  assert.deepEqual(outlineTags([heading("a", 1, { tags: ["b", "a"] }), heading("c", 1, { tags: ["a"] }), heading("d", 1)]), ["a", "b"]);
  assert.deepEqual(outlineTags([heading("a", 1)]), []);
});

test("hasOutlineFilter is true only when a heading has a TODO state or a tag", () => {
  assert.equal(hasOutlineFilter([heading("a", 1)]), false);
  assert.equal(hasOutlineFilter([heading("a", 1, { tags: [] })]), false);
  assert.equal(hasOutlineFilter([heading("a", 1, { todo: "TODO", done: false })]), true);
  assert.equal(hasOutlineFilter([heading("a", 1, { tags: ["x"] })]), true);
});
