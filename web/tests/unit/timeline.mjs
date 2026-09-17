// The timeline, against a history the test writes.
//
// What a careless change would break silently is the order -- the panel would
// still render, and the history would just be wrong about when things
// happened -- so that is what is checked here, along with the paging the
// manifest is read through. How the order is then coarsened into what the
// panel shows is checked in history-calendar.mjs.

import { checkpointOrder, loadWithStatus } from "../../src/lib/history.js";

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

const order = (points) => [...points].sort(checkpointOrder).map((point) => point.sha);

/* --------------------------------------------------------------- the order */

{
  // Oldest first, and a day apart is still an order: the calendar groups by
  // day but every reading of the manifest starts from this one comparison.
  const points = [...marks("05", "anne", 2), ...marks("03", "vincent", 2)];
  const sorted = order(points);
  check("the oldest comes first", sorted[0].startsWith("03-vincent-0"));
  check("and the newest last", sorted[3].startsWith("05-anne-1"));
  check("nothing is dropped", sorted.length === 4);
}

{
  // Two checkpoints taken in the same second are ordered by the sequence the
  // server wrote, not by whichever the JSON happened to list first.
  const tied = marks("05", "anne", 3).map((point, index) => ({
    ...point, at: "2026-09-05T09:00:00Z", seq: index + 1,
  }));
  check("a timestamp tie is broken by sequence",
    order([tied[2], tied[0], tied[1]]).join() === order(tied).join());
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
  } finally { globalThis.fetch = originalFetch; }
}

if (failures) process.exit(1);
console.log("timeline: the manifest read oldest first, and across pages");
