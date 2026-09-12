// The folding rule, against a history the test writes.
//
// The panel's one piece of judgement is what it leaves out. A working
// afternoon is thirty quiet checkpoints by one author, and a column of thirty
// identical rows is a column nobody reads; a labelled checkpoint is what
// somebody came to find and must never be folded away. Those two are what is
// checked here, because they are what a careless change would break silently:
// the panel would still render, and the history would just be harder to read.

import { coalesce, sizeDelta, timeline } from "../../src/lib/history.js";

let failures = 0;
function check(what, condition) {
  if (condition) return;
  failures += 1;
  console.error(`timeline: ${what}`);
}

/// `n` checkpoints, one per minute, on the day given, by whoever is named.
function marks(day, by, n, from = 0) {
  return Array.from({ length: n }, (_, at) => ({
    sha: `${day}-${by}-${from + at}`.padEnd(64, "0"),
    at: `2026-09-${day}T09:${String(from + at).padStart(2, "0")}:00Z`,
    by,
    why: "quiet",
    label: "",
  }));
}

const rowsOf = (days) => days.flatMap((day) => day.rows);
const shas = (rows) =>
  rows.flatMap((row) =>
    row.kind === "point" ? [row.point.sha] : [row.first.sha, ...row.hidden.map((p) => p.sha), row.last.sha],
  );

/* --------------------------------------------------------------- the order */

{
  const days = timeline([...marks("03", "vincent", 2), ...marks("05", "anne", 2)]);
  check(
    "the newest day comes first",
    days.length === 2 && rowsOf(days)[0].point.by === "anne",
  );
  const order = shas(rowsOf(days));
  check("the newest mark within a day comes first", order[0].startsWith("05-anne-1"));
}

/* -------------------------------------------------------------- the folding */

{
  // Two in a row is not a run: folding one row hides nothing and costs a click.
  const rows = rowsOf(timeline(marks("05", "vincent", 2)));
  check("a run of two is not folded", rows.every((row) => row.kind === "point"));
}

{
  const rows = rowsOf(timeline(marks("05", "vincent", 30)));
  check("a run of thirty is one folded row", rows.length === 1 && rows[0].kind === "folded");
  check("the folded row keeps its ends", rows[0].first.sha !== rows[0].last.sha);
  check("nothing is lost by folding", shas(rows).length === 30);
}

{
  // A change of hand ends a run: two people working the same afternoon is
  // exactly what somebody opens the panel to see.
  const rows = rowsOf(
    timeline([...marks("05", "vincent", 4), ...marks("05", "anne", 4, 10)]),
  );
  check("a run is one person's", rows.length === 2 && rows.every((row) => row.kind === "folded"));
  check("each run keeps its author", rows[0].first.by !== rows[1].first.by);
}

{
  // A label is somebody saying this moment matters, so it survives whatever is
  // on either side of it.
  const points = marks("05", "vincent", 9);
  points[4].label = "sent to the journal";
  const rows = rowsOf(timeline(points));
  const named = rows.filter((row) => row.kind === "point" && row.point.label);
  check("a labelled checkpoint is never folded away", named.length === 1);
  check("a label breaks the run around it", rows.length === 3);
  check("nothing is lost around a label", shas(rows).length === 9);
}

{
  // A fifteen-minute pause starts another session, even with the same author
  // and calendar day. The manifest is oldest first; timeline reverses it.
  const points = marks("05", "vincent", 7);
  points[3].at = "2026-09-05T09:20:00Z";
  points[4].at = "2026-09-05T09:19:00Z";
  const rows = rowsOf(timeline(points));
  check("a fifteen-minute gap starts a new session", rows.length === 2 && rows.every((row) => row.kind === "folded"));
  check("the gap split preserves every checkpoint", shas(rows).length === points.length);
}

{
  const points = marks("05", "vincent", 7);
  points[3].why = "restore";
  const rows = rowsOf(timeline(points));
  check("a restoration milestone remains visible", rows.some((row) => row.kind === "point" && row.point.why === "restore"));
  check("a milestone splits routine sessions", rows.length === 3);
  check("a milestone split preserves every checkpoint", shas(rows).length === points.length);
}

/* ------------------------------------------------------- one change, read */

{
  // "Meet the Model That not shown" became "Meet the Model That Survived the
  // Auditions": two word edits a person reads as one rewritten heading.
  const heading = [
    { position: 20, old: "not", insert: "Survived", currentBefore: "Meet the Model That ", currentAfter: " the Auditions Every table is one", path: "main.md" },
    { position: 29, old: "shown", insert: "the Auditions", currentBefore: "Model That Survived ", currentAfter: " Every table is one column of", path: "main.md" },
  ];
  const far = { position: 400, old: "On", insert: "Four", currentBefore: "artefact of pooling. ", currentAfter: " Decimal Places in a", path: "main.md" };
  const groups = coalesce([...heading, far]);
  check("adjacent edits are one change", groups.length === 2);
  check("the words between them are kept", groups[0].parts.length === 3 && groups[0].parts[1].keep === " ");
  check("the change begins where the first edit did", groups[0].position === 20);
  check("and spans to the end of the last insertion", groups[0].length === 29 + "the Auditions".length - 20);
  check("the context is the first's before and the last's after", groups[0].before === heading[0].currentBefore && groups[0].after === heading[1].currentAfter);
  check("a distant edit is its own change", groups[1].hunks.length === 1 && groups[1].position === 400);
  check("nothing to read is nothing", coalesce([]).length === 0);
}

/* ------------------------------------------------------------- size bars */

{
  const checkpoints = [
    { sha: "a".padEnd(64, "0"), size: 1000 },
    { sha: "b".padEnd(64, "0"), size: 1312, parent: "a".padEnd(64, "0") },
    { sha: "c".padEnd(64, "0"), size: 900, parent: "b".padEnd(64, "0") },
  ];
  const grew = sizeDelta(checkpoints[1], checkpoints);
  check("a checkpoint that grew reports it grew", grew && grew.grew === true);
  check("its title says how much", grew?.title === "+312 bytes");
  check("its width is within the drawn range", grew.width >= 2 && grew.width <= 48);

  const shrank = sizeDelta(checkpoints[2], checkpoints);
  check("a checkpoint that shrank reports it shrank", shrank && shrank.grew === false);
  check("its title uses the minus sign", shrank?.title === "−412 bytes");

  check("no parent means no bar", sizeDelta(checkpoints[0], checkpoints) === null);
  check(
    "an unknown parent means no bar",
    sizeDelta({ sha: "d".padEnd(64, "0"), size: 5, parent: "missing" }, checkpoints) === null,
  );
  check(
    "no size on either end means no bar",
    sizeDelta({ sha: "e".padEnd(64, "0"), parent: "a".padEnd(64, "0") }, checkpoints) === null,
  );

  const small = sizeDelta({ sha: "f".padEnd(64, "0"), size: 1005, parent: "a".padEnd(64, "0") }, checkpoints);
  const large = sizeDelta({ sha: "g".padEnd(64, "0"), size: 6000, parent: "a".padEnd(64, "0") }, checkpoints);
  check("a small edit still draws a bar", small.width >= 2);
  check("a large paste draws a wider one", large.width > small.width);
}

if (failures) process.exit(1);
console.log("timeline: the newest first, runs folded, and a named point never hidden");
