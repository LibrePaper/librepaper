import * as desktopClient from "../desktop/client.js";
import { homeOf, shorten } from "./run-directory.js";

/**
 * The folder a Quarto document's code runs in on this computer, shared by the
 * chip beside Run code and the prompt in the preview. `onchange(path)` gets the
 * folder once one that exists is set, or "" when none is.
 * @param {string} project
 * @param {(path: string) => void} [onchange]
 */
export function createRunDirectory(project, onchange = () => {}) {
  let status = $state(null);
  let busy = $state("");
  let error = $state("");
  let loaded = $state(false);
  let generation = 0;

  const usable = $derived(Boolean(status?.path) && status.exists !== false);
  const home = $derived(homeOf(status?.suggested));

  function apply(next) {
    status = next;
    onchange(next?.path && next.exists !== false ? next.path : "");
  }

  async function act(name, call) {
    if (busy) return false;
    busy = name;
    error = "";
    try {
      apply(await call(project));
      return true;
    } catch (cause) {
      error = cause?.message || "";
      return false;
    } finally {
      busy = "";
    }
  }

  return {
    get status() { return status; },
    get busy() { return busy; },
    get error() { return error; },
    get loaded() { return loaded; },
    get usable() { return usable; },
    get home() { return home; },
    shorten: (path) => shorten(path, home),
    /** Fetch the status; call again to refetch. */
    load() {
      const mine = ++generation;
      loaded = false;
      error = "";
      onchange("");
      desktopClient.runDirectory(project).then(
        (next) => { if (mine === generation) { loaded = true; apply(next); } },
        (cause) => { if (mine === generation) { loaded = true; error = cause?.message || ""; } },
      );
    },
    /** Forget everything, as when Run code goes off. */
    reset() {
      generation++;
      status = null;
      busy = "";
      error = "";
      loaded = false;
      onchange("");
    },
    choose: () => act("choose", desktopClient.chooseRunDirectory),
    useDefault: () => act("default", desktopClient.useDefaultRunDirectory),
    open: () => act("open", desktopClient.openRunDirectory),
  };
}
