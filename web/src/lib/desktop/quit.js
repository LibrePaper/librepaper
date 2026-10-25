// Quitting the desktop app is a question put to every window first.
//
// The app ends every window when it exits, so a local project open in a
// window the person did not quit from would lose its pending folder write.
// The app sends `librepaper:quit-requested` to each window with the id of the
// round, waits for each page to answer through the `quit-ready` shell action,
// and exits when every answer is ok or about ten seconds have passed. A page
// that could not save answers not ok. The app then stays open and sends
// `librepaper:quit-cancelled` to every window it asked.
//
// This file holds no desktop call: the page hands in the function that sends
// its answer.

export const QUIT_REQUESTED = "librepaper:quit-requested";
export const QUIT_CANCELLED = "librepaper:quit-cancelled";

/// Listens for the app's quit request and for its cancellation. `settle()`
/// resolves whether the page's work is saved, and a page with nothing to save
/// passes none and answers ok at once. A request that throws or resolves
/// false answers not ok. `answer({ id, ok })` sends the answer. `cancelled()`
/// runs when the app decided to stay open. Returns the unsubscribe.
///
/// Handling the request cancels the event, which tells the app that this page
/// answers for itself. The app answers ok for a page that does not.
/** @param {{ settle?: (() => Promise<boolean>) | null, answer: (body: { id: number, ok: boolean }) => Promise<unknown>, cancelled?: () => void, target?: EventTarget | null }} options */
export function listenForQuit({ settle = null, answer, cancelled = () => {}, target = globalThis.window }) {
  if (!target) return () => {};
  async function saved() {
    if (!settle) return true;
    try { return Boolean(await settle()); } catch { return false; }
  }
  const requested = (event) => {
    event.preventDefault();
    const id = /** @type {CustomEvent} */ (event).detail?.id;
    // An answer that cannot be sent leaves the app to its deadline.
    saved().then((ok) => answer({ id, ok })).catch(() => {});
  };
  const stays = () => cancelled();
  target.addEventListener(QUIT_REQUESTED, requested);
  target.addEventListener(QUIT_CANCELLED, stays);
  return () => {
    target.removeEventListener(QUIT_REQUESTED, requested);
    target.removeEventListener(QUIT_CANCELLED, stays);
  };
}
