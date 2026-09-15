// A main-text edit is seen twice: once by the bound source watcher, and again
// by the directory watcher, which reaches inside the files map and so hears
// every text under it. The directory watcher still matters -- an included file
// is only ever heard there -- so what is suppressed is the overlap, not the
// watcher: an event batch that touches the active text is left to the source
// watcher that is already following it.
//
// There is no separate transaction to deduplicate against. The directory
// watcher is called once per commit with that commit's events, so the batch is
// the transaction, and arriving twice for one commit is not a thing that can
// happen any more.
export function needsSourceRefresh(events, activeText) {
  if (!events) return true;
  const list = Array.isArray(events) ? events : [events];
  // `target` is a container id, and so is `activeText.id`: an event names the
  // container it happened to, not the handle this caller happens to hold.
  const active = activeText?.id;
  if (active && list.some((event) => event?.target === active)) return false;
  return true;
}
