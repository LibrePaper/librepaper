// Whether completion was asked for by the person, as opposed to woken by typing.
// CodeMirror's startCompletion always makes the query explicit, and a few
// sources answer more to an explicit query (a bare "@" lists labels, the Typst
// engine offers completions it would hold back). Waking at typing triggers must
// not change what they answer, so the views woken that way are remembered here
// until their completion closes, and their queries count as implicit.

/** @type {WeakSet<object>} */
const wokenByTyping = new WeakSet();

/** Records that the next startCompletion on this view is a typing wake. */
export function markWokenByTyping(view) {
  wokenByTyping.add(view);
}

/** Forgets a view once its completion has closed. */
export function clearWokenByTyping(view) {
  wokenByTyping.delete(view);
}

/**
 * True when the query comes from the person (Ctrl-Space or another explicit
 * start), false when it is implicit or a typing wake.
 * @param {import("@codemirror/autocomplete").CompletionContext} context
 */
export function explicitlyAsked(context) {
  return context.explicit && !(context.view && wokenByTyping.has(context.view));
}
