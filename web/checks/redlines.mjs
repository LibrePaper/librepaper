// Redlines (track changes), the browser half: hunks to paint items, and
// attribution across the checkpoints between a baseline and a target.
// `docs/specs/track-changes.md`, "Browser: redlines" and "Tests", is the
// contract.

import { attribution, itemsFor } from "../src/lib/redlines.js";

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

if (failures) {
  console.error(`redlines: ${failures} check(s) failed`);
  process.exit(1);
} else {
  console.log("redlines: ok");
}
