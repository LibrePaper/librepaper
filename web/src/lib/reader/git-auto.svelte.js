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

  function begin() {
    scheduler = createAutoSync({
      ops: {
        status: async () => {
          const s = await status(ctx());
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
    document.addEventListener("visibilitychange", onVisible);
    scheduler.start();
    state.enabled = true;
  }

  function end() {
    generation += 1;
    scheduler?.stop();
    scheduler = null;
    unsubscribe?.();
    unsubscribe = null;
    window.removeEventListener("focus", poke);
    window.removeEventListener("online", poke);
    document.removeEventListener("visibilitychange", onVisible);
    state.enabled = false;
    state.mode = "off";
    state.kind = "";
    state.reason = "";
    state.nextAt = null;
  }

  async function attach(next) {
    if (disposed) return;
    session = next;
    if (scheduler) {
      unsubscribe?.();
      unsubscribe = watch(session);
      return;
    }
    const id = stored();
    if (!state.available || !id) return;
    const seen = generation;
    try {
      const link = await connection();
      if (scheduler || disposed || seen !== generation) return;
      if (link && link.connection_id === id) begin();
      else store("");
    } catch {
      // Automatic sync stays off.
    }
  }

  async function setEnabled(on) {
    if (disposed) return false;
    generation += 1;
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
    if (!scheduler) begin();
    return true;
  }

  function dispose() {
    disposed = true;
    end();
  }

  return { state, attach, setEnabled, poke, dispose };
}
