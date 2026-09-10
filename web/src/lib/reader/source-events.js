// A main-text edit is observed both by the bound source watcher and by the
// directory's deep file watcher. The latter still matters for included files,
// so callers only suppress it when the event list contains the active text.
export function needsSourceRefresh(events, activeText, seenTransactions) {
  if (!events) return true;
  const list = Array.isArray(events) ? events : [events];
  const transaction = list.find((event) => event?.transaction)?.transaction;
  const repeated = transaction && seenTransactions?.has(transaction);
  if (transaction && seenTransactions && !repeated) seenTransactions.add(transaction);
  if (list.some((event) => event?.target === activeText)) return false;
  return !repeated;
}
