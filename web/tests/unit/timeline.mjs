// The timeline, against a history the test writes.
//
// What a careless change would break silently is the order -- the panel would
// still render, and the history would just be wrong about when things
// happened -- so that is what is checked here, along with the paging the
// manifest is read through. How the order is then coarsened into what the
// panel shows is checked in history-calendar.mjs.

import { read, labelOrder, loadWithStatus } from "../../src/lib/history.js";

let failures = 0;
function check(what, condition) {
  if (condition) return;
  failures += 1;
  console.error(`timeline: ${what}`);
}

/// `n` labels, one per minute, on the day given, by whoever is named.
function marks(day, by, n, from = 0) {
  return Array.from({ length: n }, (_, at) => ({
    sha: `${day}-${by}-${from + at}`.padEnd(64, "0"),
    at: `2026-09-${day}T09:${String(from + at).padStart(2, "0")}:00Z`,
    by,
    why: "quiet",
    label: "",
  }));
}

const order = (points) => [...points].sort(labelOrder).map((point) => point.sha);

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
  // Two labels taken in the same second are ordered by the sequence the
  // server wrote, not by whichever the JSON happened to list first.
  const tied = marks("05", "anne", 3).map((point, index) => ({
    ...point, at: "2026-09-05T09:00:00Z", seq: index + 1,
  }));
  check("a timestamp tie is broken by sequence",
    order([tied[2], tied[0], tied[1]]).join() === order(tied).join());
}

/* ---------------------------------------------------------------- the pages */

{
  // The wire is `document_labels` rows under `labels`, not `checkpoints`
  // (`handle_history`'s `label_wire`, room-v2.md's timeline): `sequence`
  // where this codebase reads `seq`, and `reason` where it reads `why`. A
  // page keyed on the old names would come back looking empty rather than
  // wrong -- `payload.labels ?? []` never throws -- so this checks the
  // real wire shape, not the one a careless mock would silently agree with.
  const wireRow = (sha, by, sequence, reason) => ({
    sha: sha.padEnd(64, "0"), sequence, at: "2026-09-05T09:00:00Z", by, label: "",
    reason, tree_sha: null, frontier: "", archive_status: "none",
  });
  const rows = [
    wireRow("05-anne-0", "anne", 1, "sync"),
    wireRow("05-anne-1", "anne", 2, "cli"),
    wireRow("05-anne-2", "anne", 3, "restore"),
  ];
  const originalFetch = globalThis.fetch;
  const urls = [];
  globalThis.fetch = async url => {
    urls.push(url);
    return { ok: true, json: async () => urls.length === 1
      ? { slug: "paper", main: "main.md", labels: [rows[2], rows[1]], next_cursor: 2 }
      : { slug: "paper", main: "main.md", labels: [rows[0]] } };
  };
  try {
    const loaded = await loadWithStatus("paper");
    check("history follows the server's page cursor", urls[1] === "/api/documents/paper/history?after=2");
    check("the manifest comes back oldest first across all pages", loaded.labels.map(point => point.seq).join() === "1,2,3");
    check("the storage row's sequence is read as seq, the name this codebase's panel and calendar use",
      loaded.labels[0].seq === 1);
    check("the storage row's reason is read as why, for the same reason",
      loaded.labels.map(point => point.why).join() === "sync,cli,restore");
    check("loadWithStatus no longer reports a durability field nothing on the wire ever fills",
      !("durability" in loaded));
  } finally { globalThis.fetch = originalFetch; }
}

{
  // A label's file entries are `librepaper_document_core::projection::
  // Entry`, whose digest field is spelled `digest`; the rest of this
  // codebase's own trees spell an asset's digest `sha`, which is what
  // `history-source.svelte.js` compares both sides of a version diff by.
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async () => ({
    ok: true,
    json: async () => ({
      sha: "abc", files: { "figure.png": { kind: "asset", id: "", digest: "deadbeef", bytes: 0 } },
      texts: {},
    }),
  });
  try {
    const point = await read("paper", "abc");
    check("a label's asset entry carries sha, the name the comparison reads",
      point.files["figure.png"].sha === "deadbeef");
    check("and keeps digest too, since nothing needs it hidden",
      point.files["figure.png"].digest === "deadbeef");
  } finally { globalThis.fetch = originalFetch; }
}

if (failures) process.exit(1);
console.log("timeline: the manifest read oldest first, and across pages");
