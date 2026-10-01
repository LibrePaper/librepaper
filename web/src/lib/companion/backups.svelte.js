// The account-wide backup status shared by the Reader pill and Settings.
// It is polled only while the companion is already paired, so this feature
// never starts a first-time companion probe by itself.
import * as client from "./client.js";

export function createBackupStatus(bridge = client) {
  let current = $state.raw({ accountId: "", paired: false, loading: false, data: null, error: "" });
  let scope = 0;
  let timer = null;
  let inFlight = null;
  let refreshQueued = false;

  function stop() {
    if (timer !== null) clearInterval(timer);
    timer = null;
  }

  async function refresh() {
    const accountId = current.accountId;
    const requestScope = scope;
    if (!accountId || !current.paired) return current;
    if (inFlight) {
      refreshQueued = true;
      return inFlight;
    }
    // A retry or a post-mutation read can arrive during a poll. Keep one
    // request active and guarantee one follow-up read after it settles.
    current = { ...current, loading: true };
    const request = (async () => {
      try {
        const data = await bridge.backups(accountId);
        if (requestScope === scope && accountId === current.accountId && current.paired) {
          current = { ...current, loading: false, data, error: "" };
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
        if (refreshQueued) {
          refreshQueued = false;
          if (requestScope === scope && accountId === current.accountId && current.paired) void refresh();
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
    refreshQueued = false;
    current = { accountId: nextId, paired: nextPaired, loading: false, data: null, error: "" };
    if (!nextPaired) return;
    void refresh();
    timer = setInterval(() => void refresh(), 30_000);
    timer.unref?.();
  }

  return {
    get status() { return current; },
    setScope,
    refresh,
    reset() { setScope("", false); },
  };
}

export const backups = createBackupStatus();
