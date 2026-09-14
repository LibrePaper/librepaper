// The timeline, against a history the test writes.
//
// The panel is a list of every version, newest first, grouped by the day the
// reader is in. What a careless change would break silently is the order --
// the panel would still render, and the history would just be wrong about
// when things happened -- so that is what is checked here, along with the
// paging and tie-breaking the list is built from.

import { loadWithStatus, sizeDelta, timeline } from "../../src/lib/history.js";

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

const shas = (days) => days.flatMap((day) => day.points.map((point) => point.sha));

/* --------------------------------------------------------------- the order */

{
  const days = timeline([...marks("03", "vincent", 2), ...marks("05", "anne", 2)]);
  check("the newest day comes first", days.length === 2 && days[0].points[0].by === "anne");
  check("the newest mark within a day comes first", shas(days)[0].startsWith("05-anne-1"));
  check("every day keeps its own marks", days[0].points.length === 2 && days[1].points.length === 2);
}

{
  // Every version is a row of its own: a project's history is read as what
  // happened and when, and a row standing for several moments is a row the
  // reader has to open before it says anything.
  const points = marks("05", "vincent", 30);
  const days = timeline(points);
  check("thirty quiet checkpoints are thirty rows", days.length === 1 && days[0].points.length === 30);
  check("nothing is dropped", shas(days).length === 30);
}

{
  const points = marks("05", "vincent", 9);
  points[4].label = "sent to the journal";
  const days = timeline(points);
  check("a named checkpoint is a row like the others", days[0].points.filter((point) => point.label).length === 1);
}

/* ---------------------------------------------------------------- the pages */

{
  const points = marks("05", "anne", 3).map((point, i) => ({ ...point, at: "2026-09-05T09:00:00Z", seq: i + 1 }));
  const originalFetch = globalThis.fetch;
  const urls = [];
  globalThis.fetch = async url => {
    urls.push(url);
    return { ok: true, json: async () => urls.length === 1
      ? { checkpoints: [points[2], points[1]], next_cursor: 2, durability: {live_save:"saved"} }
      : { checkpoints: [points[0]] } };
  };
  try {
    const loaded = await loadWithStatus("paper");
    check("history follows the server's page cursor", urls[1] === "/api/documents/paper/history?after=2");
    check("the manifest comes back oldest first across all pages", loaded.checkpoints.map(point => point.seq).join() === "1,2,3");
    check("the first page's durability is retained", loaded.durability.live_save === "saved");
    check("timeline breaks timestamp ties by sequence", shas(timeline(loaded.checkpoints))[0] === points[2].sha);
  } finally { globalThis.fetch = originalFetch; }
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
console.log("timeline: the newest first, one row per version, grouped by the reader's day");
