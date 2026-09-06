// The folding rule, against a history the test writes.
//
// The panel's one piece of judgement is what it leaves out. A working
// afternoon is thirty quiet checkpoints by one author, and a column of thirty
// identical rows is a column nobody reads; a labelled checkpoint is what
// somebody came to find and must never be folded away. Those two are what is
// checked here, because they are what a careless change would break silently:
// the panel would still render, and the history would just be harder to read.

import { timeline } from "../src/lib/history.js";

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

if (failures) process.exit(1);
console.log("timeline: the newest first, runs folded, and a named point never hidden");
