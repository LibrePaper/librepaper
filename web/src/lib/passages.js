// Where a passage went.
//
// A comment records what it was made about -- the quotation -- and, since the
// timeline, which checkpoint it was made on. When the passage is still in the
// document the reader finds it and there is nothing to say. When it is not,
// the reader has been saying "Needs re-anchoring", which tells the person who
// wrote the comment nothing they did not already know. What they want is where
// it went, and the two records together answer that: the passage was in the
// text at the comment's own checkpoint and is not in the text now, so there is
// a checkpoint between the two where it stopped being found.
//
// Finding it is a search, not a walk. "Found" only goes one way -- a passage
// that came back would be a passage that was never gone in a way anybody means
// -- so a bisection finds the moment in a handful of checkpoints rather than
// all of them, and each one it looks at is cached for every other comment that
// asks about the same moment.
//
// This is deliberately not the word-level diff. What replaced the passage is
// `history.js`'s `wordDiff` and needs the diff crate in the browser; what is
// here needs nothing beyond the checkpoints, and it is the half of
// the question a reviewer actually asks.
//
// A comment with a source anchor is searched for in the source file itself,
// which is why it is preferred over the rendered quote whenever there is one:
// it needs no render and, for a LaTeX document, no PDF either.

import { anchorOne, flatten } from "./anchor.js";
import * as figures from "./figures.js";
import * as history from "./history.js";
import * as renderers from "./renderers.js";
import { projectHtml, PROJECTION_VERSION } from "./diff-display.js";

// Passage lookups can outlive a single comment card in a long-lived reader.
// Keep enough source responses for the usual review, while never retaining
// generated pages after the active lookup has consumed them.
const CACHE_LIMIT = 64;

// A checkpoint SHA can be shared by documents and a reader can arrive with a
// link key. Partition entries by the supplied authentication context. The
// value is kept only in this private in-memory key and is never sent or
// logged; the server still checks authorization on every miss.
const scopes = new Map();
let nextScope = 1;
let cacheGeneration = 0;
function authScope(headers) {
  const entries = typeof Headers !== "undefined" && headers instanceof Headers
    ? Array.from(headers.entries())
    : Object.entries(headers || {});
  const fingerprint = JSON.stringify(entries
    .filter(([name]) => name.toLowerCase() !== "x-librepaper-client")
    .map(([name, value]) => `${name.toLowerCase()}:${String(value)}`)
    .sort());
  // Cache keys contain an opaque process-local scope, never bearer tokens or
  // link keys. Clearing the passage cache retires these scopes on sign-out or
  // document-link changes.
  if (!scopes.has(fingerprint)) scopes.set(fingerprint, `scope-${nextScope++}`);
  return scopes.get(fingerprint);
}

const cacheKey = (slug, sha, headers) => `${slug}\u0000${sha}\u0000${authScope(headers)}`;

function remember(cache, key, value) {
  cache.set(key, value);
  while (cache.size > CACHE_LIMIT) {
    const oldest = cache.keys().next().value;
    if (oldest === undefined) break;
    cache.delete(oldest);
  }
}

// Simultaneous requests for one historical page share the in-flight work, but
// generated HTML is released as soon as that work settles. Generated output
// is never retained as a history or document cache.
const pendingHtml = new Map();

/// The visible text of a checkpoint, derived from its contemporary HTML
/// render. Historical PDFs are intentionally not used for passage lookup.
///
/// It is not the agent's own walk -- that runs in another origin, over a live
/// DOM -- so the two can differ at a whitespace boundary. That is what
/// `anchorOne`'s flattened second pass is for, and it is why this is used to
/// answer "is the passage here" rather than to place anything.
export async function htmlAt(slug, sha, headers = {}, services = {}) {
  const generation = cacheGeneration;
  const pointApi = services.history || history;
  const point = await pointApi.checkpoint(slug, sha, headers);
  if (generation !== cacheGeneration) throw new Error("Comparison authorization changed");
  return (await renderTree(slug, point, headers, services)).html;
}

// The one render a historical page goes through: the checkpoint source, the
// figures it named, and the current HTML-capable renderer.
export async function renderTree(slug, point, headers = {}, services = {}) {
  const generation = cacheGeneration;
  const scope = authScope(headers);
  const rendererApi = services.renderers || renderers;
  const figureApi = services.figures || figures;
  const configuration = services.configuration || (rendererApi.htmlConfiguration
    ? await rendererApi.htmlConfiguration(point) : { identity: services.identity || JSON.stringify(point.settings || {}) });
  if (generation !== cacheGeneration) throw new Error("Comparison authorization changed");
  if (!configuration || typeof configuration.identity !== "string" || !configuration.identity) {
    throw new Error("Historical renderer configuration is unavailable");
  }
  const key = JSON.stringify([point.storage_id || slug, point.tree_sha || point.sha,
    scope, configuration.identity, PROJECTION_VERSION]);
  if (pendingHtml.has(key)) return pendingHtml.get(key);
  const pending = (async () => {
    const tree = {
      main: point.main,
      texts: point.texts || {},
      settings: point.settings || {},
      digests: { ...point.digests },
      files: point.files || {},
    };
    for (const [path, file] of Object.entries(point.files || {})) {
      if (file.kind !== "text") tree.digests[path] = file.sha;
    }
    // A historical page always uses the current HTML-capable renderer over
    // the checkpoint source. Generated output is never a history input.
    const gathered = point.assets && Object.keys(tree.digests).every((path) => point.assets[path])
      ? { assets: point.assets, urls: point.urls || {} }
      : await figureApi.gather(slug, tree.digests, headers, { strict: true });
    const capturedAssets = gathered?.assets;
    if (!capturedAssets || !Object.keys(tree.digests).every((path) =>
      Object.prototype.hasOwnProperty.call(capturedAssets, path))) {
      throw new Error("The figures this checkpoint used are unavailable");
    }
    if (generation !== cacheGeneration) throw new Error("Comparison authorization changed");
    const result = await rendererApi.render(
      { ...tree, assets: gathered.assets, urls: gathered.urls },
      point.label || "Document",
      { format: "html", configuration },
    );
    if (typeof result?.html !== "string") throw new Error("This checkpoint could not be rendered as HTML.");
    if (generation !== cacheGeneration) throw new Error("Comparison authorization changed");
    return { ...result, cacheKey: key, rendererIdentity: configuration.identity };
  })();
  pendingHtml.set(key, pending);
  pending.then(
    () => { if (pendingHtml.get(key) === pending) pendingHtml.delete(key); },
    () => { if (pendingHtml.get(key) === pending) pendingHtml.delete(key); },
  );
  return pending;
}

export async function textAt(slug, sha, headers = {}, services = {}) {
  const html = await htmlAt(slug, sha, headers, services);
  return typeof html === "string" ? visibleText(html) : null;
}

// One checkpoint fetch per document/sha can be shared while concurrent source
// lookups are active. It is released when the callers finish; generated pages
// have the separate in-flight map above and are never retained.
const points = new Map();

/// Drop historical passage responses when a reader changes link or signs out.
/// In-flight calls may finish, but their identity checks cannot repopulate a
/// map entry that has since been replaced.
export function clearPassageCache() {
  cacheGeneration++;
  pendingHtml.clear();
  points.clear();
  scopes.clear();
}

/// The text of one file of a checkpoint, as it was written. Unlike `textAt`
/// this asks for nothing but the tree the server already has on hand: no
/// render, no figures gathered, no PDF fetched. A path the checkpoint does
/// not have -- the file did not exist yet, or has since been renamed away --
/// answers null, which `holds` already reads as "not here" rather than as
/// "unknown", the same as an empty page would.
export async function sourceTextAt(slug, sha, path, headers = {}) {
  const key = cacheKey(slug, sha, headers);
  if (!points.has(key)) {
    const pending = history.checkpoint(slug, sha, headers);
    remember(points, key, pending);
    // A failed request must not poison this checkpoint forever. The identity
    // check preserves a newer retry if eviction or another caller replaced it.
    pending.catch(() => {
      if (points.get(key) === pending) points.delete(key);
    });
  }
  const pending = points.get(key);
  const generation = cacheGeneration;
  try {
    const point = await pending;
    if (generation !== cacheGeneration) throw new Error("Comparison authorization changed");
    const texts = point.texts || {};
    return Object.prototype.hasOwnProperty.call(texts, path) ? texts[path] : null;
  } finally {
    // Coalesce simultaneous lookups, but revalidate authorization and
    // retained membership before every later use of a private source tree.
    if (points.get(key) === pending) points.delete(key);
  }
}

/// The words of a page, with the markup and the things that are not words
/// taken out.
export function visibleText(html) {
  return projectHtml(html).text;
}

/// Whether a quotation is in a text, by the same match that anchors it in the
/// document: exactly, or with whitespace flattened on both sides.
export function holds(text, comment) {
  return typeof text === "string" && Boolean(anchorOne(text, comment, flatten(text)));
}

/// How a comment is read against an old checkpoint: the passage it is about,
/// as the source had it, and the file that passage was in.
///
/// This is the comment's own record, not a guess -- the server wrote it when
/// the comment was made and has not touched it since. `paths` turns the file
/// id into whatever that file is called now.
export function tracedBy(comment, paths) {
  const anchor = comment?.original_anchor;
  if (anchor?.kind !== "source_text") return null;
  const target = anchor.target;
  return {
    checkpoint: anchor.checkpoint_id || "",
    path: paths?.get?.(target.file_id) || "",
    selector: { exact: target.exact, prefix: target.prefix || "", suffix: target.suffix || "" },
  };
}

/// The first checkpoint at which a comment's passage was no longer found.
///
/// `checkpoints` is the manifest, oldest first. The search starts at the
/// comment's own checkpoint -- or at the oldest the manifest still has, for a
/// comment made before that was recorded -- and ends at the newest, where the
/// caller has already established that the passage is gone. Returns the
/// manifest entry, or null when there is nothing to say: no history to look
/// in, or a passage that turns out still to be there.
export async function wentAt(slug, traced, checkpoints, headers = {}, at = textAt, atSource = sourceTextAt) {
  if (!checkpoints?.length || !traced) return null;
  // A comment from before checkpoints were recorded on one, or one whose
  // checkpoint has since been shed, is read as made on the oldest moment the
  // manifest still has. That is the earliest thing that could be true of it.
  const own = checkpoints.findIndex((point) => point.sha === traced.checkpoint);
  let low = own >= 0 ? own : 0;
  let high = checkpoints.length - 1;
  if (low >= high) return null;

  // The passage is looked for in the source file it was quoted from, at each
  // checkpoint. A comment whose file this browser cannot name -- renamed away,
  // or a document it only has the rendering of -- falls back to the rendered
  // text of the checkpoint.
  const selector = traced.selector;
  const read = traced.path
    ? (sha) => atSource(slug, sha, traced.path, headers)
    : (sha) => at(slug, sha, headers);

  // The passage has to have been there to have gone. A comment whose own
  // checkpoint does not hold it is one whose quotation this cannot reason
  // about, and saying nothing is better than naming a moment at random.
  const ownText = await read(checkpoints[low].sha);
  if (typeof ownText !== "string" || !holds(ownText, selector)) return null;
  const newestText = await read(checkpoints[high].sha);
  if (typeof newestText !== "string" || holds(newestText, selector)) return null;

  // Invariant: it is in `low` and not in `high`. Each step halves the gap, so
  // the answer costs about five renders on a history of thirty.
  while (high - low > 1) {
    const middle = (low + high) >> 1;
    const middleText = await read(checkpoints[middle].sha);
    if (typeof middleText !== "string") return null;
    if (holds(middleText, selector)) low = middle;
    else high = middle;
  }
  return checkpoints[high];
}

/// Finds what replaced a comment's quotation between two versions. The
/// returned text is the quoted old range transformed through the edits, so
/// unchanged words between two replacements remain visible (`blue fox at calm
/// river`, rather than just the two inserted fragments). Null means the
/// quotation could not be anchored or the versions did not replace any part of
/// it. A whitespace-only insertion is retained because it can be the only
/// change inside a quoted source selection.
export async function replacementAt(oldText, newText, selector, diffApi) {
  if (typeof oldText !== "string" || typeof newText !== "string" || !selector?.exact) return null;
  const found = anchorOne(oldText, selector, flatten(oldText));
  if (!found) return null;
  const compute = diffApi || ((before, after) => history.wordDiff(before, after));
  const edits = await compute(oldText, newText);
  const overlapping = edits.filter((edit) => {
    const start = Number(edit.at) || 0;
    const end = start + (Number(edit.delete) || 0);
    // An insertion at the quoted boundary belongs to the replacement only
    // when it is inside the selection, not when it merely follows it.
    return (end > found.start && start < found.end)
      || (end === start && start >= found.start && start < found.end);
  });
  if (!overlapping.length) return null;
  const start = found.start;
  const end = found.end;
  let cursor = start;
  let replacement = "";
  for (const edit of edits) {
    const editStart = Number(edit.at) || 0;
    const editEnd = editStart + (Number(edit.delete) || 0);
    if (editEnd <= start) continue;
    if (editStart >= end) break;
    if (editStart > cursor) replacement += oldText.slice(cursor, Math.min(editStart, end));
    let insert = edit.insert || "";
    // A token hunk can begin just before the quote or end just after it. Drop
    // unchanged outside context when the edit gives us a defensible boundary.
    // When that context also changed, the diff cannot identify which part of
    // the insertion replaces the selected passage.
    if (editStart < start) {
      const outside = oldText.slice(editStart, start);
      if (outside && insert) {
        if (!insert.startsWith(outside)) return null;
        insert = insert.slice(outside.length);
      }
    }
    if (editEnd > end) {
      const outside = oldText.slice(end, editEnd);
      if (outside && insert) {
        if (!insert.endsWith(outside)) return null;
        insert = insert.slice(0, -outside.length);
      }
    }
    if (insert && (editEnd > start || (editStart >= start && editStart < end))) {
      replacement += insert;
    }
    cursor = Math.max(cursor, Math.min(editEnd, end));
  }
  if (cursor < end) replacement += oldText.slice(cursor, end);
  return replacement;
}
