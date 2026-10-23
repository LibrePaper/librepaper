// Runs automatic Git sync while the project window is open. The scheduler
// lives in gitsync/auto.js; this module owns the switch, the storage of it and
// the state the panel shows.
import { createAutoSync } from "../gitsync/auto.js";
import { createBridge } from "../gitsync/bridge.js";
import { syncApi } from "../gitsync/server.js";
import { checkRemote, pull, push, recover, status } from "../gitsync/sync.js";
import { desktop, notify } from "../desktop/client.js";

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
    const moved = snapshot.lastPullAt !== state.lastPullAt || snapshot.lastPushAt !== state.lastPushAt;
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
    unsubscribe = session?.onFiles(() => scheduler?.edited()) || null;
    window.addEventListener("focus", poke);
    window.addEventListener("online", poke);
    document.addEventListener("visibilitychange", onVisible);
    scheduler.start();
    state.enabled = true;
  }

  function end() {
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
    session = next;
    if (scheduler) {
      unsubscribe?.();
      unsubscribe = session?.onFiles(() => scheduler?.edited()) || null;
      return;
    }
    const id = stored();
    if (!state.available || !id) return;
    try {
      const link = await connection();
      if (scheduler) return;
      if (link && link.connection_id === id) begin();
      else store("");
    } catch {
      // Automatic sync stays off.
    }
  }

  async function setEnabled(on) {
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
    if (!link) {
      state.error = "Connect this computer to the repository first.";
      return false;
    }
    store(link.connection_id);
    if (!scheduler) begin();
    return true;
  }

  return { state, attach, setEnabled, poke, dispose: end };
}
