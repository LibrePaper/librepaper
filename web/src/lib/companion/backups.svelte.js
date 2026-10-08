// The account-wide backup status shared by the Reader pill and Settings.
// It is polled only while the companion is already paired, so this feature
// never starts a first-time companion probe by itself.
import * as client from "./client.js";

export function createBackupStatus(bridge = client, { runningPollMs = 1000 } = {}) {
  let current = $state.raw({ accountId: "", paired: false, loading: false, data: null, error: "" });
  let scope = 0;
  let timer = null;
  let runningTimer = null;
  let inFlight = null;
  let queuedRefresh = null;

  function stop() {
    if (timer !== null) clearInterval(timer);
    timer = null;
    if (runningTimer !== null) clearTimeout(runningTimer);
    runningTimer = null;
  }

  async function refresh() {
    const accountId = current.accountId;
    const requestScope = scope;
    if (!accountId || !current.paired) return current;
    if (inFlight) {
      if (!queuedRefresh) {
        let resolveQueued;
        const promise = new Promise((resolve) => { resolveQueued = resolve; });
        queuedRefresh = { promise, resolve: resolveQueued };
      }
      return queuedRefresh.promise;
    }
    // A retry or a post-mutation read can arrive during a poll. Keep one
    // request active and guarantee one follow-up read after it settles.
    current = { ...current, loading: true };
    const request = (async () => {
      try {
        const data = await bridge.backups(accountId);
        if (requestScope === scope && accountId === current.accountId && current.paired) {
          current = { ...current, loading: false, data, error: "" };
          // A running backup finishes in seconds, so check again soon.
          if (data?.running && runningTimer === null) {
            runningTimer = setTimeout(() => {
              runningTimer = null;
              if (requestScope === scope) void refresh();
            }, runningPollMs);
          }
        }
      } catch (error) {
        if (requestScope === scope && accountId === current.accountId && current.paired) {
          current = { ...current, loading: false, data: null, error: error?.message || "Backup status is unavailable." };
        }
      }
      return current;
    })();
    inFlight = request;
    try {
      return await request;
    } finally {
      // A scope switch detaches the old request and may already have started
      // a new one. Only the request that still owns the slot can consume its
      // queued refresh or clear that slot.
      if (inFlight === request) {
        inFlight = null;
        const followup = queuedRefresh;
        queuedRefresh = null;
        if (followup) {
          if (requestScope === scope && accountId === current.accountId && current.paired) {
            void refresh().then(followup.resolve, followup.resolve);
          } else {
            followup.resolve(current);
          }
        }
      }
    }
  }

  function setScope(accountId, paired) {
    const nextId = typeof accountId === "string" ? accountId : "";
    const nextPaired = Boolean(nextId && paired);
    if (nextId === current.accountId && nextPaired === current.paired) return;
    stop();
    scope += 1;
    inFlight = null;
    queuedRefresh?.resolve(current);
    queuedRefresh = null;
    current = { accountId: nextId, paired: nextPaired, loading: false, data: null, error: "" };
    if (!nextPaired) return;
    void refresh();
    timer = setInterval(() => void refresh(), 30_000);
  }

  return {
    get status() { return current; },
    setScope,
    refresh,
    reset() { setScope("", false); },
  };
}

export const backups = createBackupStatus();
