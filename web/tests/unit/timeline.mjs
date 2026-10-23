// The timeline, against a history the test writes.
//
// What a careless change would break silently is the order -- the panel would
// still render, and the history would just be wrong about when things
// happened -- so that is what is checked here, along with the paging the
// manifest is read through. How the order is then coarsened into what the
// panel shows is checked in history-days.mjs.

import { read, label, labelOrder, loadPage } from "../../src/lib/history.js";
import { createTimeline } from "../../src/lib/reader/timeline.svelte.js";

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
  // (`handle_history`'s `label_wire`, see room.md "Labels"): `sequence`
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
    const first = await loadPage("paper");
    const second = await loadPage("paper", {}, { after: first.next });
    check("loadPage asks for the newest page first, with the default limit",
      urls[0] === "/api/documents/paper/history?limit=200");
    check("loadPage follows the server's page cursor", urls[1] === "/api/documents/paper/history?after=2&limit=200");
    check("a page's rows stay newest first, as served", first.labels.map(point => point.seq).join() === "3,2");
    check("the next cursor is the server's, and null at the end", first.next === 2 && second.next === null);
    check("the storage row's sequence is read as seq, the name this codebase's panel and calendar use",
      first.labels[0].seq === 3);
    check("the storage row's reason is read as why, for the same reason",
      first.labels.concat(second.labels).map(point => point.why).join() === "restore,cli,sync");
  } finally { globalThis.fetch = originalFetch; }
}

{
  // The query carries `after` only when there is a cursor, and `named=1` only
  // for saved labels. A cursor that does not move back, or is not a positive
  // integer, is refused rather than followed.
  const originalFetch = globalThis.fetch;
  const urls = [];
  let nextCursor = 5;
  globalThis.fetch = async url => {
    urls.push(url);
    return { ok: true, json: async () => ({ labels: [], next_cursor: nextCursor }) };
  };
  const refused = async (run) => {
    try { await run(); return false; } catch { return true; }
  };
  try {
    await loadPage("paper", {}, { named: true, limit: 50 });
    await loadPage("paper", {}, { after: 10 });
    check("a named page asks for named=1 with its limit", urls[0] === "/api/documents/paper/history?limit=50&named=1");
    check("a page after a cursor sends that cursor and no named flag", urls[1] === "/api/documents/paper/history?after=10&limit=200");
    nextCursor = 10;
    check("a cursor that does not move back is refused", await refused(() => loadPage("paper", {}, { after: 10 })));
    nextCursor = 0;
    check("a zero cursor is refused", await refused(() => loadPage("paper")));
    nextCursor = 1.5;
    check("a fractional cursor is refused", await refused(() => loadPage("paper")));
    globalThis.fetch = async () => ({ ok: false, json: async () => ({}) });
    check("an unreadable history is refused", await refused(() => loadPage("paper")));
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

{
  // A current-label response can be lost after the server commits. The
  // timeline must retain that operation id across a retry of the same
  // normalized label, while still forwarding the document key on both tries.
  globalThis.$state ??= (value) => value;
  const calls = [];
  const history = {
    loadPage: async () => ({ labels: [], next: null }),
    label: async (...args) => {
      calls.push(args);
      if (calls.length === 1) throw new Error("response lost");
      return {};
    },
  };
  const timeline = createTimeline({ slug: "paper", key: "document-key", history });
  await timeline.name("current", "  A   label  ");
  await timeline.name("current", "A label");
  check("current-label retry retains its operation id",
    Boolean(calls[0][4]) && calls[0][4] === calls[1][4]);
  check("current-label retry keeps the document capability header",
    calls.every(([, , , headers]) => headers["X-LibrePaper-Key"] === "document-key"));
}

{
  const originalFetch = globalThis.fetch;
  let request;
  globalThis.fetch = async (_url, options) => {
    request = options;
    return { ok: true, json: async () => ({ sha: "same-label" }) };
  };
  try {
    await label("paper", "current", "Named", {
      "X-LibrePaper-Key": "document-key",
    }, "stable-request-id");
    check("label adapter sends the retry identity", JSON.parse(request.body).request_id === "stable-request-id");
    check("label adapter preserves document credentials",
      request.headers["X-LibrePaper-Key"] === "document-key");
    check("label adapter adds the shell request marker",
      request.headers["X-LibrePaper-Client"] === "shell");
  } finally { globalThis.fetch = originalFetch; }
}

/* ------------------------------------------------------- the controller */

/// Points `seq` minutes after nine, by one author, served the way the server
/// serves them: newest first, `size` at a time, with a cursor while older
/// rows remain. `named` filters to saved points, as `?named=1` does.
function served(seqs, size = 3) {
  const all = seqs.map((seq) => ({
    sha: `sha-${seq}`.padEnd(64, "0"),
    seq,
    at: new Date(Date.UTC(2026, 8, 5, 9, seq)).toISOString(),
    by: "anne",
    why: "sync",
    label: "",
  }));
  const history = {
    calls: [],
    fail: false,
    loadPage: async (_slug, _headers, { after = null, named = false } = {}) => {
      history.calls.push({ after, named });
      if (history.fail) throw new Error("older page failed");
      const pool = all
        .filter((point) => (after === null || point.seq < after) && (!named || point.label))
        .sort((left, right) => right.seq - left.seq);
      const labels = pool.slice(0, size).map((point) => ({ ...point }));
      return { labels, next: pool.length > size ? labels[labels.length - 1].seq : null };
    },
    label: async (_slug, sha, text) => {
      const point = all.find((entry) => entry.sha === sha);
      point.label = text.trim();
      return {};
    },
    all,
  };
  return history;
}

const seqsOf = (points) => points.map((point) => point.seq).join();

{
  // The first load reads the newest page only, and a refresh reads no further
  // than the rows it already had, so older pages are left for loadMore.
  const history = served([1, 2, 3, 4, 5, 6, 7]);
  const timeline = createTimeline({ slug: "paper", history });
  await timeline.load();
  check("the first load reads one page", history.calls.length === 1);
  check("the first load keeps the newest rows, oldest first", seqsOf(timeline.state.labels) === "5,6,7");
  check("and is not complete", timeline.state.loaded && !timeline.state.complete);

  await timeline.loadMore();
  check("loadMore appends the next older page", seqsOf(timeline.state.labels) === "2,3,4,5,6,7");
  check("and sets complete when the page is the last", !timeline.state.complete);

  history.all.push(...served([8]).all);
  const before = history.calls.length;
  await timeline.load();
  check("a refresh reads newer pages until it reaches a known row", history.calls.length - before === 1);
  check("a refresh adds the new row and keeps the older ones", seqsOf(timeline.state.labels) === "2,3,4,5,6,7,8");
  check("a refresh does not change complete", !timeline.state.complete);

  await timeline.loadMore();
  check("loadMore reads the last page", seqsOf(timeline.state.labels) === "1,2,3,4,5,6,7,8");
  check("and then the history is complete", timeline.state.complete);
  await timeline.loadMore();
  check("loadMore is a no-op once complete", history.calls.length === before + 2);
}

{
  // A failed older page keeps the rows already shown and says why; the next
  // successful page clears the message.
  const history = served([1, 2, 3, 4, 5, 6, 7]);
  const timeline = createTimeline({ slug: "paper", history });
  await timeline.load();
  history.fail = true;
  await timeline.loadMore();
  check("a failed older page keeps the rows", seqsOf(timeline.state.labels) === "5,6,7");
  check("and says why", timeline.state.moreProblem === "older page failed");
  check("and is not loading any more", !timeline.state.loadingMore);
  history.fail = false;
  await timeline.loadMore();
  check("a successful older page clears the message", timeline.state.moreProblem === "");
}

{
  // loadUntil reads back to the date and stops there, without reading the
  // rest of the history.
  const history = served([1, 2, 3, 4, 5, 6, 7, 8, 9]);
  const timeline = createTimeline({ slug: "paper", history });
  await timeline.load();
  const until = new Date(Date.UTC(2026, 8, 5, 9, 4)).toISOString();
  await timeline.loadUntil(until);
  check("loadUntil stops once the oldest loaded point is at or before the date",
    timeline.state.labels[0].seq === 4);
  check("and has not read the rest of the history", !timeline.state.complete);
}

{
  // loadSaved reads every page of named points, newest first.
  const history = served([1, 2, 3, 4, 5, 6, 7]);
  history.all[0].label = "first";
  history.all[2].label = "third";
  history.all[5].label = "sixth";
  const timeline = createTimeline({ slug: "paper", history });
  await timeline.load();
  await timeline.loadSaved();
  check("loadSaved collects every named page", seqsOf(timeline.state.saved) === "6,3,1");
  check("and reports that it has loaded", timeline.state.savedLoaded && !timeline.state.savedLoading);
  check("and reads its pages as named", history.calls[history.calls.length - 1].named);
}

{
  // A rename patches the old row in place, so a refresh that does not read
  // that row again still shows the new name.
  const history = served([1, 2, 3, 4, 5, 6, 7]);
  const timeline = createTimeline({ slug: "paper", history });
  await timeline.load();
  await timeline.loadMore();
  await timeline.name(history.all[1].sha, "  Renamed   draft ");
  const renamed = timeline.state.labels.find((point) => point.seq === 2);
  check("name patches an old row locally, trimmed", renamed.label === "Renamed draft");
  check("and the rest of the rows are kept", seqsOf(timeline.state.labels) === "2,3,4,5,6,7");
}

if (failures) process.exit(1);
console.log("timeline: the manifest read oldest first, and across pages");
