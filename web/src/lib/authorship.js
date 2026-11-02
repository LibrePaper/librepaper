// Who wrote which part of the document.
//
// Every character in a Loro text remembers the operation that inserted it, and
// every operation belongs to a change that carries a timestamp when the
// document records them. This module reads those back into contiguous runs of
// text, groups the runs by person, and turns the result into the marks the
// editor draws. Nothing here writes to the document.
//
// Offsets are UTF-16 throughout, the same as CodeMirror and loro-crdt in JS,
// so a run's bounds can be handed to the editor as they are.
import { SHELL_HEADERS } from "./api.js";
import { m } from "./paraglide/messages.js";
import { locale } from "./locale.js";
import { presenceColour } from "./presence-colour.js";

const asked = (headers) => ({ ...SHELL_HEADERS, ...headers });

/// Contiguous runs of the current text by who inserted them. Returns
/// `[{from, to, peer, at}]` in UTF-16 offsets, ascending, covering the whole
/// text, with adjacent runs of the same peer merged. `peer` is the Loro PeerID
/// string, or "" when the document does not know who inserted a character.
/// `at` is the latest change timestamp in the run, in unix seconds, or 0 when
/// none was recorded.
export function authorshipRuns(doc, text) {
  const source = text.toString();
  const runs = [];
  // The last Change read for each peer. A change is many characters, so the
  // timestamp is looked up once per change rather than once per character.
  const changes = new Map();
  let offset = 0;
  for (const char of source) {
    const width = char.length;
    const cursor = text.getCursor(offset, 0);
    const id = cursor?.pos();
    cursor?.free?.();
    const peer = id ? id.peer : "";
    const at = id ? timestampOf(doc, changes, id) : 0;
    const last = runs[runs.length - 1];
    if (last && last.peer === peer) {
      last.to = offset + width;
      last.at = Math.max(last.at, at);
    } else {
      runs.push({ from: offset, to: offset + width, peer, at });
    }
    offset += width;
  }
  return runs;
}

/// The timestamp of the change that holds `id`, in seconds, or 0 when the
/// document cannot say. A shallow document may not have the change at all.
function timestampOf(doc, changes, id) {
  const known = changes.get(id.peer);
  if (known && id.counter >= known.counter && id.counter < known.counter + known.length) {
    return known.timestamp || 0;
  }
  try {
    const change = doc.getChangeAt({ peer: id.peer, counter: id.counter });
    changes.set(id.peer, change);
    return change.timestamp || 0;
  } catch {
    // Remember the miss for this one operation, so it is not asked again.
    changes.set(id.peer, { counter: id.counter, length: 1, timestamp: 0 });
    return 0;
  }
}

/// Loads who each recorded Loro peer is. The response lists the peers the
/// server has seen for this document. Returns a Map from peer string to
/// `{name, accountId, agent, firstSeen}`, with `firstSeen` a Date.
export async function loadAuthors(slug, headers = {}) {
  const response = await fetch(`/api/documents/${slug}/authors`, {
    headers: asked(headers),
    cache: "no-store",
  });
  if (!response.ok) throw new Error(m.history_authors_failed());
  const payload = await response.json();
  const authors = new Map();
  for (const row of payload.authors || []) {
    authors.set(row.peer, {
      name: row.name || "",
      accountId: row.account_id || null,
      agent: Boolean(row.agent),
      firstSeen: new Date(row.first_seen),
    });
  }
  return authors;
}

/// The key a run is grouped under: the account when there is one, else the
/// name, else "unknown". Peers absent from the map are unknown.
function keyOf(peer, authors) {
  const author = peer ? authors.get(peer) : undefined;
  if (!author) return "unknown";
  if (author.accountId) return `account:${author.accountId}`;
  if (author.name) return `name:${author.name}`;
  return "unknown";
}

/// Groups runs by person and says who wrote how much. Returns
/// `{rows, since, missing}`:
///  - `rows`: `[{key, name, agent, unknown, colour, chars, share, last}]`,
///    sorted by characters descending with the unknown row last. Several
///    peers of one person share one row. `chars` counts UTF-16 units, `share`
///    is `chars / total`, and `last` is the Date of the latest change in the
///    row, or null when none was recorded.
///  - `since`: the earliest first-seen Date in the map, or null.
///  - `missing`: true when a run's peer is absent from the map, so the caller
///    should fetch the list again.
export function summarize(runs, authors) {
  const byKey = new Map();
  let total = 0;
  let missing = false;
  for (const run of runs) {
    if (run.peer && !authors.has(run.peer)) missing = true;
    const key = keyOf(run.peer, authors);
    const author = authors.get(run.peer);
    const chars = run.to - run.from;
    total += chars;
    let row = byKey.get(key);
    if (!row) {
      row = {
        key,
        name: key === "unknown" ? m.history_authors_unknown() : author.name,
        agent: key === "unknown" ? false : author.agent,
        unknown: key === "unknown",
        colour: "",
        chars: 0,
        lastAt: 0,
      };
      if (!row.unknown && row.agent) row.name = m.history_authors_agent({ name: row.name });
      byKey.set(key, row);
    }
    row.chars += chars;
    row.lastAt = Math.max(row.lastAt, run.at || 0);
  }

  const rows = [...byKey.values()].sort((a, b) => {
    if (a.unknown !== b.unknown) return a.unknown ? 1 : -1;
    return b.chars - a.chars;
  });
  const taken = [];
  for (const row of rows) {
    if (row.unknown) continue;
    row.colour = presenceColour(row.key, taken);
    taken.push(row.colour);
  }

  let since = null;
  for (const author of authors.values()) {
    if (since === null || author.firstSeen < since) since = author.firstSeen;
  }

  return {
    rows: rows.map((row) => ({
      key: row.key,
      name: row.name,
      agent: row.agent,
      unknown: row.unknown,
      colour: row.colour,
      chars: row.chars,
      share: total ? row.chars / total : 0,
      last: row.lastAt ? new Date(row.lastAt * 1000) : null,
    })),
    since,
    missing,
  };
}

/// The decorations to draw: `[{from, to, key, colour, title}]` for every run
/// whose row is not unknown. The title says who and when, in the interface
/// language.
export function markSpans(runs, authors, rows) {
  const byKey = new Map(rows.map((row) => [row.key, row]));
  const format = new Intl.DateTimeFormat(locale(), { dateStyle: "medium", timeStyle: "short" });
  const spans = [];
  for (const run of runs) {
    const key = keyOf(run.peer, authors);
    const row = byKey.get(key);
    if (!row || row.unknown) continue;
    const title = run.at
      ? m.authorship_hover({ name: row.name, date: format.format(new Date(run.at * 1000)) })
      : m.authorship_hover_undated({ name: row.name });
    spans.push({ from: run.from, to: run.to, key, colour: row.colour, title });
  }
  return spans;
}
