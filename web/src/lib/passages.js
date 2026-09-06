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
// step 8 of `docs/specs/history.md` and needs the diff crate in the browser;
// what is here needs nothing that is not already built, and it is the half of
// the question a reviewer actually asks.

import { anchorOne, flatten } from "./anchor.js";
import * as figures from "./figures.js";
import * as history from "./history.js";
import * as renderers from "./renderers.js";

// One rendering per checkpoint per page, whatever asks for it. A document
// under review has a handful of comments on one or two moments, so this is
// nearly always one entry deep.
const rendered = new Map();

/// The visible text of a checkpoint, the way the agent takes it from the
/// frame: the document rendered, and the words in it with the markup gone.
///
/// It is not the agent's own walk -- that runs in another origin, over a live
/// DOM -- so the two can differ at a whitespace boundary. That is what
/// `anchorOne`'s flattened second pass is for, and it is why this is used to
/// answer "is the passage here" rather than to place anything.
export async function textAt(slug, sha, headers = {}) {
  if (rendered.has(sha)) return rendered.get(sha);
  const pending = (async () => {
    const point = await history.checkpoint(slug, sha, headers);
    const tree = { main: point.main, texts: point.texts || {}, digests: {} };
    for (const [path, file] of Object.entries(point.files || {})) {
      if (file.kind !== "text") tree.digests[path] = file.sha;
    }

    // Readers must never compile historical LaTeX: the PDF is already the
    // rendering of that exact checkpoint, and asking for a browser compiler
    // would make passage lookup depend on a local distribution choice.
    if (renderers.formatOf(tree.main) === "latex") {
      const response = await fetch(`/api/documents/${slug}/renderings/${sha}`, { headers });
      if (!response.ok) return null; // unavailable is unknown, not empty text
      const { text } = await import("./pdf/render.js");
      return text(await response.arrayBuffer());
    }

    // A checkpoint is a whole directory. Rehydrate every referenced figure
    // before rendering so an image-dependent Typst source is evaluated in the
    // same tree that was stored, rather than silently compiling with holes.
    const gathered = await figures.gather(slug, tree.digests, headers);
    if (Object.keys(gathered.assets).length !== Object.keys(tree.digests).length) return null;
    const { html } = await renderers.render(
      { ...tree, assets: gathered.assets, urls: gathered.urls },
      point.label || "Document",
    );
    return typeof html === "string" ? visibleText(html) : null;
  })();
  rendered.set(sha, pending);
  return pending;
}

/// The words of a page, with the markup and the things that are not words
/// taken out.
export function visibleText(html) {
  const parsed = new DOMParser().parseFromString(html, "text/html");
  for (const gone of parsed.querySelectorAll("script, style, template")) gone.remove();
  return parsed.body?.textContent || "";
}

/// Whether a quotation is in a text, by the same match that anchors it in the
/// document: exactly, or with whitespace flattened on both sides.
export function holds(text, comment) {
  return typeof text === "string" && Boolean(anchorOne(text, comment, flatten(text)));
}

/// The first checkpoint at which a comment's passage was no longer found.
///
/// `checkpoints` is the manifest, oldest first. The search starts at the
/// comment's own checkpoint -- or at the oldest the manifest still has, for a
/// comment made before that was recorded -- and ends at the newest, where the
/// caller has already established that the passage is gone. Returns the
/// manifest entry, or null when there is nothing to say: no history to look
/// in, or a passage that turns out still to be there.
export async function wentAt(slug, comment, checkpoints, headers = {}, at = textAt) {
  if (!checkpoints?.length) return null;
  // A comment from before checkpoints were recorded on one, or one whose
  // checkpoint has since been shed, is read as made on the oldest moment the
  // manifest still has. That is the earliest thing that could be true of it.
  const own = checkpoints.findIndex((point) => point.sha === comment.revision);
  let low = own >= 0 ? own : 0;
  let high = checkpoints.length - 1;
  if (low >= high) return null;

  // The passage has to have been there to have gone. A comment whose own
  // checkpoint does not hold it is one whose quotation this cannot reason
  // about -- a figure annotation, or a passage the renderer no longer emits --
  // and saying nothing is better than naming a moment at random.
  const ownText = await at(slug, checkpoints[low].sha, headers);
  if (typeof ownText !== "string" || !holds(ownText, comment)) return null;
  const newestText = await at(slug, checkpoints[high].sha, headers);
  if (typeof newestText !== "string" || holds(newestText, comment)) return null;

  // Invariant: it is in `low` and not in `high`. Each step halves the gap, so
  // the answer costs about five renders on a history of thirty.
  while (high - low > 1) {
    const middle = (low + high) >> 1;
    const middleText = await at(slug, checkpoints[middle].sha, headers);
    if (typeof middleText !== "string") return null;
    if (holds(middleText, comment)) low = middle;
    else high = middle;
  }
  return checkpoints[high];
}
