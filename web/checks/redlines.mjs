// Redlines (track changes), the browser half: hunks to paint items, and
// attribution across the checkpoints between a baseline and a target.

import { attribution, attributeChain, authorIndex, itemsFor } from "../src/lib/redlines.js";

let failures = 0;
function check(what, condition) {
  if (condition) return;
  failures += 1;
  console.error(`redlines: ${what}`);
}

/* ---------------------------------------------------------- attribution */

{
  // Oldest first, as the manifest lists them.
  const checkpoints = [
    { sha: "a", by: "vincent" },
    { sha: "b", by: "vincent" },
    { sha: "c", by: "sam" },
    { sha: "d", by: "sam" },
  ];
  check(
    "disagreement across the whole span since the baseline is several people",
    attribution(checkpoints, "a", null) === "several people", // b:vincent, c:sam, d:sam differ
  );
  check(
    "agreement through a single later checkpoint is named",
    attribution(checkpoints, "a", "b") === "vincent",
  );
  check(
    "disagreement among the checkpoints since the baseline is several people",
    attribution(checkpoints, "a", "c") === "several people",
  );
  check(
    "agreement among several checkpoints since the baseline is that name",
    attribution(checkpoints, "b", "c") === "sam",
  );
  check(
    "the baseline itself contributes nothing -- only what came after it",
    attribution(checkpoints, "d", null) === "",
  );
  check("an unknown baseline attributes nothing", attribution(checkpoints, "nope", null) === "");
  check("a checkpoint with no `by` is not counted", attribution([{ sha: "a", by: "" }, { sha: "b", by: "" }], "a", null) === "");
}

/* -------------------------------------------------------------- itemsFor */

{
  const insertHunk = { position: 10, kind: "insert", insert: "new words", old: "" };
  const items = itemsFor([insertHunk], "vincent");
  check("an insert hunk becomes one insert item", items.length === 1 && items[0].kind === "insert");
  check("the insert item spans the inserted text", items[0].start === 10 && items[0].end === 10 + "new words".length);
  check("the insert item carries who", items[0].who === "vincent");
  check("an insert item has no `at`", items[0].at === undefined);
}

{
  const deleteHunk = { position: 4, kind: "delete", insert: "", old: "gone" };
  const items = itemsFor([deleteHunk], "sam");
  check("a delete hunk becomes one delete item", items.length === 1 && items[0].kind === "delete");
  check("the delete item sits at the hunk's position", items[0].at === 4);
  check("the delete item carries the removed text", items[0].text === "gone");
  check("a delete item has no `start`/`end`", items[0].start === undefined && items[0].end === undefined);
}

{
  const replaceHunk = { position: 7, kind: "replace", insert: "slow", old: "quick" };
  const items = itemsFor([replaceHunk], "several people");
  check("a replace hunk becomes two items", items.length === 2);
  const insert = items.find((item) => item.kind === "insert");
  const del = items.find((item) => item.kind === "delete");
  check("its insert item spans the new text at the hunk's position", insert && insert.start === 7 && insert.end === 11);
  check("its delete item sits at the same position with the old text", del && del.at === 7 && del.text === "quick");
}

{
  // An edit with nothing on one side of a `replace` -- which `hunks()` never
  // actually produces, since that is an insert or a delete, not a replace --
  // is defended anyway: an empty side contributes no item.
  check("an empty insert side contributes nothing", itemsFor([{ position: 0, kind: "replace", insert: "", old: "x" }], "").length === 1);
  check("an empty delete side contributes nothing", itemsFor([{ position: 0, kind: "replace", insert: "x", old: "" }], "").length === 1);
  check("no hunks makes no items", itemsFor([], "vincent").length === 0);
}

/* ----------------------------------------------------------- authorIndex */

{
  const checkpoints = [
    { sha: "a", by: "vincent" },
    { sha: "b", by: "sam" },
    { sha: "c", by: "vincent" },
    { sha: "d", by: "" },
  ];
  const authors = authorIndex(checkpoints);
  check("the first author to appear is index 0", authors.get("vincent") === 0);
  check("the second author to appear is index 1", authors.get("sam") === 1);
  check("a checkpoint with no `by` contributes no entry", authors.size === 2);
  check("an author who never appears has no entry", authors.has("nope") === false);
}

/* ------------------------------------------------------- attributeChain */

{
  // Two authors editing different sentences: each hunk keeps its own
  // author, since neither step's insertion overlaps the other's.
  const steps = [
    { by: "vincent", hunks: [{ at: 5, delete: 0, insert: "AAA", old: "" }] },
    { by: "sam", hunks: [{ at: 30, delete: 0, insert: "BBB", old: "" }] },
  ];
  const hunks = [
    { position: 5, kind: "insert", insert: "AAA", old: "" },
    { position: 30, kind: "insert", insert: "BBB", old: "" },
  ];
  const attributed = attributeChain(steps, hunks, "several people");
  check("the first author's own sentence is attributed to them", attributed[0].who === "vincent");
  check("the second author's own sentence is attributed to them", attributed[1].who === "sam");
}

{
  // One author re-editing another's insertion: sam's step deletes exactly
  // what vincent inserted and replaces it, so vincent's span is fully
  // superseded and the surviving text is attributed to sam -- the latest
  // author wins.
  const steps = [
    { by: "vincent", hunks: [{ at: 10, delete: 0, insert: "hello", old: "" }] },
    { by: "sam", hunks: [{ at: 10, delete: 5, insert: "world", old: "hello" }] },
  ];
  const hunks = [{ position: 10, kind: "insert", insert: "world", old: "" }];
  const attributed = attributeChain(steps, hunks, "several people");
  check("the latest author to touch a passage is credited for it", attributed[0].who === "sam");
}

{
  // A step's own deletion is matched to a range-level delete hunk by final
  // offset and text.
  const steps = [{ by: "sam", hunks: [{ at: 4, delete: 4, insert: "", old: "gone" }] }];
  const hunks = [{ position: 4, kind: "delete", insert: "", old: "gone" }];
  const attributed = attributeChain(steps, hunks, "several people");
  check("a matched deletion is attributed to the author who deleted it", attributed[0].who === "sam");
}

{
  // Nothing to match: no steps at all, and a step whose deletion does not
  // line up with the range-level hunk (different text at that offset).
  // Both fall back to the range-level name.
  const hunks = [{ position: 0, kind: "insert", insert: "new words", old: "" }];
  check("no steps falls back to the range-level name", attributeChain([], hunks, "several people")[0].who === "several people");

  const mismatched = [{ by: "sam", hunks: [{ at: 4, delete: 3, insert: "", old: "old" }] }];
  const deleteHunks = [{ position: 4, kind: "delete", insert: "", old: "different" }];
  check(
    "an unmatched deletion falls back to the range-level name",
    attributeChain(mismatched, deleteHunks, "several people")[0].who === "several people",
  );
}

{
  // `itemsFor` prefers a hunk's own `who` over the fallback it is given.
  const hunks = [
    { position: 0, kind: "insert", insert: "hi", old: "", who: "vincent" },
    { position: 20, kind: "insert", insert: "bye", old: "" },
  ];
  const items = itemsFor(hunks, "several people");
  check("a hunk with `who` keeps its own attribution", items[0].who === "vincent");
  check("a hunk without `who` falls back to the given name", items[1].who === "several people");
}

if (failures) {
  console.error(`redlines: ${failures} check(s) failed`);
  process.exit(1);
} else {
  console.log("redlines: ok");
}
