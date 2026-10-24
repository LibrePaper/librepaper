// Runs automatic Git sync while the project window is open. The scheduler
// lives in gitsync/auto.js; this module owns the switch, the storage of it and
// the state the panel shows.
import { createAutoSync } from "../gitsync/auto.js";
import { createBridge } from "../gitsync/bridge.js";
import { syncApi } from "../gitsync/server.js";
import { checkRemote, pull, push, recover, status } from "../gitsync/sync.js";
import { desktop, notify } from "../desktop/client.js";
import { CHANGES } from "../track-changes.js";

const storageKey = (slug) => `librepaper:git-auto:${slug}`;

// `input()` answers { slug, key, session, rules, me, title } as the Reader holds them now.
export function createGitAuto(input) {
  const state = $state({
    available: Boolean(desktop()),
    enabled: false,
    mode: "off",
    kind: "",
    reason: "",
    lastPullAt: null,
    lastPushAt: null,
    nextAt: null,
    revision: 0,
    error: "",
  });

  const git = createBridge();
  let session = null;
  let scheduler = null;
  let unsubscribe = null;
  let notified = "";
  // `generation` moves whenever the switch is touched or sync ends, so a call
  // that awaited across such a change knows not to start sync afterwards.
  let generation = 0;
  let disposed = false;
  // The connection the running scheduler was enabled for.
  let boundId = "";
  let retryTimer = null;

  function stored() {
    try {
      return localStorage.getItem(storageKey(input().slug)) || "";
    } catch {
      return "";
    }
  }

  function store(value) {
    try {
      if (value) localStorage.setItem(storageKey(input().slug), value);
      else localStorage.removeItem(storageKey(input().slug));
    } catch {
      // Not stored: the switch then lasts until the window closes.
    }
  }

  function ctx() {
    const { slug, key, rules, me } = input();
    return {
      slug,
      session,
      rules,
      git,
      api: syncApi(slug, key),
      me: { name: me?.name || me?.handle || "", handle: me?.handle || me?.name || "" },
    };
  }

  // The link when the project is connected on this computer, else null.
  async function connection() {
    const s = await status(ctx());
    const link = s.link;
    if (link && s.git?.connected && s.git.connection_id === link.connection_id) return link;
    return null;
  }

  function onChange(snapshot) {
    // A pause starting or ending also moves the revision, so an open panel reloads its status.
    const moved =
      snapshot.lastPullAt !== state.lastPullAt || snapshot.lastPushAt !== state.lastPushAt || snapshot.kind !== state.kind;
    state.mode = snapshot.mode;
    state.kind = snapshot.kind;
    state.reason = snapshot.reason;
    state.lastPullAt = snapshot.lastPullAt;
    state.lastPushAt = snapshot.lastPushAt;
    state.nextAt = snapshot.nextAt;
    if (moved) state.revision += 1;
    if (snapshot.mode === "paused") {
      const pause = `${snapshot.kind}${snapshot.reason}`;
      if (pause !== notified) {
        notified = pause;
        const title = input().title;
        notify("Git sync paused", `${title || "This project"}: ${snapshot.reason}`).catch(() => {});
      }
    } else if (snapshot.mode === "idle") {
      notified = "";
    }
  }

  const poke = () => scheduler?.poke();
  const onVisible = () => {
    if (document.visibilityState === "visible") poke();
  };

  // Hears file edits and also decisions on tracked suggestions, which touch
  // only the `changes` map and which `onFiles` does not report. Returns one
  // function that ends both subscriptions.
  function watch(target) {
    if (!target) return null;
    const stopFiles = target.onFiles(() => scheduler?.edited());
    const stopChanges = target.doc.subscribe((batch) => {
      if (batch.events.some((event) => event.path?.[0] === CHANGES)) scheduler?.edited();
    });
    return () => {
      stopFiles?.();
      stopChanges?.();
    };
  }

  // Another window of this project changed the switch, so this one follows it.
  function onStorage(event) {
    if (event.key === storageKey(input().slug) && event.newValue !== boundId) end();
  }

  function begin(id) {
    boundId = id;
    scheduler = createAutoSync({
      ops: {
        status: async () => {
          const s = await status(ctx());
          const link = s.link;
          const live = link && s.git?.connected && s.git.connection_id === link.connection_id;
          if (!live || link.connection_id !== boundId) {
            // The connection this switch was enabled for is gone or replaced.
            store("");
            end();
            return { conflict: null, pending: [] };
          }
          return { conflict: s.conflict, pending: s.pending || [] };
        },
        recover: () => recover(ctx()),
        check: () => checkRemote(ctx()),
        pull: () => pull(ctx()),
        push: () => push(ctx(), ""),
      },
      onChange,
    });
    unsubscribe = watch(session);
    window.addEventListener("focus", poke);
    window.addEventListener("online", poke);
    window.addEventListener("storage", onStorage);
    document.addEventListener("visibilitychange", onVisible);
    scheduler.start();
    state.enabled = true;
  }

  function cancelRetry() {
    clearTimeout(retryTimer);
    retryTimer = null;
    window.removeEventListener("online", retryStartup);
  }

  function retryStartup() {
    cancelRetry();
    startup();
  }

  function end() {
    generation += 1;
    cancelRetry();
    boundId = "";
    scheduler?.stop();
    scheduler = null;
    unsubscribe?.();
    unsubscribe = null;
    window.removeEventListener("focus", poke);
    window.removeEventListener("online", poke);
    window.removeEventListener("storage", onStorage);
    document.removeEventListener("visibilitychange", onVisible);
    state.enabled = false;
    state.mode = "off";
    state.kind = "";
    state.reason = "";
    state.nextAt = null;
  }

  // Starts sync from the remembered switch when this computer is still connected.
  async function startup() {
    if (scheduler || disposed) return;
    const id = stored();
    if (!state.available || !id) return;
    cancelRetry();
    const seen = generation;
    try {
      const link = await connection();
      if (scheduler || disposed || seen !== generation) return;
      if (link && link.connection_id === id) begin(id);
      else store("");
    } catch {
      // The check failed, so the switch stays and the check runs again later.
      if (disposed || seen !== generation) return;
      retryTimer = setTimeout(retryStartup, 60000);
      window.addEventListener("online", retryStartup);
    }
  }

  async function attach(next) {
    if (disposed) return;
    session = next;
    if (scheduler) {
      unsubscribe?.();
      unsubscribe = watch(session);
      return;
    }
    await startup();
  }

  async function setEnabled(on) {
    if (disposed) return false;
    generation += 1;
    cancelRetry();
    const seen = generation;
    state.error = "";
    if (!on) {
      store("");
      end();
      return true;
    }
    let link = null;
    try {
      link = await connection();
    } catch {
      link = null;
    }
    if (disposed || seen !== generation) return false;
    if (!link) {
      state.error = "Connect this computer to the repository first.";
      return false;
    }
    store(link.connection_id);
    if (!scheduler) begin(link.connection_id);
    return true;
  }

  function dispose() {
    disposed = true;
    end();
  }

  return { state, attach, setEnabled, poke, dispose };
}
