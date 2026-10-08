// The companion control state that Settings pages share: /state loaded and
// polled, and actions guarded so a reply for an older companion is dropped.

import * as control from "./control.js";
import * as localBridge from "./client.js";

export function createMachineView() {
  let available = $state(false);
  // A stored credential is not a running companion, so track whether it answers.
  let reachable = $state(true);
  let state = $state(null);
  let error = $state("");
  let loadError = $state("");
  let notice = $state("");
  let pending = $state("");
  let connectError = $state("");
  let epoch = 0;
  let requestId = 0;
  let currentScope = "";

  const list = (value) => Array.isArray(value) ? value : [];
  const id = (value) => encodeURIComponent(String(value));

  async function load(expectedScope = currentScope, expectedEpoch = epoch) {
    if (!available) return;
    const current = ++requestId;
    try {
      const result = await control.request("/state");
      if (current !== requestId || expectedEpoch !== epoch || expectedScope !== control.scope() || !control.available()) return;
      state = result;
      reachable = true;
      loadError = "";
    } catch (cause) {
      if (current === requestId && expectedEpoch === epoch && expectedScope === control.scope()) {
        if (cause?.name === "Unreachable") {
          reachable = false;
          state = null;
          loadError = "";
        } else {
          loadError = cause?.message || "Could not load this computer's settings.";
        }
      }
    }
  }

  async function act(key, success, path, options = {}) {
    if (!available || pending) return;
    const expectedScope = currentScope;
    const expectedEpoch = epoch;
    pending = key;
    error = "";
    notice = "";
    try {
      await control.request(path, options);
      if (expectedEpoch !== epoch || expectedScope !== control.scope() || !control.available()) return;
      notice = success;
      await load(expectedScope, expectedEpoch);
      return true;
    } catch (cause) {
      if (expectedEpoch === epoch && expectedScope === control.scope()) {
        if (cause?.name === "Unreachable") {
          reachable = false;
          state = null;
        }
        error = cause?.message || "The request could not be completed.";
      }
      return false;
    } finally {
      if (expectedEpoch === epoch) pending = "";
    }
  }

  async function manageThisComputer() {
    if (pending) return;
    pending = "connect";
    connectError = "";
    try {
      await control.connect(localBridge.address());
      control.showSettings();
    } catch (cause) {
      connectError = cause?.message || "Could not connect to this computer's companion.";
    } finally {
      pending = "";
    }
  }

  function start() {
    let alive = true;

    const updateAccess = (access) => {
      if (!alive) return;
      const nextScope = access?.scope || control.scope();
      if (currentScope !== nextScope || available !== Boolean(access?.available)) {
        currentScope = nextScope;
        available = Boolean(access?.available);
        reachable = true;
        epoch++;
        requestId++;
        state = null;
        error = "";
        loadError = "";
        notice = "";
        connectError = "";
        pending = "";
      }
      if (available) void load(currentScope, epoch);
    };

    const unsubscribe = control.subscribe(updateAccess);
    const timer = setInterval(() => { if (available) void load(currentScope, epoch); }, 5000);
    const visibility = () => { if (document.visibilityState === "visible" && available) void load(currentScope, epoch); };
    document.addEventListener("visibilitychange", visibility);

    return () => {
      alive = false;
      clearInterval(timer);
      document.removeEventListener("visibilitychange", visibility);
      unsubscribe?.();
      requestId++;
    };
  }

  return {
    get available() { return available && reachable; },
    get state() { return state; },
    get error() { return error; },
    get loadError() { return loadError; },
    get notice() { return notice; },
    get pending() { return pending; },
    get connectError() { return connectError; },
    act,
    manageThisComputer,
    start,
    list,
    id,
  };
}
