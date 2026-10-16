// The desktop app management state that Settings pages share: /state loaded
// and polled, and actions guarded so a reply that arrives after the view
// stopped is dropped.

import * as desktopClient from "./client.js";

export function createMachineView() {
  let available = $state(false);
  // The desktop app answers or it does not; a failed load marks it unreachable.
  let reachable = $state(true);
  let state = $state(null);
  let error = $state("");
  let loadError = $state("");
  let notice = $state("");
  let pending = $state("");
  let epoch = 0;
  let requestId = 0;

  const list = (value) => Array.isArray(value) ? value : [];
  const id = (value) => encodeURIComponent(String(value));

  async function load(expectedEpoch = epoch) {
    if (!available) return;
    const current = ++requestId;
    try {
      const result = await desktopClient.manage("/state");
      if (current !== requestId || expectedEpoch !== epoch || !available) return;
      state = result;
      reachable = true;
      loadError = "";
    } catch (cause) {
      if (current === requestId && expectedEpoch === epoch) {
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
    const expectedEpoch = epoch;
    pending = key;
    error = "";
    notice = "";
    try {
      await desktopClient.manage(path, options);
      if (expectedEpoch !== epoch || !available) return;
      notice = success;
      await load(expectedEpoch);
      return true;
    } catch (cause) {
      if (expectedEpoch === epoch) {
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

  function start() {
    available = Boolean(desktopClient.desktop());
    reachable = true;
    epoch++;
    requestId++;
    state = null;
    error = "";
    loadError = "";
    notice = "";
    pending = "";
    if (available) void load(epoch);
    const timer = setInterval(() => { if (available) void load(epoch); }, 5000);
    const visibility = () => { if (document.visibilityState === "visible" && available) void load(epoch); };
    document.addEventListener("visibilitychange", visibility);

    return () => {
      clearInterval(timer);
      document.removeEventListener("visibilitychange", visibility);
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
    act,
    start,
    list,
    id,
  };
}
