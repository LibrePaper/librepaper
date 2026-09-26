// What happens when something throws where nobody was catching.
//
// Two halves, because there are two ways a page dies. A Svelte boundary
// catches what throws while rendering -- a component's setup, its effects,
// its template -- and that is most of it. It cannot catch what throws in an
// event handler or a promise nobody awaited, because by then Svelte is not on
// the stack. So the boundary handles the first and the listeners below handle
// the second.
//
// Neither reloads on its own. A reader in the middle of writing has work in
// the browser that a reload would interrupt, and a page that reloads itself on
// an error it did not understand can reload forever. Both say what happened
// and leave the choice.

/// Errors seen so far, so a fault that repeats -- an effect that throws on
/// every frame, a rejected poll -- is reported once rather than shouting.
const seen = new Set();

const describe = (error) => {
  const message = error?.message || String(error || "");
  return message.slice(0, 200) || "an unknown error";
};

/// The single place an unexpected failure is written down. Kept to the
/// console: there is no error-reporting service to send it to, and inventing
/// one here would send a reader's document text somewhere they never agreed
/// to. What the reader is *shown* is the boundary's notice, not this.
export function report(error, where = "") {
  const key = `${where}:${describe(error)}`;
  if (seen.has(key)) return false;
  seen.add(key);
  console.error(`[librepaper] unhandled error${where ? ` in ${where}` : ""}:`, error);
  return true;
}

/// Catches what the boundary cannot: a throw inside a click handler, and a
/// promise that rejects with nobody waiting. Without these, both are silent --
/// the page looks fine and simply stops doing the thing that was asked.
///
/// Returns the way to take them off again, which is what a test wants.
export function watchForUnhandled(target = globalThis) {
  const onError = (event) => report(event.error || event.message, "an event handler");
  const onRejection = (event) => report(event.reason, "a promise");
  target.addEventListener?.("error", onError);
  target.addEventListener?.("unhandledrejection", onRejection);
  return () => {
    target.removeEventListener?.("error", onError);
    target.removeEventListener?.("unhandledrejection", onRejection);
  };
}
