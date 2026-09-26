// Where an orphaned comment's passage went, and what replaced it.
//
// A comment that can no longer be anchored used to be labelled "Needs
// re-anchoring", which told the person who wrote it nothing they did not
// already know. What is useful is the version at which the passage stopped
// being found, and the text that stands where it stood. Both are answered by
// reading old versions, which takes a request per comment, so this owns the
// answers and the bookkeeping that keeps a slow walk from overwriting a
// newer one.
//
// Three things can make a walk stale: a newer walk, a change to the source,
// and a change to the visible text. All three are checked between every
// request, which is why `now()` is asked for the current pair rather than
// handed one: a walk that started a second ago has to compare itself against
// what is true at the moment it is about to write.

import * as defaultPassages from "../passages.js";
import { keyHeaders } from "../api.js";
import { createGeneration } from "./generation.js";
import { MOMENT } from "../moment.js";

export function createPassageTrace({
  slug,
  key = "",
  passages = defaultPassages,
  // The source generation and the visible text, as they stand right now.
  now = () => ({ source: 0, visible: "" }),
  // The manifest, read only if a walk needs one and none has been read.
  loadLabels = async () => [],
  // What each of this document's files is called, by its stable id: a
  // comment names the file it is about by id, and an old label is read
  // by path.
  paths = () => new Map(),
}) {
  const state = $state({ went: {}, replacements: {} });

  const walks = createGeneration();
  // What the last completed walk was of. A walk with the same source, the
  // same visible text and the same comments would reach the same answers, so
  // it is not made.
  let last = null;

  async function trace({ comments, tree, labels = [] }) {
    const { source, visible } = now();
    const lost = comments.filter((comment) => comment.orphaned);
    const ids = lost
      .map((comment) => `${comment.id}:${comment.original_anchor?.frontier || ""}`)
      .join("|");
    if (last?.source === source && last?.visible === visible && last?.ids === ids) return;
    last = { source, visible, ids };
    const stale = walks.begin();
    const went = {};
    const replacements = {};
    let list = labels;
    if (lost.length && !list.length) list = await loadLabels();
    const current = () => {
      const state_ = now();
      return !stale() && state_.source === source && state_.visible === visible;
    };
    for (const comment of lost) {
      if (!current()) return;
      try {
        const traced = passages.tracedBy(comment, paths());
        const point = await passages.wentAt(slug, traced, list, keyHeaders(key));
        if (point) went[comment.id] = point;
        if (!traced?.frontier) continue;
        const oldText = await passages.sourceTextAt(
          slug, MOMENT + traced.frontier, { file_id: traced.file_id }, keyHeaders(key),
        );
        const against = traced.path ? tree.texts[traced.path] ?? null : null;
        const replacement = await passages.replacementAt(oldText, against, traced.selector);
        if (replacement !== null) replacements[comment.id] = replacement;
      } catch {
        // A missing label cannot establish a replacement. The walk is
        // forgotten rather than recorded, so the next one tries again.
        if (!stale()) last = null;
      }
    }
    if (current()) {
      state.went = went;
      state.replacements = replacements;
    }
  }

  return { state, trace };
}
