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
//
// Errors are also noted in a small in-page buffer (see support/log.js). It
// lives in memory only and nothing leaves the page on its own: the user can
// copy it out from settings, and check it first.

import { record } from "./support/log.js";
import { scrubMessage } from "./support/redact.js";

/// Errors seen so far, so a fault that repeats -- an effect that throws on
/// every frame, a rejected poll -- is reported once rather than shouting.
const seen = new Set();

const describe = (error) => {
  const message = error?.message || String(error || "");
  return message.slice(0, 200) || "an unknown error";
};

/// Messages from these places can quote the document, so only the error name
/// and the stack are kept for them.
const QUIET = /editor|engine|render|latex|typst|quarto/i;

/// At most 12 stack lines, each scrubbed, without the first line, which only
/// repeats the message.
const cleanStack = (stack) =>
  String(stack || "").split("\n").slice(1, 13).map((line) => scrubMessage(line, 200));

/// The single place an unexpected failure is written down. It goes to the
/// console and into the in-page buffer that only the user can copy out from
/// settings. There is no error-reporting service to send it to, and inventing
/// one here would send a reader's document text somewhere they never agreed
/// to. What the reader is *shown* is the boundary's notice, not this.
export function report(error, where = "") {
  const key = `${where}:${describe(error)}`;
  if (seen.has(key)) return false;
  seen.add(key);
  record({
    kind: "error",
    where,
    name: error?.name || "Error",
    message: QUIET.test(where) ? "" : scrubMessage(describe(error)),
    stack: cleanStack(error?.stack),
  });
  console.error(`[librepaper] unhandled error${where ? ` in ${where}` : ""}:`, error);
  return true;
}

/// Notes something that went wrong but did not stop the page, in the same
/// in-page buffer. The message is scrubbed first.
export function warn(message, where = "") {
  record({ kind: "warning", where, message: scrubMessage(message) });
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
